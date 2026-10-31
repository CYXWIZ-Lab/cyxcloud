# How CyxCloud fits together

The pieces, who is a client, what joins the storage, and how a node runs.
Written from the code on branch `distributed-training`, 2026-10-22.

## The pieces

| Crate / process | Role | Runs where |
| --- | --- | --- |
| `cyxcloud-gateway` | The manager, and the only thing clients talk to. Owns the catalogue (through `cyxcloud-metadata`), splits and encodes files, places shards on nodes, rebuilds files on read, issues dataset access tokens, serves the S3 API, the dataset REST API and the three gRPC services (NodeService, DataService, DataStreamService). | One process per cloud. Local dev: the dev PC, gRPC 50060, HTTP 8180 (`deploy/local/run-service.ps1 gateway`). |
| `cyxcloud-metadata` | The gateway's memory: Postgres tables for users, buckets, files, chunks, chunk locations, nodes, datasets, dataset files, access tokens, shares; a Redis cache; the placement engine; health and quorum helpers. A library inside the gateway. | Postgres and Redis (containers in local dev). |
| `cyxcloud-node` | A member of the distributed storage. Holds shards on its disk (RocksDB through `cyxcloud-storage`), serves store-shard and get-shard over gRPC to the gateway, registers and heartbeats with the gateway, signs in to the CyxWiz API as a machine. | Every storage machine. |
| `cyxcloud-rebalancer` | A daemon inside the gateway process. Every 60 s (`REBALANCER_SCAN_INTERVAL`) it scans chunks whose shard count is under target and plans moves or repairs between nodes. | With the gateway. |
| `cyxcloud-network` | Node-to-node layer: gRPC for shard transfer, libp2p Kademlia for peer discovery. Not on the gateway's data path today. | Library in the node. |
| `cyxcloud-protocol` | The `.proto` files: `node.proto` (register, heartbeat, store, get), `data.proto` (StreamData: one whole file, see `stream_file.md`), `datastream.proto` (datasets, tokens, upload, delete, see `datasets_from_the_engine.md`). | Shared; the Engine compiles verbatim copies. |
| `cyxcloud-cli` | A client: login, dataset list, config. | Any machine. |

## Who is a client, who joins

Clients never hold data for others:

