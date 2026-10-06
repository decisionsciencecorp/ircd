# Private lab

A lab is a dsc-ircd process on a machine you control, with self-signed TLS, a handful of SASL accounts, and an operator line you are willing to throw away. This page is the layout. It does not publish a hostname, an address, or a password. Put those in the config on the host, mode `0600`, and keep them out of git.

## Layout

| Piece | Suggestion |
|-------|------------|
| Config | `/srv/ircd/config.toml` (mode `0600`) |
| Binary | `target/release/ircd` from a release build of this repo |
| Certificates | `/srv/ircd/certs/` from `ircd gen-cert`, or a real certificate |
| History | `/srv/ircd/data/history.sqlite3` |
| Logs | stdout, or redirect to a file. `RUST_LOG=ircd=info` |

Start and stop are whatever your host already uses (a shell script, a systemd unit, a container). The process is foreground Tokio. It does not daemonize itself.

## Listeners

A typical lab exposes three sockets on loopback or on a firewalled address:

| Socket | Example bind | Client URL |
|--------|--------------|------------|
| Plaintext | `127.0.0.1:6667` | `irc://` |
| TLS | `127.0.0.1:6697` | `ircs://` with the lab CA or an insecure client |
| WebSocket | `127.0.0.1:7667` | `ws://` |

Generate the lab certificate on the host:

```bash
ircd gen-cert --out /srv/ircd/certs --cn irc.lab.example
```

Point the TLS listen at `cert.pem` and `key.pem`. If you also want WSS, add a listen block with `websocket = true` and `tls = true`.

`ircc --tls` accepts any certificate. A browser will not. For a browser lab, either trust the generated cert or use a real name and a real certificate, and set `[websocket] allowed_origins` to the page's exact origin.

## Accounts

Copy the shape from `config.example.toml`, then replace every password.

```toml
[oper]
enabled = true
name = "admin"
password = "replace-me"

[[accounts]]
name = "alice"
password = "replace-me"
```

Those accounts exist so a client can try SASL PLAIN. They are not a user directory. When the lab becomes something people rely on, empty `[[accounts]]`, set `[security] require_tls_for_auth` or `production`, and put registration in services beside the daemon. [security.md](security.md) is the checklist. [OPS-HANDBOOK.md](OPS-HANDBOOK.md) is the moderation map.

## Smoke

From a checkout on the same host:

```bash
cargo run -p ircc -- --host 127.0.0.1 --port 6667 \
  --nick otto --join '#test' --msg 'lab' --quit
```

Repeat with `--tls` against the TLS port. For WebSocket, confirm the upgrade with a client that offers `Sec-WebSocket-Protocol: irc` and, if it is a browser, an allowlisted `Origin`.

## What not to commit

- The filled-in `config.toml`
- `certs/key.pem`
- `data/history.sqlite3`
- A note that contains the operator password or a public DNS name for a lab that still has the sample passwords

`config.example.toml` is the only config that belongs in the repository.
