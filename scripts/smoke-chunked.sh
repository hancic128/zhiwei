#!/usr/bin/env bash
#
# Verify node-agent HTTP client correctly decodes chunked transfer-encoding.
#
# Background (2026-09-21): a node reported
#   WARN failed to fetch probe config error=probe config parse failed
#   DEBUG failed to fetch cert config error=trailing characters at line 1 column 2
# Root cause: node-agent HTTP client used bare `read_to_end` + split on `\r\n\r\n`,
# doesn't handle HTTP/1.1 chunked. Render / Cloudflare force chunked on HTTP/1.1 + close,
# response body becomes `<hex>\r\n...body...\r\n0\r\n\r\n`, JSON parse fails
# with "trailing characters at line 1 column 2".
#
# Usage:
#   ./scripts/smoke-chunked.sh
#   ./scripts/smoke-chunked.sh --against monitor.example.com   # real edge
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
    sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

TARGET=""
while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help) usage 0 ;;
        --against) TARGET="$2"; shift 2 ;;
        *) echo "Unknown argument: $1" >&2; usage 2 ;;
    esac
done

say() { printf '\033[36m==>\033[0m %s\n' "$*"; }
ok()  { printf '\033[32mPASS:\033[0m %s\n' "$*"; }
die() { printf '\033[31mFAIL:\033[0m %s\n' "$*" >&2; exit 1; }

# ============ 1) Unit tests must pass first ============
say "Running node-agent chunked decoding unit tests"
( cd "$ROOT" && cargo test -p zhiwei-node-agent --bin zhiwei-node http:: -- --test-threads=1 )

# ============ 2) End-to-end: local chunked JSON server, nc captures raw bytes ============
say "Starting fake server emulating Render edge (chunked JSON response)"

PY=$(mktemp -t zhiwei-smoke.XXXXXX.py)
PORT=$(python3 -c '
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
')

cat > "$PY" <<'PYEOF'
import http.server, socketserver, sys
PORT = int(sys.argv[1])

class ChunkedHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = (
            b'{"node_id":"smoke","probes":[{"id":"p1","service":"s","name":"n",'
            b'"kind":"tcp","target":{"host":"127.0.0.1","port":1},'
            b'"expect":{},"interval_seconds":60,"timeout_ms":1000}]}'
        )
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Transfer-Encoding", "chunked")
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(f"{len(body):x}\r\n".encode())
        self.wfile.write(body)
        self.wfile.write(b"\r\n0\r\n\r\n")

    def log_message(self, *_): pass

with socketserver.ThreadingTCPServer(("127.0.0.1", PORT), ChunkedHandler) as srv:
    srv.allow_reuse_address = True
    srv.serve_forever()
PYEOF

python3 "$PY" "$PORT" &
SERVER_PID=$!
# Wait for server to come up
for _ in 1 2 3 4 5 6 7 8 9 10; do
    if (echo > "/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; then break; fi
    sleep 0.1
done

cleanup() { kill "$SERVER_PID" 2>/dev/null || true; rm -f "$PY"; }
trap cleanup EXIT

# Use nc to capture raw bytes (nc is cleaner than openssl s_client for plain HTTP here)
RAW=$( (printf 'GET /v1/probe-config?node_id=smoke HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n'; sleep 0.3) \
  | nc -w 2 127.0.0.1 "$PORT" ) || true

if [ -z "$RAW" ]; then
    die "nc got no response"
fi

# Use python to split headers/resp, strip chunked frames, assert JSON is valid
PARSED=$(RAW="$RAW" python3 <<'PYEOF'
import os, json
raw = os.environ["RAW"].encode("utf-8")
sep = raw.find(b"\r\n\r\n")
if sep < 0:
    print("__NO_SEP__"); raise SystemExit(0)
body = raw[sep+4:]
assert b"0\r\n" in body, f"chunked terminator not found in body, actual tail: {body[-20:]!r}"
out = bytearray()
i = 0
while i < len(body):
    nl = body.find(b"\r\n", i)
    if nl < 0: break
    size = int(body[i:nl].split(b";")[0], 16)
    if size == 0: break
    out += body[nl+2:nl+2+size]
    i = nl + 2 + size + 2
inner = bytes(out).decode("utf-8")
json.loads(inner)  # must parse
print("OK")
PYEOF
) || true

if [ "$PARSED" = "OK" ]; then
    ok "body is chunked, deframed JSON is valid"
else
    die "chunked JSON parse failed: $PARSED"
fi

# ============ 3) (Optional) Real edge verification ============
if [ -n "$TARGET" ]; then
    say "Capturing chunked raw bytes from $TARGET (plain HTTP; prod uses TLS, just checking body frame format)"
    # Real edge has TLS; use openssl s_client `-ign_eof` + plain HTTP/1.1
    # Note: openssl s_client starts TLS by default; some edges respond to plain HTTP too;
    # using starttls none or raw bytes is more reliable
    RAW=$( (printf 'GET /healthz HTTP/1.1\r\nHost: %s\r\nConnection: close\r\n\r\n'; sleep 0.3) \
      | openssl s_client -connect "$TARGET:443" -servername "$TARGET" -quiet -ign_eof 2>/dev/null ) || true
    if printf '%s' "$RAW" | head -1 | grep -qi 'Transfer-Encoding: chunked'; then
        ok "$TARGET does return chunked on HTTP/1.1 + close"
    else
        printf '\033[33mWARN:\033[0m %s did not return chunked, edge behavior may have changed? raw headers: %s\n' \
            "$TARGET" "$(printf '%s' "$RAW" | head -3 | tr '\n' '|')"
    fi
fi

printf '\n\033[32m✓ smoke-chunked all passed.\033[0m\n'
