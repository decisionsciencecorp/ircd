//! Per-connection IRC session.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use ircd_core::{numeric, server_notice, RawLine};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tracing::info;

use crate::{BusMsg, Shared, VERSION};

/// Starter IRCv3 caps (advertise even when payload is still thin).
const SUPPORTED_CAPS: &[&str] = &[
    "multi-prefix",
    "server-time",
    "message-tags",
    "away-notify",
];

pub async fn handle_client<R, W>(
    reader: R,
    mut writer: W,
    peer: SocketAddr,
    shared: Arc<Mutex<Shared>>,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let (conn_id, cfg) = {
        let mut g = shared.lock().await;
        let id = g.next_id;
        g.next_id += 1;
        (id, Arc::clone(&g.config))
    };
    let server_name = cfg.server.name.as_str();

    let mut reader = BufReader::new(reader);
    let mut bus_rx = shared.lock().await.bus.subscribe();

    writer
        .write_all(
            server_notice(
                server_name,
                &format!("*** dsc-ircd {VERSION} — Unreal-inspired clean-room Rust IRCd"),
            )
            .as_bytes(),
        )
        .await?;
    writer
        .write_all(server_notice(server_name, "*** Looking up your hostname...").as_bytes())
        .await?;
    writer
        .write_all(
            server_notice(
                server_name,
                &format!("*** Connected as peer {peer}; hostname checks deferred in v0"),
            )
            .as_bytes(),
        )
        .await?;

    let mut nick: Option<String> = None;
    let mut user: Option<String> = None;
    let mut realname: Option<String> = None;
    let mut registered = false;
    let mut cap_negotiating = false;
    let mut enabled_caps: HashSet<String> = HashSet::new();
    let mut line_buf = String::new();
    let mut channels: HashSet<String> = HashSet::new();

    loop {
        tokio::select! {
            bus = bus_rx.recv() => {
                match bus {
                    Ok(msg) if msg.skip_conn != conn_id && channels.contains(&msg.target) => {
                        let line = if enabled_caps.contains("server-time") {
                            tag_server_time(&msg.line)
                        } else {
                            msg.line.clone()
                        };
                        writer.write_all(line.as_bytes()).await?;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
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
                    handle_cap(
                        &mut writer,
                        server_name,
                        &msg,
                        nick.as_deref(),
                        &mut cap_negotiating,
                        &mut enabled_caps,
                    ).await?;
                    if msg.params.first().map(|s| s.eq_ignore_ascii_case("END")).unwrap_or(false) {
                        try_register(
                            &mut writer,
                            server_name,
                            &cfg.server.motd,
                            &mut registered,
                            nick.as_deref(),
                            user.as_deref(),
                            realname.as_deref(),
                            cap_negotiating,
                        ).await?;
                    }
                    continue;
                }

                if msg.command_eq("NICK") {
                    let Some(desired) = msg.params.first().cloned() else { continue };
                    if desired.is_empty()
                        || desired.len() > cfg.server.max_nick_length
                        || desired.contains(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
                    {
                        writer.write_all(
                            numeric(server_name, 432, nick.as_deref().unwrap_or("*"), &["Erroneous Nickname"]).as_bytes(),
                        ).await?;
                        continue;
                    }
                    {
                        let mut g = shared.lock().await;
                        if let Some(other) = g.nicks.get(&desired) {
                            if *other != conn_id {
                                writer.write_all(
                                    numeric(server_name, 433, nick.as_deref().unwrap_or("*"), &[desired.as_str(), "Nickname is already in use"]).as_bytes(),
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
                        server_name,
                        &cfg.server.motd,
                        &mut registered,
                        nick.as_deref(),
                        user.as_deref(),
                        realname.as_deref(),
                        cap_negotiating,
                    ).await?;
                    continue;
                }

                if msg.command_eq("USER") {
                    user = msg.params.first().cloned();
                    realname = msg.params.get(3).cloned().or_else(|| msg.params.last().cloned());
                    try_register(
                        &mut writer,
                        server_name,
                        &cfg.server.motd,
                        &mut registered,
                        nick.as_deref(),
                        user.as_deref(),
                        realname.as_deref(),
                        cap_negotiating,
                    ).await?;
                    continue;
                }

                if msg.command_eq("PING") {
                    let token = msg.params.first().map(String::as_str).unwrap_or(server_name);
                    writer.write_all(format!("PONG :{token}\r\n").as_bytes()).await?;
                    continue;
                }

                if msg.command_eq("QUIT") {
                    break;
                }

                if !registered {
                    writer.write_all(
                        numeric(server_name, 451, nick.as_deref().unwrap_or("*"), &["You have not registered"]).as_bytes(),
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
                        if !chan.starts_with('#')
                            || chan.len() > cfg.server.max_channel_length
                        {
                            continue;
                        }
                        let names_list = {
                            let mut g = shared.lock().await;
                            g.channels
                                .entry(chan.to_string())
                                .or_default()
                                .insert(nick_s.to_string());
                            let mut names: Vec<String> = g
                                .channels
                                .get(chan)
                                .map(|m| m.iter().cloned().collect())
                                .unwrap_or_default();
                            names.sort();
                            // multi-prefix: no channel modes yet — plain nicks.
                            names.join(" ")
                        };
                        channels.insert(chan.to_string());
                        let join_line = format!(":{prefix} JOIN :{chan}\r\n");
                        writer.write_all(join_line.as_bytes()).await?;
                        let _ = shared.lock().await.bus.send(BusMsg {
                            target: chan.to_string(),
                            line: join_line,
                            skip_conn: conn_id,
                        });
                        writer.write_all(
                            numeric(server_name, 332, nick_s, &[chan, "dsc-ircd v0 test channel"]).as_bytes(),
                        ).await?;
                        writer.write_all(
                            numeric(server_name, 353, nick_s, &["=", chan, names_list.as_str()]).as_bytes(),
                        ).await?;
                        writer.write_all(
                            numeric(server_name, 366, nick_s, &[chan, "End of /NAMES list"]).as_bytes(),
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
                            numeric(server_name, 404, nick_s, &[target.as_str(), "Cannot send to channel"]).as_bytes(),
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
            }
        }
    }

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

async fn handle_cap<W>(
    writer: &mut W,
    server_name: &str,
    msg: &RawLine,
    nick: Option<&str>,
    cap_negotiating: &mut bool,
    enabled_caps: &mut HashSet<String>,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let nick_s = nick.unwrap_or("*");
    let sub = msg.params.first().map(String::as_str).unwrap_or("");
    if sub.eq_ignore_ascii_case("LS") {
        *cap_negotiating = true;
        let list = SUPPORTED_CAPS.join(" ");
        writer
            .write_all(format!(":{server_name} CAP {nick_s} LS :{list}\r\n").as_bytes())
            .await?;
        return Ok(());
    }
    if sub.eq_ignore_ascii_case("LIST") {
        let list = enabled_caps.iter().cloned().collect::<Vec<_>>().join(" ");
        writer
            .write_all(format!(":{server_name} CAP {nick_s} LIST :{list}\r\n").as_bytes())
            .await?;
        return Ok(());
    }
    if sub.eq_ignore_ascii_case("REQ") {
        *cap_negotiating = true;
        let mut ack = Vec::new();
        let mut nak = Vec::new();
        let tokens = msg
            .params
            .get(1)
            .map(String::as_str)
            .unwrap_or("")
            .split_whitespace();
        for tok in tokens {
            let disable = tok.starts_with('-');
            let name = tok.trim_start_matches('-');
            if !SUPPORTED_CAPS.iter().any(|c| *c == name) {
                nak.push(tok.to_string());
                continue;
            }
            if disable {
                enabled_caps.remove(name);
            } else {
                enabled_caps.insert(name.to_string());
            }
            ack.push(tok.to_string());
        }
        if !ack.is_empty() {
            writer
                .write_all(
                    format!(":{server_name} CAP {nick_s} ACK :{}\r\n", ack.join(" ")).as_bytes(),
                )
                .await?;
        }
        if !nak.is_empty() {
            writer
                .write_all(
                    format!(":{server_name} CAP {nick_s} NAK :{}\r\n", nak.join(" ")).as_bytes(),
                )
                .await?;
        }
        return Ok(());
    }
    if sub.eq_ignore_ascii_case("END") {
        *cap_negotiating = false;
        return Ok(());
    }
    writer
        .write_all(
            numeric(
                server_name,
                410,
                nick_s,
                &[sub, "Invalid CAP subcommand"],
            )
            .as_bytes(),
        )
        .await?;
    Ok(())
}

fn tag_server_time(line: &str) -> String {
    // RFC3339-ish UTC; millisecond precision is enough for lab.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let millis = now.subsec_millis();
    // Keep formatting simple without chrono dependency.
    let days = secs / 86400;
    let tod = secs % 86400;
    let hour = tod / 3600;
    let min = (tod % 3600) / 60;
    let sec = tod % 60;
    // Civil date from Unix day (algorithm from Howard Hinnant)
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let stamp = format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}.{millis:03}Z");
    if let Some(rest) = line.strip_prefix('@') {
        // already tagged
        format!("@time={stamp};{rest}")
    } else {
        format!("@time={stamp} {line}")
    }
}

async fn try_register<W>(
    writer: &mut W,
    server_name: &str,
    motd: &str,
    registered: &mut bool,
    nick: Option<&str>,
    user: Option<&str>,
    _realname: Option<&str>,
    cap_negotiating: bool,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    if *registered {
        return Ok(());
    }
    if cap_negotiating {
        return Ok(());
    }
    let (Some(nick), Some(_user)) = (nick, user) else {
        return Ok(());
    };
    *registered = true;
    writer
        .write_all(
            numeric(
                server_name,
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
                server_name,
                2,
                nick,
                &[&format!(
                    "Your host is {server_name}, running version dsc-ircd-{VERSION}"
                )],
            )
            .as_bytes(),
        )
        .await?;
    writer
        .write_all(
            numeric(
                server_name,
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
                server_name,
                4,
                nick,
                &[server_name, &format!("dsc-ircd-{VERSION}"), "i", "nt"],
            )
            .as_bytes(),
        )
        .await?;
    let motd_start = format!("- {server_name} Message of the day -");
    writer
        .write_all(numeric(server_name, 375, nick, &[motd_start.as_str()]).as_bytes())
        .await?;
    for line in motd.lines() {
        let body = format!("- {line}");
        writer
            .write_all(numeric(server_name, 372, nick, &[body.as_str()]).as_bytes())
            .await?;
    }
    writer
        .write_all(numeric(server_name, 376, nick, &["End of /MOTD command"]).as_bytes())
        .await?;
    info!(%nick, "client registered");
    Ok(())
}
