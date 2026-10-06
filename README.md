# ircd

Single-node IRC daemon written in Rust by Decision Science Corp.

It speaks modern client-to-server IRC and the IRCv3 capabilities it advertises: capability negotiation, message tags, server time, account tags, batch, away-notify, SASL PLAIN, and draft CHATHISTORY. Listeners are plaintext TCP, TLS, WebSocket, and WebSocket-over-TLS.

The feature model is [UnrealIRCd](https://www.unrealircd.org/). The IRCv3 shape follows notes from [Ergo](https://github.com/ergochat/ergo). This tree is a clean-room Rust program. It does not contain Unreal or Ergo source. See [docs/REFERENCE.md](docs/REFERENCE.md).

Version **0.1.0**. One process, one machine, no server-to-server linking. NickServ and ChanServ are not inside this binary. Put Atheme or Anope beside it when a network needs services.

## Documentation

| Guide | Read it for |
|-------|-------------|
| [Getting started](docs/getting-started.md) | Build, run, connect |
| [Configuration](docs/configuration.md) | Every TOML key and the CLI |
| [Protocol](docs/protocol.md) | Commands, modes, numerics, capabilities |
| [Operations](docs/OPS-HANDBOOK.md) | Ops, bans, kills, escalation |
| [Security](docs/security.md) | What a public bind has to get right |
| [Architecture](docs/architecture.md) | Crates, sessions, fanout, history |
| [Smoke client](docs/client.md) | The `ircc` CLI |
| [Testing](docs/TESTING.md) | How contributors run the suite |
| [irctest](docs/IRCTEST.md) | External conformance gate |
| [Acceptance matrix](docs/PROTOCOL-MATRIX.md) | What is implemented, partial, or out of scope |
| [Lab recipe](docs/LAB.md) | How to stand up a private lab |
| [Doc index](docs/README.md) | The same map, with a reading order |

`config.example.toml` is a commented lab config. It is not a production config.

## Quick start

You need a recent stable Rust toolchain (edition 2021). SQLite is bundled through `rusqlite`; you do not install a system SQLite library to build.

```bash
cargo run -p ircd -- --bind 127.0.0.1:6667
```

Second terminal:

```bash
cargo run -p ircc -- --host 127.0.0.1 --port 6667 --nick otto --join '#test' --msg 'hello' --quit
```

A standing config:

```bash
cargo run -p ircd -- --config ./config.example.toml
```

TLS, WebSocket, and capability negotiation are in [Getting started](docs/getting-started.md).

## Workspace

| Crate | Role |
|-------|------|
| `ircd` | Server library and `ircd` binary |
| `ircd-core` | Line parser, casemap, typed commands, tag helpers |
| `ircc` | CLI smoke client, not a user-facing IRC app |
| `fuzz/` | libFuzzer targets, outside the Cargo workspace |

## Status

The client protocol that this server advertises is implemented and covered by in-tree end-to-end tests. The living scorecard is [docs/PROTOCOL-MATRIX.md](docs/PROTOCOL-MATRIX.md).

Still out of scope: server linking, an in-daemon services package, channel keys, voice (`+v`), ban exceptions (`+e` / `+I`), and any capability the server does not advertise. A client that `CAP REQ`s an unknown capability gets `NAK`.

## License

Copyright (c) 2026 Decision Science Corp.

Program source (`crates/`, `fuzz/`, `tools/`, `.github/`, Cargo manifests, `tarpaulin.toml`) is under the **GNU Affero General Public License, version 3 only** ([LICENSE-AGPL-3.0](LICENSE-AGPL-3.0)).

Everything else in this repository, including this README, `docs/`, and `config.example.toml`, is under **Creative Commons Attribution-ShareAlike 4.0 International** ([LICENSE-CC-BY-SA-4.0](LICENSE-CC-BY-SA-4.0)).

See [LICENSE](LICENSE). Upstream references keep their own licenses. This tree does not redistribute their source.
