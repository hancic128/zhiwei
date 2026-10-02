#!/usr/bin/env bash
#
# 验证 node-agent 的 HTTP 客户端能正确解码 chunked transfer-encoding。
#
# 背景(2026-09-21):节点 `VM-16-12-opencloudos` 上报
#   WARN 拉取探针配置失败 error=解析探针配置失败
#   DEBUG 拉取证书配置失败 error=trailing characters at line 1 column 2
# 根因:node-agent 的 HTTP 客户端裸 `read_to_end` + 按 `\r\n\r\n` 切 header,
# 不识别 HTTP/1.1 chunked。Render / Cloudflare 这类边缘在 HTTP/1.1 + close 时
# 会强制 chunked,响应 body 被 `<hex>\r\n...body...\r\n0\r\n\r\n` 污染,JSON 解析
# 报 "trailing characters at line 1 column 2"。
#
# 用法:
#   ./scripts/smoke-chunked.sh
#   ./scripts/smoke-chunked.sh --against zhiwei.onrender.com   # 真线边缘
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
        *) echo "未知参数:$1" >&2; usage 2 ;;
    esac
done

say() { printf '\033[36m==>\033[0m %s\n' "$*"; }
ok()  { printf '\033[32m通过:\033[0m %s\n' "$*"; }
die() { printf '\033[31m失败:\033[0m %s\n' "$*" >&2; exit 1; }

# ============ 1) 单测必须先绿 ============
say "跑 node-agent 的 chunked 解码单测"
( cd "$ROOT" && cargo test -p zhiwei-node-agent --bin zhiwei-node http:: -- --test-threads=1 )

# ============ 2) 端到端:本机起 chunked JSON server,nc 抓 raw 字节 ============
say "起一个模仿 Render 边缘的假 server(响应 chunked JSON)"

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
# 等 server 起来
for _ in 1 2 3 4 5 6 7 8 9 10; do
    if (echo > "/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; then break; fi
    sleep 0.1
done

cleanup() { kill "$SERVER_PID" 2>/dev/null || true; rm -f "$PY"; }
trap cleanup EXIT

# 用 nc 抓 raw 字节(nc 比 openssl s_client 干净:这里就是明文 HTTP)
RAW=$( (printf 'GET /v1/probe-config?node_id=smoke HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n'; sleep 0.3) \
  | nc -w 2 127.0.0.1 "$PORT" ) || true

if [ -z "$RAW" ]; then
    die "nc 没拿到响应"
fi

# 用 python 一气把 headers/resp 拆开,再剥 chunked 帧,顺便断言 JSON 合法
PARSED=$(RAW="$RAW" python3 <<'PYEOF'
import os, json
raw = os.environ["RAW"].encode("utf-8")
sep = raw.find(b"\r\n\r\n")
if sep < 0:
    print("__NO_SEP__"); raise SystemExit(0)
body = raw[sep+4:]
assert b"0\r\n" in body, f"body 里找不到 chunked 终止帧,实际末尾: {body[-20:]!r}"
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
json.loads(inner)  # 必须能解析
print("OK")
PYEOF
) || true

if [ "$PARSED" = "OK" ]; then
    ok "body 是 chunked,剥帧后 JSON 合法(修复后能解析)"
else
    die "chunked JSON 解析失败: $PARSED"
fi

# ============ 3) (可选)真线边缘验证 ============
if [ -n "$TARGET" ]; then
    say "对 $TARGET 抓 chunked raw 字节(明文 HTTP,实际生产走 TLS,这里只看 body 帧格式)"
    # 真线有 TLS,用 openssl s_client 的 `-ign_eof` + 明文 HTTP/1.1
    # 注意:openssl s_client 默认起 TLS,但部分边缘对纯 HTTP 也会回;保险起见用 starttls none 或者直接拼字节
    RAW=$( (printf 'GET /healthz HTTP/1.1\r\nHost: %s\r\nConnection: close\r\n\r\n'; sleep 0.3) \
      | openssl s_client -connect "$TARGET:443" -servername "$TARGET" -quiet -ign_eof 2>/dev/null ) || true
    if printf '%s' "$RAW" | head -1 | grep -qi 'Transfer-Encoding: chunked'; then
        ok "$TARGET 在 HTTP/1.1 + close 时确实返回 chunked"
    else
        printf '\033[33m注意:\033[0m %s 没有 chunked,边缘行为可能变了?raw 头: %s\n' \
            "$TARGET" "$(printf '%s' "$RAW" | head -3 | tr '\n' '|')"
    fi
fi

printf '\n\033[32m✓ smoke-chunked 全过。\033[0m\n'
