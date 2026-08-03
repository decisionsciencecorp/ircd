# Reference strategy

## Decision (Mark, 2026-08-02)

Take the **most supported open-source IRCd** as the north-star feature/ops model and **implement a clean-room server in Rust** under Decision Science Corp.

| Role | Project | Why |
|------|---------|-----|
| **Primary reference (ops + feature breadth)** | [UnrealIRCd](https://www.unrealircd.org/) ([GitHub](https://github.com/unrealircd/unrealircd)) | Most widely deployed / actively maintained production IRCd; modular; strong IRCv3; battle-tested at network scale. |
| **Modern IRCv3 design reference** | [Ergo](https://github.com/ergochat/ergo) | Designed around IRCv3 (tags, history, web-friendly transport). Read for architecture — not our runtime language. |
| **Minimal baseline** | [ircd-hybrid](https://github.com/ircd-hybrid/ircd-hybrid) | Scope check: required vs nice-to-have. |

## This is not a source fork

UnrealIRCd is **GPL-2.0**. This repository is a **new Rust codebase**. We:

- Study Unreal’s **behavior**, docs, config surface, and protocol feature set.
- Study Ergo for **IRCv3 patterns** (CHATHISTORY, message tags, WebSocket-oriented design).
- Do **not** copy Unreal (or Ergo) source into this tree.
- Do **not** treat `liamzdenek/ircd-rs` (last push 2016) as a base — it is abandoned.

See Tasks [Doc #972](https://tasks.decisionsciencecorp.com/admin/doc.php?id=972).

## v0 server goals (draft)

1. Single-node TCP + TLS IRCd.
2. NICK/USER/CAP registration, JOIN/PART/PRIVMSG, basic modes.
3. IRCv3 capability negotiation path (grow toward message-tags / server-time / CHATHISTORY).
4. Config shape familiar to Unreal admins where it does not fight Rust structure.
5. Clean handoff to Tauri/web + Swift clients on the Mark × Cody board (Tasks project 48).

## Config mapping (Unreal concepts → dsc-ircd TOML)

We use **TOML** (`config.example.toml`), not Unreal’s block language. Knobs map roughly:

| Unreal idea | dsc-ircd |
|-------------|---------|
| `me { name … }` | `[server] name` |
| MOTD file / `motd` | `[server] motd` (inline string; multi-line OK) |
| Admin block | `[server] admin_name`, `admin_email` |
| Nick length / channel length limits | `[server] max_nick_length`, `max_channel_length` |
| `listen { ip; port; }` | `[[listen]] bind = "ip:port"` |
| `listen { … options { tls; } }` + cert files | `[[listen]] tls = true` + `cert` / `key` |
| `oper { }` | `[oper]` (`enabled`, `name`, `password`) |
| Channel history / replay | `[history]` sqlite path + `CHATHISTORY LATEST` + JOIN auto-replay |
| NickServ-style accounts | `[[accounts]]` + IRCv3 **SASL PLAIN** (`AUTHENTICATE`) + `account-tag` |
| Connection class / flood | `[limits]` — `max_clients`, `max_clients_per_ip`, `flood_lines_per_window`, `flood_window_secs` |

CLI `--bind` / `--tls-bind` replace the listen list when present (handy for lab). Prefer `--config` for standing instances.


## Limits knobs (`[limits]`)

| Key | Default | Effect |
|-----|---------|--------|
| `max_clients` | 256 | Global concurrent connections |
| `max_clients_per_ip` | 32 | Per-IP connection cap |
| `max_line_bytes` | 8192 | Max IRC line including CR/LF |
| `max_channels` | 1024 | Server-wide channel count |
| `max_channels_per_client` | 64 | Channels one client may join |
| `max_members_per_channel` | 512 | Membership cap per channel |
| `max_topic_bytes` | 390 | Topic text octet cap |
| `flood_lines_per_window` / `flood_window_secs` | 30 / 10 | Recv flood guard |

History retention: `[history] max_per_channel`, `max_total_rows` (see A6).


## WebSocket (`[websocket]`)

| Key | Default | Effect |
|-----|---------|--------|
| `allowed_origins` | `[]` | Exact `Origin` allowlist; empty denies browser Origins |
| `allow_missing_origin` | `true` | Native clients with no Origin |
| `require_irc_subprotocol` | `true` | Require `Sec-WebSocket-Protocol: irc` |

Admission runs before the WS upgrade. Policy helpers: `ws_policy` / `evaluate_ws_handshake`.


## Standalone queries (C1)

Registered clients may issue: `PRIVMSG`/`NOTICE` (channel + nick), `NAMES`, `LIST`, `WHO`, `WHOIS`, `MOTD`, `VERSION`, `LUSERS`. Direct `PRIVMSG` to unknown nick → **401**; missing params → **461**. `NOTICE` stays silent on errors.


## Moderation (C2)

- `INVITE nick #chan` — ops (or any member when channel is not +i); numeric **341**; target receives INVITE; invite-only (+i) JOIN needs prior invite (**473** otherwise).
- `MODE #chan +b/-b mask` — nick or `nick!*@*` masks; JOIN of banned nick → **474**. `MODE #chan b` lists bans (**367/368**).
- `MODE #chan +i/-i` — invite-only.
- `PART #chan :reason` and `QUIT :reason` propagate reasons on the fanout line.
