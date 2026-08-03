#!/usr/bin/env bash
# C6 — run curated progval/irctest markers against dsc-ircd.
# See docs/IRCTEST.md for the allowlist and marker rationale.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

IRCTEST_DIR="${IRCTEST_DIR:-${HOME}/irctest}"
if [[ ! -d "$IRCTEST_DIR/irctest" ]]; then
  echo "irctest checkout not found at IRCTEST_DIR=$IRCTEST_DIR" >&2
  echo "Clone: git clone --depth 1 https://github.com/progval/irctest.git \"\$IRCTEST_DIR\"" >&2
  exit 2
fi

source "${HOME}/.cargo/env" 2>/dev/null || true
cargo build -p ircd --quiet
export IRCTEST_DSC_IRCD="${IRCTEST_DSC_IRCD:-$ROOT/target/debug/ircd}"
export PYTHONPATH="${ROOT}/tools/irctest${PYTHONPATH:+:$PYTHONPATH}"

# Curated expression — keep in sync with docs/IRCTEST.md § Curated CI markers.
# Intentionally does NOT exclude advertised IRCv3 caps (message-tags / server-time /
# account-tag / batch). Cap modules that are not yet green are omitted via -k
# selection of probes that exercise the same surface in-tree (protocol_c3_e2e).
MARKERS="${IRCTEST_MARKERS:-(RFC1459 or RFC2812 or modern or IRCv3) and not Ergo and not deprecated and not strict and not services and not implementation-specific}"
# Node-id / name allowlist for the first CI gate (expand as probes go green).
KEXPR="${IRCTEST_K:-testPing or testPingNoToken or testPrivmsg or testPrivmsgToUser or testPrivmsgNonexistentChannel or testJoinNamreply or testInvalidCapSubcommand or testNoReq or testQuitDisconnects or testQuitErrors or (ChannelQuit and testQuit) or testPart or testAwayAck or testAwayPrivmsg or testAwayWhois or (testAway and not Userhost and not Empty) or testAwayNotify or testAwayNotifyOnJoin}"

cd "$IRCTEST_DIR"
set +e
python3 -m pytest \
  --controller dsc_ircd \
  -m "$MARKERS" \
  -k "$KEXPR" \
  --tb=short \
  -q \
  "$@"
rc=$?
set -e
exit "$rc"
