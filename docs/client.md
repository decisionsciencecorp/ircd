# ircc

`ircc` is a small CLI used to smoke-test dsc-ircd. It is not a daily-driver IRC client. It has no reconnect, no capability state machine beyond one optional `CAP` burst, and its TLS mode does not verify certificates.

```bash
cargo run -p ircc -- --host 127.0.0.1 --port 6667 --nick otto
```

## Flags

| Flag | Default | Meaning |
|------|---------|---------|
| `--host` | `127.0.0.1` | Server host. |
| `--port` | `6667`, or `6697` when `--tls` is set and the port was left at 6667 | TCP port. |
| `--nick` | `ircc` | `NICK` sent at connect. |
| `--user` | `ircc` | `USER` ident. |
| `--tls` | off | TLS. Accepts any certificate. Lab only. |
| `--cap` | off | Send one fixed `CAP` burst before registration. See below. |
| `--join` | none | `JOIN` this channel after welcome. |
| `--msg` | none | `PRIVMSG` this text to the joined channel, then continue. Requires `--join`. |
| `--quit` | off | `QUIT` after the scripted join/msg. |
| `--help` | | Print the same summary. |

Sent lines are printed with a `>>` prefix. Server lines are printed as they arrive.

## What `--cap` sends

```text
CAP LS 302
CAP REQ :multi-prefix server-time message-tags away-notify
CAP END
```

`multi-prefix` is not advertised by dsc-ircd. `CAP REQ` is atomic, so this burst is `NAK`d and **none** of the listed caps are enabled. The flag is still useful for watching negotiation. To enable caps, send your own `CAP REQ` from a real client, or type the lines yourself without `--cap`.

## Interactive input

Without `--quit`, stdin is read for the life of the connection.

- `/join #chan` is sent as `JOIN #chan`.
- `/msg nick text` is sent as `PRIVMSG nick :text`.
- `/quit` is sent as `QUIT :ircc` and the client exits.
- Any other `/command …` is sent with the slash removed, as raw IRC. `/whois otto` becomes `WHOIS otto`.
- A line with no slash, after you passed `--join`, is `PRIVMSG` to that channel only. It does not follow a later `/join`.
- A line with no slash and no `--join` is sent as raw IRC.

Examples:

```bash
# Register, join, say one line, disconnect
cargo run -p ircc -- --host 127.0.0.1 --port 6667 \
  --nick otto --join '#test' --msg 'hello' --quit

# TLS lab (insecure verify)
cargo run -p ircc -- --tls --host 127.0.0.1 --port 6697 \
  --nick otto --join '#test' --quit

# Ask for advertised capabilities
cargo run -p ircc -- --cap --host 127.0.0.1 --port 6667 --nick otto
```

## What ircc will not do

- WebSocket. Use a browser or a small script against `ws://` for that path. The server side is covered in [getting-started.md](getting-started.md).
- SASL. Authenticate from a real client, or send `AUTHENTICATE` yourself as raw lines once you know the exchange in [protocol.md](protocol.md).
- Certificate verification. `--tls` exists so a `gen-cert` lab answers at all.

For a terminal client against a lab server, irssi is the better tool. `tools/c1_irssi_smoke.sh` is the scripted form used in development.
