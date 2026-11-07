# TLS between machines: the operator runbook

How traffic between CyxWiz machines is encrypted, and what to do in
production, step by step, by following this page alone. TOFIX192 builds it
in stages; each section says whether it is in place or still to come.

## The model

| Kind of hop | Who proves what | How |
| --- | --- | --- |
| Clients to the server side (Engine, `cyxcloud` CLI, storage and server nodes, browsers) | the server proves its name | the front door holds a certificate for the server's name, checked by the client against the system roots, or against the CyxWiz test CA file it is given (LAN / test setups) |
| Inside the server machine (front door to central, gateway, API; the website to the door) | nobody (one host, private Docker network) | plain; the services are not published. Encryption inside the network (mTLS) is a later step |
| To a storage or server node (gateway, rebalancer, central, the Engine) | the node proves it holds its key | *(to come, TOFIX192 T2/T3)* each node has a self-signed certificate whose fingerprint is bound to its node key at registration and pinned by whoever dials it |

Plain text is meant only for this machine or a private LAN. Clients refuse
`http://` to a public address *(to come, T1c)*.

## Where we are

| Hop | TLS |
| --- | --- |
| Clients to the front door | **available**: the front door serves TLS when given a certificate (below); the HP test server still runs plain *(T1d switches it)* |
| Front door to the services | plain on the private network, by design |
| Clients trusting a private CA | **available** in the CLI, storage node, Engine and server node (below) |
| Gateway / rebalancer to storage nodes | plain *(to come, T2)* |
| Central and the Engine to server nodes | plain *(to come, T3)* |
| Browser to the website | plain on its own port *(to come, T4: through the front door)* |

## The front door with TLS

The door (`cyxwiz-front`, [front.md](front.md)) speaks TLS on `listen` when
`front.toml` names a certificate chain and its key, both PEM:

```toml
listen = "0.0.0.0:443"
tls_cert = "/etc/cyxwiz/certs/fullchain.pem"
tls_key  = "/etc/cyxwiz/certs/privkey.pem"
internal_listen = "0.0.0.0:8000"
```

- The TLS port answers TLS only (HTTP/2 for gRPC and HTTP/1.1, by ALPN). A
  plain request there gets no answer.
- `internal_listen` is a second, plain listener with the same routes,
  sign-in check, limits and audit, for the server's own services: the
  compose healthcheck and the website calling `http://front:8000`. **Publish
  only `listen`.** Anything that reaches the internal port skips TLS. It is
  refused without TLS (then `listen` alone is the plain port).
- The services learn which listener a request came through from
  `X-Forwarded-Proto` (`https` / `http`); the audit line has `tls=true/false`.
- The key file is mode 600, owned by the server's deploy user; the door's
  container reads it as root (the containers do not yet run as their own
  user, a follow-up). It is never committed. `deploy/server/certs/` is
  where the server compose mounts them from.

Check it from another machine:

```
curl --cacert ca.crt https://SERVER/health      # {"front":"ok",...} (Windows: add --ssl-no-revoke)
curl http://SERVER:443/health                    # no answer: TLS only
```

## The door's certificate

Which certificate depends on whether the server has a public domain name.

- **With a domain** (production, e.g. `cloud.cyxwiz.ai`): a public
  certificate. Either Let's Encrypt (certbot or another ACME client on the
  server, writing `fullchain.pem` / `privkey.pem`), or, behind Cloudflare, a
  Cloudflare origin certificate. Clients then need nothing: the system
  roots trust it. Renewal is the ACME client's job (every 60-90 days); after
  each renewal restart the door (see "Renewing").
- **Without a domain** (a LAN or test server, like the HP at
  192.168.1.180): a private **CyxWiz test CA**, made on the server, signs the
  door's certificate, and each client is given the CA's `ca.crt`. The rest
  of this section is that setup.

### Making the test CA (once per server)

On the server, in the deploy folder (cyxwiz_web `deploy/server`):

```
./new-certs.sh ca
```

- It makes an EC P-256 key and a self-signed CA certificate, valid 5 years,
  in `~/.cyxwiz-ca/` (mode 700; set `CA_DIR` for another place).
- `ca.key` never leaves that folder: anyone who has it can make
  certificates every client trusts. Do not copy it into the repo, the
  containers, or a backup that others can read. If the server is
  rebuilt, keep the folder (or make a new CA and give every client the new
  `ca.crt`, below).
- It refuses to run when a CA is already there. Making a new CA by
  mistake would leave every client trusting the old one.

### The door's certificate (and renewing it)

