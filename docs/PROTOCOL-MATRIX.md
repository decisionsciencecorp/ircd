# Full client protocol acceptance matrix — dsc-ircd

**Bar (Mark, 2026-08-03):** full Modern IRC + advertised IRCv3 — **not** a v0 subset.  
**Tip probed:** F1–F4 landing (query verbs, tags/SASL edges, CHATHISTORY, KILL/WALLOPS)
**Program:** Tasks [Doc #976](https://tasks.decisionsciencecorp.com/admin/doc.php?id=976) · Matrix [Doc #977](https://tasks.decisionsciencecorp.com/admin/doc.php?id=977) · Epic [#2251](https://tasks.decisionsciencecorp.com/admin/view.php?id=2251)  
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
| Full WHOX field set | **Not advertised** — classic WHO 352 only (F1c honesty) |

---

## A. Registration, CAP, liveness

| Surface | Status | Tip notes | Phase | In-tree / irctest |
|---------|--------|-----------|-------|-------------------|
| NICK + USER → 001–004 | PASS | Registration path | — | `protocol_*_e2e` |
| 005 ISUPPORT | PASS | CASEMAPPING, CHANTYPES, PREFIX, NICKLEN, CHANNELLEN, CHANMODES=b,,,nti, NETWORK, UTF8*, CLIENTTAGDENY, TARGMAX; `CHATHISTORY` + `MSGREFTYPES` when history on; **no WHOX** | — | register / C7 / F slices |
| 005 `MSGREFTYPES` | PASS | `msgid,timestamp` when history enabled | F3 | `protocol_c7_e2e` / `protocol_f_slices_e2e` |
| CAP LS/LIST/REQ/END + atomic NAK | PASS | Mixed unknown → full NAK | — | `protocol_c3_e2e`, irctest CAP probes |
| `cap-notify` | PASS | Always on; cannot disable | — | CAP e2e |
| PING / PONG | PASS | Bare PING → 409 | — | curated `testPing*` |
| QUIT + ERROR Closing Link | PASS | Reason preserved | — | curated `testQuit*` |
| 421 unknown command | PASS | | — | routes e2e |
| 461 need more params (core verbs) | PASS | Core + F1d verbs | F1d | `protocol_f_slices_e2e` |
| PASS (server password) | PASS | Optional `server.password`; 464 when wrong; no-op when unset | F1d | session |
| Flood / line limits / disconnect cleanup | PASS | Gate A | — | `gate_a_acceptance_e2e` |

---

## B. Messaging & channels

| Surface | Status | Tip notes | Phase | Evidence |
|---------|--------|-----------|-------|----------|
| Channel PRIVMSG | PASS | Membership / `+n` checks | — | curated Privmsg / JOIN |
| Direct user PRIVMSG | PASS | | — | curated `testPrivmsgToUser` |
| NOTICE (channel + user) | PASS | No error replies (correct) | — | protocol e2e |
| TAGMSG | PASS | Requires `message-tags`; C7 | — | `protocol_c7_e2e` |
| Client-only `+` tag relay | PASS | Escape on relay (F2a) | F2a | `protocol_c7_e2e` |
| JOIN / PART / NICK fanout | PASS | ClientId identity; ascii casemap | — | Gate A / curated |
| TOPIC | PASS | `+t` op check | — | protocol e2e |
| KICK | PASS | Authoritative membership | — | protocol e2e |
| MODE `+o` / `+n` / `+t` | PASS | Multi-arg `+oo` (C8) | — | `protocol_c8_e2e` |
| MODE `+i` + INVITE list | PASS | Invite-only JOIN + invite consume; INVITE 403/442/482/401/443/341 | — | `protocol_c2_e2e` / `protocol_f1b_e2e` |
| MODE `+b` bans | PASS | Add/remove/list (367/368) + JOIN 474; simple masks; no `+e`/`+I` (NON-GOAL) | — | `protocol_c2_e2e` |
| User modes | PARTIAL | Silent ignore for non-oper umodes (oper path separate) | F4 | — |
| Standalone NAMES | PASS | Wire-split to `max_line_bytes`; TARGMAX=NAMES:1 | F1c | curated JoinNamreply / F slices |
| LIST | PASS | Basic channel filter | F1c | `protocol_f_slices_e2e` |
| WHO | PASS | Channel/mask; `H`/`G` (+ `@` for ops); username from USER; **WHOX not advertised** | F1c | F slices |
| WHOIS | PASS | 311/301/312/313/330/319/318 | F1c | F slices |
| INVITE verb | PASS | 341 + delivery; 403/442/482/401/443; invite-notify unadvertised (ok) | — | `protocol_f1b_e2e` |
| AWAY | PASS | Set/clear → 306/305; WHOIS/PRIVMSG 301; WHO `G`/`H` | — | `protocol_f1a_e2e` |
| USERHOST / ISON / TIME / INFO | PASS | 302 / 303 / 391 / 371+374 | F1d | `protocol_f_slices_e2e` |

---

## C. Advertised IRCv3 capabilities

| Cap | Advertised? | Status | Tip notes | Phase |
|-----|-------------|--------|-----------|-------|
| `cap-notify` | always | PASS | | — |
| `message-tags` | always | PASS | TAGMSG + `+` relay; escape; tag-block size → **417** | F2a |
| `server-time` | always | PASS | Timestamp adapt on bus lines | F2a / F2b |
| `account-tag` | always | PASS | On authed PRIVMSG/NOTICE/TAGMSG fanout; WHOIS 330 | F2b |
| `batch` | always | PASS | CHATHISTORY framing + `@batch=` with CRLF-correct BATCH ± | F2b / F3 |
| `sasl=PLAIN` | when accounts | PASS | Chunked AUTHENTICATE; 905/906/907; TLS-only when required | F2c |
| `draft/chathistory` | when history | PASS | LATEST/BEFORE/AFTER/AROUND/BETWEEN/TARGETS; MSGREFTYPES; JOIN auto-replay suppressed when negotiated | F3 |
| `away-notify` | always | PASS | Shared-channel notify on set/clear/join; not to self | — |

Do **not** advertise a new cap in the same commit that leaves behavior incomplete.

---

## D. CHATHISTORY (command + ISUPPORT)

| Item | Status | Phase |
|------|--------|-------|
| CAP `draft/chathistory` | PASS | F3 |
| `005 CHATHISTORY=<n>` | PASS (when history on; max 200) | — |
| `005 MSGREFTYPES` | PASS (`msgid,timestamp`) | F3 |
| Subcommand LATEST | PASS | — |
| BEFORE / AFTER / AROUND / BETWEEN / TARGETS | PASS | F3 |
| msgid / timestamp refs | PASS | F3 |
| Suppress JOIN auto-replay when negotiated | PASS | F3 |
| Batch type vocabulary | PASS (`draft/chathistory` + `@batch=`) | F2b / F3 |

---

## E. Oper / moderation (client-visible)

| Surface | Status | Phase |
|---------|--------|-------|
| OPER / ADMIN | PASS (lab) | — |
| Channel op kick/topic/mode subset | PASS | — |
| KILL | PASS | Oper-only; ERROR to victim | F4 |
| WALLOPS | PASS | Oper-only fanout to opers | F4 |
| Claimed CHANMODES completeness | PASS for `b,,,nti` (no `+e`/`+I`) | — |

---

## F. Transport (client protocol adjacent)

| Surface | Status | Notes |
|---------|--------|-------|
| TCP plaintext | PASS | |
| TLS | PASS | Auth policy Gate A |
| WebSocket / WSS | PASS (daemon) | irctest WS harness still UNSUPPORTED (controller allowlist) — F5 notes |
| WS CRLF normalize | PASS | C13 |

---

## G. Curated irctest gate (F5)

**In curated `-k`:** Ping*, Privmsg*, JoinNamreply, Cap invalid/NoReq, Quit*, Part, Away* (F1a).

**Still in-tree e2e (not yet curated `-k`):** message-tags / server-time / account-tag / batch / sasl / chathistory / KILL — `protocol_f_slices_e2e` + prior protocol_* suites. Never hide via `-m not …`.

**Allowlist (intentional):** WS harness, services, STS, unadvertised caps, `+e`/`+I`, Ergo-specific — see `IRCTEST.md`.

---

## Execute map (children on list #372)

| Phase | Task | Status |
|-------|------|--------|
| F0 | #2252 | done |
| F1a | #2253 | done |
| F1b | #2254 | done |
| F1c | #2255 | done |
| F1d | #2256 | done |
| F2a | #2257 | done |
| F2b | #2258 | done |
| F2c | #2259 | done |
| F3 | #2260 | done |
| F4 | #2261 | done |
| F5 | #2262 | curated expand + matrix note |
| F6 | #2263 | docs closeout |

**Order:** F0 → F1 → F2 → F3 → F4 → F5 → F6.
