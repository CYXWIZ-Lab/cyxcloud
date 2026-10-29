# Running a storage node

Offer disk space to CyxCloud from any machine (Linux, macOS, Windows) with the
`cyxcloud` CLI. The node holds shards of other people's files: never a whole
file, and the server checks every shard it reads back against its hash.
Client-side encryption, so that a node cannot read the shards either, is
TOFIX176 P3.

## Five commands

```
cyxcloud login --server HOST          # sign in once; the node earns for this account
cyxcloud node init                    # where, how much, which name
cyxcloud node start                   # run in the background
cyxcloud node status                  # what it is doing
cyxcloud doctor                       # what is wrong, and what to do
```

`HOST` is the server's host name or IP (dev stack: `localhost`, the HP setup:
`192.168.1.116`). An `https://` address means one front door for everything
(TOFIX168, when it exists).

### `cyxcloud node init`

Asks three things when run in a terminal, or takes flags:

| Flag | Default | Meaning |
| --- | --- | --- |
| `--data-dir` | `~/.cyxcloud/node/data` | folder for the shards |
| `--offer` | 90 % of free space, at most 500 GB | e.g. `700GB`, `1TB` |
| `--name` | the machine's host name | shown in lists |
| `--port` | 50051 | port the server dials for shards (open it in the firewall) |
| `--status-port` | 9090 | local port for `node status` (127.0.0.1 only is enough) |
| `--public-address` | this machine's address towards the server | what the server dials back |
| `--force` | | set up again; the key and the node id are kept, the folder and the offer are asked again (defaults without a terminal) |

It refuses an offer larger than the disk's free space. It writes:

| File | What |
| --- | --- |
| `~/.cyxcloud/node/config.toml` | the daemon's configuration |
| `~/.cyxcloud/node/key` | Ed25519 key; the node id is derived from it (owner-only) |
| `~/.cyxcloud/node/access_secret` | what the server presents on shard traffic (owner-only) |
| `~/.cyxcloud/node_credentials.json` | the account the node earns for (owner-only); another account's leftover file is replaced |

Owner-only means mode 600 on Linux and macOS, and on Windows an ACL with
inheritance removed: the user, SYSTEM and Administrators only (the same rule
OpenSSH applies to private keys).

### Sign-in: what the node runs on

The account sign-in from `cyxcloud login` is needed once, to set the machine
up. After that the node runs on credentials that do not expire:

| Talking to | Credential | Expires |
| --- | --- | --- |
| the gateway | the node key signs every registration; the gateway's node token (24 h) carries heartbeats and is renewed by registering again | never (the key) |
| the CyxWiz API | the machine `api_key` the API issued at registration carries heartbeats | never |

The account sign-in is needed again only when the API no longer knows the
machine (its key is refused). The node then logs, once:
"Registering this machine needs a current sign-in: run `cyxcloud login`, then
`cyxcloud node sign-in`, then restart the node". `cyxcloud node sign-in`
gives the node the CLI's current sign-in and changes nothing else (folder,
offer, key); it refuses when the node belongs to another account (moving a
node to another account is `node init --force`).

### `cyxcloud node start | stop | status | logs`

- `start` runs `cyxcloud-node` (next to `cyxcloud`, on the PATH, or
  `CYXCLOUD_NODE_BIN`) in the background, writes `node.pid` and `node.log`
  under `~/.cyxcloud/node/`, and waits until the node has registered (up to
  2 minutes; opening a large store takes a while). `--foreground` runs it in
  the terminal instead.
- `stop` stops the process in `node.pid` after checking it is a
  cyxcloud-node.
- `status` reads `http://127.0.0.1:<status-port>/status`; `--watch`
  refreshes every 2 s, `--json` prints the raw report. Exit code 1 when the
  node is not running.
- `logs [-n 50] [-f]` prints the log without colour codes and without any line
  that mentions a password.

Example (this PC, 2026-10-26):

```
  cloud-node-pc · 2bd0ae5a · ONLINE for 8 min
  machine     Intel(R) Core(TM) i7-8750H CPU @ 2.20GHz (6 cores, 12 threads) · 48 GB RAM · Windows 11 (26200) · host CJ_PC
  storage     D:/dev/cyxwiz_web/deploy/local/data/cloud-node · 19 MB used of 20 GB offered (36 GB free on the disk)
  shards      91 held
  server      http://localhost:50060 · heartbeat 6 s ago
  listening   127.0.0.1:50070
  version     cyxcloud-node 0.1.0
```

### `cyxcloud doctor`

Checks the version, the server address, the gateway and the sign-in server
(reachable, TLS or not, clock skew from the `Date` header), the sign-in
(confirmed with the server, time left), and a node on this machine (running,
registered, its port accepting connections, offer against free space). Exit
code 0 all good (warnings allowed), 1 something for you to do, 2 the network or
the server.

### `cyxcloud node service install | uninstall`

Start the node with the machine. No administrator rights are needed on any OS.

| OS | What is installed | Starts | Restarts on failure |
| --- | --- | --- | --- |
| Linux | systemd user unit `~/.config/systemd/user/cyxcloud-node.service` | at boot when lingering is on (`loginctl enable-linger`, tried automatically; else `sudo loginctl enable-linger $USER`), otherwise at login | yes, after 10 s |
| macOS | launchd agent `~/Library/LaunchAgents/ai.cyxwiz.cyxcloud-node.plist` | at login | yes (KeepAlive) |
| Windows | Run key value `CyxCloud Node` under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, running `cyxcloud node start` | at logon | no (`cyxcloud node start` again) |

Once a systemd unit or launchd agent is installed, `cyxcloud node start` and
`stop` go through it. `node status` shows how the node starts (`at boot`).
On Windows, a scheduled logon task or a real Windows service would need
administrator rights (and, for a service, a service control handler in the
daemon); install prints the `netsh` line that opens the node's port for
other machines, to run as administrator.

## Not there yet (TOFIX177 slice 2)

- `cyxcloud node upgrade` (signed releases, TOFIX175).
- Reputation, audits and earnings in `node status` (TOFIX176 P4, P8).
- The API still takes the machine's owner from the request body, and several
  of its machine routes need no sign-in (TOFIX183); the node already sends
  its account sign-in when it registers.
