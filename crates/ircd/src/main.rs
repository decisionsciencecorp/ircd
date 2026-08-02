//! DSC IRCd bootstrap listener.
//!
//! Enough protocol for a client to register, join `#test`, and chat.
//! Feature growth tracks UnrealIRCd as the ops/reference model (clean-room).

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use ircd_core::{numeric, server_notice, RawLine};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, Mutex};
use tracing::{info, warn};

const SERVER_NAME: &str = "ircd.dsc.local";
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
struct BusMsg {
    /// Channel name including `#`, or `*` for server-wide (unused for now).
    target: String,
    line: String,
    /// Skip delivering back to this connection id.
    skip_conn: u64,
}

struct Shared {
    next_id: u64,
    /// nick -> connection id (single nick registration for v0)
    nicks: HashMap<String, u64>,
    /// channel -> member nicks
    channels: HashMap<String, HashSet<String>>,
    bus: broadcast::Sender<BusMsg>,
}

impl Shared {
    fn new() -> Self {
        let (bus, _) = broadcast::channel(256);
        Self {
            next_id: 1,
            nicks: HashMap::new(),
            channels: HashMap::new(),
            bus,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ircd=info".into()),
        )
        .init();

    let bind = std::env::args()
        .skip_while(|a| a != "--bind")
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:6667".to_string());

    let listener = TcpListener::bind(&bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    info!("dsc-ircd {VERSION} listening on {bind} (server name {SERVER_NAME})");

    let shared = Arc::new(Mutex::new(Shared::new()));

    loop {
        let (socket, peer) = listener.accept().await?;
        let shared = Arc::clone(&shared);
        tokio::spawn(async move {
            if let Err(e) = handle_client(socket, peer, shared).await {
                warn!(%peer, error = %e, "client session ended");
            }
        });
    }
}

async fn handle_client(
    socket: TcpStream,
    peer: SocketAddr,
    shared: Arc<Mutex<Shared>>,
) -> Result<()> {
    let conn_id = {
        let mut g = shared.lock().await;
        let id = g.next_id;
        g.next_id += 1;
        id
    };

    let (reader, mut writer) = socket.into_split();
    let mut reader = BufReader::new(reader);
    let mut bus_rx = shared.lock().await.bus.subscribe();

    writer
        .write_all(
            server_notice(
                SERVER_NAME,
                &format!("*** dsc-ircd {VERSION} — Unreal-inspired clean-room Rust IRCd"),
            )
            .as_bytes(),
        )
        .await?;
    writer
        .write_all(
            server_notice(SERVER_NAME, "*** Looking up your hostname...").as_bytes(),
        )
        .await?;
    writer
        .write_all(
            server_notice(
                SERVER_NAME,
                &format!("*** Connected as peer {peer}; hostname checks deferred in v0"),
            )
            .as_bytes(),
        )
        .await?;

    let mut nick: Option<String> = None;
    let mut user: Option<String> = None;
    let mut realname: Option<String> = None;
    let mut registered = false;
    let mut line_buf = String::new();
    let mut channels: HashSet<String> = HashSet::new();

    loop {
        tokio::select! {
            bus = bus_rx.recv() => {
                match bus {
                    Ok(msg) if msg.skip_conn != conn_id && channels.contains(&msg.target) => {
                        writer.write_all(msg.line.as_bytes()).await?;
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            read = reader.read_line(&mut line_buf) => {
                let n = read?;
                if n == 0 {
                    break;
                }
                let raw = std::mem::take(&mut line_buf);
                let Some(msg) = RawLine::parse(&raw) else { continue };

                if msg.command_eq("CAP") {
                    // Minimal CAP: NAK end so classic clients proceed.
                    if msg.params.first().map(|s| s.eq_ignore_ascii_case("LS")).unwrap_or(false) {
                        let nick_s = nick.as_deref().unwrap_or("*");
                        writer.write_all(
                            format!(":{SERVER_NAME} CAP {nick_s} LS :\r\n").as_bytes(),
                        ).await?;
                    } else if msg.params.first().map(|s| s.eq_ignore_ascii_case("END")).unwrap_or(false) {
                        // ignore
                    } else {
                        let nick_s = nick.as_deref().unwrap_or("*");
                        writer.write_all(
                            format!(":{SERVER_NAME} CAP {nick_s} NAK :\r\n").as_bytes(),
                        ).await?;
                    }
                    continue;
                }

                if msg.command_eq("NICK") {
                    let Some(desired) = msg.params.first().cloned() else { continue };
                    if desired.is_empty() || desired.contains(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-') {
                        writer.write_all(
                            numeric(SERVER_NAME, 432, nick.as_deref().unwrap_or("*"), &["Erroneous Nickname"]).as_bytes(),
                        ).await?;
                        continue;
                    }
                    {
                        let mut g = shared.lock().await;
                        if let Some(other) = g.nicks.get(&desired) {
                            if *other != conn_id {
                                writer.write_all(
                                    numeric(SERVER_NAME, 433, nick.as_deref().unwrap_or("*"), &[desired.as_str(), "Nickname is already in use"]).as_bytes(),
                                ).await?;
                                continue;
                            }
                        }
                        if let Some(old) = nick.take() {
                            g.nicks.remove(&old);
                        }
                        g.nicks.insert(desired.clone(), conn_id);
                    }
                    nick = Some(desired);
                    try_register(
                        &mut writer,
                        &mut registered,
                        nick.as_deref(),
                        user.as_deref(),
                        realname.as_deref(),
                    ).await?;
                    continue;
                }

                if msg.command_eq("USER") {
                    user = msg.params.first().cloned();
                    realname = msg.params.get(3).cloned().or_else(|| msg.params.last().cloned());
                    try_register(
                        &mut writer,
                        &mut registered,
                        nick.as_deref(),
                        user.as_deref(),
                        realname.as_deref(),
                    ).await?;
                    continue;
                }

                if msg.command_eq("PING") {
                    let token = msg.params.first().map(String::as_str).unwrap_or(SERVER_NAME);
                    writer.write_all(format!("PONG :{token}\r\n").as_bytes()).await?;
                    continue;
                }

                if msg.command_eq("QUIT") {
                    break;
                }

                if !registered {
                    writer.write_all(
                        numeric(SERVER_NAME, 451, nick.as_deref().unwrap_or("*"), &["You have not registered"]).as_bytes(),
                    ).await?;
                    continue;
                }

                let nick_s = nick.as_deref().unwrap_or("*");
                let user_s = user.as_deref().unwrap_or("user");
                let host = "dsc.local";
                let prefix = format!("{nick_s}!{user_s}@{host}");

                if msg.command_eq("JOIN") {
                    let Some(chan_list) = msg.params.first() else { continue };
                    for chan in chan_list.split(',') {
                        let chan = chan.trim();
                        if !chan.starts_with('#') {
                            continue;
                        }
                        {
                            let mut g = shared.lock().await;
                            g.channels.entry(chan.to_string()).or_default().insert(nick_s.to_string());
                        }
                        channels.insert(chan.to_string());
                        let join_line = format!(":{prefix} JOIN :{chan}\r\n");
                        writer.write_all(join_line.as_bytes()).await?;
                        let _ = shared.lock().await.bus.send(BusMsg {
                            target: chan.to_string(),
                            line: join_line,
                            skip_conn: conn_id,
                        });
                        writer.write_all(
                            numeric(SERVER_NAME, 332, nick_s, &[chan, "dsc-ircd v0 test channel"]).as_bytes(),
                        ).await?;
                        writer.write_all(
                            numeric(SERVER_NAME, 353, nick_s, &["=", chan, nick_s]).as_bytes(),
                        ).await?;
                        writer.write_all(
                            numeric(SERVER_NAME, 366, nick_s, &[chan, "End of /NAMES list"]).as_bytes(),
                        ).await?;
                    }
                    continue;
                }

                if msg.command_eq("PRIVMSG") {
                    let (Some(target), Some(text)) = (msg.params.first(), msg.params.get(1)) else {
                        continue;
                    };
                    if !target.starts_with('#') || !channels.contains(target) {
                        writer.write_all(
                            numeric(SERVER_NAME, 404, nick_s, &[target.as_str(), "Cannot send to channel"]).as_bytes(),
                        ).await?;
                        continue;
                    }
                    let line = format!(":{prefix} PRIVMSG {target} :{text}\r\n");
                    let _ = shared.lock().await.bus.send(BusMsg {
                        target: target.clone(),
                        line,
                        skip_conn: conn_id,
                    });
                    continue;
                }

                if msg.command_eq("PART") {
                    let Some(chan) = msg.params.first() else { continue };
                    channels.remove(chan);
                    {
                        let mut g = shared.lock().await;
                        if let Some(members) = g.channels.get_mut(chan) {
                            members.remove(nick_s);
                        }
                    }
                    let line = format!(":{prefix} PART {chan}\r\n");
                    writer.write_all(line.as_bytes()).await?;
                    let _ = shared.lock().await.bus.send(BusMsg {
                        target: chan.clone(),
                        line,
                        skip_conn: conn_id,
                    });
                    continue;
                }

                // Unknown command — soft ignore for v0 noise.
            }
        }
    }

    // cleanup
    if let Some(n) = nick {
        let mut g = shared.lock().await;
        g.nicks.remove(&n);
        for chan in &channels {
            if let Some(members) = g.channels.get_mut(chan) {
                members.remove(&n);
            }
            let user_s = user.as_deref().unwrap_or("user");
            let line = format!(":{n}!{user_s}@dsc.local QUIT :Connection closed\r\n");
            let _ = g.bus.send(BusMsg {
                target: chan.clone(),
                line,
                skip_conn: conn_id,
            });
        }
    }

    Ok(())
}

async fn try_register(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    registered: &mut bool,
    nick: Option<&str>,
    user: Option<&str>,
    _realname: Option<&str>,
) -> Result<()> {
    if *registered {
        return Ok(());
    }
    let (Some(nick), Some(_user)) = (nick, user) else {
        return Ok(());
    };
    *registered = true;
    writer
        .write_all(
            numeric(
                SERVER_NAME,
                1,
                nick,
                &[&format!("Welcome to the DSC Internet Relay Network {nick}")],
            )
            .as_bytes(),
        )
        .await?;
    writer
        .write_all(
            numeric(
                SERVER_NAME,
                2,
                nick,
                &[&format!("Your host is {SERVER_NAME}, running version dsc-ircd-{VERSION}")],
            )
            .as_bytes(),
        )
        .await?;
    writer
        .write_all(
            numeric(
                SERVER_NAME,
                3,
                nick,
                &["This server was created for the Mark × Cody IRC rebuild"],
            )
            .as_bytes(),
        )
        .await?;
    writer
        .write_all(
            numeric(
                SERVER_NAME,
                4,
                nick,
                &[SERVER_NAME, &format!("dsc-ircd-{VERSION}"), "i", "nt"],
            )
            .as_bytes(),
        )
        .await?;
    writer
        .write_all(
            numeric(
                SERVER_NAME,
                376,
                nick,
                &["End of /MOTD command"],
            )
            .as_bytes(),
        )
        .await?;
    info!(%nick, "client registered");
    Ok(())
}
