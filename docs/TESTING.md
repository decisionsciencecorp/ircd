# Testing

The suite is the definition of what the server claims. A behavior change needs a test in the same change. A new `CAP LS` token needs a test before it is advertised.

GitHub Actions (`.github/workflows/ci.yml`) runs the gates below on every push and pull request to `main`.

## Gates

| Gate | Command | Pass |
|------|---------|------|
| Format | `cargo fmt --all -- --check` | clean |
| Clippy | `cargo clippy -p ircd -p ircd-core --tests -- -D warnings` | clean, with the allow list in the workflow |
| Tests | `cargo test -p ircd -p ircd-core --tests --lib` | all green |
| Doc tests | `cargo test -p ircd -p ircd-core --doc` | every rustdoc example runs |
| Audit | `cargo audit` | no known advisory in the pinned tree |
| Soak | `bash tools/c5_soak.sh` | short connect / join / privmsg loop |
| Coverage | `cargo tarpaulin` as below | **≥ 90%** on `ircd` + `ircd-core` |
| irctest | `bash tools/irctest/run_curated.sh` | curated probes in [IRCTEST.md](IRCTEST.md) |

Coverage excludes the smoke client and the bind paths that need a live socket or a certificate dance:

```bash
cargo tarpaulin -p ircd -p ircd-core \
  --exclude-files '**/ircc/**' \
  --exclude-files '**/ircd/src/main.rs' \
  --exclude-files '**/ircd/src/ws.rs' \
  --exclude-files '**/ircd/src/tls.rs' \
  --ignore-tests --fail-under 90 --timeout 300
```

`main`, the TLS acceptor, and the WebSocket acceptor stay outside that 90% number. Protocol behavior is tested through in-process duplex I/O in `crates/ircd/tests/`, which drives `session` without binding a port. Do not exclude `tests/` in a way that skips running them. `--ignore-tests` only keeps test-only lines out of the percentage.

Benches and fuzz are local tools. CI does not run them.

```bash
cargo bench -p ircd
cargo +nightly fuzz run rawline_parse -- -runs=10000
cargo +nightly fuzz run tags_adapt -- -runs=10000
```

Fuzz needs nightly and `cargo-fuzz`. Drop `-- -runs=…` for a long run. The corpus lives under `fuzz/corpus/`. The fuzz package is outside the workspace (`workspace.exclude = ["fuzz"]` and an empty `[workspace]` in `fuzz/Cargo.toml`). That split is what lets `cargo fuzz` run.

## Layout

| Path | What it covers |
|------|----------------|
| `crates/ircd-core` unit and doc tests | `RawLine`, numerics, tags, casemap |
| `crates/ircd-core/tests/rawline_proptest.rs` | Arbitrary input must not panic; constructed lines round-trip |
| `crates/ircd/src/**` unit tests | Config validation, history queries, CAP REQ, WebSocket policy |
| `crates/ircd/tests/protocol_*_e2e.rs` | Duplex end-to-end for commands and caps |
| `crates/ircd/tests/gate_a_acceptance_e2e.rs` | The public-host acceptance pack |
| `crates/ircd/benches/` | History and routing (`criterion`) |
| `fuzz/fuzz_targets/` | `rawline_parse`, `tags_adapt` |
| `tarpaulin.toml` | Default excludes and the 90% floor |
| `tools/c5_soak.sh` | Short external smoke against a spawned binary |
| `tools/c1_irssi_smoke.sh` | Optional irssi script, not part of CI |
| `tools/irctest/` | Controller and curated irctest runner |

End-to-end tests use `tests/common/mod.rs`. Coverage lands on `session` because the test calls the library, not a child process.

## Gate A

One adversarial test file for the bugs that make a public bind unsafe:

```bash
cargo test -p ircd --test gate_a_acceptance_e2e
```

| Test | What it locks |
|------|----------------|
| `a1_cap_ls_has_no_false_ads` | `CAP LS` matches implemented caps |
| `a2_nick_steal_does_not_transfer_ops` | Ops follow `ClientId`, not the nick string |
| `a3_oversized_line_417` | Over-long input is rejected |
| `a4_kick_revokes_channel_send` | A kicked client cannot keep sending |
| `a5_plaintext_sasl_blocked_when_tls_required` | Auth policy honors TLS-required |
| `a6_session_has_no_lock_across_await` | The state mutex is not held across await |
| `a7_channel_quotas` | Channel and membership caps |
| `a8_casemap_and_cap_end` | ASCII casemap and `CAP END` |
| `a10_ws_origin_policy` | WebSocket origin rules |
| `a11_refuse_world_readable_secret` | Keys and the history file must be private |
| `a12_part_nonmember_442` | `PART` of a channel you are not on |

## Routing benches

Channel fanout uses per-connection bounded outboxes (`OUTBOX_CAP` = 64, `try_send` drops on a slow consumer). The session loop waits on that outbox and on the socket.

```bash
cargo test -p ircd --test protocol_b1_e2e
cargo bench -p ircd --bench routing_bench
```

The routing bench reports fanout to 1, 10, 100, and 1000 recipients. Treat the numbers as a local snapshot, not a service level.

## irctest

The external suite and the allowlist of things we deliberately do not claim are in [IRCTEST.md](IRCTEST.md). In-tree tests remain the gate for advertised capabilities. irctest is an extra client-shaped check, not a substitute.

## Defects this suite was built to keep dead

**Tags parsed as the command.** A line like `@time=… :nick PRIVMSG #c :hi` used to fail because the parser treated `@tags` as the verb. `RawLine::parse` now splits tags first. Unit tests, doc tests, proptest, and `rawline_parse` cover it.

**Tag adaptation duplicating `time`.** `tag_server_time`, `adapt_bus_line`, and `prepend_tag` live in `ircd-core::tags`. Tests require a single `time` tag and require `msgid` / `account` to follow the caps the client actually enabled.

**`cargo fuzz` and the workspace.** Fuzz crates want to be their own workspace. The root manifest excludes `fuzz/`, and `fuzz/Cargo.toml` declares an empty `[workspace]`.

**Coverage and `select!`.** LLVM coverage under-attributes some `tokio::select!` branches. When the overall number sits just under 90%, add a direct unit test of the extracted function before rearranging the session loop. The loop itself stays event-driven. An earlier experiment replaced `select!` with polling to please the profiler. That was reverted. Do not do it again to chase a percentage.

## Adding a test

1. Parser or tag invariant: unit test in `ircd-core`, and a fuzz target if the input is attacker-controlled bytes.
2. Command or numeric: `crates/ircd/tests/protocol_*_e2e.rs`, asserting the numeric and the fanout another client sees.
3. New capability: behavior test first, then add the name in `advertised_caps`.
4. Run `cargo test -p ircd -p ircd-core --tests --lib` and `cargo test -p ircd -p ircd-core --doc`.
5. If you touched session logic, run the tarpaulin command before merging. The floor is 90%.
