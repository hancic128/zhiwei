# Architecture

> High-level architecture of ZhiWei.

## Overview

ZhiWei follows a **hub-and-spoke** model:

```mermaid
graph TB
    subgraph Monitor["zhiwei-monitor (Data Plane)"]
        UI["Web UI<br/>(Static)"]
        API["REST API<br/>(JSON)"]
        Store["Certificate Store<br/>(SQLite)"]
        UI --> API
        UI --> Store
        API --> Store
        Auth["Ed25519 Request Signature Auth"]
        API --> Auth
        Store --> Auth
    end

    subgraph NodeA["zhiwei-node (Host A)"]
        C1["Collector<br/>CPU · Memory<br/>Disk · Network<br/>Process"]
        P1["Probes<br/>HTTP · TCP · TLS"]
        S1["Signer"]
        C1 --> S1
        P1 --> S1
        S1 -->|Ed25519 Signed Requests<br/>every 30s telemetry| Monitor
    end

    subgraph NodeB["zhiwei-node (Host B)"]
        C2["Collector<br/>CPU · Memory<br/>Disk · Network<br/>Process"]
        P2["Probes<br/>HTTP · TCP · TLS"]
        S2["Signer"]
        C2 --> S2
        P2 --> S2
        S2 -->|Ed25519 Signed Requests<br/>every 30s telemetry| Monitor
    end
```

## Components

### zhiwei-monitor (Server)

The central server component. Responsibilities:
- Node enrollment and identity management
- Telemetry ingestion and storage
- Alert evaluation and notification
- Certificate tracking
- Web UI hosting
- REST API

Technology:
- **Language**: Rust
- **Web Framework**: Axum
- **Database**: SQLite with WAL mode
- **Storage**: Local filesystem for certificates and logs

### zhiwei-node (Agent)

Runs on each monitored host. Responsibilities:
- System information collection
- Service health probing
- Certificate discovery
- Container monitoring
- Request signing

Technology:
- **Language**: Rust
- **System Info**: sysinfo crate
- **Networking**: rustls for HTTPS

### zhiwei-ops (Operations Server)

Handles remote command execution. Responsibilities:
- Command signing and dispatch
- Response collection
- Audit logging

## Authentication

### Node Identity (Ed25519 Request Signing)

Nodes authenticate using Ed25519 digital signatures, not TLS client certificates.

```mermaid
sequenceDiagram
    participant Node
    participant Monitor

    Note over Node: Generate Ed25519 keypair
    Node->>Monitor: POST /v1/enroll<br/>{ bootstrap_token, public_key }
    Note over Monitor: Validate token
    Monitor->>Monitor: Store public_key as node identity
    Monitor->>Node: { node_id, ops_public_key }
    Note over Node: Store node_id and ops_public_key
```

**Request Signing** — every request includes:
- `X-ZhiWei-Timestamp`: Unix timestamp (within ±300s window)
- `X-ZhiWei-Nonce`: Random unique value (anti-replay)
- `X-ZhiWei-Signature`: Ed25519 signature of `{method, path, timestamp, nonce, body}`

Monitor validates: (1) timestamp within ±300s, (2) nonce not seen before, (3) signature valid for registered node_id.

### Why Not mTLS?

```mermaid
graph LR
    A["node"] -->|"HTTPS"| B["PaaS Edge<br/>TLS terminated"]
    B -->|"plain HTTP"| C["container<br/>monitor"]

    Note over A,B: Traditional mTLS requires edge<br/>to forward client certificates
    Note over B,C: Edge terminates TLS — certificates lost
    Note over C: mTLS breaks here
```

Traditional mTLS requires the TLS terminator to forward client certificates.
This breaks on platforms that terminate TLS at the edge (Render, Railway, etc.).

Ed25519 signing works regardless of TLS termination point because:
- Authentication happens at the application layer
- TLS only provides encryption, not identity
- Ed25519 signatures prove node identity cryptographically

### Command Channel (Bidirectional Signing)

```mermaid
sequenceDiagram
    participant Ops as Ops Server
    participant Monitor
    participant Node

    Ops->>Ops: Sign command with ops private key
    Ops->>Node: Signed command
    Note over Node: Validate signature<br/>using stored ops public key
    Node->>Node: Execute command
    Node->>Node: Sign response with node private key
    Node->>Monitor: Signed response
    Note over Monitor: Validate signature<br/>using stored node public key
```

This creates a **closed loop**:
- Monitor can't forge commands (no ops private key)
- Node can't forge responses (no monitor private key)

## Data Flow

### Telemetry Collection

```mermaid
flowchart LR
    A["sysinfo"] --> B["Collector"]
    B --> C["Signer"]
    C -->|"HTTPS POST /v1/telemetry"| D["Monitor<br/>Validate"]
    D --> E["SQLite<br/>Insert"]
    E --> F["WebSocket / Polling"]
    F --> G["UI"]
```

### Alert Evaluation

```mermaid
flowchart LR
    A["Telemetry Insert"] --> B["Alert Rules Engine"]
    B --> C["Check thresholds"]
    B --> D["Check probe status"]
    B --> E["Check cert expiry"]
    C --> F["Webhook<br/>Notify"]
    D --> F
    E --> F
```

## Security Model

| Threat | Mitigation |
| --- | --- |
| Node impersonation | Ed25519 signature required |
| Replay attacks | Nonce + timestamp window |
| Command forgery | Ops key signing (monitor can't forge) |
| Response forgery | Node key signing (node can't fake) |
| TLS downgrade | Application-layer authentication |
| Secret disclosure | No secrets in logs; bootstrap tokens expire |

## Scalability

ZhiWei is designed for single-operator scenarios, not enterprise scale.

**Storage**:
- SQLite with WAL for concurrent reads
- Retention policy: 7 days for telemetry, 30 days for alerts

**Performance**:
- ~1.6 MB/day per node for telemetry (vs 57 MB with traditional approach)
- 30-second collection interval (configurable)

**Limits**:
- Tested with ~20 nodes
- Designed for personal use, not enterprise
