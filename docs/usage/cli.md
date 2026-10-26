# The cyxcloud CLI

One binary, one server address, one sign-in.

## Server and sign-in

```
cyxcloud login --server HOST [--email E] [--password-file F]
cyxcloud whoami
cyxcloud logout
cyxcloud config show
cyxcloud config set server HOST
```

- `--server HOST` (or `config set server HOST`) sets every service address at
  once and is remembered in `~/.cyxcloud/config.toml` under `[server] host`.
  A plain host maps to the server side's three services until the front door
  exists (TOFIX168): gateway HTTP `:8180`, gateway gRPC `:50060`, sign-in
  `:3002`. An `https://HOST` address means one front door for all three. The
  separate keys (`gateway.http_url`, `gateway.grpc_url`, `auth.api_url`) can
  still be set one by one.
- Changing the server drops the old sign-in: a sign-in belongs to the server
  that issued it.
- The password is prompted for, or read from the first line of
  `--password-file` (scripts). It never goes on the command line.
- `~/.cyxcloud/credentials.json` is owner-only on every OS. Its expiry is the
  token's own `exp` claim.

## Storage node

See `node.md`: `cyxcloud node init | start | stop | status | logs`.

## Health

`cyxcloud doctor` checks the path from this machine to the server and the node
on this machine, and says what to do; exit codes 0 / 1 (you) / 2 (network or
server).

## Datasets

```
cyxcloud dataset put <FILE|FOLDER>... --name NAME [--description D]
cyxcloud dataset ls [--shared] [--public] [--filter TEXT]
cyxcloud dataset info NAME|ID
cyxcloud dataset get NAME|ID [-o DIR]
cyxcloud dataset rm NAME|ID [--force]
cyxcloud dataset verify ID | share ID --with EMAIL
cyxcloud token create NAME|ID [--ttl 1h]
```

- These are the same datasets the Engine shows under Data Input > CyxCloud:
  both use the gateway's gRPC dataset service.
- `put` uploads each file (every file directly in a folder) in 1 MB pieces with
  its BLAKE3 hash; the gateway checks the bytes against it, then the files
  become one dataset.
- `get` streams each file and checks it against the gateway's SHA-256 before
  it is moved into place (`*.partial` until then). A file name the server sends
  is reduced to its last part, so it cannot write outside the folder.
- A dataset is named by its name or by its id (the first 8 characters are
  enough when they are unique).
- `rm` asks first in a terminal; scripts pass `--force`.
- `token create` prints only the token on stdout (the rest on stderr), for a
  training node; lifetime 1 minute to 30 days.
- `verify` and `share` still use the gateway's REST API (share takes an email
  address).

The older bucket and key commands (`upload`, `download`, `list`, `delete`,
`status`) are gone: they used the gateway's S3 API, which has no
authentication yet (TOFIX171). `cyxcloud doctor` replaces `status`.

`register` creates the account through the API (POST /api/users) and then
signs in; `logout` forgets the local sign-in (the API has no logout route; the
token expires on its own).
