# Design reference

Decision Science Corp, 2026. This server exists so a small network can run a modern IRC daemon we can read, test, and license on our own terms.

## What we copied, and what we did not

UnrealIRCd is the north star for operator expectations: listeners, oper, channel modes, flood, a config surface an Unreal admin can map. Ergo is the north star for IRCv3 shape: tags, history, a server that is pleasant over WebSocket. ircd-hybrid is the check on what is baseline versus optional.

| Role | Project | Use |
|------|---------|-----|
| Ops and feature breadth | [UnrealIRCd](https://www.unrealircd.org/) | Behavior, docs, config ideas |
| IRCv3 shape | [Ergo](https://github.com/ergochat/ergo) | Tags, history, batch, web-friendly transport |
| Scope check | [ircd-hybrid](https://github.com/ircd-hybrid/ircd-hybrid) | Required versus nice-to-have |

UnrealIRCd is GPL-2.0. This repository is a new Rust program.

- We study behavior, docs, and the protocol feature set.
- We do not copy Unreal or Ergo source into this tree.
- We do not start from abandoned Rust IRCds.

The license split for **this** tree is AGPL-3.0-only for program source and CC BY-SA 4.0 for everything else. See [LICENSE](../LICENSE).

## Goals

1. One process. TCP, TLS, and WebSocket.
2. A client can register, talk, join, and moderate with the commands in [protocol.md](protocol.md).
3. Every capability in `CAP LS` is implemented and tested. Advertising a cap without a test is a defect.
4. Config is TOML, with names an Unreal admin can recognize. The key list is [configuration.md](configuration.md).
5. Services stay beside the daemon. NickServ and ChanServ are Atheme or Anope, not a second server hidden in this binary.

## Non-goals

| Surface | Why it is out |
|---------|----------------|
| Server-to-server linking | Different program. History is local SQLite on purpose. |
| In-daemon NickServ / ChanServ | Services packages already exist. The daemon stays a daemon. |
| Unreal module parity | We are not porting the module tree. |
| WHOX, voice, halfop, keys, `+e`, `+I` | Not advertised. Adding them means implementing them and then advertising them. |
| A graphical client | Out of this repository. `ircc` is a smoke tool. |

The scorecard that tracks pass, partial, and missing is [PROTOCOL-MATRIX.md](PROTOCOL-MATRIX.md).

## Config shape

TOML, not Unreal's block language. The mapping:

| Unreal idea | dsc-ircd |
|-------------|----------|
| `me { name }` | `[server] name` |
| MOTD | `[server] motd` inline |
| Admin | `admin_name`, `admin_email` |
| `listen` | `[[listen]] bind`, `tls`, `websocket` |
| `oper` | `[oper]` single line |
| Flood / class | `[limits]` |
| Channel history | `[history]` plus `CHATHISTORY` |

CLI listen flags replace the file's listen list. They exist so a lab can start without editing TOML. A standing instance should use `--config`.

## Clients

This repository is the server. Terminal, browser, and native clients are separate work. The server's job is to be boring on the wire: numerics when a command fails, capabilities that match behavior, and a WebSocket port that speaks the same IRC.

`tools/irctest/` runs a curated slice of [progval/irctest](https://github.com/progval/irctest) against the binary. See [IRCTEST.md](IRCTEST.md).
