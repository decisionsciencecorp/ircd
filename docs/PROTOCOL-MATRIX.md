# Full client protocol acceptance matrix — dsc-ircd

**Bar (Mark, 2026-08-03):** full Modern IRC + advertised IRCv3 — **not** a v0 subset.  
**Tip probed:** `61d9043` (F0 matrix + F1a AWAY/`away-notify`)  
**Program:** Tasks [Doc #976](https://tasks.decisionsciencecorp.com/admin/doc.php?id=976) · Matrix [Doc #977](https://tasks.decisionsciencecorp.com/admin/doc.php?id=977) · Epic [#2251](https://tasks.decisionsciencecorp.com/admin/view.php?id=2251) · F0 [#2252](https://tasks.decisionsciencecorp.com/admin/view.php?id=2252)  
**Standing coverage:** [#2214](https://tasks.decisionsciencecorp.com/admin/view.php?id=2214) (≥90% tarpaulin after code slices)  
**irctest allowlist:** [`IRCTEST.md`](IRCTEST.md)

## Status legend

| Status | Meaning |
|--------|---------|
| **PASS** | Implemented; in-tree e2e (and curated irctest where listed) defend the claim |
| **PARTIAL** | Present but incomplete vs Modern/IRCv3 / advertised contract |
| **MISSING** | Needed for full client compliance; 421 or silent/wrong today |
| **NON-GOAL** | Explicitly out of this program (not a protocol skip disguised as “later”) |

## Non-goals (do not mark as protocol skips)

| Surface | Why out |
|---------|---------|
| Server-to-server linking / multi-node history | Network topology; separate epic if needed |
| In-daemon NickServ / ChanServ | Atheme/Anope **beside** dsc-ircd ([#2160](https://tasks.decisionsciencecorp.com/admin/view.php?id=2160)) |
| Unreal module ecosystem parity | Ops reference only; not a GPL port checklist |
| Public zero1 bind | Ops ([#2210](https://tasks.decisionsciencecorp.com/admin/view.php?id=2210)); parallel after Gate A |
| Ban/invite **exceptions** (`+e` / `+I`) | Optional; not claimed in `optional_behaviors` / IRCTEST allowlist |
| Caps we do **not** advertise (`multi-prefix`, `echo-message`, `account-notify`, `extended-join`, `labeled-response`, `setname`, `MONITOR`, STS, …) | NAK on REQ is correct until we implement + advertise |

---

## A. Registration, CAP, liveness

| Surface | Status | Tip notes | Phase | In-tree / irctest |
|---------|--------|-----------|-------|-------------------|
| NICK + USER → 001–004 | PASS | Registration path | — | `protocol_*_e2e` |
| 005 ISUPPORT | PASS | CASEMAPPING=ascii, CHANTYPES, PREFIX=(o)@, NICKLEN, CHANNELLEN, CHANMODES=,,,nt, NETWORK, UTF8*, WHOX, CLIENTTAGDENY, TARGMAX; `CHATHISTORY=<n>` when history on | — | register / C7 |
| 005 `MSGREFTYPES` | MISSING | Omitted while LATEST ignores refs — required when F3 lands refs | F3 | — |
| CAP LS/LIST/REQ/END + atomic NAK | PASS | Mixed unknown → full NAK | — | `protocol_c3_e2e`, irctest CAP probes |
| `cap-notify` | PASS | Always on; cannot disable | — | CAP e2e |
| PING / PONG | PASS | Bare PING → 409 | — | curated `testPing*` |
| QUIT + ERROR Closing Link | PASS | Reason preserved | — | curated `testQuit*` |
| 421 unknown command | PASS | | — | routes e2e |
| 461 need more params (core verbs) | PARTIAL | Present on many paths; residual gaps → F1d | F1d | — |
| PASS (server password) | NON-GOAL* | Unsupported when unset; irctest controller allowlist. *If we add connection password later, treat as config feature — not blocking client protocol claim.* | — | IRCTEST.md |
| Flood / line limits / disconnect cleanup | PASS | Gate A | — | `gate_a_acceptance_e2e` |

---

## B. Messaging & channels

| Surface | Status | Tip notes | Phase | Evidence |
|---------|--------|-----------|-------|----------|
| Channel PRIVMSG | PASS | Membership / `+n` checks | — | curated Privmsg / JOIN |
| Direct user PRIVMSG | PASS | | — | curated `testPrivmsgToUser` |
| NOTICE (channel + user) | PASS | No error replies (correct) | — | protocol e2e |
| TAGMSG | PASS | Requires `message-tags`; C7 | — | `protocol_c7_e2e` |
| Client-only `+` tag relay | PASS | C7 | — | `protocol_c7_e2e` |
| JOIN / PART / NICK fanout | PASS | ClientId identity; ascii casemap | — | Gate A / curated |
| TOPIC | PASS | `+t` op check | — | protocol e2e |
| KICK | PASS | Authoritative membership | — | protocol e2e |
| MODE `+o` / `+n` / `+t` | PASS | Multi-arg `+oo` (C8) | — | `protocol_c8_e2e` |
| MODE `+i` + INVITE list | PARTIAL | Invite-only JOIN + invite consume work; INVITE numerics/edge cases incomplete | F1b | session INVITE/`mode_i` |
| MODE `+b` bans | PARTIAL | Add/remove/list (367/368) + JOIN deny; mask matching simple; no `+e`/`+I` (NON-GOAL) | F1b | cmd_precheck bans |
| User modes | MISSING | Silently ignored (except oper path separate) | F4 | — |
| Standalone NAMES | PARTIAL | Works; wire split / multi-channel TARGMAX honesty TBD | F1c | C1 / curated JoinNamreply |
| LIST | PARTIAL | Basic; filters/limits TBD | F1c | C1 |
| WHO | PARTIAL | Channel/mask; flags `H`/`H@` only; **WHOX advertised but not implemented** | F1c | C1 |
| WHOIS | PARTIAL | 311/312/319/318; no account/away/oper detail | F1c | C1 |
| INVITE verb | PARTIAL | 341 + delivery; non-member error shape weak; no invite-notify | F1b | session |
| AWAY | PASS | Set/clear → 306/305; WHOIS/PRIVMSG 301; WHO `G`/`H` | — | `protocol_f1a_e2e` |
| USERHOST / ISON / TIME / INFO | MISSING | → 421 | F1d | — |

---

## C. Advertised IRCv3 capabilities

| Cap | Advertised? | Status | Tip notes | Phase |
|-----|-------------|--------|-----------|-------|
| `cap-notify` | always | PASS | | — |
| `message-tags` | always | PARTIAL | TAGMSG + `+` relay OK; **escape/size/417** incomplete | F2a |
| `server-time` | always | PARTIAL | Timestamp adapt OK; depends on full tag rules | F2a / F2b |
| `account-tag` | always | PARTIAL | On authed PRIVMSG/NOTICE/TAGMSG fanout; not all user-originated / caused numerics | F2b |
| `batch` | always | PARTIAL | CHATHISTORY framing + `@batch=`; nesting/vocabulary incomplete | F2b / F3 |
| `sasl=PLAIN` | when accounts | PARTIAL | Happy path; chunking / 905 / 907 / reauth edges | F2c |
| `draft/chathistory` | when history | PARTIAL | **LATEST only**; BATCH type; auto-replay still on; no MSGREFTYPES | F3 |
| `away-notify` | always | PASS | Shared-channel notify on set/clear/join; not to self | — |

Do **not** advertise a new cap in the same commit that leaves behavior incomplete.

---

## D. CHATHISTORY (command + ISUPPORT)

| Item | Status | Phase |
|------|--------|-------|
| CAP `draft/chathistory` | PARTIAL (LATEST) | F3 |
| `005 CHATHISTORY=<n>` | PASS (when history on; max 200) | — / F3 expand |
| `005 MSGREFTYPES` | MISSING | F3 |
| Subcommand LATEST | PASS | — |
| BEFORE / AFTER / AROUND / BETWEEN / TARGETS | MISSING (explicit reject today) | F3 |
| msgid / timestamp refs | MISSING (ignored) | F3 |
| Suppress JOIN auto-replay when negotiated | MISSING | F3 |
| Batch type vocabulary | PARTIAL | F2b / F3 |

---

## E. Oper / moderation (client-visible)

| Surface | Status | Phase |
|---------|--------|-------|
| OPER / ADMIN | PASS (lab) | — |
| Channel op kick/topic/mode subset | PASS | — |
| KILL | MISSING | F4 |
| WALLOPS | MISSING | F4 |
| Claimed CHANMODES completeness | PARTIAL (`,,,nt` + runtime `i`/`b` not fully reflected in 005) | F1b / F4 |

---

## F. Transport (client protocol adjacent)

| Surface | Status | Notes |
|---------|--------|-------|
| TCP plaintext | PASS | |
| TLS | PASS | Auth policy Gate A |
| WebSocket / WSS | PASS (daemon) | irctest WS harness **UNSUPPORTED** (controller allowlist) → F5 |
| WS CRLF normalize | PASS | C13 |

---

## G. Curated irctest gate (expand in F5)

**In curated `-k` today:** Ping*, Privmsg*, JoinNamreply, Cap invalid/NoReq, Quit*, Part.

**Advertised caps not yet in curated `-k`:** message-tags / server-time / account-tag / batch / sasl / chathistory modules — defended by in-tree e2e until promoted (never hide via `-m not …`).

**Allowlist (intentional):** WS harness, PASS, services, STS, unadvertised caps, `+e`/`+I`, Ergo-specific — see `IRCTEST.md`.

---

## Execute map (children on list #372)

| Phase | Task | Closes matrix rows |
|-------|------|--------------------|
| F0 | #2252 | This document (freeze) |
| F1a | #2253 | AWAY + `away-notify` |
| F1b | #2254 | INVITE / `+i` / `+b` honesty |
| F1c | #2255 | WHO / WHOIS / NAMES / LIST / WHOX |
| F1d | #2256 | USERHOST / ISON / TIME / INFO / residual numerics |
| F2a | #2257 | message-tags escape / size / 417 |
| F2b | #2258 | account-tag + server-time breadth; batch invariants |
| F2c | #2259 | SASL PLAIN edges |
| F3 | #2260 | CHATHISTORY complete + MSGREFTYPES |
| F4 | #2261 | KILL / WALLOPS / CHANMODES claim |
| F5 | #2262 | Expand irctest + WS harness |
| F6 | #2263 | Docs #973/#974/#976 closeout |

**Order:** F0 → F1 → F2 → F3 → F4 → F5 → F6. Tag foundation (F2a) before CHATHISTORY refs (F3).

---

## Must-answer (F0)

1. **Does the matrix list every Modern IRC surface we will claim, with pass/fail/missing vs tip?** Yes — sections A–E above vs tip `409e873`.
2. **Is #2174 retargeted so “done” means full bar, not CLI v0 smoke?** Yes — tracker alias; closes with this artifact.
3. **Are Non-goals explicit?** Yes — table at top + IRCTEST allowlist; S2S / in-daemon services / Unreal modules / zero1 bind are not protocol excuses.
