# Operations

You are holding a kick or a ban because someone is wrecking a channel. This is the map for dsc-ircd. Network kill is a different belt. Read the ladder at the bottom before you use it.

The daemon is this repository. NickServ and ChanServ are not inside it. They live in Atheme or Anope beside the process. Deployment hostnames, certificates, and passwords live in your config file, not in this handbook.

Wire details and numerics are in [protocol.md](protocol.md). The public-bind checklist is [security.md](security.md).

## Three belts

| Belt | Who | Where it lives |
|------|-----|----------------|
| Channel modes, `KICK`, `TOPIC`, `INVITE` | Channel ops (`+o`) | This daemon |
| `OPER`, `KILL`, `WALLOPS` | The single O-line | This daemon |
| Register, identify, channel ownership | Services admins | Atheme or Anope next to the daemon |

`[[accounts]]` in the config is a bootstrap SASL table for tests. It is not how people get a nick. Do not tell users to ask an operator to edit the server config.

## Roles

**Channel op.** The first person to join a new channel gets `+o`. They can kick, set the topic while `+t` is on (the default), set `+b` and `+i`, invite, and grant `+o` to someone else. Ops belong to the connection, not the nick string. Renaming does not hand ops to whoever takes the old nick. Stealing a nick does not steal the channel.

**IRC operator.** One name and one plaintext password in `[oper]`. Success is `381`. A wrong password is `464`. `enabled = false` is `491`. An operator can kick where they are present, `KILL` a nick off the server, and `WALLOPS` other operators. On a public port, `OPER` belongs on TLS. Set `require_tls_for_auth` or `production`.

**Services admin.** Nick fights and founder recovery. `KILL` does not transfer an account. Send identity problems to services.

There is no voice (`+v`) and no halfop. `PREFIX` is `(o)@` only.

## Register and identify

When services are installed next to the daemon:

1. Connect with TLS.
2. Pick a nick. If it is taken, pick another.
3. Register with NickServ. The exact command depends on Atheme versus Anope.
4. Identify on later connections, or use SASL once the client and services are wired.
5. Register the channel with ChanServ when the name is the one you mean to keep.

Until that package exists, the only accounts are `[[accounts]]` and SASL PLAIN. Rotate those passwords before anyone but you can reach the port. See [security.md](security.md).

A `[server] password`, if set, is a connection front door. Wrong `PASS` is `464` and registration does not finish. It is not a ban and not an account.

## Channel discipline

New channels start as `+nt`: people outside the channel cannot send, and only ops can change the topic.

### Kick

```text
KICK #channel nick :reason here
```

You need `+o` or an O-line. Otherwise `482`. The reason is part of the fanout. Write it as if it will be screenshotted.

Kick removes them from the channel. They can rejoin until you ban.

### Ban

```text
MODE #channel +b nick
MODE #channel +b nick!*@*
```

Use the tightest mask that stops the abuse. List bans with `MODE #channel b` (`367`, then `368`). Remove with `MODE #channel -b nick!*@*`. A banned `JOIN` is `474`.

Bans are in memory. A restart clears them. There is no exception list (`+e`) and no invite-exception list (`+I`).

A ban is not a network kill.

### Invite-only

```text
MODE #channel +i
INVITE nick #channel
```

`+i` makes an uninvited join `473`. You must be an op to set the mode (`482` otherwise). On a channel that is not `+i`, any member may `INVITE`. Success is `341`, and the target sees `INVITE`. The invite is consumed on join.

### Topic

```text
TOPIC #channel :plain text
```

With `+t`, only ops can set it. Control characters are rejected. The length cap is `max_topic_bytes` (default 390).

### More ops

```text
MODE #channel +o othernick
MODE #channel -o othernick
```

Give ops to someone you trust to kick and to stop kicking. Several ops can be set at once: `MODE #channel +oo alice bob`.

`MODE #channel` with no flags replies `324` with the current modes.

## Network levers

### Become an operator

```text
OPER <oline-name> <password>
```

The name and password are `[oper]` in the config. Prefer TLS whenever the host is not localhost. The password is stored in cleartext. Keep the file mode `0600` and rotate it like a root password.

### Kill

```text
KILL nick :reason
```

Operators only. Anyone else gets `481`. The target is disconnected with `ERROR`.

Use this for network abuse: open proxies in the middle of an attack, compromised bots, a client that is ignoring channel bans by rejoining faster than you can type. If kick plus ban would do it, kick plus ban.

`KILL` does not ban the host. They can reconnect with another nick unless something else stops them.

### Wallops

```text
WALLOPS :heads up for other operators
```

Operators only. It is how you coordinate. It is not a second channel for arguments.

## Limits you will feel

These are config, not modes. Defaults are in [configuration.md](configuration.md).

| Symptom | Knob |
|---------|------|
| New connections dropped immediately | `max_clients`, `max_clients_per_ip` |
| Client kicked for talking too fast | `flood_lines_per_window`, `flood_window_secs` (default 30 lines / 10 seconds) |
| Join fails on a huge channel | `max_members_per_channel` (512) |
| User cannot join another channel | `max_channels_per_client` (64) |
| Server will not create another channel | `max_channels` (1024) |
| Unregistered socket disappears | `registration_timeout_secs` (60) |
| Quiet client disappears | `idle_timeout_secs` (off unless you set it) |

A full outbox (64 pending lines) drops messages for that one slow client. The channel does not wait for them.

## Escalate

1. One person, one room. Kick. Ban if they come back. Set the topic if the room needs the rule written down.
2. Same person, many rooms. Ban per channel. `KILL` only if they are hopping faster than bans can be set. `WHOIS` the nick before you kill.
3. Stolen nick or account fight. Services. A daemon `KILL` does not move ownership.
4. Server down, TLS expired, nobody can authenticate. That is the host, not a kick.
5. Legal, safety, credible threats. Stop playing IRC admin. Keep logs. Hand it to whoever owns safety for the network.

If you are unsure which belt you are holding, ask in the operator channel before `KILL`. A wrong kill is louder than a wrong kick.

## What this daemon will not do

- No NickServ inside the process.
- No silent success. A moderation command that fails returns a numeric. If a client shows success anyway, fix the client.
- No voice, no channel keys, no persistent bans across restart.

Services commands stay on the services one-pager, next to this file, once that package is actually installed.

## Cheat sheet

```text
OPER name pass
KILL nick :reason
WALLOPS :message

KICK #chan nick :reason
MODE #chan +b nick!*@*
MODE #chan -b nick!*@*
MODE #chan b
MODE #chan +i
MODE #chan -i
INVITE nick #chan
TOPIC #chan :text
MODE #chan +o nick
MODE #chan -o nick
WHOIS nick
```

## Related

- [protocol.md](protocol.md) — numerics and capabilities
- [security.md](security.md) — before a public bind
- [configuration.md](configuration.md) — the knobs named above
- [LAB.md](LAB.md) — private lab layout, without live passwords
