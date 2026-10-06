# Configuration

dsc-ircd reads one TOML file (`--config`) plus optional CLI listen overrides. The annotated lab file is `config.example.toml` at the repository root.

Missing sections use the defaults below. Extra keys are ignored. `ircd` calls `Config::validate` before it listens, and that check is what rejects an empty server name, a nick length outside 1–50, a TLS listen without a cert and key, a plaintext listen while `production` is on, or a handshake cap of zero.

## CLI

```text
ircd [--config FILE]
     [--bind HOST:PORT]
     [--tls-bind HOST:PORT]
     [--ws-bind HOST:PORT]
     [--wss-bind HOST:PORT]
     [--tls-cert FILE --tls-key FILE]

ircd gen-cert [--out DIR] [--cn NAME]
```

| Flag | Effect |
|------|--------|
| `--config` | Load this TOML file. Without it, built-in defaults are used. |
| `--bind` | Add a plaintext TCP listen. |
| `--tls-bind` | Add a TLS TCP listen. Requires `--tls-cert` and `--tls-key`. |
| `--ws-bind` | Add a plaintext WebSocket listen. |
| `--wss-bind` | Add a TLS WebSocket listen. Uses the same cert and key flags. |
| `--tls-cert` / `--tls-key` | PEM certificate and private key for every TLS listen added on the CLI. |
| `gen-cert` | Write a self-signed lab cert. Default directory `./certs`, default CN `ircd.dsc.local`. |

If **any** listen flag is present, those flags become the entire `[[listen]]` list. The file's listen blocks are not merged in. Other sections (`[server]`, `[limits]`, accounts, and so on) still come from the file.

Help: `ircd --help`.

## `[server]`

| Key | Default | Meaning |
|-----|---------|---------|
| `name` | `ircd.dsc.local` | Name used as the prefix on server numerics. Must be non-empty. |
| `motd` | A one-line welcome | Message of the day. Multi-line strings are sent as one `372` per line. |
| `admin_name` | empty | Shown by `ADMIN`. |
| `admin_email` | empty | Shown by `ADMIN`. |
| `max_nick_length` | `30` | Nick octet cap. Valid range is 1 through 50. Advertised as `NICKLEN`. |
| `max_channel_length` | `50` | Channel name octet cap, including `#`. Advertised as `CHANNELLEN`. |
| `password` | empty | Connection password. Empty means `PASS` is not required. A wrong `PASS` fails registration with `464`. |

Nicks are ASCII letters, digits, `_`, and `-`. Channels start with `#` and cannot contain spaces, commas, or control characters. Identity comparison uses ASCII case-folding (`CASEMAPPING=ascii`).

## `[[listen]]`

One block per socket.

| Key | Default | Meaning |
|-----|---------|---------|
| `bind` | required | `host:port`. Empty bind is rejected. |
| `tls` | `false` | TLS on this socket. Requires `cert` and `key`. |
| `websocket` | `false` | IRC-over-WebSocket text frames instead of raw TCP lines. |
| `cert` | unset | PEM certificate path. Required when `tls = true`. |
| `key` | unset | PEM private key path. Required when `tls = true`. |

A WebSocket block with `tls = true` is WSS. Certificate and key files must not be group- or world-readable. The server refuses to start if they are. See [security.md](security.md).

Default with no file and no CLI flags: one plaintext listen on `127.0.0.1:6667`.

## `[oper]`

One operator line. Disabled unless you turn it on.

| Key | Default | Meaning |
|-----|---------|---------|
| `enabled` | `false` | `false` makes every `OPER` fail with `491`. |
| `name` | empty | O-line name compared to the first `OPER` parameter. |
| `password` | empty | Plaintext compare against the second parameter. |

This password is not hashed. Keep the config file mode `0600`. On a public host, set `[security] require_tls_for_auth` or `production` so `OPER` is rejected on plaintext sockets.

There is one O-line, not a list.

## `[history]`

SQLite channel history for `CHATHISTORY` and for JOIN auto-replay.

