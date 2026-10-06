# irctest

[progval/irctest](https://github.com/progval/irctest) is an external IRC conformance suite. This repository ships a controller so that suite can launch the `ircd` binary. We run a curated subset in CI. We do not claim every test in upstream irctest.

The acceptance matrix for our own suite is [PROTOCOL-MATRIX.md](PROTOCOL-MATRIX.md).

## Run it

```bash
git clone --depth 1 https://github.com/progval/irctest.git ~/irctest
python3 -m venv ~/irctest-venv
~/irctest-venv/bin/pip install -r ~/irctest/requirements.txt
export PATH="$HOME/irctest-venv/bin:$PATH"
export IRCTEST_DIR="$HOME/irctest"

cargo build -p ircd
bash tools/irctest/run_curated.sh
```

| Variable | Meaning |
|----------|---------|
| `IRCTEST_DSC_IRCD` | Path to the `ircd` binary. Default: `target/debug/ircd`. |
| `IRCTEST_DIR` | irctest checkout. Default: `~/irctest`. |
| `IRCTEST_MARKERS` | Override the pytest `-m` expression. |
| `IRCTEST_K` | Override the pytest `-k` name filter. |
| `IRCTEST_DEBUG_LOGS=1` | Show server stdout and stderr. |

The controller module is `tools/irctest/dsc_ircd.py`. The runner puts `tools/irctest` on `PYTHONPATH` and passes `--controller dsc_ircd`.

## What CI selects

The `irctest` job in `.github/workflows/ci.yml` runs `tools/irctest/run_curated.sh`.

Markers:

```text
(RFC1459 or RFC2812 or modern or IRCv3)
and not Ergo and not deprecated and not strict
and not services and not implementation-specific
```

Name filter. These probes are part of the gate:

| Probe | Why |
|-------|-----|
| `testPing` / `testPingNoToken` | `PING` / `PONG`, including the no-token case |
| `testPrivmsg` / `testPrivmsgToUser` / `testPrivmsgNonexistentChannel` | Channel and user routing |
| `testJoinNamreply` | `JOIN` and `RPL_NAMREPLY` |
| `testInvalidCapSubcommand` / `testNoReq` | `CAP` without requesting unknown caps |
| `testQuit` / `testQuitDisconnects` / `testQuitErrors` | `QUIT`, `ERROR`, TCP close |
| `testPart` | Leave a channel |
| `testAway` / `testAwayAck` / `testAwayPrivmsg` / `testAwayWhois` | `AWAY`, `305`/`306`, `301` |
| `testAwayNotify` / `testAwayNotifyOnJoin` | `away-notify` to peers and on join |

Advertised capabilities are **not** excluded with a marker like `-m 'not message-tags'`. Their full irctest modules are not all in the `-k` list yet. Wire behavior for those caps is gated by `protocol_c3_e2e`, `protocol_c7_e2e`, `protocol_f1a_e2e`, and `protocol_f_slices_e2e`. When an individual irctest case is green, add it to `-k`. Do not hide an advertised cap by excluding its marker.

## Allowlist

The controller raises `NotImplementedByController`, or simply does not claim the behavior, for surfaces this server does not implement.

| Area | Status | Notes |
|------|--------|-------|
| WebSocket in irctest | Unsupported in the controller | The daemon speaks WebSocket. The irctest WebSocket harness is not wired. |
| Services (Anope / Atheme) | Unsupported | No services controller. Marker `not services`. |
| STS | Unsupported | Not advertised. `supports_sts = False`. |
| `multi-prefix`, `echo-message`, `account-notify`, `extended-join`, `labeled-response`, `setname`, `MONITOR` | Not advertised | `CAP REQ` of these is a `NAK`. |
| `+e` / `+I` | Absent | Not in `optional_behaviors`. |
| Ergo- or implementation-specific tests | Out of scope | Excluded by marker. |
| Connection `PASS` | Optional | Implemented when `server.password` is set. Lab configs leave it empty. |

`away-notify` **is** advertised. It is covered in-tree by `protocol_f1a_e2e` and by the AWAY probes in the `-k` list.

## What the controller tells irctest

`DscIrcdController.capabilities` in `tools/irctest/dsc_ircd.py` declares the base set the binary advertises, using irctest's `Capabilities` enum:

- `message-tags`
- `server-time`
- `account-tag`
- `batch`
- `away-notify`

`cap-notify` is always on in the daemon and is not a member of that enum. `sasl` and `draft/chathistory` are omitted here because the generated lab config has no accounts and sets `[history] enabled = false`.

Optional behavior claimed: `CAP_REQ_MINUS` (a `CAP REQ` token may start with `-`). `supports_sts` is false. `supported_sasl_mechanisms` is empty for this config.

The lab config this controller writes uses server name `My.Little.Server` (irctest expects that in `PONG`), a plaintext port that irctest allocates, operator `operuser` / `operpassword`, and no accounts or history block. CI therefore does not advertise `sasl` or `draft/chathistory` unless a later probe asks for a config that enables them.

## Changing the gate

If you advertise a new capability:

1. Add the in-tree end-to-end test.
2. Add the matching irctest cases to `-k` once they pass against this controller.
3. Update the allowlist table on this page if something moved from "not claimed" to "claimed".
