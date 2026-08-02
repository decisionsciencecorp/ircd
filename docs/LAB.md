# Lab instance (NewDev)

Standing `dsc-ircd` on Dallas **NewDev** for Mark × Cody client smoke.

| Path | Address |
|------|---------|
| Plaintext IRC | `64.95.11.220:6667` |
| TLS IRC (self-signed) | `64.95.11.220:6697` |
| WebSocket | `ws://64.95.11.220:7667` |
| Server name | `irc.lab.newdev.dsc` |

## On the host

- Config: `/root/ircd-lab/config.toml`
- Binary: `/root/projects/ircd/target/release/ircd`
- Start / stop: `/root/ircd-lab/start.sh` · `/root/ircd-lab/stop.sh`
- Logs: `/root/ircd-lab/logs/ircd.log`
- Certs: `/root/ircd-lab/certs/` (`ircd gen-cert`)

## Lab auth (not production)

- SASL PLAIN: `alice` / `alice-lab`, `bob` / `bob-lab`
- OPER: `admin` / `lab-oper-change-me`
- TLS is self-signed — use `ircc --tls` (insecure verify) or equivalent for smoke.

Also recorded on Tasks **#2172** and Program Doc **#973**.
