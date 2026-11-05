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
| Clients trusting a private CA | *(to come, T1c)* |
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
- The key file must be readable by the door only (mode 600, owner the
  container user); it is never committed. `deploy/server/certs/` is where
  the server compose mounts them from.

Check it from another machine:

```
curl --cacert ca.crt https://SERVER/health      # {"front":"ok",...}
curl http://SERVER:443/health                    # no answer: TLS only
```

## To come in this runbook

- Making the CyxWiz test CA and the server certificate, and installing the
  CA on the LAN machines (T1b).
- The client settings that trust it: Engine, server node, CLI, storage node
  (T1c).
- Switching the HP test server to TLS, and checking every client (T1d).
- Renewal and rotation, and what to do when a certificate expires or a key
  leaks (T1b/T1d).
- Node certificates and pinning (T2, T3).
