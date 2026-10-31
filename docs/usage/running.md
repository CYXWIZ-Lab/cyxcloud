# Running CyxCloud

CyxCloud is the CyxWiz storage network: a gateway that clients talk to (S3-style
HTTP API plus gRPC), storage nodes that hold erasure-coded chunks, and a CLI. The
gateway keeps metadata in Postgres and caches in Redis.

| Part | Crate / binary | Default ports |
|---|---|---|
| Gateway | `cyxcloud-gateway` | HTTP `0.0.0.0:8180`, gRPC `0.0.0.0:50052` |
| Storage node | `cyxcloud-node` | gRPC 50051, libp2p 4001, metrics 9090 |
| CLI | `cyxcloud-cli` (binary `cyxcloud`) | - |
| Rebalancer | embedded in the gateway | - |

Port clashes on a CyxWiz development PC: the CyxWiz central server uses 50051
and 8080, and a CyxWiz server node uses 50052. When everything runs on one
machine, give the gateway another gRPC port (for example `--grpc-addr
0.0.0.0:50060`) and storage nodes another gRPC port (`--port 50061`).

## 1. Prerequisites

- Rust (version pinned by `rust-toolchain.toml`).
- `protoc` on `PATH` or in the `PROTOC` environment variable.
- Postgres and Redis (Docker is easiest). Without Postgres, run the gateway with
  `--memory-only` (data is lost on exit).

## 2. Build and test

```powershell
cargo build -p cyxcloud-gateway
cargo build -p cyxcloud-node
cargo build -p cyxcloud-cli
cargo test -p cyxcloud-gateway
```

**Older CPUs (no AVX2).** On Linux and macOS x86-64, `reed-solomon-erasure`
(erasure coding, in cyxcloud-core) compiles its SIMD code with
`-march=haswell`, so a binary built on a recent machine dies with "illegal
instruction" (SIGILL, exit 132) on a CPU without AVX2, such as the HP's
i7-2620M: the gateway on its first upload. Build for any x86-64 since ~2009
with:

```sh
RUST_REED_SOLOMON_ERASURE_ARCH=x86-64-v2 cargo build --release -p cyxcloud-gateway
```

The crate's build script does not rerun when the variable changes: run
`cargo clean --release -p reed-solomon-erasure` first in a target folder that
built it without. The Dockerfiles here and in cyxwiz_web `deploy/server` set
it. Windows (MSVC) builds do not compile the SIMD code and are not affected.

## 3. Run the gateway

```powershell
$env:DATABASE_URL = 'postgres://cyxcloud:<password>@localhost:5433/cyxcloud'
$env:REDIS_URL    = 'redis://localhost:6380'
$env:JWT_SECRET   = '<same secret as the CyxWiz web API>'
cargo run -p cyxcloud-gateway -- --http-addr 0.0.0.0:8180 --grpc-addr 0.0.0.0:50060 --cors-permissive
```

It applies its database migrations (`cyxcloud-metadata/migrations`) at start.
Check it: `Invoke-WebRequest http://localhost:8180/health` returns 200. The
startup line shows the database URL with the password hidden.

| Option / variable | Meaning |
|---|---|
| `--http-addr`, `--grpc-addr` | listen addresses |
| `DATABASE_URL` / `--database-url` | Postgres; omit or `--memory-only` for in-memory |
| `REDIS_URL` / `--redis-url` | Redis cache |
| `JWT_SECRET` | verifies user tokens; required when `CYXCLOUD_ENV=production`, random (tokens never verify) otherwise |
| `ADMIN_JWT_SECRET` | verifies admin tokens on `/api/v1/admin` (the admin site's Drive page); unset or shorter than 32 bytes, those routes answer 503 |
| `--cors-permissive` | allow all origins (development only) |
| `--grpc-auth` | require a token on gRPC (default on) |
| `TLS_CERT`, `TLS_KEY`, `TLS_CA_CERT`, `TLS_REQUIRE_CLIENT_CERT` | TLS |
| `CORS_ALLOWED_ORIGINS` | allowed origins when not permissive |

## 4. Run a storage node

```powershell
$env:CENTRAL_SERVER_ADDR = 'http://localhost:50060'      # the gateway's gRPC address
$env:CYXWIZ_API_URL = 'http://localhost:3002'            # CyxWiz web API, for sign-in
$env:CYXWIZ_EMAIL = '<account email>'
$env:CYXWIZ_PASSWORD = '<account password>'
cargo run -p cyxcloud-node -- --data-dir D:\cyx-storage\node1 --port 50061 --metrics-port 9091
```

Other settings: `NODE_ID`, `NODE_NAME`, `NODE_REGION`, `PUBLIC_ADDRESS`,
`STORAGE_CAPACITY_GB`, `--config` (default `config.toml`), `--logout` (removes
saved credentials). Storage nodes can run on other machines on the network:
point `CENTRAL_SERVER_ADDR` at the gateway PC's LAN address.

## 5. Everything in Docker

`docker-compose.yml` runs Postgres, Redis, the gateway, three storage nodes and
the CyxWiz web API. Secrets come from an untracked `.env`:

```powershell
Copy-Item .env.example .env    # then fill in the values
docker compose up -d --build
```

`.env.example` lists the names (MongoDB URI for the web API, `JWT_SECRET`,
`POSTGRES_PASSWORD`, the storage nodes' account password). Never commit `.env`.

## 6. Status

CyxCloud is early-phase and being made production ready. The Solana payment
pool, on-chain subscriptions and wallet sign-in are being removed: storage
rewards and plans move to account credits, and nodes sign in with the owner's
CyxWiz account.
