# Getting started

dsc-ircd is one Rust binary. It listens for IRC clients, keeps channel state in memory, and can store channel history in a local SQLite file.

## Requirements

- A recent **stable** Rust toolchain. The crate edition is 2021. GitHub Actions uses the stable channel.
- A C toolchain is not required for SQLite. `rusqlite` builds its own copy.

```bash
rustc --version
cargo --version
```

## Build

From the repository root:

```bash
cargo build -p ircd --release
```

The binary is `target/release/ircd`. Development runs can skip `--release`:

```bash
cargo run -p ircd -- --bind 127.0.0.1:6667
```

With no `--config` and no listen flags, the default config listens on `127.0.0.1:6667` only. The server name is `ircd.dsc.local`. History is on and writes `./data/history.sqlite3`. There is no operator and no SASL account until you add them.

## Connect

Any IRC client works. The in-tree smoke client is `ircc`:

```bash
cargo run -p ircc -- \
  --host 127.0.0.1 --port 6667 \
  --nick otto --join '#test' --msg 'hello' --quit
```

Omit `--quit` for an interactive session. After `--join`, a line that does not start with `/` is sent as `PRIVMSG` to that channel. A line that starts with `/` is sent as a raw IRC command (`/join #other`, `/whois otto`). Details are in [client.md](client.md).

irssi, against the same listener:

```text
/server 127.0.0.1 6667
/join #test
```

## Use a config file

Copy `config.example.toml` and edit it. Then:

```bash
cargo run -p ircd -- --config ./config.toml
```

Every key is documented in [configuration.md](configuration.md). The example file includes lab accounts and an operator password of `change-me`. Those are placeholders. Replace them before the process is reachable by anyone but you.

CLI listen flags **replace** the `[[listen]]` list from the file when any of them is present. `--bind`, `--tls-bind`, `--ws-bind`, and `--wss-bind` together become the whole listen set. They do not append to the file.

## TLS

Generate a lab certificate once. This is a self-signed cert for local use.

```bash
cargo run -p ircd -- gen-cert --out ./certs --cn ircd.dsc.local
```

That writes `certs/cert.pem` and `certs/key.pem`. The process refuses a key or history database that is group- or world-readable.

Run plaintext and TLS together:

```bash
cargo run -p ircd -- \
  --bind 127.0.0.1:6667 \
  --tls-bind 127.0.0.1:6697 \
  --tls-cert ./certs/cert.pem \
  --tls-key ./certs/key.pem
```

Smoke the TLS port with `ircc`. `--tls` **does not verify** the certificate. It is a lab switch.

```bash
cargo run -p ircc -- --tls --host 127.0.0.1 --port 6697 \
  --nick otto --join '#test' --msg 'hello tls' --quit
```

A public listener needs a real certificate and [security.md](security.md). Set `[security] production = true` when every listen block is TLS. That setting also forces SASL and `OPER` onto TLS.

## WebSocket

IRC lines travel in WebSocket **text** frames. The server normalizes CR, LF, and CRLF inside a frame.

```bash
cargo run -p ircd -- --bind 127.0.0.1:6667 --ws-bind 127.0.0.1:7667
```

A client connects to `ws://127.0.0.1:7667` and sends one IRC line per text frame.

Default WebSocket policy:

- `Sec-WebSocket-Protocol: irc` is required.
- A missing `Origin` header is allowed (native clients).
- Any `Origin` header is **denied** until you list it under `[websocket] allowed_origins`.

A browser client will fail the handshake until its exact origin is allowlisted. See [configuration.md](configuration.md).

WSS is TLS plus WebSocket:

```bash
cargo run -p ircd -- \
  --wss-bind 127.0.0.1:7668 \
  --tls-cert ./certs/cert.pem \
  --tls-key ./certs/key.pem
```

## Capability negotiation

```bash
cargo run -p ircc -- --cap --host 127.0.0.1 --port 6667 \
  --nick otto --join '#test' --msg 'hello' --quit
```

`--cap` sends `CAP LS 302`, then `CAP REQ :multi-prefix server-time message-tags away-notify`, then `CAP END`, before `NICK` / `USER`. The server will not finish registration while capability negotiation is open. That request list includes `multi-prefix`, which this server does not advertise, so the whole `REQ` is `NAK`d and none of those caps turn on. See [client.md](client.md).

## Logs

The binary logs with `tracing`. The default filter is `ircd=info`. Override it:

```bash
RUST_LOG=ircd=debug cargo run -p ircd -- --config ./config.toml
```

## What to read next

- Bind it only on localhost until [security.md](security.md) is done.
- Command and mode behavior: [protocol.md](protocol.md).
- Channel ops and network kills: [OPS-HANDBOOK.md](OPS-HANDBOOK.md).
