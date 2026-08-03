# irctest (progval) — dsc-ircd controller and curated CI

Decision Science Corp · `dsc-ircd` · Tasks **#2235** (C6) · Doc **#974** Ongoing conformance gate  
**Full-compliance acceptance matrix:** [`PROTOCOL-MATRIX.md`](PROTOCOL-MATRIX.md) · Tasks Doc **#976** / F0 **#2252**

This tree ships an in-repo [progval/irctest](https://github.com/progval/irctest) **controller** so CI (and local lab) can drive the real `ircd` binary without forking irctest.

## Quick start

```bash
# One-time: clone irctest somewhere writable
git clone --depth 1 https://github.com/progval/irctest.git ~/irctest
pip3 install --user -r ~/irctest/requirements.txt

# From this repo
bash tools/irctest/run_curated.sh
```

Environment:

| Variable | Meaning |
|----------|---------|
| `IRCTEST_DSC_IRCD` | Absolute path to the `ircd` binary (default: `target/debug/ircd` after `cargo build -p ircd`) |
| `IRCTEST_DIR` | irctest checkout (default: `~/irctest`) |
| `IRCTEST_MARKERS` | Override pytest `-m` expression |
| `IRCTEST_K` | Override pytest `-k` curated node filter |
| `IRCTEST_DEBUG_LOGS=1` | Surface server stdout/stderr through irctest |

Controller module: `tools/irctest/dsc_ircd.py` (`--controller dsc_ircd` with `PYTHONPATH=tools/irctest`).

## Curated CI markers

CI job **`irctest`** runs `tools/irctest/run_curated.sh` with:

**Markers (`-m`):**

```text
(RFC1459 or RFC2812 or modern or IRCv3)
and not Ergo and not deprecated and not strict
and not services and not implementation-specific
```

**Name filter (`-k`) — curated probes that must stay green:**

| Probe | Why it is in the gate |
|-------|------------------------|
| `testPing` / `testPingNoToken` | Modern PING/PONG + server name |
| `testPrivmsg` / `testPrivmsgToUser` / `testPrivmsgNonexistentChannel` | Core message routing |
| `testJoinNamreply` | JOIN + RPL_NAMREPLY shape |
| `testInvalidCapSubcommand` / `testNoReq` | CAP negotiation without REQ of unsupported caps |
| `testQuit` / `testQuitDisconnects` / `testQuitErrors` | QUIT fanout, ERROR ack, TCP close |
| `testPart` | Channel leave |

**Advertised IRCv3 caps** (`cap-notify`, `message-tags`, `server-time`, `account-tag`, `batch`, plus conditional `sasl` / `draft/chathistory`) are **not** excluded with `-m 'not message-tags …'`. Full irctest modules for those caps are not yet in the curated `-k` list; wire conformance for advertised caps remains gated by in-tree **`protocol_c3_e2e`** (and Gate A A1: do not advertise without tests). Expand `-k` as individual irctest cases go green — never hide an advertised cap behind a marker exclusion.

## Allowlist — intentionally unsupported (irctest)

Controllers raise `NotImplementedByController` (or omit optional behaviors) for surfaces we are **not** claiming. This is the documented allowlist for C6:

| Area | Status | Notes |
|------|--------|-------|
| WebSocket listeners | Unsupported in controller | Binary has WS; irctest websocket harness not wired |
| Connection `PASS` / link password | Unsupported | No server-password gate in lab config |
| Services packages (Anope/Atheme) | Unsupported | No services controller; `-m 'not services'` |
| STS | Unsupported | `supports_sts = False`; cap not advertised |
| `multi-prefix`, `echo-message`, `away-notify`, `account-notify`, `extended-join`, `labeled-response`, `setname`, `MONITOR`, … | Not advertised | Do not REQ in curated probes; NAK is correct if a client asks |
| Ban/invite exception modes (`+e` / `+I`) | Optional behavior absent | Not in `optional_behaviors` |
| Ergo / Sable / implementation-specific tests | Out of scope | Marker-excluded |

If a future change **advertises** a new cap, add focused in-tree e2e **and** promote matching irctest cases into the curated `-k` list in the same slice.

## Controller capabilities declaration

`DscIrcdController.capabilities` matches `BASE_CAPS` in `crates/ircd/src/session/cap.rs` (minus `cap-notify`, which irctest does not model as a `Capabilities` enum member):

- `message-tags`
- `server-time`
- `account-tag`
- `batch`

Optional behavior claimed: `CAP_REQ_MINUS` (runtime `CAP REQ -cap`).

Lab config uses server name **`My.Little.Server`** (irctest PONG / source expectation), plaintext listen on the port irctest allocates, oper `operuser`/`operpassword`, and **no** accounts/history blocks so CI does not advertise `sasl` / `draft/chathistory` unless a later curated probe needs them.
