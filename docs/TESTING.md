# Testing — what we run, and what it caught

Decision Science Corp · `dsc-ircd` · Tasks list **369** (Server — testing & coverage)

Heavy `cargo` work runs on **NewDev** (`64.95.11.220`), typically under `/root/projects/ircd-test`. Termux is the edit host; do not expect tarpaulin/fuzz builds there.

## Bars

| Gate | Tool | Pass criteria |
|------|------|----------------|
| Coverage | `cargo tarpaulin` | ≥ **90%** on `ircd` + `ircd-core` (CLI/`main`/`ws`/`tls`/`ircc` excluded) |
| Property | `proptest` | Parser / tag helpers never panic; constructed round-trips |
| Docs | `cargo test --doc` | Every ` ``` ` example is a real test |
| Perf | `cargo bench -p ircd` | History + routing benches compile and run |
| Fuzz | `cargo +nightly fuzz` | Harnesses build; overnight corpus optional |

## Commands (NewDev)

```bash
source /root/.cargo/env
cd /root/projects/ircd-test   # or clone path

# Unit + integration + e2e
cargo test -p ircd -p ircd-core --tests --lib

# Doc examples
cargo test -p ircd -p ircd-core --doc

# Coverage (fail under 90)
cargo tarpaulin -p ircd -p ircd-core \
  --exclude-files '**/ircc/**' \
  --exclude-files '**/ircd/src/main.rs' \
  --exclude-files '**/ircd/src/ws.rs' \
  --exclude-files '**/ircd/src/tls.rs' \
  --ignore-tests --fail-under 90 --timeout 300

# Benches
cargo bench -p ircd

# Fuzz smoke (needs nightly + cargo-fuzz)
cargo +nightly fuzz run rawline_parse -- -runs=10000
cargo +nightly fuzz run tags_adapt -- -runs=10000
# Overnight: omit -runs and leave running; corpus under fuzz/corpus/
```

Verified snapshot (2026-08-02, NewDev): **90.15%** (`1263/1401`) with `--fail-under 90`.

## Layout

| Path | Role |
|------|------|
| `crates/ircd-core` unit + doc tests | `RawLine`, numerics, tag helpers |
| `crates/ircd-core/tests/rawline_proptest.rs` | Arbitrary bytes / strings → parse; constructed round-trip |
| `crates/ircd/src/{config,history,state,session}.rs` | Unit tests (incl. CAP `apply_cap_req`) |
| `crates/ircd/tests/protocol_*_e2e.rs` | Duplex e2e for common protocol routes |
| `crates/ircd/benches/` | CHATHISTORY / routing Criterion benches |
| `fuzz/` | libFuzzer targets `rawline_parse`, `tags_adapt` |
| `tarpaulin.toml` | Default exclude + fail-under |

E2E uses in-process duplex I/O (`tests/common/mod.rs`) so coverage attributes to `session` without a live TCP bind.

## What testing caught

### 1. IRCv3 `@tags` were treated as the command (`ircd-core`)

**Symptom:** Lines like `@time=… :nick PRIVMSG #c :hi` failed to parse as `PRIVMSG`.

**Cause:** `RawLine::parse` did not strip a leading `@tags` block before reading the command token.

**Fix:** Parse tags into `RawLine.tags`, then command/params from the remainder. Covered by unit + doc + proptest + fuzz target.

### 2. Duplicate / wrong `server-time` tagging (`tags`)

**Symptom:** Helpers could re-prepend `time=` or drop sibling tags inconsistently when adapting bus lines for CAP combinations.

**Fix:** `tag_server_time` / `adapt_bus_line` / `prepend_tag` consolidated in `ircd-core::tags` with explicit unit tests (no duplicate `time=`, keep `msgid`/`account` when caps allow, strip when neither CAP is on).

### 3. Coverage undercount from `tokio::select!` (`session`)

**Symptom (historical):** Tarpaulin under-attributed `select!` branches. A6 temporarily used `try_recv`+timed reads; **B1 restored `select!`** for correct scheduling — keep e2e dense enough to hold ≥90%.

**Symptom (was):** Tarpaulin reported session coverage far below real exercise of the read/bus loop (~89.99% overall stuck one line under the bar).

**Cause:** `select!` branches are poorly attributed under LLVM coverage for this crate.

