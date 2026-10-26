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

## Data

`upload`, `download`, `list`, `delete`, `status` and `dataset ...` are the
older bucket/key commands on the gateway's S3 and REST APIs. They are being
replaced by `cyxcloud dataset put | ls | get | info | rm | share` (TOFIX177
slice 2), the same objects the Engine's Data Input > CyxCloud shows.

Known gaps (TOFIX177 slice 2): `cyxcloud register` calls a route the CyxWiz API
does not have (accounts are created on the web); `logout` only forgets the
local sign-in (the API has no logout route).