- The CyxWiz Engine (`DataStreamClient`): lists datasets, uploads files, creates datasets, mints access tokens.
- The CyxWiz server node (`CloudDatasetFetcher`): streams one file with a token when it trains.
- The web app: its Drive uses the S3 API with the signed-in user's token (through the front door); each user has one bucket, `user-<their id>`, and reaches only that one.
- The admin site: its Drive page reads `GET /api/v1/admin/stats` (node counts; capacity and free space of the online nodes, bytes stored on all; chunks) and `GET /api/v1/admin/nodes[?status=]` (each node's capacity, use, chunks, heartbeat, version), and `POST /api/v1/admin/nodes/:id/maintenance {"on": bool}` to hold an online or recovering node in maintenance or release it (409 otherwise), with an admin token (HS256, `ADMIN_JWT_SECRET`, issuer `cyxwiz-admin-dashboard`, `role: "admin"`). It talks to the gateway on the server's network, not through the front door.
- The CLI.

Joining the storage means running `cyxcloud-node`. The gateway is the manager; nodes are the members; everything else is a client.

## What happens to a file

1. An upload reaches the gateway, which records a file row with the file's BLAKE3 content hash (`UploadFile`, or S3 PUT).
2. The file is cut into 4 MB chunks (`DEFAULT_CHUNK_SIZE`). Each chunk is Reed-Solomon encoded into 10 data shards plus 4 parity shards (`DATA_SHARDS`, `PARITY_SHARDS`).
3. The placement engine picks an online node per shard; the gateway pushes the shards out; every shard gets a chunk row and a location row.
4. A read fetches at least 10 of the 14 shards per chunk from whichever nodes hold them. Every shard's bytes must hash to its id: a node that answers with other bytes is skipped, counted against (`nodes.failure_count`) and logged, and the shard is taken from another node. The decoded file must hash to what was recorded at upload, or the read fails with a clear error (`AppState::read_file`, shared by S3 GET and StreamData). The node refuses to serve a stored shard that no longer hashes to its id (`DATA_LOSS`).
5. A node that goes dark leaves chunks under target; the rebalancer plans a repair onto other nodes.

The placement engine hands each shard to the online node that holds the fewest shards of the chunk so far; ties go to the best score (least used storage, proximity to the uploader when known, datacenter and rack diversity, bandwidth). Measured 2026-10-22 with this PC's node and the HP's: a 9 MB file (3 chunks) landed 7 + 7 shards per chunk on the two nodes and streamed back intact.

What a node loss costs: a chunk needs any 10 of its 14 shards. With one node everything is on it; with two nodes each holds 7, so losing either makes every file unreadable until it is back (the gateway answers at once with `DATA_LOSS: not enough shards are held by online nodes (have 7, need 10)`). From four nodes on, no node holds more than 4 shards of a chunk and one node can be lost without data loss. A node that restarts keeps its identity (saved credentials and node id; the gateway matches it by peer id), so its shards stay its own.

## Who a node is, and who may talk to it

- **Identity.** On first start a node makes an Ed25519 key at `~/.cyxcloud/node/key` (owner-only). Its node id is derived from the public key (the first 16 bytes of its BLAKE3 hash, as hex), so the id cannot be chosen or copied: a registration carries the public key and a signature over the node id, its listen address and a timestamp (300 s of skew allowed), and the gateway accepts a known id only with the key it was registered with. A `[node] id` in the config that is not the key's id stops the node.
- **Node token.** `RegisterNode` answers with a node token; heartbeats, metric reports and drains must carry it and act only on that node. A user's sign-in token is refused for node RPCs.
- **Access secret.** The node also makes `~/.cyxcloud/node/access_secret` and hands it to the gateway at registration. Every chunk RPC to the node (store, get, delete, stream, verify) must present it as `authorization: Bearer <secret>`; anything else on the network is refused with UNAUTHENTICATED before storage is touched. The gateway and the rebalancer learn the secrets from the `nodes` table when they start and when a node registers.
- Not yet: TLS on the node's own port (the secret travels in clear on the local network until TOFIX176 P2's mTLS lands; the gateway's TLS covers node-to-gateway when enabled).

## Running a node

### With the CLI (recommended, 2026-10-26)

`cyxcloud login --server HOST`, `cyxcloud node init`, `cyxcloud node start`, `cyxcloud node status`, `cyxcloud doctor`: see `node.md`. The sections below are the manual ways.

### As a daemon (the HP, 2026-10-22)

```
~/cyxwiz/run-node.sh
```

The script sets the sign-in credentials from a mode-600 env file (`CYXWIZ_EMAIL`, `CYXWIZ_PASSWORD`, `CYXWIZ_API_URL`) and runs:

```
cyxcloud-node --config ~/cyxwiz/config/node.toml
```

The config:

```toml
[node]
name = "hp-linux"
[storage]
data_dir = "/home/mrcj/cyxwiz/node-data"
max_capacity_gb = 700
compression = true
[network]
bind_address = "0.0.0.0"
grpc_port = 50051          # shard traffic from the gateway
p2p_port = 4001            # libp2p discovery
public_address = "192.168.1.180"   # what the gateway dials back
[metrics]
enabled = false
[central]
address = "http://192.168.1.116:50060"   # the gateway's gRPC
register = true
heartbeat_interval_secs = 30
[cyxwiz_api]
base_url = "http://192.168.1.116:3002"   # sign-in as a machine
register = true
```

Without the env file the node prompts for the login on its console. A login
that does not reach the API (API starting, slow link) is retried 6 times, 5 s
apart; a refused credential stops the node. Start it detached and read its
log without colour codes or secrets:

```
setsid nohup ~/cyxwiz/run-node.sh > ~/cyxwiz/node.log 2>&1 < /dev/null &
sed "s/\x1b\[[0-9;]*m//g" ~/cyxwiz/node.log | grep -v -i password
```

A systemd unit around the script is the next step; there is none in the repo
yet.

### In Docker (Dockerfile.node)

```
docker build -f Dockerfile.node -t cyxcloud-node .
docker run -d --name cyxcloud-node \
  -p 50051:50051 -p 4001:4001 \
  -v /path/on/the/disk:/data \
  -e NODE_NAME=hp-linux -e STORAGE_CAPACITY_GB=700 \
  -e CENTRAL_SERVER_ADDR=http://192.168.1.116:50060 \
  -e CYXWIZ_API_URL=http://192.168.1.116:3002 \
  -e CYXWIZ_EMAIL=... -e CYXWIZ_PASSWORD=... \
  cyxcloud-node
```

The image compiles the node in its build stage. `docker-compose.yml` has two
node services (`cyxcloud-node1`, `cyxcloud-node2`) for a local cluster;
`scripts/run_cluster.sh` starts one.

## Known weaknesses (tracked in TOFIX171)

- The gateway is the single manager and the single data path: every byte goes through it twice.
- Fewer than four nodes cannot survive a node loss (see above); the rebalancer cannot repair what it has no spare node for.
- Node-to-node transfer and peer discovery exist as code; nothing routes through them.
- Node registration is JWT-free by design (it is signed instead), with no rate limit yet.
- The rebalancer's 60 s scans and the metadata pool (10 connections) can starve requests.

## Node maintenance

An admin holds a storage node in `maintenance` from the admin site's Drive page
(the gateway's admin route above). A node in maintenance:

- takes no new chunks (placement and repair targets read online nodes only);
- still serves the chunks it holds (chunk reads accept `online` and
  `maintenance` nodes);
- stays in maintenance across heartbeats and re-registration (a restart),
  and is not marked offline by the monitor;
- returns to `online` only when an admin releases it (the monitor then marks
  it offline if its heartbeats have stopped).
