# Client protocol

This is the wire behavior of dsc-ircd **0.1.0**. It is one server talking to clients. There is no server-to-server protocol.

The conformance scorecard, with test names, is [PROTOCOL-MATRIX.md](PROTOCOL-MATRIX.md). This page is the manual.

Lines are UTF-8 IRC messages terminated by CRLF on TCP. On WebSocket, one text frame is one or more lines; CR, LF, and CRLF are accepted as separators. The maximum line size, including CRLF, defaults to 8192 octets (`max_line_bytes`). A longer line closes the connection with `ERROR` and the reason `Input line too long`. A tag block over 4096 octets, or a tag block with an empty key or a broken escape, is numeric `417` and the connection stays up.

## Registration

A client is not in the network until both `NICK` and `USER` have been accepted and capability negotiation is closed.

```text
CAP LS 302
CAP REQ :message-tags server-time away-notify
CAP END
NICK ada
USER ada 0 * :Ada
```

`CAP` negotiation sets a hold. `CAP END` (or never sending `CAP LS` / `CAP REQ`) releases it. Welcome numerics then go out:

| Numeric | Meaning |
|---------|---------|
| `001`–`004` | Welcome, host, created, `RPL_MYINFO` |
| `005` | `ISUPPORT` |
| `375` / `372` / `376` | MOTD |

`004` currently advertises user-mode token `i` and channel-mode token `nt`. That line is narrower than reality. Trust `005` `CHANMODES` for channel modes. User modes are not implemented: a `MODE` aimed at a nick is ignored.

If `[server] password` is set, the client must send `PASS` before registration. A mismatch is `464`. An empty password means `PASS` is optional and ignored.

Registration must finish within `registration_timeout_secs` (default 60) or the server closes the connection.

### Nicks and channels

| Rule | Behavior |
|------|----------|
| Nick charset | ASCII letters, digits, `_`, `-` |
| Nick length | `max_nick_length` (default 30, hard cap 50) |
| Case | `CASEMAPPING=ascii` (ASCII fold only) |
| Channel prefix | `#` only (`CHANTYPES=#`) |
| Channel length | `max_channel_length` (default 50), including `#` |
| Channel charset | No spaces, commas, or control characters |

The first client to join a new channel receives channel operator (`+o`). Nick changes keep the same connection identity, so ops stay with the connection rather than the old nick string.

`ISUPPORT` also includes `UTF8ONLY` and `UTF8MAPPING=rfc8265`. Nick validation is still the ASCII charset above. Do not send a non-ASCII nick and expect it to register.

## ISUPPORT (`005`)

Always:

```text
CASEMAPPING=ascii
CHANTYPES=#
PREFIX=(o)@
NICKLEN=<max_nick_length>
CHANNELLEN=<max_channel_length>
CHANMODES=b,,,nti
NETWORK=DSC
UTF8MAPPING=rfc8265
UTF8ONLY
CLIENTTAGDENY=
TARGMAX=NAMES:1,LIST:1,KICK:1,WHOIS:1,PRIVMSG:4,NOTICE:4,INVITE:0
```

`PREFIX=(o)@` means channel op is `@` and there is no voice or halfop.

`CHANMODES=b,,,nti` means:

- Type A (list): `b`
- Type B and C: none
- Type D (flag): `n`, `t`, `i`

When history is enabled, these tokens are added:

```text
CHATHISTORY=200
MSGREFTYPES=msgid,timestamp
```

`TARGMAX` is enforced. `NAMES`, `LIST`, `KICK`, and `WHOIS` take one target. `PRIVMSG` and `NOTICE` accept up to four. `INVITE:0` means the server does not advertise a multi-target invite form; send one nick and one channel.

## Capabilities

`CAP` subcommands: `LS`, `LIST`, `REQ`, `END`. Anything else is `410`.

`CAP REQ` is atomic. One unknown token, or an attempt to disable `cap-notify`, `NAK`s the whole request and changes nothing. A leading `-` disables a capability you already enabled, except `cap-notify`, which stays on.