**Fix:** Prefer `bus_rx.try_recv()` plus a timed `read_line` in the session loop so hits land on real statements. Additional CAP logic extracted to `apply_cap_req` with sync unit tests so REQ ACK/NAK/disable paths count.

### 4. Workspace membership broke `cargo fuzz`

**Symptom:** `cargo fuzz run` failed: fuzz package believed it was in the workspace without being a member.

**Fix:** Root `workspace.exclude = ["fuzz"]` and an empty `[workspace]` table in `fuzz/Cargo.toml`.

## Scope notes

- **`main` / TLS acceptor / WebSocket** stay out of the 90% bar (lab bind paths); protocol truth lives in `session` + `ircd-core`.
- **proptest** finds panic/invariant bugs; **fuzz** finds parser crashes and weird UTF-8/byte sequences overnight — keep both.
- Integration tests still appear in some tarpaulin line totals; `--ignore-tests` reduces noise but e2e must remain *run* (do not `--exclude-files '**/tests/**'` if that skips executing them).

## Gate A acceptance pack (A9)

Single adversarial suite covering public-host blockers for slices **A1–A8 + A10–A12**:

```bash
cargo test -p ircd --test gate_a_acceptance_e2e
```

| Test | Slice |
|------|-------|
| `a1_cap_ls_has_no_false_ads` | A1 |
| `a2_nick_steal_does_not_transfer_ops` | A2 |
| `a3_oversized_line_417` | A3 |
| `a4_kick_revokes_channel_send` | A4 |
| `a5_plaintext_sasl_blocked_when_tls_required` | A5 |
| `a6_session_has_no_lock_across_await` | A6 |
| `a7_channel_quotas` | A7 |
| `a8_casemap_and_cap_end` | A8 |
| `a10_ws_origin_policy` | A10 |
| `a11_refuse_world_readable_secret` | A11 |
| `a12_part_nonmember_442` | A12 |

Must stay green on NewDev before public zero1 bind (with Tasks #2210 / #2193). Also re-run the tarpaulin command in §Coverage after Gate A landings.



## Gate B — member-targeted routing (B1)

Channel fanout uses per-`ClientId` bounded outboxes (`OUTBOX_CAP=64`, `try_send` drop on slow consumers), not broadcast-to-all-subscribers. Session loop is event-driven `tokio::select!` (outbox preferred).

```bash
cargo test -p ircd --test protocol_b1_e2e
cargo bench -p ircd --bench routing_bench
```

Honest fanout sizes: **1 / 10 / 100 / 1000** recipients (`fanout_channel/*`). Snapshot (NewDev, 2026-08-02): ~168 ns / 1.37 µs / 14.1 µs / 172 µs; slow-consumer policy ~486 ns; legacy `bus.send` ~48 ns (not member-accurate).



## C1b interop smoke

| Path | Status (2026-08-03) |
|------|---------------------|
| Terminal (irssi + tools/c1_irssi_smoke.sh) | Green on NewDev against local plaintext listen |
| Protocol duplex e2e (protocol_c1_e2e) | Green |
| Tauri / web IRC | Stub — client repo not in-tree yet; re-run when present |
| Native Mac client | Stub — same; re-run when present |

Gaps filed: no automated Tauri/Mac harness until those clients exist. Server surface for queries is covered by C1 wire tests.

## CI quality gates (C5)

GitHub Actions: `.github/workflows/ci.yml`

| Gate | Command |
|------|---------|
| fmt | `cargo fmt --all -- --check` |
| clippy | `cargo clippy -p ircd -p ircd-core --tests -- -D warnings` |
| tests | `cargo test -p ircd -p ircd-core --tests --lib` |
| docs | `cargo test -p ircd -p ircd-core --doc` |
| audit | `cargo audit` (no rustls-pemfile — PEM via `rustls::pki_types`) |
| soak | `bash tools/c5_soak.sh` |
| coverage | tarpaulin fail-under **90** (same flags as above) |

## Tasks trail

| ID | Slice |
|----|--------|
| #2203 | Tarpaulin ≥90% |
| #2204 | proptest |
| #2205 | Doc tests |
| #2206 | cargo bench |
| #2207 | cargo-fuzz |
| #2208 | This document |

List: **369** · Project: **48** (Mark × Cody IRC).
