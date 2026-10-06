# Security

dsc-ircd will accept a public bind. The defaults will not survive one. This page is the checklist that has to be true before the socket is reachable beyond localhost.

## Defaults are a lab

Out of the box the server:

- Listens on `127.0.0.1:6667` with no TLS
- Compares `OPER` and SASL passwords as plaintext strings in the TOML file
- Allows SASL and `OPER` on plaintext if you configure them
- Has no idle timeout
- Allows 256 clients, 32 per IP, 30 lines per 10 seconds
- Denies browser WebSocket origins, and allows clients that send no `Origin`

`config.example.toml` repeats lab accounts (`alice` / `alice-pass`, `bob` / `bob-pass`) and an operator password of `change-me`. Those strings are documentation. They are not a deployment.

## Production flag

```toml
[security]
production = true
require_tls_for_auth = true
```

`production = true` does two things at startup:

1. Refuses the config if any `[[listen]]` block has `tls = false`, or if the listen list is empty.
2. Implies `require_tls_for_auth`.

`require_tls_for_auth` alone still allows a plaintext listen, and rejects `AUTHENTICATE` and `OPER` on any connection that is not TLS or WSS. Use it when you intentionally keep a localhost plaintext port next to a public TLS port. Use `production` when every listener is public.

Certificates must be real for any client that verifies. `ircd gen-cert` is a lab tool. `ircc --tls` does not verify and must not be the client you point at the public internet.

## Secrets on disk

`OPER` passwords, account passwords, TLS private keys, and the history database are secrets.

- Config file mode `0600`. The server does not chmod your TOML for you.
- TLS keys and the SQLite history file are refused if they are group- or world-accessible (`mode & 077 != 0`). The server creates new ones as `0600` inside a `0700` directory.
- Do not commit a filled-in `config.toml`. Commit `config.example.toml` only.
- One O-line, stored in cleartext. Rotate it the way you rotate a host root password. There is no oper class and no hashed form in this version.

Built-in `[[accounts]]` are the same shape: cleartext, static, no lockout. Prefer an empty list on a public host and put registration in Atheme or Anope beside the daemon.

## Network exposure

| Control | Where |
|---------|--------|
| Bind address | `[[listen]] bind`. Loopback until the rest of this page is done. |
| Connection password | `[server] password`. This is a front door (`464` on failure), not an account system. |
| Global and per-IP caps | `[limits] max_clients`, `max_clients_per_ip` |
| Handshake slots | `[security] max_handshake_inflight` (default 64) |
| Registration deadline | `registration_timeout_secs` (default 60, `0` disables) |
| Idle deadline | `idle_timeout_secs` (default `0`, disabled) |
| Line size | `max_line_bytes` (default 8192, including CRLF) |
| Flood | `flood_lines_per_window` / `flood_window_secs` (default 30 / 10) |

A client over the cap is closed with `ERROR` and never registered. A flood or an idle timeout drops the connection and its channel membership.

WebSocket: an empty `allowed_origins` denies every `Origin`. List exact origins before a browser can connect. Leave `require_irc_subprotocol` on.

## What operators can do

An IRC operator can `KILL` any nick and `WALLOPS` other operators, and can act as a channel op where they are present. `KILL` disconnects the target. It does not ban the host and it does not transfer a services account, because this process has no services accounts beyond the static SASL table.

Channel ops can kick, ban, set `+i`, and set the topic when `+t` is on. Bans are in-memory masks. They die with the process.

## What this server does not protect you from

- Password guessing against the static SASL table or the single O-line. There is no throttle specific to `AUTHENTICATE` beyond the global line flood.
- A stolen config file. Passwords in it are usable as-is.
- Another compromised server. There is no linking, so there is also no trust of a peer IRCd.
- Clients that ignore numerics. The server answers failed moderation commands with numerics; it does not have a second, silent policy.
- Legal or safety incidents. Those leave the IRC toolbelt. See the escalation ladder in [OPS-HANDBOOK.md](OPS-HANDBOOK.md).

## AGPL and network use

The program is AGPL-3.0-only. Running a modified version as a network service carries the Affero source obligation. Read [LICENSE-AGPL-3.0](../LICENSE-AGPL-3.0) section 13 before you ship a private fork on a public port.
