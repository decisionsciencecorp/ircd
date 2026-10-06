# Architecture

dsc-ircd is one Tokio process. It accepts clients, runs one session task per connection, and keeps nick and channel membership in a single in-memory `Shared` state. Channel history, when enabled, is a local SQLite file. Nothing in this process speaks to another IRCd.

```text
TCP / TLS / WebSocket accept
        │
        ▼
   admission (global cap, per-IP cap, handshake slots)
        │
        ▼
   session task  ──── outbox (mpsc, 64) ◀─── fanout from Shared
        │
        ▼
   Shared (Mutex): nicks, channels, opers, accounts-in-use
        │
        ▼
   HistoryStore (spawn_blocking → sqlite)
```

## Crates

| Path | Responsibility |
|------|----------------|
| `crates/ircd-core` | `RawLine` parser, `Command` enum, ASCII casemap, tag split / escape / adapt. `forbid(unsafe_code)`. No I/O. |
| `crates/ircd` | Config, admission, TLS, WebSocket, session, state, history. The `ircd` binary in `src/main.rs` is a thin CLI over this library. |
| `crates/ircc` | Smoke client. Not linked into the server. |
| `fuzz/` | `cargo-fuzz` targets for the parser and tag adapter. Excluded from the workspace so `cargo fuzz` can own its own `[workspace]`. |

Session logic is split under `crates/ircd/src/session/`:

| Module | Role |
|--------|------|
| `mod.rs` | Connection loop and command dispatch |
| `cap.rs` | `CAP` and `AUTHENTICATE` |
| `register.rs` | `001`–`005` and MOTD |
| `chathistory.rs` | `CHATHISTORY` parsing and `BATCH` framing |
| `message.rs` | Client-tag relay |
| `io.rs` | Line read, flood window, registration and idle deadlines |

## Identity

A connection gets a `ClientId` (`u64`) at accept time. Nicks point at that id. Channel membership, ops, and invites are sets of ids.

That split is why a nick change does not move ops to whoever grabs the old name. Authorization checks the id, then looks up the current nick for the wire.

## Session loop

Each connection:

1. Admits or rejects before any welcome. Rejection is `ERROR` and a close.
2. Registers a bounded outbox (`OUTBOX_CAP` = 64).
3. Reads lines and, in the same task, drains the outbox.

Fanout is member-targeted. A channel message is `try_send` to each member's outbox except the sender. A full outbox drops that message for the slow client. The server does not stall the channel on one stuck socket, and it does not broadcast to clients who are not in the channel.

The global `Shared` mutex is not held across `.await` on client I/O. History queries run on `spawn_blocking` so SQLite does not sit on the runtime. Those two rules are covered by `gate_a_acceptance_e2e` (`a6_session_has_no_lock_across_await`) and by `lock_discipline`.

## Admission and deadlines

Before a session exists, the accept path checks:

- `max_clients`
- `max_clients_per_ip`
- `max_handshake_inflight` for TLS and WebSocket

Inside the session:

- `registration_timeout_secs` until `NICK`+`USER` complete (and `CAP END` if negotiation started)
- `idle_timeout_secs` after registration, when non-zero
- a flood window of `flood_lines_per_window` lines per `flood_window_secs`

Dropping the connection removes the nick, channel membership, invites, and the admission slot. Cleanup is tied to the session task exit, not to a well-formed `QUIT`.

## History

`HistoryStore` is SQLite at `[history] path`. PRIVMSG lines that hit a channel are appended with a msgid and a timestamp. Retention is `max_per_channel`, plus `max_total_rows` when that is non-zero.

JOIN auto-replay and `CHATHISTORY` both read this store. They are documented in [protocol.md](protocol.md).

The database file and its directory are forced private (`0600` / `0700`). A file that is already group- or world-accessible is refused.

## TLS and WebSocket

TLS is rustls. `gen-cert` uses rcgen and writes PEMs through the same private-file helper as the history database.

WebSocket is tokio-tungstenite. Handshake policy (origin allowlist, `irc` subprotocol) runs in `ws_policy` before the connection is an IRC session. Frames are normalized to IRC lines in `ws_lines`.

`ircc --tls` installs a verifier that accepts any certificate. The server does not. Production clients must verify.

## What is shared on purpose

Config is immutable after startup (`Arc<Config>` inside `Shared`). There is no reload command. Restart the process to change binds, accounts, or limits.

There is one O-line and a static account list. Neither is a services database.

## Where to change things

| Change | Start here |
|--------|------------|
| New verb | `ircd-core/src/command.rs`, then the match in `session/mod.rs`, then an e2e test |
| New capability | `session/cap.rs` `BASE_CAPS` or `advertised_caps`, tests in the same commit as the behavior |
| New config key | `config.rs`, `config.example.toml`, [configuration.md](configuration.md) |
| Parser / tags | `ircd-core`, plus the fuzz targets in `fuzz/fuzz_targets/` |

A capability that is advertised without an end-to-end test is a bug. The suite's name for that rule is Gate A test `a1_cap_ls_has_no_false_ads`.