| Key | Default | Meaning |
|-----|---------|---------|
| `enabled` | `true` | `false` hides the `draft/chathistory` capability and the `CHATHISTORY` verb. |
| `path` | `./data/history.sqlite3` | Database file. Parent directory is created mode `0700`. The file must not be group- or world-readable. |
| `max_per_channel` | `1000` | Rows kept per channel. |
| `max_total_rows` | `0` | Global cap across channels. `0` means no global cap. |
| `auto_replay_on_join` | `50` | How many recent messages to replay on JOIN. Clamped to 200, which is also the per-query `CHATHISTORY` maximum. |

When history is enabled, registration advertises `CHATHISTORY=200` and `MSGREFTYPES=msgid,timestamp`, and `CAP LS` includes `draft/chathistory`.

A client that negotiates `draft/chathistory` does **not** get JOIN auto-replay. It is expected to ask with `CHATHISTORY`.

## `[[accounts]]`

Built-in SASL PLAIN accounts. Repeat the block per account.

| Key | Meaning |
|-----|---------|
| `name` | Account name. |
| `password` | Plaintext password. |

An empty list means SASL is not advertised. A non-empty list adds `sasl=PLAIN` to `CAP LS`.

These accounts are a bootstrap. They are not NickServ. Passwords are not hashed. Do not reuse them on a public hostname. Production networks should run Atheme or Anope beside this daemon and treat this table as empty. See [OPS-HANDBOOK.md](OPS-HANDBOOK.md).

## `[limits]`

| Key | Default | Meaning |
|-----|---------|---------|
| `max_clients` | `256` | Concurrent connections. |
| `max_clients_per_ip` | `32` | Concurrent connections from one IP. |
| `flood_lines_per_window` | `30` | Inbound lines allowed per client per window. |
| `flood_window_secs` | `10` | Flood window length. Values below 1 are treated as 1 second. |
| `max_line_bytes` | `8192` | Maximum IRC line size including CR/LF. A longer line closes the connection. A tag block over 4096 octets, or a malformed tag block, is numeric `417` and the connection stays up. |
| `max_channels` | `1024` | Channels that may exist at once. |
| `max_channels_per_client` | `64` | Channels one client may join. |
| `max_members_per_channel` | `512` | Members in one channel. |
| `max_topic_bytes` | `390` | Topic text octet cap. Control characters in topics are rejected. |

Exceeding a connection cap closes the socket with `ERROR` before registration. Flood violations disconnect the client.

## `[security]`

| Key | Default | Meaning |
|-----|---------|---------|
| `production` | `false` | When `true`, every listen must be TLS, at least one listen must exist, and SASL / `OPER` require TLS. |
| `require_tls_for_auth` | `false` | Reject `AUTHENTICATE` and `OPER` on non-TLS connections. Implied by `production`. |
| `registration_timeout_secs` | `60` | Disconnect if `NICK` + `USER` (and `CAP END`, if negotiating) do not finish in time. `0` disables the timer. |
| `idle_timeout_secs` | `0` | Disconnect a registered client with no input for this long. `0` disables it. |
| `handshake_timeout_secs` | `15` | Deadline for the TLS or WebSocket handshake. |
| `max_handshake_inflight` | `64` | Concurrent handshakes. `0` is rejected at startup. |

`production = true` with any `tls = false` listen fails validation. The process does not start.

## `[websocket]`

Applies to every WebSocket listen.

| Key | Default | Meaning |
|-----|---------|---------|
| `allowed_origins` | `[]` | Exact, case-sensitive `Origin` values. An empty list **denies** every browser `Origin`. |
| `allow_missing_origin` | `true` | Allow a handshake that sends no `Origin` (typical of native clients). |
| `require_irc_subprotocol` | `true` | Require the client to offer `Sec-WebSocket-Protocol: irc`. |

Policy is checked before the upgrade completes. A denied handshake never becomes an IRC session.

## Unreal-shaped names

Operators coming from UnrealIRCd can use this sketch. The authoritative keys are the tables above.

| Unreal idea | dsc-ircd |
|-------------|----------|
| `me { name }` | `[server] name` |
| MOTD file | `[server] motd` (inline) |
| Admin block | `admin_name`, `admin_email` |
| `listen { ip; port; }` | `[[listen]] bind` |
| `options { tls; }` plus certs | `tls`, `cert`, `key` |
| `oper { }` | `[oper]` (one line) |
| Connection class / flood | `[limits]` |

There is no Unreal block language, no module list, and no `set` block.
