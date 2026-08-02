# ircd — DSC IRC server (Rust)

Clean-room **Rust** IRC daemon for Decision Science Corp’s Mark × Cody IRC rebuild.

**Reference model:** [UnrealIRCd](https://www.unrealircd.org/) (most supported open-source production IRCd).  
**IRCv3 design notes:** [Ergo](https://github.com/ergochat/ergo).  
**Not** a git/source fork of Unreal (GPL-2.0) — see [`docs/REFERENCE.md`](docs/REFERENCE.md).

## Status

Bootstrap. Speaks enough IRC to accept a client, register a nick, and echo on a test channel. Supports **plaintext** and **TLS** (rustls) binds, IRCv3 **CAP**, multi-user channels, and a minimal ops toolkit (**OPER**, channel **+o/+n/+t**, **TOPIC**, **KICK**, **MODE**).

## Quick start

```bash
# terminal A — plaintext lab (CLI) or --config ./config.example.toml
cargo run -p ircd -- --bind 127.0.0.1:6667

# terminal B — CLI smoke client (not a product GUI)
cargo run -p ircc -- --host 127.0.0.1 --port 6667 --nick otto --join '#test' --msg 'hello' --quit
```

### TLS (lab self-signed)

```bash
# once
cargo run -p ircd -- gen-cert --out ./certs --cn ircd.dsc.local

# terminal A — plaintext + TLS
cargo run -p ircd -- \
  --bind 127.0.0.1:6667 \
  --tls-bind 127.0.0.1:6697 \
  --tls-cert ./certs/cert.pem \
  --tls-key ./certs/key.pem

# terminal B — TLS smoke (accepts any cert; lab only)
cargo run -p ircc -- --tls --host 127.0.0.1 --port 6697 --nick otto \
  --join '#test' --msg 'hello tls' --quit
```

Production networks should use real certificates; `gen-cert` is for local lab only.  
`--tls` on `ircc` **disables certificate verification** — never point that at the public internet as a trust model.

CAP smoke:

```bash
cargo run -p ircc -- --cap --host 127.0.0.1 --port 6667 --nick otto \
  --join '#test' --msg 'hello' --quit
```

Interactive: omit `--quit` and type `/join #test`, `/msg #test hi`, or bare lines (PRIVMSG to joined channel).

## Workspace

| Crate | Role |
|-------|------|
| `ircd` | Binary — listen loop, TLS, session |
| `ircd-core` | Protocol parsing helpers + shared types |
| `ircc` | CLI smoke client |

## Board

Tasks project: [Mark × Cody — IRC client](https://tasks.decisionsciencecorp.com/admin/project.php?id=48)  
Landscape research: [Doc #972](https://tasks.decisionsciencecorp.com/admin/doc.php?id=972)

## License

MIT — Decision Science Corp. Upstream references retain their own licenses; we do not redistribute their source here.