```
./new-certs.sh server cyxwiz-hp 192.168.1.180
```

- Pass every name and address clients use to reach the server: host names
  become DNS names, addresses become IP names in the certificate. A client
  that connects by a name not listed refuses the server. Add the address
  of every network the server is reached on.
- It writes `certs/fullchain.pem` (the certificate, valid 397 days),
  `certs/privkey.pem` (mode 600) and `certs/ca.crt` (a copy of the CA's
  public certificate, to hand out). `certs/` is git-ignored except its
  README.
- It checks the new certificate against the CA before replacing the old
  files, and replaces the key and certificate together.
- **Renewing:** run the same command again before it expires, then
  restart the door (`docker compose up -d --force-recreate front`). Clients
  need nothing new: they trust the CA, not this certificate.
- `./new-certs.sh show` prints what is there: names, expiry dates and
  SHA-256 fingerprints, never a key. It warns when the door's certificate
  expires within 30 days. Put it in a monthly check.

### Giving clients the CA

Copy `certs/ca.crt` (public, safe to send) to each client machine, and give
its path to the client (below). Before trusting a copy that came over email
or a chat, compare its fingerprint with the one `./new-certs.sh show`
prints on the server:

```
openssl x509 -in ca.crt -noout -fingerprint -sha256
```

Tools and browsers can also use it:

- `curl --cacert ca.crt https://192.168.1.180/health`. On Windows, curl
  uses the system TLS (Schannel), which wants revocation information that
  a private CA does not publish: add `--ssl-no-revoke`, or the request fails
  with `CERT_TRUST_REVOCATION_STATUS_UNKNOWN`.
- A browser on a test machine: import `ca.crt` as a trusted authority
  (Windows: `certutil -addstore -f Root ca.crt` as administrator; macOS:
  Keychain Access, System, then "Always Trust"; Linux:
  `/usr/local/share/ca-certificates/cyxwiz-test.crt` and
  `update-ca-certificates`). Only on machines you control, and remove it
  when the test server is gone.

### The client settings

A client given a CA trusts **only** that CA for the server; without one it
uses the system's trusted roots (a public certificate). No client has a
switch that skips certificate checks.

| Client | Setting |
| --- | --- |
| `cyxcloud` CLI | `cyxcloud login --server https://HOST --ca-cert ca.crt` (saved as `[server] ca_cert`; `config set server.ca_cert PATH`, empty clears; env `CYXCLOUD_CA_CERT` for one run). `cyxcloud doctor` shows which CA is in use. |
| Storage node | `[network] tls_ca_cert` in its config (written by `cyxcloud node init` from the CLI's saved CA), or env `TLS_CA_CERT`. Used for the gateway, DataStream and sign-in. |
| Engine | Preferences > Servers > **Certificate authority (optional)** under the address (saved as `servers.ca_file` in `engine_config.json`; env `CYXWIZ_CA_FILE` when empty). Shows the CA's SHA-256 to compare with `new-certs.sh show`. |
| Server node | Settings > Network, the same row (saved as `network.server_ca_file` in `server_config.yaml`). |

Plain `http://` is refused by every client to anything but this machine
or a private network (loopback, RFC 1918, link-local, 100.64/10, IPv6 ULA
and link-local, one-label LAN names, `.local` / `.lan` / `.internal` /
`.home.arpa`). On the LAN it is allowed, with a note.

### When something goes wrong

| Problem | What to do |
| --- | --- |
| The door's certificate expired (clients fail with "certificate expired") | `./new-certs.sh server <same names>` and restart the door. Clients need nothing. |
| A client says the name does not match | The client used a name or address that is not in the certificate: `./new-certs.sh show` lists them. Issue a new one with the missing name and restart the door. |
| `privkey.pem` leaked (copied off the server, in a backup others read) | Issue a new door certificate at once (a new key comes with it) and restart the door. The old certificate stays valid until it expires (a private CA has no revocation list), so also replace the CA if the leak is serious. |
| `ca.key` leaked, or the CA is lost | Replace the CA (below). |

### Replacing the CA

1. Move the old one aside: `mv ~/.cyxwiz-ca ~/.cyxwiz-ca.old-$(date +%F)`.
2. `./new-certs.sh ca`, then `./new-certs.sh server <names...>`, and
   restart the door.
3. Give every client the new `certs/ca.crt` and remove the old one from
   wherever it was installed. Until then those clients refuse the server.
4. When every client works, delete the old CA folder.

## To come in this runbook

- Switching the HP test server to TLS, and checking every client (T1d).
- Node certificates and pinning (T2, T3).
