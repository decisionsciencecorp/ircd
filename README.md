# ircd — DSC IRC server (Rust)

Clean-room **Rust** IRC daemon for Decision Science Corp’s Mark × Cody IRC rebuild.

**Reference model:** [UnrealIRCd](https://www.unrealircd.org/) (most supported open-source production IRCd).  
**IRCv3 design notes:** [Ergo](https://github.com/ergochat/ergo).  
**Not** a git/source fork of Unreal (GPL-2.0) — see [`docs/REFERENCE.md`](docs/REFERENCE.md).

## Status

Bootstrap / early scaffold. Speaks enough IRC to accept a client, register a nick, and echo on a test channel.

## Quick start

```bash
# terminal A — server
cargo run -p ircd -- --bind 127.0.0.1:6667

# terminal B — CLI smoke client (not a product GUI)
cargo run -p ircc -- --host 127.0.0.1 --port 6667 --nick otto --join '#test' --msg 'hello' --quit
```

Interactive: omit `--quit` and type `/join #test`, `/msg #test hi`, or bare lines (PRIVMSG to joined channel).

## Workspace

| Crate | Role |
|-------|------|
| `ircd` | Binary — listen loop, CLI |
| `ircd-core` | Protocol parsing helpers + shared types |

## Board

Tasks project: [Mark × Cody — IRC client](https://tasks.decisionsciencecorp.com/admin/project.php?id=48)  
Landscape research: [Doc #972](https://tasks.decisionsciencecorp.com/admin/doc.php?id=972)

## License

MIT — Decision Science Corp. Upstream references retain their own licenses; we do not redistribute their source here.
