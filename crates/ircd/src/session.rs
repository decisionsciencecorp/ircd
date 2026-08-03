//! Per-connection IRC session.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use ircd_core::tags::{adapt_bus_line, prepend_tag};
use ircd_core::{ascii_casefold, field_has_control, numeric, server_notice, valid_channel_name, Nick, RawLine};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tracing::info;

use crate::state::Shared;
use crate::VERSION;

/// Caps advertised without per-cap conformance tests must stay empty (A1 / Doc #974).
/// SASL is added conditionally in `advertised_caps` when accounts exist.
const BASE_CAPS: &[&str] = &["cap-notify"];

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

/// Apply a CAP REQ token list; returns (ACK tokens, NAK tokens).
fn apply_cap_req(
    req: &str,
    advertised: &[String],
    enabled_caps: &mut HashSet<String>,
) -> (Vec<String>, Vec<String>) {
    let tokens: Vec<&str> = req.split_whitespace().collect();
    if tokens.is_empty() {
        return (Vec::new(), Vec::new());
    }
    // Atomic: any unknown or forbidden disable → NAK entire REQ, no mutations.
    let mut nak_all = false;
    for tok in &tokens {
        let disable = tok.starts_with('-');
        let name = tok.trim_start_matches('-');
        let canon = name.split('=').next().unwrap_or(name);
        if disable && canon.eq_ignore_ascii_case("cap-notify") {
            nak_all = true;
            break;
        }
        let known = advertised.iter().any(|c| cap_name_matches(name, c))
            || canon.eq_ignore_ascii_case("cap-notify");
        if !known {
            nak_all = true;
            break;
        }
    }
    if nak_all {
        return (
            Vec::new(),
            tokens.iter().map(|t| (*t).to_string()).collect(),
        );
    }
    let mut ack = Vec::new();
    for tok in &tokens {
        let disable = tok.starts_with('-');
        let name = tok.trim_start_matches('-');
        let canon = name.split('=').next().unwrap_or(name).to_string();
        if disable {
            if !canon.eq_ignore_ascii_case("cap-notify") {
                enabled_caps.retain(|c| !cap_name_matches(c, &canon));
            }
        } else {
            enabled_caps.insert(canon);
        }
        ack.push((*tok).to_string());
    }
    (ack, Vec::new())
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
    mut reader: R,
    mut writer: W,
    peer: SocketAddr,
    shared: Arc<Mutex<Shared>>,
    secure: bool,
    pre_admitted: bool,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let (conn_id, cfg) = {
        let mut g = shared.lock().await;
        if !pre_admitted {
            if let Err(reason) = g.try_admit(peer) {
                drop(g);
                let _ = writer
                    .write_all(format!("ERROR :Closing Link: [{peer}] ({reason})\r\n").as_bytes())
                    .await;
                info!(%peer, %reason, "connection rejected");
                return Ok(());
            }
        }
        let id = g.next_id;
        g.next_id += 1;
        (id, Arc::clone(&g.config))
    };
    // Admission counters + nick/channel membership cleanup on *all* exits (H-03).
    let _conn_guard = ConnRelease {
        shared: Arc::clone(&shared),
        peer,
        conn_id,
    };
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<String>(crate::state::OUTBOX_CAP);
    {
        let mut g = shared.lock().await;
        g.register_outbox(conn_id, out_tx);
    }
    let server_name = cfg.server.name.as_str();
    let flood_limit = cfg.limits.flood_lines_per_window;
    let flood_window = Duration::from_secs(cfg.limits.flood_window_secs.max(1));
    let mut flood_count = 0u32;
    let mut flood_window_start = Instant::now();
    let session_start = Instant::now();
    let mut last_activity = Instant::now();
    let reg_deadline = crate::admission::registration_deadline(&cfg);
    let idle_deadline = crate::admission::idle_deadline(&cfg);

    // Byte-at-a-time line assembly; outbound via per-id outbox (H-07/H-08).

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
    let mut enabled_caps: HashSet<String> = HashSet::from(["cap-notify".to_string()]);
    let mut line_buf: Vec<u8> = Vec::with_capacity(256);
    let max_line = cfg.limits.max_line_bytes.max(64);
    let mut channels: HashSet<String> = HashSet::new();
    let mut account: Option<String> = None;
    let mut sasl_state = SaslState::Idle;
    let has_accounts = !cfg.accounts.is_empty();
    let cap_list = advertised_caps(has_accounts);

    // Event-driven select! between member outbox and timed reads (H-07/H-08).
    // Coverage must not dictate production scheduling — prefer outbox (biased).
    loop {
        let b = tokio::select! {
            biased;
            maybe = out_rx.recv() => {
                match maybe {
                    None => return Ok(()),
                    Some(raw_line) => {
                        let line = adapt_bus_line(&raw_line, &enabled_caps);
                        writer.write_all(line.as_bytes()).await?;
                        if let Some(n) = nick.as_deref() {
                            if let Some(raw) =
                                ircd_core::RawLine::parse(line.trim_end_matches(['\r', '\n']))
                            {
                                if raw.command_eq("KICK")
                                    && raw.params.get(1).map(|t| t.as_str()) == Some(n)
                                {
                                    if let Some(chan) = raw.params.first() {
                                        channels.remove(chan);
                                    }
                                }
                            }
                        }
                        continue;
                    }
                }
            }
            read = tokio::time::timeout(Duration::from_millis(50), reader.read_u8()) => {
                match read {
                    Ok(Ok(b)) => b,
                    Ok(Err(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                        break;
                    }
                    Ok(Err(e)) => return Err(e.into()),
                    Err(_) => {
                if !registered {
                    if let Some(limit) = reg_deadline {
                        if session_start.elapsed() >= limit {
                            let _ = writer
                                .write_all(b"ERROR :Closing Link: Registration timeout
")
                                .await;
                            break;
                        }
                    }
                } else if let Some(limit) = idle_deadline {
                    if last_activity.elapsed() >= limit {
                        let _ = writer
                            .write_all(b"ERROR :Closing Link: Idle timeout
")
                            .await;
                        break;
                    }
                }
                continue; // timed out — keep partial line_buf, poll outbox via select
                    }
                }
            }
        };
        if line_buf.len() >= max_line {
            let _ = writer
                .write_all(
                    format!("ERROR :Closing Link: [{peer}] (Input line too long)\r\n").as_bytes(),
                )
                .await;
                        let _ = writer.flush().await;
            info!(%peer, max_line, "oversized IRC line; closing");
            break;
        }
        line_buf.push(b);
        if b != b'\n' {
            continue;
        }
        let raw = String::from_utf8_lossy(&line_buf).into_owned();
        line_buf.clear();
        // Simple recv flood guard (line count per window).
        if flood_window_start.elapsed() >= flood_window {
            flood_window_start = Instant::now();
            flood_count = 0;
        }
        flood_count = flood_count.saturating_add(1);
        if flood_count > flood_limit {
            let _ = writer
                .write_all(
                    format!("ERROR :Closing Link: [{peer}] (Excess Flood)\r\n").as_bytes(),
                )
                .await;
            info!(%peer, "excess flood; closing");
            break;
        }
        last_activity = Instant::now();
        let Some(msg) = RawLine::parse(&raw) else {
            continue;
        };

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
                            cfg.server.max_nick_length,
                            cfg.server.max_channel_length,
                        ).await?;
                    }
                    continue;
                }

                if msg.command_eq("AUTHENTICATE") {
                    if !secure && cfg.security.tls_required_for_auth() {
                        writer
                            .write_all(
                                numeric(
                                    server_name,
                                    904,
                                    nick.as_deref().unwrap_or("*"),
                                    &["TLS required for authentication"],
                                )
                                .as_bytes(),
                            )
                            .await?;
                        continue;
                    }
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
                    let Some(desired_raw) = msg.params.first().cloned() else {
                        writer.write_all(
                            numeric(server_name, 431, nick.as_deref().unwrap_or("*"), &["No nickname given"]).as_bytes(),
                        ).await?;
                        continue;
                    };
                    let Ok(parsed) = Nick::parse(&desired_raw, cfg.server.max_nick_length) else {
                        writer.write_all(
                            numeric(server_name, 432, nick.as_deref().unwrap_or("*"), &["Erroneous Nickname"]).as_bytes(),
                        ).await?;
                        continue;
                    };
                    let desired = parsed.as_str().to_string();
                    let desired_key = parsed.key().as_str().to_string();
                    let old_nick = nick.clone();
                    let nick_taken = {
                        let mut g = shared.lock().await;
                        if let Some(other) = g.nicks.get(&desired_key) {
                            if *other != conn_id {
                                true
                            } else {
                                if let Some(ref old) = old_nick {
                                    g.nicks.remove(&ascii_casefold(old));
                                }
                                g.nicks.insert(desired_key.clone(), conn_id);
                                g.id_to_nick.insert(conn_id, desired.clone());
                                false
                            }
                        } else {
                            if let Some(ref old) = old_nick {
                                g.nicks.remove(&ascii_casefold(old));
                            }
                            g.nicks.insert(desired_key, conn_id);
                            g.id_to_nick.insert(conn_id, desired.clone());
                            false
                        }
                    };
                    if nick_taken {
                        writer.write_all(
                            numeric(server_name, 433, nick.as_deref().unwrap_or("*"), &[desired.as_str(), "Nickname is already in use"]).as_bytes(),
                        ).await?;
                        continue;
                    }
                    nick = Some(desired.clone());
                    if let Some(old) = old_nick {
                        if registered && old != desired {
                            let user_s = user.as_deref().unwrap_or("user");
                            let line = format!(":{old}!{user_s}@dsc.local NICK :{desired}\r\n");
                            writer.write_all(line.as_bytes()).await?;
                            let g = shared.lock().await;
                            for chan in &channels {
                                let _ = g.fanout_channel(chan, &line, conn_id);
                            }
                        }
                    }
                    try_register(
                        &mut writer,
                        server_name,
                        &cfg.server.motd,
                        &mut registered,
                        nick.as_deref(),
                        user.as_deref(),
                        realname.as_deref(),
                        cap_negotiating,
                        cfg.server.max_nick_length,
                        cfg.server.max_channel_length,
                    ).await?;
                    continue;
                }

                if msg.command_eq("USER") {
                    let Some(u) = msg.params.first().cloned() else { continue };
                    if u.is_empty() || field_has_control(&u) || u.contains(' ') {
                        writer.write_all(
                            numeric(server_name, 461, nick.as_deref().unwrap_or("*"), &["USER", "Invalid username"]).as_bytes(),
                        ).await?;
                        continue;
                    }
                    let rn = msg.params.get(3).cloned().or_else(|| msg.params.last().cloned());
                    if let Some(ref r) = rn {
                        if field_has_control(r) {
                            writer.write_all(
                                numeric(server_name, 461, nick.as_deref().unwrap_or("*"), &["USER", "Invalid realname"]).as_bytes(),
                            ).await?;
                            continue;
                        }
                    }
                    user = Some(u);
                    realname = rn;
                    try_register(
                        &mut writer,
                        server_name,
                        &cfg.server.motd,
                        &mut registered,
                        nick.as_deref(),
                        user.as_deref(),
                        realname.as_deref(),
                        cap_negotiating,
                        cfg.server.max_nick_length,
                        cfg.server.max_channel_length,
                    ).await?;
                    continue;
                }

                if msg.command_eq("PING") {
                    let token = msg.params.first().map(String::as_str).unwrap_or(server_name);
                    writer.write_all(format!(":{server_name} PONG {server_name} :{token}\r\n").as_bytes()).await?;
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
                    if !secure && cfg.security.tls_required_for_auth() {
                        writer.write_all(
                            numeric(server_name, 464, nick_s, &["TLS required for authentication"]).as_bytes(),
                        ).await?;
                        continue;
                    }
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
                        if !valid_channel_name(chan, cfg.server.max_channel_length) {
                            writer.write_all(
                                numeric(server_name, 403, nick_s, &[chan, "No such channel"]).as_bytes(),
                            ).await?;
                            continue;
                        }
                        // Client-local channel quota (checked before taking the lock for create).
                        if !channels.contains(chan)
                            && channels.len() >= cfg.limits.max_channels_per_client
                        {
                            writer.write_all(
                                numeric(server_name, 405, nick_s, &[chan, "You have joined too many channels"]).as_bytes(),
                            ).await?;
                            continue;
                        }
                        let join_result = {
                            let mut g = shared.lock().await;
                            // Ensure display nick is indexed before NAMES (first NICK may race).
                            if let Some(ref n) = nick {
                                g.id_to_nick.insert(conn_id, n.clone());
                                g.nicks.entry(ascii_casefold(n)).or_insert(conn_id);
                            }
                            let exists = g.channels.contains_key(chan);
                            if !exists && g.channels.len() >= cfg.limits.max_channels {
                                Err(405)
                            } else if g
                                .channels
                                .get(chan)
                                .map(|c| c.members.contains(&conn_id))
                                .unwrap_or(false)
                            {
                                Ok(None)
                            } else {
                                let (topic, members, ops, over) = {
                                    let ch = g.channels.entry(chan.to_string()).or_default();
                                    if ch.members.len() >= cfg.limits.max_members_per_channel {
                                        (None, Default::default(), Default::default(), true)
                                    } else {
                                        let first = ch.members.is_empty();
                                        ch.members.insert(conn_id);
                                        if first {
                                            ch.ops.insert(conn_id);
                                        }
                                        (
                                            ch.topic.clone(),
                                            ch.members.clone(),
                                            ch.ops.clone(),
                                            false,
                                        )
                                    }
                                };
                                if over {
                                    Err(471)
                                } else {
                                    let snap = crate::state::NamesSnapshot::from_sets(
                                        &members,
                                        &ops,
                                        &g.id_to_nick,
                                    );
                                    Ok(Some((snap, topic)))
                                }
                            }
                        };
                        let (names_snap, topic) = match join_result {
                            Ok(None) => continue, // already a member
                            Ok(Some(pair)) => pair,
                            Err(405) => {
                                writer.write_all(
                                    numeric(server_name, 405, nick_s, &[chan, "Too many channels on this server"]).as_bytes(),
                                ).await?;
                                continue;
                            }
                            Err(471) => {
                                writer.write_all(
                                    numeric(server_name, 471, nick_s, &[chan, "Cannot join channel (+l)"]).as_bytes(),
                                ).await?;
                                continue;
                            }
                            Err(_) => continue,
                        };
                        channels.insert(chan.to_string());
                        let join_line = format!(":{prefix} JOIN :{chan}\r\n");
                        writer.write_all(join_line.as_bytes()).await?;
                        let _ = shared.lock().await.fanout_channel(chan, &join_line, conn_id);
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
                        // Split 353 payloads to wire size (ties C2 / max_line_bytes).
                        let overhead = server_name.len()
                            + nick_s.len()
                            + chan.len()
                            + 24; // " 353  =  :\r\n" plus margins
                        let max_payload = cfg.limits.max_line_bytes.saturating_sub(overhead).max(16);
                        for chunk in names_snap.split_for_wire(max_payload) {
                            writer.write_all(
                                numeric(server_name, 353, nick_s, &["=", chan, chunk.as_str()]).as_bytes(),
                            ).await?;
                        }
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
                                if let Ok(rows) = store.latest_async(chan.to_string(), lim).await {
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
                    let rows = store
                        .latest_async(target.to_string(), limit)
                        .await
                        .unwrap_or_default();
                    // BATCH / chathistory CAP not advertised until Protocol P2 (#2227).
                    for h in rows {
                        let line = adapt_bus_line(&h.tagged_privmsg(), &enabled_caps);
                        writer.write_all(line.as_bytes()).await?;
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
                                Some(ch) if ch.members.contains(&conn_id) => true,
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
                            match store
                                .append_async(target.clone(), prefix.clone(), text.clone())
                                .await
                            {
                                Ok(h) => h.tagged_privmsg(),
                                Err(_) => format!(":{prefix} PRIVMSG {target} :{text}\r\n"),
                            }
                        } else {
                            format!(":{prefix} PRIVMSG {target} :{text}\r\n")
                        };
                        if let Some(acc) = account.as_deref() {
                            line = prepend_tag(&line, "account", acc);
                        }
                        let _ = shared.lock().await.fanout_channel(target.as_str(), &line, conn_id);
                    }
                    continue;
                }

                if msg.command_eq("PART") {
                    let Some(chan) = msg.params.first() else { continue };
                    if !channels.contains(chan) {
                        writer.write_all(
                            numeric(server_name, 442, nick_s, &[chan.as_str(), "You're not on that channel"]).as_bytes(),
                        ).await?;
                        continue;
                    }
                    let is_member = {
                        let g = shared.lock().await;
                        g.channels.get(chan.as_str()).map(|c| c.members.contains(&conn_id)).unwrap_or(false)
                    };
                    if !is_member {
                        channels.remove(chan);
                        writer.write_all(
                            numeric(server_name, 442, nick_s, &[chan.as_str(), "You're not on that channel"]).as_bytes(),
                        ).await?;
                        continue;
                    }
                    channels.remove(chan);
                    let line = format!(":{prefix} PART {chan}\r\n");
                    writer.write_all(line.as_bytes()).await?;
                    {
                        let mut g = shared.lock().await;
                        let _ = g.fanout_channel(chan.as_str(), &line, conn_id);
                        if let Some(ch) = g.channels.get_mut(chan) {
                            ch.remove_member(conn_id);
                            if ch.members.is_empty() {
                                g.channels.remove(chan);
                            }
                        }
                    }
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
                    if field_has_control(&new_topic) {
                        writer.write_all(
                            numeric(server_name, 461, nick_s, &["TOPIC", "Invalid topic"]).as_bytes(),
                        ).await?;
                        continue;
                    }
                    if new_topic.len() > cfg.limits.max_topic_bytes {
                        writer.write_all(
                            numeric(server_name, 461, nick_s, &["TOPIC", "Topic too long"]).as_bytes(),
                        ).await?;
                        continue;
                    }
                    let allowed = {
                        let g = shared.lock().await;
                        match g.channels.get(chan.as_str()) {
                            Some(ch) if is_oper || ch.is_op(conn_id) => true,
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
                    let _ = shared.lock().await.fanout_channel(chan.as_str(), &line, conn_id);
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
                    let (allowed, target_id) = {
                        let g = shared.lock().await;
                        let target_id = g.nicks.get(&ascii_casefold(target_nick.as_str())).copied();
                        match (g.channels.get(chan.as_str()), target_id) {
                            (Some(ch), Some(tid)) if ch.members.contains(&tid) => {
                                (is_oper || ch.is_op(conn_id), Some(tid))
                            }
                            _ => (false, target_id),
                        }
                    };
                    if !allowed {
                        let on_chan = {
                            let g = shared.lock().await;
                            target_id
                                .and_then(|tid| g.channels.get(chan.as_str()).map(|c| c.members.contains(&tid)))
                                .unwrap_or(false)
                        };
                        if !on_chan {
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
                    let line = format!(":{prefix} KICK {chan} {target_nick} :{reason}\r\n");
                    // Fanout to current members (incl. victim) before remove — H-07.
                    {
                        let mut g = shared.lock().await;
                        if let Some(tid) = target_id {
                            let mut ids: Vec<u64> = g
                                .channels
                                .get(chan.as_str())
                                .map(|c| {
                                    c.members
                                        .iter()
                                        .copied()
                                        .filter(|id| *id != conn_id)
                                        .collect()
                                })
                                .unwrap_or_default();
                            if !ids.contains(&tid) {
                                ids.push(tid);
                            }
                            let _ = g.fanout_ids(&ids, &line);
                            if let Some(ch) = g.channels.get_mut(chan.as_str()) {
                                ch.remove_member(tid);
                                if ch.members.is_empty() {
                                    g.channels.remove(chan.as_str());
                                }
                            }
                        }
                    }
                    // Deliver to kicker too (standard).
                    writer.write_all(line.as_bytes()).await?;
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
                            Some(ch) => is_oper || ch.is_op(conn_id),
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
                                let Some(tid) = g.nicks.get(&ascii_casefold(&who)).copied() else {
                                    drop(g);
                                    writer.write_all(
                                        numeric(server_name, 441, nick_s, &[who.as_str(), target.as_str(), "They aren't on that channel"]).as_bytes(),
                                    ).await?;
                                    continue;
                                };
                                let Some(chan) = g.channels.get_mut(target.as_str()) else { continue };
                                if !chan.members.contains(&tid) {
                                    drop(g);
                                    writer.write_all(
                                        numeric(server_name, 441, nick_s, &[who.as_str(), target.as_str(), "They aren't on that channel"]).as_bytes(),
                                    ).await?;
                                    continue;
                                }
                                if adding {
                                    chan.ops.insert(tid);
                                } else {
                                    chan.ops.remove(&tid);
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
                        let _ = shared.lock().await.fanout_channel(target.as_str(), &line, conn_id);
                    }
                    continue;
                }

                writer.write_all(
                    numeric(
                        server_name,
                        421,
                        nick_s,
                        &[msg.command.as_str(), "Unknown command"],
                    )
                    .as_bytes(),
                )
                .await?;
                continue;
    }

    if let Some(n) = nick {
        let mut g = shared.lock().await;
        g.nicks.remove(&ascii_casefold(&n));
        g.id_to_nick.remove(&conn_id);
        for chan in &channels {
            let user_s = user.as_deref().unwrap_or("user");
            let line = format!(":{n}!{user_s}@dsc.local QUIT :Connection closed\r\n");
            let _ = g.fanout_channel(chan, &line, conn_id);
            if let Some(ch) = g.channels.get_mut(chan) {
                ch.remove_member(conn_id);
            }
            if g.channels.get(chan).map(|c| c.members.is_empty()).unwrap_or(false) {
                g.channels.remove(chan);
            }
        }
        g.unregister_outbox(conn_id);
    }

    Ok(())
}

/// Releases admission counters and clears this connection's nick/channel state (H-03).
/// Idempotent with the happy-path cleanup at the end of `handle_client`.
struct ConnRelease {
    shared: Arc<Mutex<Shared>>,
    peer: SocketAddr,
    conn_id: u64,
}

impl Drop for ConnRelease {
    fn drop(&mut self) {
        let shared = Arc::clone(&self.shared);
        let peer = self.peer;
        let conn_id = self.conn_id;
        tokio::spawn(async move {
            let mut g = shared.lock().await;
            g.unregister_outbox(conn_id);
            g.release(peer);
            if let Some(nick) = g.id_to_nick.remove(&conn_id) {
                g.nicks.remove(&ascii_casefold(&nick));
            }
            let affected: Vec<String> = g
                .channels
                .iter()
                .filter(|(_, ch)| ch.members.contains(&conn_id))
                .map(|(name, _)| name.clone())
                .collect();
            for chan in affected {
                if let Some(ch) = g.channels.get_mut(&chan) {
                    ch.remove_member(conn_id);
                    let empty = ch.members.is_empty();
                    if empty {
                        g.channels.remove(&chan);
                    }
                }
            }
        });
    }
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
        let req = msg.params.get(1).map(String::as_str).unwrap_or("");
        let (ack, nak) = apply_cap_req(req, advertised, enabled_caps);
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

async fn try_register<W>(
    writer: &mut W,
    server_name: &str,
    motd: &str,
    registered: &mut bool,
    nick: Option<&str>,
    user: Option<&str>,
    _realname: Option<&str>,
    cap_negotiating: bool,
    nick_len: usize,
    chan_len: usize,
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
    let isupport = format!(
        "CASEMAPPING=ascii CHANTYPES=# PREFIX=(o)@ NICKLEN={nick_len} CHANNELLEN={chan_len} CHANMODES=,,,nt NETWORK=DSC :are supported by this server"
    );
    writer
        .write_all(numeric(server_name, 5, nick, &[isupport.as_str()]).as_bytes())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_name_matches_ignores_value_suffix() {
        assert!(cap_name_matches(
            "sasl",
            "sasl=PLAIN,EXTERNAL"
        ));
        assert!(cap_name_matches("SASL=PLAIN", "sasl"));
        assert!(!cap_name_matches("message-tags", "server-time"));
    }

    #[test]
    fn apply_cap_req_ack_nak_enable_disable() {
        let advertised = advertised_caps(true);
        let mut enabled = HashSet::new();
        // Mixed unknown → atomic NAK, no enable
        let (ack, nak) = apply_cap_req(
            "sasl bogon",
            &advertised,
            &mut enabled,
        );
        assert!(ack.is_empty());
        assert_eq!(nak.len(), 2);
        assert!(enabled.is_empty());
        // Clean REQ ACKs
        let (ack, nak) = apply_cap_req("sasl", &advertised, &mut enabled);
        assert!(ack.iter().any(|t| t == "sasl"));
        assert!(nak.is_empty());
        assert!(enabled.contains("sasl"));
        // Cannot disable cap-notify
        let (ack, nak) = apply_cap_req("-cap-notify", &advertised, &mut enabled);
        assert!(ack.is_empty());
        assert!(!nak.is_empty());
    }

    #[test]
    fn advertised_caps_truthful_base_empty_sasl_conditional() {
        let with = advertised_caps(true);
        assert!(with.iter().any(|c| c.starts_with("sasl")));
        assert!(!with.iter().any(|c| c == "away-notify"));
        assert!(!with.iter().any(|c| c == "message-tags"));
        assert!(!with.iter().any(|c| c == "batch"));
        assert!(!with.iter().any(|c| c == "chathistory"));
        assert!(!with.iter().any(|c| c == "account-tag"));
        assert!(!with.iter().any(|c| c == "server-time"));
        assert!(!with.iter().any(|c| c == "multi-prefix"));
        let without = advertised_caps(false);
        assert!(without.iter().any(|c| c == "cap-notify"));
        assert_eq!(without.len(), 1);
    }

    #[test]
    fn has_cap_is_case_insensitive() {
        let mut set = HashSet::new();
        set.insert("server-time".into());
        assert!(has_cap(&set, "SERVER-TIME"));
        assert!(!has_cap(&set, "message-tags"));
    }
}
