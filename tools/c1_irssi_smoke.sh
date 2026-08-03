#!/usr/bin/env bash
# C1 terminal-client smoke — irssi + nc against local dsc-ircd.
set -euo pipefail
source /root/.cargo/env
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
CFG=$(mktemp)
PORT=19667
cat >"$CFG" <<TOML
[server]
name = "smoke.test"
motd = "c1 smoke"
admin_name = "Otto"
admin_email = "otto@example.com"

[[listen]]
bind = "127.0.0.1:${PORT}"
tls = false

[limits]
max_clients = 32
max_clients_per_ip = 32
TOML
cargo build -p ircd --quiet
./target/debug/ircd --config "$CFG" &
PID=$!
cleanup() { kill "$PID" 2>/dev/null || true; rm -f "$CFG"; }
trap cleanup EXIT
for i in $(seq 1 50); do
  if (echo >/dev/tcp/127.0.0.1/$PORT) >/dev/null 2>&1; then break; fi
  sleep 0.1
done
if ! command -v irssi >/dev/null; then
  apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq irssi
fi
timeout 8 irssi --config=/dev/null -n smokebot -c 127.0.0.1 -p "$PORT" >/tmp/c1-irssi.log 2>&1 &
IRSSI_PID=$!
sleep 2
python3 - <<PY | tee /tmp/c1-smoke.out
import socket, time
s = socket.create_connection(("127.0.0.1", $PORT), timeout=5)
def send(line):
    s.sendall((line + "\r\n").encode())
send("NICK smoke")
send("USER smoke 0 * :smoke")
time.sleep(0.4)
send("VERSION")
send("JOIN #smoke")
send("LUSERS")
send("WHOIS smoke")
send("QUIT :bye")
buf = b""
s.settimeout(3)
try:
    while True:
        chunk = s.recv(4096)
        if not chunk:
            break
        buf += chunk
        if b" 318 " in buf or b"ERROR" in buf:
            break
except Exception:
    pass
print(buf.decode(errors="replace"))
s.close()
PY
kill "$IRSSI_PID" 2>/dev/null || true
grep -E " 001 | 351 | 353 | 251 | 311 " /tmp/c1-smoke.out
echo "C1 smoke OK"
