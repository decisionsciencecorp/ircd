# Moderation & ops handbook — day-one public invite

You’re holding a kick or a ban because someone is wrecking a channel. This is the map. Network kill is a different belt — read the escalate ladder before you pull it.

**Daemon:** [`decisionsciencecorp/ircd`](https://github.com/decisionsciencecorp/ircd) (`dsc-ircd`).  
**Services (locked):** NickServ/ChanServ live in **Atheme or Anope beside** the IRCd — not a mini-NickServ inside the daemon. Tasks [#2160](https://tasks.decisionsciencecorp.com/admin/view.php?id=2160) · Program [Doc #973](https://tasks.decisionsciencecorp.com/admin/doc.php?id=973).

---

## Two toolbelts

| Belt | Who | Job |
|------|-----|-----|
| Channel modes + KICK / TOPIC / INVITE | Channel ops (`+o`) | Room-level discipline. In the daemon. |
| OPER + KILL + WALLOPS | IRC operators (O-line) | Network-level. In the daemon. |
| NickServ / ChanServ / friends | Services package | Register, identify, channel ownership. **Beside** the daemon. |

Built-in SASL PLAIN accounts in `config.toml` are lab/bootstrap. Production register/identify is the services package. Do not teach end users “edit the server config to get a nick.”

---

## Roles

**Channel op (`+o`):** first creator of a channel gets ops in v0. Kick, topic (when `+t`), `+b` / `+i` / invite, hand out more ops. Everyday belt.

**IRC operator (`OPER`):** network staff. Kick where you have presence, `KILL` a nick off the net, `WALLOPS` other opers. Wrong password → 464. Disabled O-line → 491. Production posture: OPER and SASL ride TLS — plaintext auth is a non-starter.

**Services admin:** Atheme/Anope staff. Nick fights, founder recovery, drop/restore. Escalate here when the problem is *identity* or *ownership*, not a one-channel scuffle.

---

## Register & identify

Day-one public invite assumes services sit next to `dsc-ircd`. Exact hostname waits on naming [#2159](https://tasks.decisionsciencecorp.com/admin/view.php?id=2159) and zero1 confirm [#2210](https://tasks.decisionsciencecorp.com/admin/view.php?id=2210).

1. Connect over **TLS**.
2. Pick a nick. Taken? Pick another — don’t NICK-spam services.
3. Register with NickServ (Atheme vs Anope differ in flags, not in the idea).
4. Identify on later connects (or SASL once wired to the client).
5. Channel you mean to keep → register with ChanServ once the name is the long-term one.

Daemon-only SASL (`[accounts]`) is for lab smoke. Rotate those passwords before any public bind.

Optional connection `PASS`: wrong password blocks registration with 464. That’s a front door, not a ban.

---

## Channel discipline

### Kick

```
KICK #channel nick :reason here
```

Need channel ops (or OPER). Not op → 482. Reasons show up on the wire — write them like you’ll see them in a screenshot tomorrow.

Kick removes them from the channel. They can rejoin unless you ban.

### Ban

```
MODE #channel +b nick!user@host
```

Tightest mask that stops the abuse without collateral. List: `MODE #channel +b`. Remove: `-b`. Banned JOIN → 474. Ban ≠ network kill.

### Invite-only

```
MODE #channel +i
INVITE nick #channel
```

`+i` blocks uninvited joins (473). Not op → 482.

### Topic

```
TOPIC #channel :plain text, no control characters
```

With `+t` (default), only ops set topic. Control characters are rejected — don’t paste binary into a topic.

### Voice / more ops

```
MODE #channel +o othernick
MODE #channel +v othernick
```

Hand out ops when you trust someone to kick *and* to stop kicking. Voice is the lighter lever.

---

## Network levers (OPER)

### Become an oper

```
OPER <oline-name> <password>
```

TLS when production security says so. Success → 381. O-line secrets live in server config — not NickServ passwords. Rotate them like root.

### Kill

```
KILL nick :reason
```

Oper-only. Non-opers → 481. Use for network abuse, open proxies mid-attack, compromised bots — **not** “I disagree in #general.” If kick + ban would do it, kick + ban.

### Wallops

```
WALLOPS :heads up for other opers
```

Oper-only. Coordinated response, not venting.

---

## Escalate — ladder, not panic

1. **One bad actor, one room** → kick → ban if they return → topic note if the room needs a rule reminder.
2. **Same actor, many rooms** → bans per channel, or OPER kill if they’re hopping faster than bans. WHOIS the nick!user@host before you kill.
3. **Stolen nick / account fight** → services staff. Daemon KILL does not transfer ownership.
4. **Server misconfig / auth outage / TLS down** → daemon ops + infra. Not a kick problem.
5. **Legal / safety / CSAM / credible threat** → stop playing IRC admin. Preserve logs. Escalate to Mark (and counsel if Mark says so). Do not “quietly MODE +b” and hope.

Unsure which belt? Ask in the oper channel before you KILL. A wrong kill is louder than a wrong kick.

---

## Lab vs production

| | Lab (NewDev) | Production (pending zero1) |
|--|--------------|----------------------------|
| Host | `64.95.11.220` — `docs/LAB.md` | Candidate zero1 — [#2210](https://tasks.decisionsciencecorp.com/admin/view.php?id=2210) |
| TLS | Self-signed OK for smoke | Real certs; clients verify |
| SASL | Config accounts (`alice` / `bob` lab) | Prefer services; rotate leftover config accounts |
| OPER | Lab O-line in LAB.md — change before public | Unique secrets; TLS required |
| Logs | `/root/ircd-lab/logs/ircd.log` | Prod path with deploy [#2193](https://tasks.decisionsciencecorp.com/admin/view.php?id=2193) |

Do not paste lab OPER passwords into public channels. Do not reuse lab O-lines on the community hostname.

---

## What we will not do in the daemon

- **No in-daemon NickServ.** Services stay beside the IRCd. Product decision, not a missing-feature ticket.
- **No silent drops** for supported moderation verbs — you get a numeric. Client lies about success → fix the client.
- **No Wikipedia ops manual.** This stays short on purpose. Unreal module folklore is reference material, not our v0 checklist.

---

## Cheat sheet

```
OPER name pass
KILL nick :reason
WALLOPS :message

KICK #chan nick :reason
MODE #chan +b nick!user@host
MODE #chan -b nick!user@host
MODE #chan +b
MODE #chan +i
INVITE nick #chan
TOPIC #chan :text
MODE #chan +o nick
WHOIS nick
```

Services commands are package-specific — keep their one-pager next to this file when the services host is live.

---

## Related

- Canonical Tasks copy: [Doc #984](https://tasks.decisionsciencecorp.com/admin/doc.php?id=984)
- Program: [Doc #973](https://tasks.decisionsciencecorp.com/admin/doc.php?id=973)
- Protocol program: [Doc #976](https://tasks.decisionsciencecorp.com/admin/doc.php?id=976)
- Lab: `docs/LAB.md`
- Deploy / DNS blocked on Mark: [#2210](https://tasks.decisionsciencecorp.com/admin/view.php?id=2210) · [#2192](https://tasks.decisionsciencecorp.com/admin/view.php?id=2192) · [#2193](https://tasks.decisionsciencecorp.com/admin/view.php?id=2193)
- Post-game audit: [Doc #979](https://tasks.decisionsciencecorp.com/admin/doc.php?id=979)

*Mark Hopkins voice kit v2.0 · private-collaboration · Tasks #2196*
