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
const BASE_CAPS: &[&str] = &[
    "multi-prefix",
    "server-time",
    "message-tags",
    "away-notify",
    "batch",
    "chathistory",
    "account-tag",
];

fn advertised_caps(has_accounts: bool) -> Vec<String> {
    let mut caps: Vec<String> = BASE_CAPS.iter().map(|s| (*s).to_string()).collect();
    if has_accounts {
        caps.push("sasl=PLAIN".to_string());
    }
    caps
}

fn cap_name_matches(requested: &str, advertised: &str) -> bool {
    let req = requested.split('=').next().unwrap_or(requested);
    let adv = advertised.split('=').next().unwrap_or(advertised);
    req.eq_ignore_ascii_case(adv)
}

fn has_cap(enabled: &HashSet<String>, name: &str) -> bool {
    enabled
        .iter()
        .any(|c| c.split('=').next().unwrap_or(c).eq_ignore_ascii_case(name))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SaslState {
    Idle,
    AwaitPlain,
}

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
    let mut is_oper = false;
    let mut cap_negotiating = false;
    let mut enabled_caps: HashSet<String> = HashSet::new();
    let mut line_buf = String::new();
    let mut channels: HashSet<String> = HashSet::new();
    let mut account: Option<String> = None;
    let mut sasl_state = SaslState::Idle;
    let has_accounts = !cfg.accounts.is_empty();
    let cap_list = advertised_caps(has_accounts);

    loop {
        tokio::select! {
            bus = bus_rx.recv() => {
                match bus {
                    Ok(msg) if msg.skip_conn != conn_id && channels.contains(&msg.target) => {
                        let line = adapt_bus_line(&msg.line, &enabled_caps);
                        writer.write_all(line.as_bytes()).await?;
                        // Drop local membership if we were kicked.
                        if let Some(n) = nick.as_deref() {
                            if let Some(raw) = ircd_core::RawLine::parse(line.trim_end_matches(['\r','\n'])) {
                                if raw.command_eq("KICK")
                                    && raw.params.get(1).map(|t| t.as_str()) == Some(n)
                                {
                                    channels.remove(&msg.target);
                                }
                            }
                        }
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
                        &cap_list,
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

                if msg.command_eq("AUTHENTICATE") {
                    handle_authenticate(
                        &mut writer,
                        server_name,
                        &msg,
                        nick.as_deref(),
                        user.as_deref(),
                        &cfg.accounts,
                        &enabled_caps,
                        &mut sasl_state,
                        &mut account,
                    )
                    .await?;
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

                if msg.command_eq("ADMIN") {
                    writer.write_all(
                        numeric(server_name, 256, nick_s, &[&format!("Administrative info about {server_name}")]).as_bytes(),
                    ).await?;
                    let admin = if cfg.server.admin_name.is_empty() {
                        "DSC Admin"
                    } else {
                        cfg.server.admin_name.as_str()
                    };
                    let email = if cfg.server.admin_email.is_empty() {
                        "unset@localhost"
                    } else {
                        cfg.server.admin_email.as_str()
                    };
                    writer.write_all(
                        numeric(server_name, 257, nick_s, &[&format!("{admin}")]).as_bytes(),
                    ).await?;
                    writer.write_all(
                        numeric(server_name, 258, nick_s, &["Decision Science Corp"]).as_bytes(),
                    ).await?;
                    writer.write_all(
                        numeric(server_name, 259, nick_s, &[email]).as_bytes(),
                    ).await?;
                    continue;
                }

                if msg.command_eq("OPER") {
                    let (Some(name), Some(pass)) = (msg.params.first(), msg.params.get(1)) else {
                        writer.write_all(
                            numeric(server_name, 461, nick_s, &["OPER", "Not enough parameters"]).as_bytes(),
                        ).await?;
                        continue;
                    };
                    let oper = &cfg.oper;
                    if !oper.enabled || oper.name.is_empty() {
                        writer.write_all(
                            numeric(server_name, 491, nick_s, &["No O-lines for your host"]).as_bytes(),
                        ).await?;
                        continue;
                    }
                    if name == &oper.name && pass == &oper.password {
                        is_oper = true;
                        writer.write_all(
                            numeric(server_name, 381, nick_s, &["You are now an IRC operator"]).as_bytes(),
                        ).await?;
                        info!(%nick_s, "client obtained OPER");
                    } else {
                        writer.write_all(
                            numeric(server_name, 464, nick_s, &["Password incorrect"]).as_bytes(),
                        ).await?;
                    }
                    continue;
                }

                if msg.command_eq("JOIN") {
                    let Some(chan_list) = msg.params.first() else { continue };
                    for chan in chan_list.split(',') {
                        let chan = chan.trim();
                        if !chan.starts_with('#')
                            || chan.len() > cfg.server.max_channel_length
                        {
                            continue;
                        }
                        let (names_list, topic, already) = {
                            let mut g = shared.lock().await;
                            let ch = g.channels.entry(chan.to_string()).or_default();
                            let already = ch.members.contains(nick_s);
                            if !already {
                                let first = ch.members.is_empty();
                                ch.members.insert(nick_s.to_string());
                                if first {
                                    ch.ops.insert(nick_s.to_string());
                                }
                            }
                            (ch.names_prefixed(), ch.topic.clone(), already)
                        };
                        if already {
                            continue;
                        }
                        channels.insert(chan.to_string());
                        let join_line = format!(":{prefix} JOIN :{chan}\r\n");
                        writer.write_all(join_line.as_bytes()).await?;
                        let _ = shared.lock().await.bus.send(BusMsg {
                            target: chan.to_string(),
                            line: join_line,
                            skip_conn: conn_id,
                        });
                        match &topic {
                            Some(t) => {
                                writer.write_all(
                                    numeric(server_name, 332, nick_s, &[chan, t.as_str()]).as_bytes(),
                                ).await?;
                            }
                            None => {
                                writer.write_all(
                                    numeric(server_name, 331, nick_s, &[chan, "No topic is set"]).as_bytes(),
                                ).await?;
                            }
                        }
                        writer.write_all(
                            numeric(server_name, 353, nick_s, &["=", chan, names_list.as_str()]).as_bytes(),
                        ).await?;
                        writer.write_all(
                            numeric(server_name, 366, nick_s, &[chan, "End of /NAMES list"]).as_bytes(),
                        ).await?;
                        // Auto-replay recent channel history (Ergo-style convenience).
                        let (hist, limit) = {
                            let g = shared.lock().await;
                            (
                                g.history.clone(),
                                g.config.history.auto_replay_on_join,
                            )
                        };
                        if let (Some(store), lim) = (hist, limit) {
                            if lim > 0 {
                                if let Ok(rows) = store.latest(chan, lim) {
                                    for h in rows {
                                        let line = adapt_bus_line(&h.tagged_privmsg(), &enabled_caps);
                                        writer.write_all(line.as_bytes()).await?;
                                    }
                                }
                            }
                        }
                    }
                    continue;
                }

                if msg.command_eq("CHATHISTORY") {
                    let sub = msg.params.first().map(String::as_str).unwrap_or("");
                    if !sub.eq_ignore_ascii_case("LATEST") {
                        writer.write_all(
                            numeric(
                                server_name,
                                400,
                                nick_s,
                                &["CHATHISTORY", "Only LATEST is implemented"],
                            )
                            .as_bytes(),
                        )
                        .await?;
                        continue;
                    }
                    let target = msg.params.get(1).map(String::as_str).unwrap_or("");
                    let limit = msg
                        .params
                        .get(3)
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or(50)
                        .clamp(1, 200);
                    if !target.starts_with('#') || !channels.contains(target) {
                        writer.write_all(
                            numeric(
                                server_name,
                                442,
                                nick_s,
                                &[target, "You're not on that channel"],
                            )
                            .as_bytes(),
                        )
                        .await?;
                        continue;
                    }
                    let store = { shared.lock().await.history.clone() };
                    let Some(store) = store else {
                        writer.write_all(
                            numeric(
                                server_name,
                                400,
                                nick_s,
                                &["CHATHISTORY", "History is disabled"],
                            )
                            .as_bytes(),
                        )
                        .await?;
                        continue;
                    };
                    let rows = store.latest(target, limit).unwrap_or_default();
                    let batch_id = format!("ch{}", conn_id);
                    let use_batch = enabled_caps.contains("batch");
                    if use_batch {
                        writer
                            .write_all(
                                format!(
                                    ":{server_name} BATCH +{batch_id} chathistory {target}\r\n"
                                )
                                .as_bytes(),
                            )
                            .await?;
                    }
                    for h in rows {
                        let line = adapt_bus_line(&h.tagged_privmsg(), &enabled_caps);
                        writer.write_all(line.as_bytes()).await?;
                    }
                    if use_batch {
                        writer
                            .write_all(
                                format!(":{server_name} BATCH -{batch_id}\r\n").as_bytes(),
                            )
                            .await?;
                    }
                    continue;
                }

                if msg.command_eq("PRIVMSG") {
                    let (Some(target), Some(text)) = (msg.params.first(), msg.params.get(1)) else {
                        continue;
                    };
                    if target.starts_with('#') {
                        let (allowed, hist) = {
                            let g = shared.lock().await;
                            let allowed = match g.channels.get(target.as_str()) {
                                Some(ch) if ch.members.contains(nick_s) => true,
                                Some(ch) if !ch.mode_n => true,
                                Some(_) => false,
                                None => false,
                            };
                            (allowed, g.history.clone())
                        };
                        if !allowed {
                            writer.write_all(
                                numeric(server_name, 404, nick_s, &[target.as_str(), "Cannot send to channel"]).as_bytes(),
                            ).await?;
                            continue;
                        }
                        let mut line = if let Some(store) = hist {
                            match store.append(target, &prefix, text) {
                                Ok(h) => h.tagged_privmsg(),
                                Err(_) => format!(":{prefix} PRIVMSG {target} :{text}\r\n"),
                            }
                        } else {
                            format!(":{prefix} PRIVMSG {target} :{text}\r\n")
                        };
                        if let Some(acc) = account.as_deref() {
                            line = prepend_tag(&line, "account", acc);
                        }
                        let _ = shared.lock().await.bus.send(BusMsg {
                            target: target.clone(),
                            line,
                            skip_conn: conn_id,
                        });
                    }
                    continue;
                }

                if msg.command_eq("PART") {
                    let Some(chan) = msg.params.first() else { continue };
                    channels.remove(chan);
                    {
                        let mut g = shared.lock().await;
                        if let Some(ch) = g.channels.get_mut(chan) {
                            ch.remove_nick(nick_s);
                            if ch.members.is_empty() {
                                g.channels.remove(chan);
                            }
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

                if msg.command_eq("TOPIC") {
                    let Some(chan) = msg.params.first() else { continue };
                    if !channels.contains(chan) {
                        writer.write_all(
                            numeric(server_name, 442, nick_s, &[chan.as_str(), "You're not on that channel"]).as_bytes(),
                        ).await?;
                        continue;
                    }
                    if msg.params.len() == 1 {
                        let topic = {
                            let g = shared.lock().await;
                            g.channels.get(chan.as_str()).and_then(|c| c.topic.clone())
                        };
                        match topic {
                            Some(t) => {
                                writer.write_all(
                                    numeric(server_name, 332, nick_s, &[chan.as_str(), t.as_str()]).as_bytes(),
                                ).await?;
                            }
                            None => {
                                writer.write_all(
                                    numeric(server_name, 331, nick_s, &[chan.as_str(), "No topic is set"]).as_bytes(),
                                ).await?;
                            }
                        }
                        continue;
                    }
                    let new_topic = msg.params.get(1).cloned().unwrap_or_default();
                    let allowed = {
                        let g = shared.lock().await;
                        match g.channels.get(chan.as_str()) {
                            Some(ch) if is_oper || ch.is_op(nick_s) => true,
                            Some(ch) if !ch.mode_t => true,
                            _ => false,
                        }
                    };
                    if !allowed {
                        writer.write_all(
                            numeric(server_name, 482, nick_s, &[chan.as_str(), "You're not channel operator"]).as_bytes(),
                        ).await?;
                        continue;
                    }
                    {
                        let mut g = shared.lock().await;
                        if let Some(ch) = g.channels.get_mut(chan.as_str()) {
                            if new_topic.is_empty() {
                                ch.topic = None;
                            } else {
                                ch.topic = Some(new_topic.clone());
                            }
                        }
                    }
                    let line = format!(":{prefix} TOPIC {chan} :{new_topic}\r\n");
                    writer.write_all(line.as_bytes()).await?;
                    let _ = shared.lock().await.bus.send(BusMsg {
                        target: chan.clone(),
                        line,
                        skip_conn: conn_id,
                    });
                    continue;
                }

                if msg.command_eq("KICK") {
                    let (Some(chan), Some(target_nick)) = (msg.params.first(), msg.params.get(1)) else {
                        writer.write_all(
                            numeric(server_name, 461, nick_s, &["KICK", "Not enough parameters"]).as_bytes(),
                        ).await?;
                        continue;
                    };
                    let reason = msg.params.get(2).map(String::as_str).unwrap_or(nick_s);
                    let allowed = {
                        let g = shared.lock().await;
                        match g.channels.get(chan.as_str()) {
                            Some(ch) if ch.members.contains(target_nick) => {
                                is_oper || ch.is_op(nick_s)
                            }
                            _ => false,
                        }
                    };
                    if !allowed {
                        let g = shared.lock().await;
                        if g.channels.get(chan.as_str()).map(|c| c.members.contains(target_nick)) != Some(true) {
                            writer.write_all(
                                numeric(server_name, 441, nick_s, &[target_nick.as_str(), chan.as_str(), "They aren't on that channel"]).as_bytes(),
                            ).await?;
                        } else {
                            writer.write_all(
                                numeric(server_name, 482, nick_s, &[chan.as_str(), "You're not channel operator"]).as_bytes(),
                            ).await?;
                        }
                        continue;
                    }
                    {
                        let mut g = shared.lock().await;
                        if let Some(ch) = g.channels.get_mut(chan.as_str()) {
                            ch.remove_nick(target_nick);
                            if ch.members.is_empty() {
                                g.channels.remove(chan.as_str());
                            }
                        }
                    }
                    let line = format!(":{prefix} KICK {chan} {target_nick} :{reason}\r\n");
                    // Deliver to kicker too (standard).
                    writer.write_all(line.as_bytes()).await?;
                    let _ = shared.lock().await.bus.send(BusMsg {
                        target: chan.clone(),
                        line,
                        skip_conn: conn_id,
                    });
                    continue;
                }

                if msg.command_eq("MODE") {
                    let Some(target) = msg.params.first() else { continue };
                    if !target.starts_with('#') {
                        // user modes: ignore for v0 except oper might set +o later
                        continue;
                    }
                    if msg.params.len() == 1 {
                        let modes = {
                            let g = shared.lock().await;
                            g.channels
                                .get(target.as_str())
                                .map(|c| c.mode_chars())
                                .unwrap_or_else(|| "+nt".into())
                        };
                        writer.write_all(
                            numeric(server_name, 324, nick_s, &[target.as_str(), modes.as_str()]).as_bytes(),
                        ).await?;
                        continue;
                    }
                    let mode_str = msg.params.get(1).map(String::as_str).unwrap_or("");
                    let mode_arg = msg.params.get(2).cloned();
                    let privileged = {
                        let g = shared.lock().await;
                        match g.channels.get(target.as_str()) {
                            Some(ch) => is_oper || ch.is_op(nick_s),
                            None => false,
                        }
                    };
                    if !privileged {
                        writer.write_all(
                            numeric(server_name, 482, nick_s, &[target.as_str(), "You're not channel operator"]).as_bytes(),
                        ).await?;
                        continue;
                    }

                    let mut adding = true;
                    let mut applied = String::new();
                    let mut applied_args: Vec<String> = Vec::new();
                    for ch in mode_str.chars() {
                        match ch {
                            '+' => adding = true,
                            '-' => adding = false,
                            'o' => {
                                let Some(who) = mode_arg.clone() else { continue };
                                let mut g = shared.lock().await;
                                let Some(chan) = g.channels.get_mut(target.as_str()) else { continue };
                                if !chan.members.contains(&who) {
                                    drop(g);
                                    writer.write_all(
                                        numeric(server_name, 441, nick_s, &[who.as_str(), target.as_str(), "They aren't on that channel"]).as_bytes(),
                                    ).await?;
                                    continue;
                                }
                                if adding {
                                    chan.ops.insert(who.clone());
                                } else {
                                    chan.ops.remove(&who);
                                }
                                applied.push(if adding { '+' } else { '-' });
                                applied.push('o');
                                applied_args.push(who);
                            }
                            't' => {
                                let mut g = shared.lock().await;
                                if let Some(chan) = g.channels.get_mut(target.as_str()) {
                                    chan.mode_t = adding;
                                    applied.push(if adding { '+' } else { '-' });
                                    applied.push('t');
                                }
                            }
                            'n' => {
                                let mut g = shared.lock().await;
                                if let Some(chan) = g.channels.get_mut(target.as_str()) {
                                    chan.mode_n = adding;
                                    applied.push(if adding { '+' } else { '-' });
                                    applied.push('n');
                                }
                            }
                            _ => {
                                writer.write_all(
                                    numeric(server_name, 472, nick_s, &[&ch.to_string(), "is unknown mode char to me"]).as_bytes(),
                                ).await?;
                            }
                        }
                    }
                    if !applied.is_empty() {
                        let args = if applied_args.is_empty() {
                            String::new()
                        } else {
                            format!(" {}", applied_args.join(" "))
                        };
                        let line = format!(":{prefix} MODE {target} {applied}{args}\r\n");
                        writer.write_all(line.as_bytes()).await?;
                        let _ = shared.lock().await.bus.send(BusMsg {
                            target: target.clone(),
                            line,
                            skip_conn: conn_id,
                        });
                    }
                    continue;
                }
            }
        }
    }

    if let Some(n) = nick {
        let mut g = shared.lock().await;
        g.nicks.remove(&n);
        for chan in &channels {
            if let Some(ch) = g.channels.get_mut(chan) {
                ch.remove_nick(&n);
                let empty = ch.members.is_empty();
                if empty {
                    // remove after send prep
                }
            }
            let user_s = user.as_deref().unwrap_or("user");
            let line = format!(":{n}!{user_s}@dsc.local QUIT :Connection closed\r\n");
            let _ = g.bus.send(BusMsg {
                target: chan.clone(),
                line,
                skip_conn: conn_id,
            });
            if g.channels.get(chan).map(|c| c.members.is_empty()).unwrap_or(false) {
                g.channels.remove(chan);
            }
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
    advertised: &[String],
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let nick_s = nick.unwrap_or("*");
    let sub = msg.params.first().map(String::as_str).unwrap_or("");
    if sub.eq_ignore_ascii_case("LS") {
        *cap_negotiating = true;
        let list = advertised.join(" ");
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
            let known = advertised.iter().any(|c| cap_name_matches(name, c));
            if !known {
                nak.push(tok.to_string());
                continue;
            }
            let canon = name.split('=').next().unwrap_or(name).to_string();
            if disable {
                enabled_caps.retain(|c| !cap_name_matches(c, &canon));
            } else {
                enabled_caps.insert(canon);
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

async fn handle_authenticate<W>(
    writer: &mut W,
    server_name: &str,
    msg: &RawLine,
    nick: Option<&str>,
    user: Option<&str>,
    accounts: &[crate::config::AccountSection],
    enabled_caps: &HashSet<String>,
    sasl_state: &mut SaslState,
    account: &mut Option<String>,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    use base64::Engine;

    let nick_s = nick.unwrap_or("*");
    if !has_cap(enabled_caps, "sasl") {
        writer
            .write_all(
                numeric(
                    server_name,
                    904,
                    nick_s,
                    &["SASL authentication failed"],
                )
                .as_bytes(),
            )
            .await?;
        return Ok(());
    }
    let param = msg.params.first().map(String::as_str).unwrap_or("");
    match *sasl_state {
        SaslState::Idle => {
            if param.eq_ignore_ascii_case("PLAIN") {
                *sasl_state = SaslState::AwaitPlain;
                writer
                    .write_all(b"AUTHENTICATE +\r\n")
                    .await?;
            } else if param == "*" {
                *sasl_state = SaslState::Idle;
                writer
                    .write_all(
                        numeric(server_name, 906, nick_s, &["SASL authentication aborted"])
                            .as_bytes(),
                    )
                    .await?;
            } else {
                writer
                    .write_all(
                        numeric(
                            server_name,
                            908,
                            nick_s,
                            &["PLAIN", "are the available SASL mechanisms"],
                        )
                        .as_bytes(),
                    )
                    .await?;
            }
        }
        SaslState::AwaitPlain => {
            *sasl_state = SaslState::Idle;
            if param == "*" {
                writer
                    .write_all(
                        numeric(server_name, 906, nick_s, &["SASL authentication aborted"])
                            .as_bytes(),
                    )
                    .await?;
                return Ok(());
            }
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(param) else {
                writer
                    .write_all(
                        numeric(server_name, 904, nick_s, &["SASL authentication failed"])
                            .as_bytes(),
                    )
                    .await?;
                return Ok(());
            };
            // PLAIN: [authzid] \0 authcid \0 password
            let parts: Vec<&[u8]> = bytes.split(|&b| b == 0).collect();
            if parts.len() != 3 {
                writer
                    .write_all(
                        numeric(server_name, 904, nick_s, &["SASL authentication failed"])
                            .as_bytes(),
                    )
                    .await?;
                return Ok(());
            }
            let authcid = String::from_utf8_lossy(parts[1]);
            let password = String::from_utf8_lossy(parts[2]);
            let matched = accounts.iter().find(|a| {
                a.name.eq_ignore_ascii_case(authcid.as_ref()) && a.password == password.as_ref()
            });
            let Some(acc) = matched else {
                writer
                    .write_all(
                        numeric(server_name, 904, nick_s, &["SASL authentication failed"])
                            .as_bytes(),
                    )
                    .await?;
                return Ok(());
            };
            *account = Some(acc.name.clone());
            let user_s = user.unwrap_or("user");
            let host = "dsc.local";
            let full = format!("{nick_s}!{user_s}@{host}");
            writer
                .write_all(
                    numeric(
                        server_name,
                        900,
                        nick_s,
                        &[full.as_str(), acc.name.as_str(), &format!("You are now logged in as {}", acc.name)],
                    )
                    .as_bytes(),
                )
                .await?;
            writer
                .write_all(
                    numeric(server_name, 903, nick_s, &["SASL authentication successful"])
                        .as_bytes(),
                )
                .await?;
            info!(account = %acc.name, nick = %nick_s, "SASL PLAIN success");
        }
    }
    Ok(())
}

fn prepend_tag(line: &str, key: &str, value: &str) -> String {
    let line = line.trim_end_matches(['\r', '\n']);
    if let Some(rest) = line.strip_prefix('@') {
        format!("@{key}={value};{rest}\r\n")
    } else {
        format!("@{key}={value} {line}\r\n")
    }
}

/// Adapt a (possibly tagged) bus line to the client's negotiated caps.
fn adapt_bus_line(line: &str, caps: &HashSet<String>) -> String {
    let (tags, rest) = split_tags(line);
    let Some(tags) = tags else {
        if has_cap(caps, "server-time") {
            return tag_server_time(line);
        }
        return ensure_crlf(line);
    };
    let want_msg = has_cap(caps, "message-tags");
    let want_time = has_cap(caps, "server-time");
    let want_account = has_cap(caps, "account-tag");
    if !want_msg && !want_time && !want_account {
        return ensure_crlf(rest);
    }
    let mut keep = Vec::new();
    for part in tags.split(';') {
        if part.is_empty() {
            continue;
        }
        if part.starts_with("msgid=") && want_msg {
            keep.push(part);
        } else if part.starts_with("time=") && (want_time || want_msg) {
            keep.push(part);
        } else if part.starts_with("account=") && (want_account || want_msg) {
            keep.push(part);
        } else if want_msg
            && !part.starts_with("msgid=")
            && !part.starts_with("time=")
            && !part.starts_with("account=")
        {
            keep.push(part);
        }
    }
    if keep.is_empty() {
        ensure_crlf(rest)
    } else {
        format!("@{} {}\r\n", keep.join(";"), rest)
    }
}

fn split_tags(line: &str) -> (Option<&str>, &str) {
    let line = line.trim_end_matches(['\r', '\n']);
    if let Some(rest) = line.strip_prefix('@') {
        if let Some((tags, body)) = rest.split_once(' ') {
            return (Some(tags), body);
        }
    }
    (None, line)
}

fn ensure_crlf(line: &str) -> String {
    let line = line.trim_end_matches(['\r', '\n']);
    format!("{line}\r\n")
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
