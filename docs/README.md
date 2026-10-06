# Documentation

Guides for running, operating, and extending dsc-ircd. Program source is AGPL-3.0-only. These documents are CC BY-SA 4.0. See the repository [LICENSE](../LICENSE).

## Reading order

1. [Getting started](getting-started.md) if you want a process listening on localhost.
2. [Configuration](configuration.md) before you point it at anything but loopback.
3. [Security](security.md) before you point it at a public address.
4. [Protocol](protocol.md) if you are writing or debugging a client.
5. [Operations](OPS-HANDBOOK.md) if you are holding a kick, a ban, or an O-line.
6. [Architecture](architecture.md) if you are changing the server.
7. [Testing](TESTING.md) and [irctest](IRCTEST.md) before you send a change.
8. [Acceptance matrix](PROTOCOL-MATRIX.md) when you need the pass/partial/missing scorecard.

## Map

| Document | What it is |
|----------|------------|
| [getting-started.md](getting-started.md) | Build, first connection, TLS, WebSocket |
| [configuration.md](configuration.md) | TOML reference and CLI flags |
| [protocol.md](protocol.md) | Wire manual: commands, modes, caps, history |
| [OPS-HANDBOOK.md](OPS-HANDBOOK.md) | Channel and network moderation |
| [security.md](security.md) | Public-bind posture, secrets, limits |
| [architecture.md](architecture.md) | Process shape: sessions, state, fanout, storage |
| [client.md](client.md) | `ircc` smoke client |
| [TESTING.md](TESTING.md) | Unit, e2e, coverage, fuzz, CI |
| [IRCTEST.md](IRCTEST.md) | progval/irctest controller and curated gate |
| [PROTOCOL-MATRIX.md](PROTOCOL-MATRIX.md) | Conformance scorecard |
| [REFERENCE.md](REFERENCE.md) | Why this server exists and what it refuses to copy |
| [LAB.md](LAB.md) | Private lab layout. No live hostnames or passwords |

`config.example.toml` at the repository root is the annotated lab config those guides refer to.