| Capability | When advertised | What it does here |
|------------|-----------------|-------------------|
| `cap-notify` | always | New caps would be announced. Clients cannot turn this off. |
| `message-tags` | always | Tags on `PRIVMSG` / `NOTICE` / `TAGMSG`. Client-only tags (`+tag`) are relayed. A tag block that does not fit is `417`. |
| `server-time` | always | `time` tag on relayed lines, as an IRC timestamp. |
| `account-tag` | always | `account` tag on messages from a SASL-authenticated client, and WHOIS `330`. |
| `batch` | always | `CHATHISTORY` replies are wrapped in `BATCH +/-` of type `draft/chathistory`, with `@batch=` on the inner lines. |
| `away-notify` | always | `AWAY` / unaway is sent to other clients who share a channel. Not echoed to yourself. |
| `sasl=PLAIN` | at least one `[[accounts]]` entry | `AUTHENTICATE` with mechanism `PLAIN` only. |
| `draft/chathistory` | history store enabled | Negotiating it suppresses JOIN auto-replay. The `CHATHISTORY` verb itself follows the history store, not only this cap. |

Capabilities that are **not** advertised (`multi-prefix`, `echo-message`, `account-notify`, `extended-join`, `labeled-response`, `setname`, `sasl` mechanisms other than PLAIN, STS, and others) `NAK` if requested. Do not treat a `NAK` as a server bug.

## Commands

Verbs are ASCII case-insensitive. An unknown verb is `421`. A known verb with too few parameters is `461`, except `NOTICE`, which stays quiet on errors.

| Command | Behavior |
|---------|----------|
| `PING` | `PONG` with the token. A bare `PING` with no token is `409`. |
| `QUIT` | Fanout includes the reason. The server sends `ERROR` and closes. |
| `JOIN` | `#` channels. Creates the channel and grants `+o` to the founder. Respects `+i`, `+b`, per-client and per-channel caps. Channel keys (`+k`) are not implemented; a key parameter is not a password check. |
| `PART` | Reason is included on the fanout. Parting a channel you are not on is `442`. |
| `PRIVMSG` | Channel or nick. Channel send requires membership when `+n` is set (the default). Unknown nick is `401`. |
| `NOTICE` | Same routing as `PRIVMSG`. Failures produce no numeric. |
| `TAGMSG` | Tag-only message. Requires `message-tags`. |
| `TOPIC` | Read with no text. Set with text. `+t` (default) limits changes to channel ops. Control characters are rejected. Length is `max_topic_bytes`. |
| `NAMES` | One channel. Ops are prefixed with `@`. Long lists are split to fit `max_line_bytes`. |
| `LIST` | Optional single channel filter. `321` / `322` / `323`. |
| `WHO` | Channel or mask. Flags include `H` or `G` (here / gone) and `@` for ops. WHOX is not advertised and not implemented. |
| `WHOIS` | `311`, `301` when away, `312`, `319`, `318`. `313` when the nick is an operator. `330` when the nick has a SASL account. |
| `INVITE` | Ops may invite. On a channel that is not `+i`, any member may invite. Success is `341` and the target receives `INVITE`. `403` unknown channel, `442` you are not on it, `482` not op when ops are required, `401` no such nick, `443` already on the channel. |
| `KICK` | Channel op or IRC operator. Not op is `482`. Reason is on the wire. Kick does not ban. |
| `MODE` | See below. |
| `AWAY` | With text: set away, reply `306`. With no text: clear, reply `305`. WHOIS and a `PRIVMSG` to an away nick include `301`. `WHO` uses `G` while away and `H` otherwise. |
| `MOTD` | `375` / `372` / `376`. |
| `VERSION` | Server version string. |
| `LUSERS` | Counts of clients and channels. |
| `ADMIN` | `257` / `258` / `259` from the admin fields. |
| `USERHOST` | `302`. |
| `ISON` | `303`. |
| `TIME` | `391`. |
| `INFO` | `371` / `374`. |
| `OPER` | See [OPS-HANDBOOK.md](OPS-HANDBOOK.md). Success `381`. Bad password `464`. O-line disabled `491`. |
| `KILL` | IRC operators only. Others get `481`. The target receives `ERROR` and is disconnected. |
| `WALLOPS` | IRC operators only. Delivered to other operators. |
| `CHATHISTORY` | See below. |
| `AUTHENTICATE` | SASL. See below. |
| `PASS` | Connection password during registration. |

`PRIVMSG` and `NOTICE` to a channel you are not in fail when `+n` is set. `NOTICE` still does not send an error numeric.

## Channel modes

New channels start as `+nt`: no external messages, topic restricted to ops.

| Mode | Kind | Effect |
|------|------|--------|
| `+o` / `-o` | param, nick | Grant or remove channel operator. Target must be on the channel or the reply is `441`. |
| `+n` / `-n` | flag | External messages. On by default. |
| `+t` / `-t` | flag | Topic restricted to ops. On by default. |
| `+i` / `-i` | flag | Invite-only. Uninvited `JOIN` is `473`. |
| `+b` / `-b` | param, mask | Ban. `JOIN` of a banned nick is `474`. `MODE #chan b` (no mask) lists bans as `367` and ends with `368`. |
| query | `MODE #chan` | `324` with the current mode string. |

