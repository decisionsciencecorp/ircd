//! Minimal interactive/smoke IRC client for testing dsc-ircd.
//!
//! Examples:
//!   cargo run -p ircc -- --host 127.0.0.1 --port 6667 --nick otto
//!   cargo run -p ircc -- --host 127.0.0.1 --port 6667 --nick otto --join '#test' --msg 'hi' --quit

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::{info, warn};

struct Args {
    host: String,
    port: u16,
    nick: String,
    user: String,
    join: Option<String>,
    msg: Option<String>,
    quit_after: bool,
}

fn parse_args() -> Result<Args> {
    let mut host = "127.0.0.1".to_string();
    let mut port = 6667u16;
    let mut nick = "ircc".to_string();
    let mut user = "ircc".to_string();
    let mut join = None;
    let mut msg = None;
    let mut quit_after = false;

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--host" => host = it.next().context("--host needs value")?,
            "--port" => port = it.next().context("--port needs value")?.parse()?,
            "--nick" => nick = it.next().context("--nick needs value")?,
            "--user" => user = it.next().context("--user needs value")?,
            "--join" => join = Some(it.next().context("--join needs value")?),
            "--msg" => msg = Some(it.next().context("--msg needs value")?),
            "--quit" => quit_after = true,
            "-h" | "--help" => {
                println!(
                    "ircc — dsc-ircd smoke CLI\n\n\
                     --host HOST --port PORT --nick NICK [--user USER]\n\
                     [--join #chan] [--msg text] [--quit]\n\n\
                     Without --quit, reads stdin lines as raw IRC or bare channel chat if JOINed."
                );
                std::process::exit(0);
            }
            other => bail!("unknown arg {other}"),
        }
    }
    if msg.is_some() && join.is_none() {
        bail!("--msg requires --join");
    }
    Ok(Args {
        host,
        port,
        nick,
        user,
        join,
        msg,
        quit_after,
    })
}

async fn send_line(writer: &mut tokio::net::tcp::OwnedWriteHalf, line: &str) -> Result<()> {
    writer.write_all(line.as_bytes()).await?;
    print!(">> {}", line.trim_end_matches(['\r', '\n']));
    println!();
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ircc=info".into()),
        )
        .init();

    let args = parse_args()?;
    let addr = format!("{}:{}", args.host, args.port);
    info!("connecting to {addr} as {}", args.nick);

    let stream = TcpStream::connect(&addr)
        .await
        .with_context(|| format!("connect {addr}"))?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    send_line(&mut writer, &format!("NICK {}\r\n", args.nick)).await?;
    send_line(
        &mut writer,
        &format!("USER {} 0 * :ircc smoke\r\n", args.user),
    )
    .await?;

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let interactive = !args.quit_after;
    if interactive {
        let tx_stdin = tx.clone();
        tokio::spawn(async move {
            let stdin = BufReader::new(tokio::io::stdin());
            let mut lines = stdin.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx_stdin.send(line).is_err() {
                    break;
                }
            }
        });
    }

    let join_chan = args.join.clone();
    let mut line_buf = String::new();
    let mut registered = false;
    let mut joined = args.join.is_none();
    let mut msg_sent = args.msg.is_none();
    let mut quit_sent = false;

    loop {
        tokio::select! {
            read = reader.read_line(&mut line_buf) => {
                let n = read?;
                if n == 0 {
                    warn!("server closed connection");
                    break;
                }
                let raw = std::mem::take(&mut line_buf);
                let display = raw.trim_end_matches(['\r','\n']);
                println!("<< {display}");

                if let Some(msg) = ircd_core::RawLine::parse(display) {
                    if msg.command_eq("PING") {
                        let token = msg.params.first().map(String::as_str).unwrap_or("ircd");
                        send_line(&mut writer, &format!("PONG :{token}\r\n")).await?;
                    }
                    if msg.command == "001" && !registered {
                        registered = true;
                        if let Some(chan) = &args.join {
                            send_line(&mut writer, &format!("JOIN {chan}\r\n")).await?;
                        } else if args.quit_after {
                            send_line(&mut writer, "QUIT :ircc smoke done\r\n").await?;
                            quit_sent = true;
                            break;
                        }
                    }
                    // JOIN echo from server (or names end) marks channel ready
                    if registered
                        && !joined
                        && (msg.command_eq("JOIN")
                            || msg.command == "353"
                            || msg.command == "366")
                    {
                        if !joined {
                            joined = true;
                            if let (Some(chan), Some(text)) = (&args.join, &args.msg) {
                                send_line(
                                    &mut writer,
                                    &format!("PRIVMSG {chan} :{text}\r\n"),
                                )
                                .await?;
                                msg_sent = true;
                            } else {
                                msg_sent = true;
                            }
                            if args.quit_after && msg_sent {
                                send_line(&mut writer, "QUIT :ircc smoke done\r\n").await?;
                                quit_sent = true;
                                break;
                            }
                        }
                    }
                }

                // If server never echoes JOIN but we already registered, avoid hang:
                // after any post-001 line once JOIN was sent, treat as joined for smoke.
                if args.quit_after
                    && registered
                    && !joined
                    && args.join.is_some()
                    && display.contains("JOIN")
                {
                    // handled above
                }
            }
            line = rx.recv(), if interactive => {
                let Some(line) = line else { break };
                let out = if line.starts_with('/') {
                    let rest = line.trim_start_matches('/');
                    if let Some(chan) = rest.strip_prefix("join ").or_else(|| rest.strip_prefix("JOIN ")) {
                        format!("JOIN {chan}\r\n")
                    } else if let Some(rest) = rest.strip_prefix("msg ").or_else(|| rest.strip_prefix("MSG ")) {
                        if let Some((target, text)) = rest.split_once(' ') {
                            format!("PRIVMSG {target} :{text}\r\n")
                        } else {
                            format!("{rest}\r\n")
                        }
                    } else if rest.eq_ignore_ascii_case("quit") {
                        "QUIT :ircc\r\n".to_string()
                    } else {
                        format!("{rest}\r\n")
                    }
                } else if let Some(chan) = &join_chan {
                    format!("PRIVMSG {chan} :{line}\r\n")
                } else {
                    format!("{line}\r\n")
                };
                send_line(&mut writer, &out).await?;
                if out.starts_with("QUIT") {
                    quit_sent = true;
                    break;
                }
            }
        }
    }

    let _ = (joined, msg_sent, quit_sent);
    Ok(())
}
