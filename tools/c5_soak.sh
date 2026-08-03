#!/usr/bin/env bash
# C5 — small concurrent load smoke against in-process duplex isn't needed;
# this drives a short local TCP soak when a binary is available.
set -euo pipefail
source "${HOME}/.cargo/env" 2>/dev/null || true
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
cargo build -p ircd --quiet
CFG=$(mktemp)
PORT=19669
cat >"$CFG" <<TOML
[server]
name = "soak.test"
motd = "soak"
admin_name = "Otto"
admin_email = "otto@example.com"

[[listen]]
bind = "127.0.0.1:${PORT}"
tls = false

[limits]
max_clients = 64
max_clients_per_ip = 64
TOML
./target/debug/ircd --config "$CFG" &
PID=$!
cleanup() { kill "$PID" 2>/dev/null || true; rm -f "$CFG"; }
trap cleanup EXIT
for i in $(seq 1 50); do
  if (echo >/dev/tcp/127.0.0.1/$PORT) >/dev/null 2>&1; then break; fi
  sleep 0.1
done
python3 - <<PY
import socket, concurrent.futures, time
PORT=$PORT

def one(i):
    s = socket.create_connection(("127.0.0.1", PORT), timeout=5)
    def send(line):
        s.sendall((line + "\r\n").encode())
    send(f"NICK u{i}")
    send(f"USER u{i} 0 * :u{i}")
    time.sleep(0.05)
    send("JOIN #soak")
    send("PRIVMSG #soak :hi")
    send("QUIT :done")
    s.settimeout(2)
    try:
        while s.recv(4096):
            pass
    except Exception:
        pass
    s.close()
    return i

with concurrent.futures.ThreadPoolExecutor(max_workers=8) as ex:
    list(ex.map(one, range(16)))
print("C5 soak OK (16 clients)")
PY