Several parameter modes may be set in one command. `MODE #chan +oo alice bob` consumes one nick per `o`.

Ban masks are the nick, or a simple `nick!*@*` style mask. There is no `+e` exception list and no `+I` invite-exception list.

Not implemented, and not in `PREFIX` or `CHANMODES`: `+v` voice, `+h` halfop, `+k` key, `+l` limit, `+s`, `+p`, `+m`. The operations handbook used to mention voice. This server does not have it.

Only a channel op or an IRC operator may change modes. Anyone else gets `482`.

User modes (`MODE yournick +i` and similar) are ignored. No numeric is returned.

## SASL PLAIN

Advertised only when `[[accounts]]` is non-empty, as `sasl=PLAIN`.

The client must `CAP REQ sasl` before `AUTHENTICATE`. Without the cap, the server replies `904`.

`AUTHENTICATE PLAIN` starts the exchange. The payload is standard SASL PLAIN (`authorization NUL authentication NUL password`), base64-encoded. The server accepts IRCv3 chunking at 400 octets per line and rejects a total above 1024. Success is `903`. Failure is `904`. Abort is `906`. A second authentication after success is `907`. An offered mechanism other than `PLAIN` is `905`.

When `require_tls_for_auth` or `production` is set, `AUTHENTICATE` on a plaintext (or non-WSS) connection is refused.

The account name is then visible to clients that enabled `account-tag`, and on `WHOIS` as `330`.

## CHATHISTORY

Available when `[history] enabled` is true. Each query returns at most 200 messages. The default limit when the client omits one is 50.

Subcommands:

| Subcommand | Shape |
|------------|--------|
| `LATEST` | `CHATHISTORY LATEST #chan [selector] [limit]` |
| `BEFORE` | `CHATHISTORY BEFORE #chan <selector> [limit]` |
| `AFTER` | `CHATHISTORY AFTER #chan <selector> [limit]` |
| `AROUND` | `CHATHISTORY AROUND #chan <selector> [limit]` — selector is required |
| `BETWEEN` | `CHATHISTORY BETWEEN #chan <a> <b> [limit]` — both selectors must be the same kind |
| `TARGETS` | `CHATHISTORY TARGETS * [limit]` — channels that have history |

Selectors are a `msgid` or a timestamp (`MSGREFTYPES=msgid,timestamp`). `*` means an open end where the subcommand allows it. `AROUND` rejects `*`. `BETWEEN` rejects mixed msgid and timestamp bounds.

The client must be on the channel. Replies are normal `PRIVMSG` lines. If the client enabled `batch`, they are wrapped:

```text
:server BATCH +chathist draft/chathistory
@batch=chathist;msgid=…;time=… :nick!user@host PRIVMSG #chan :text
:server BATCH -chathist
```

JOIN auto-replay sends the last `auto_replay_on_join` messages (default 50, max 200) to a joining client who did **not** negotiate `draft/chathistory`.

## Message tags

With `message-tags`, a client may send tags on `PRIVMSG`, `NOTICE`, and `TAGMSG`. Client-only tags (the name starts with `+`) are relayed to other clients. The server adds `msgid`, `time` (when `server-time` is on), `account` (when `account-tag` is on and the sender authenticated), and `batch` on history playback.

Tag values are escaped on relay per the IRCv3 message-tags escaping rules.

## Away notify

`AWAY :lunch` sets the status. `AWAY` with no parameter clears it. Clients who share a channel and who enabled `away-notify` see the change. The sender does not get a copy of their own notify. `WHO` and `WHOIS` reflect the same flag even for clients that did not enable the capability.

## WebSocket

Text frames carry IRC. The subprotocol is `irc` when `require_irc_subprotocol` is true (the default). Origin policy is in [configuration.md](configuration.md). Admission limits (global clients, per-IP, handshake cap) run before the upgrade is accepted.

## Intentionally absent

These are product boundaries, not unfinished claims:

- Server-to-server linking and multi-server history
- NickServ, ChanServ, or any services protocol inside this process
- Channel keys, voice, halfop, ban exceptions, invite exceptions
- WHOX
- Any capability not listed in the table above
- Hashed passwords, multiple O-lines, and oper classes

If a client needs services, run Atheme or Anope beside this daemon. That decision is recorded in [REFERENCE.md](REFERENCE.md) and [OPS-HANDBOOK.md](OPS-HANDBOOK.md).
