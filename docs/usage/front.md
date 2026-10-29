# The front door (`cyxwiz-front`)

One address for the whole CyxWiz server side (TOFIX168). Every client — the
Engine, the `cyxcloud` CLI, storage and server nodes, the website — talks to
the front door; the front door talks to the services on a private network.

```
client ──TLS──▶ cyxwiz-front :8000/443 ──▶ central   :50051 (gRPC cyxwiz.*)
                 sign-in check            ├▶ gateway  :50060 (gRPC cyxcloud.*)
                 limits, audit            ├▶ gateway  :8180  (/api/datasets/, /api/v1/auth/)
                                          └▶ API      :3002  (/api/*)
```

## Run it

```
JWT_SECRET=... ADMIN_JWT_SECRET=... cyxwiz-front [--config front.toml]
```

Locally: `deploy/local/run-service.ps1 front` (cyxwiz_web), part of
`start-local.ps1`. Without a config file it listens on `0.0.0.0:8000`, plain
HTTP, and routes to the services on this machine.

Clients then use one address:

```
cyxcloud login --server http://HOST:8000      # front door without TLS (LAN)
cyxcloud login --server https://cloud.cyxwiz.ai   # with TLS
```

## What it does to every request

| Step | Rule |
| --- | --- |
| Limit | at most `requests_per_minute` (default 1200) per client address; the address is the connection's, or `CF-Connecting-IP` when the connection comes from a `trusted_proxies` address (a Cloudflare tunnel). Over the limit: HTTP 429 / gRPC RESOURCE_EXHAUSTED. |
| Route | gRPC by package, HTTP by path prefix; the longest prefix wins. Nothing else is served (HTTP 404 / gRPC UNIMPLEMENTED). |
| Sign-in | a request with `Authorization: Bearer` must carry a valid, unexpired token signed with `JWT_SECRET` or `ADMIN_JWT_SECRET`; otherwise HTTP 401 / gRPC UNAUTHENTICATED, and the service never sees it. A request without a token goes through and the service decides (sign-in, node registration and dataset streams need none). The services check tokens again. |
| Size | an HTTP body above `http_body_bytes` (default 16 MB) is refused (413) or cut. gRPC streams (dataset uploads) are left to the services. |
| Forward | gRPC over HTTP/2, HTTP over HTTP/1.1; streams and trailers pass through; `X-Forwarded-For`, `-Proto` and `-Host` are added, connection headers dropped. A service that does not answer: 502 / gRPC UNAVAILABLE. |
| Audit | one log line per request (target `audit`): client address, method, path, route, status, account (the token's `sub`), milliseconds. Never a token. |

Not routed on purpose: the gateway's S3 API (`/s3`, no sign-in yet,
TOFIX171), every service's `/metrics` and `/health`, the gateway's `/ws`, and
central's REST (admin, internal).

`GET /health` is the door's own: `{"front":"ok","services":{"central":"up",
"gateway":"up","api":"up"}}`, 503 when a service does not accept a
connection. `cyxcloud doctor` reads it through the door.

## front.toml

```toml
listen = "0.0.0.0:443"
tls_cert = "/etc/cyxwiz/front/fullchain.pem"   # both or neither
tls_key  = "/etc/cyxwiz/front/privkey.pem"
trusted_proxies = ["127.0.0.1"]                # e.g. cloudflared on this box
client_ip_header = "cf-connecting-ip"

[limits]
requests_per_minute = 1200
http_body_bytes = 16777216

# Without [[route]] entries: the local defaults below.
[[route]]
name = "central"
prefix = "/cyxwiz."
upstream = "http://central:50051"
grpc = true

[[route]]
name = "gateway"
prefix = "/cyxcloud."
upstream = "http://gateway:50060"
grpc = true

[[route]]
name = "gateway"
prefix = "/api/datasets/"
upstream = "http://gateway:8180"

[[route]]
name = "gateway"
prefix = "/api/v1/auth/"
upstream = "http://gateway:8180"

[[route]]
name = "api"
prefix = "/api/"
upstream = "http://api:3002"
```

Unknown keys are refused, so a typo does not silently change the door.

## Not there yet

- Engine and node configured by the one address (TOFIX168 step 3).
- The whole server side in one Docker compose behind the door (step 4).
- Limits shared across several doors (Redis), per-account quotas (TOFIX176 P6).
