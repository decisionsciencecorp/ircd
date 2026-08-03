//! Per-connection IRC session.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use ircd_core::tags::{adapt_bus_line, prepend_tag};
use ircd_core::{
    ascii_casefold, field_has_control, numeric, server_notice, valid_channel_name, Command, Nick,
    RawLine,
};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tracing::info;

use crate::cmd_precheck;
use crate::state::Shared;
use crate::VERSION;

mod cap;
mod io;
mod message;
mod register;

use cap::{advertised_caps, handle_authenticate, handle_cap, has_cap, SaslState};
use io::{read_line_outcome, session_deadline_at, sleep_until_deadline, IoEvent, LineOutcome};
use message::relay_client_tags;
use register::try_register;

pub async fn handle_client<R, W>(
    reader: R,
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
        (g.alloc_conn_id(), g.config())
    };
    // Admission counters + nick/channel membership cleanup on *all* exits (H-03).
    let _conn_guard = ConnRelease {
        shared: Arc::clone(&shared),
        peer,
        conn_id,
    };
    let (out_tx, mut out_rx) =
        tokio::sync::mpsc::channel::<std::sync::Arc<str>>(crate::state::OUTBOX_CAP);
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

    // Buffered line I/O (C9 / Q-01): BufReader + read_until — no 50ms read_u8 tick.
    // Outbound via per-id outbox (H-07/H-08). Deadlines use exact sleep, not polling.

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
    let max_line = cfg.limits.max_line_bytes.max(64);
    let mut reader = BufReader::with_capacity(max_line.min(8192).max(256), reader);
    let mut line_buf: Vec<u8> = Vec::with_capacity(256);
    let mut channels: HashSet<String> = HashSet::new();
    let mut quit_reason: Option<String> = None;
    let mut quit_announced = false;
    let mut account: Option<String> = None;
    let mut sasl_state = SaslState::Idle;
    let has_accounts = !cfg.accounts.is_empty();
    let has_history = cfg.history.enabled;
    let cap_list = advertised_caps(has_accounts, has_history);

    // Event-driven select! between member outbox and buffered reads (H-07/H-08 / C9).
    // Coverage must not dictate production scheduling — prefer outbox (biased).
    // fill_buf/consume is cancel-safe: partial bytes stay in line_buf when outbox wins.
    'session: loop {
        let raw = loop {
            let deadline_at = session_deadline_at(
                registered,
                session_start,
                last_activity,
                reg_deadline,
                idle_deadline,
            );

            let event = tokio::select! {
                biased;
                maybe = out_rx.recv() => IoEvent::Outbox(maybe),
                outcome = read_line_outcome(&mut reader, &mut line_buf, max_line) => {
                    IoEvent::Read(outcome)
                }
                _ = sleep_until_deadline(deadline_at) => IoEvent::Deadline,
            };

            match event {
                IoEvent::Outbox(None) => return Ok(()),
                IoEvent::Outbox(Some(raw_line)) => {
                    let line = adapt_bus_line(raw_line.as_ref(), &enabled_caps);
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
                IoEvent::Read(Ok(LineOutcome::Eof)) => break 'session,
                IoEvent::Read(Ok(LineOutcome::Oversized)) => {
                    let _ = writer
                        .write_all(
                            format!("ERROR :Closing Link: [{peer}] (Input line too long)\r\n")
                                .as_bytes(),
                        )
                        .await;
                    let _ = writer.flush().await;
                    info!(%peer, max_line, "oversized IRC line; closing");
                    break 'session;
                }
                IoEvent::Read(Ok(LineOutcome::Complete)) => {
                    let raw = String::from_utf8_lossy(&line_buf).into_owned();
                    line_buf.clear();
                    break raw;
                }
                IoEvent::Read(Err(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    break 'session;
                }
                IoEvent::Read(Err(e)) => return Err(e.into()),
                IoEvent::Deadline => {
                    if !registered {
                        if reg_deadline.is_some() {
                            let _ = writer
                                .write_all(b"ERROR :Closing Link: Registration timeout\r\n")
                                .await;
                            break 'session;
                        }
                    } else if idle_deadline.is_some() {
                        let _ = writer
                            .write_all(b"ERROR :Closing Link: Idle timeout\r\n")
                            .await;
                        break 'session;
                    }
                    continue;
                }
            }
        };

        // Simple recv flood guard (line count per window).
        if flood_window_start.elapsed() >= flood_window {
            flood_window_start = Instant::now();
            flood_count = 0;
        }
        flood_count = flood_count.saturating_add(1);
        if flood_count > flood_limit {
            let _ = writer
                .write_all(format!("ERROR :Closing Link: [{peer}] (Excess Flood)\r\n").as_bytes())
                .await;
            info!(%peer, "excess flood; closing");
            break;
        }
        last_activity = Instant::now();
        let Some(msg) = RawLine::parse(&raw) else {
            continue;
        };
        let cmd = Command::from_raw(&msg);
        let _lane = cmd.lane();

        match &cmd {
            Command::Cap { .. } => {
                handle_cap(
                    &mut writer,
                    server_name,
                    &msg,
                    nick.as_deref(),
                    &mut cap_negotiating,
                    &mut enabled_caps,
                    &cap_list,
                    &shared,
                    conn_id,
                )
                .await?;
                if msg
                    .params
                    .first()
                    .map(|s| s.eq_ignore_ascii_case("END"))
                    .unwrap_or(false)
                {
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
                        if has_history {
                            Some(register::CHATHISTORY_ISUPPORT_MAX)
                        } else {
                            None
                        },
                    )
                    .await?;
                }
                continue;
            }

            Command::Authenticate { .. } => {
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

            Command::Nick { .. } => {
                let Some(desired_raw) = msg.params.first().cloned() else {
                    writer
                        .write_all(
                            numeric(
                                server_name,
                                431,
                                nick.as_deref().unwrap_or("*"),
                                &["No nickname given"],
                            )
                            .as_bytes(),
                        )
                        .await?;
                    continue;
                };
                let Ok(parsed) = Nick::parse(&desired_raw, cfg.server.max_nick_length) else {
                    writer
                        .write_all(
                            numeric(
                                server_name,
                                432,
                                nick.as_deref().unwrap_or("*"),
                                &["Erroneous Nickname"],
                            )
                            .as_bytes(),
                        )
                        .await?;
                    continue;
                };
                let desired = parsed.as_str().to_string();
                let desired_key = parsed.key().as_str().to_string();
                let old_nick = nick.clone();
                let nick_taken = {
                    let mut g = shared.lock().await;
                    if let Some(other) = g.nick_id(&desired_key) {
                        if other != conn_id {
                            true
                        } else {
                            g.replace_nick(conn_id, old_nick.as_deref(), desired.clone());
                            false
                        }
                    } else {
                        g.replace_nick(conn_id, old_nick.as_deref(), desired.clone());
                        false
                    }
                };
                if nick_taken {
                    writer
                        .write_all(
                            numeric(
                                server_name,
                                433,
                                nick.as_deref().unwrap_or("*"),
                                &[desired.as_str(), "Nickname is already in use"],
                            )
                            .as_bytes(),
                        )
                        .await?;
                    continue;
                }
                nick = Some(desired.clone());
                if let Some(old) = old_nick {
                    if registered && old != desired {
                        let user_s = user.as_deref().unwrap_or("user");
                        let line = format!(":{old}!{user_s}@dsc.local NICK :{desired}\r\n");
                        writer.write_all(line.as_bytes()).await?;
                        let payload: std::sync::Arc<str> = std::sync::Arc::from(line.as_str());
                        let g = shared.lock().await;
                        for chan in &channels {
                            let ids = g.channel_member_ids(chan, conn_id);
                            let senders = g.clone_outboxes_for(&ids);
                            for tx in senders {
                                let _ = tx.try_send(std::sync::Arc::clone(&payload));
                            }
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
                    if has_history {
                        Some(register::CHATHISTORY_ISUPPORT_MAX)
                    } else {
                        None
                    },
                )
                .await?;
                continue;
            }

            Command::User { .. } => {
                let Some(u) = msg.params.first().cloned() else {
                    continue;
                };
                if u.is_empty() || field_has_control(&u) || u.contains(' ') {
                    writer
                        .write_all(
                            numeric(
                                server_name,
                                461,
                                nick.as_deref().unwrap_or("*"),
                                &["USER", "Invalid username"],
                            )
                            .as_bytes(),
                        )
                        .await?;
                    continue;
                }
                let rn = msg
                    .params
                    .get(3)
                    .cloned()
                    .or_else(|| msg.params.last().cloned());
                if let Some(ref r) = rn {
                    if field_has_control(r) {
                        writer
                            .write_all(
                                numeric(
                                    server_name,
                                    461,
                                    nick.as_deref().unwrap_or("*"),
                                    &["USER", "Invalid realname"],
                                )
                                .as_bytes(),
                            )
                            .await?;
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
                    if has_history {
                        Some(register::CHATHISTORY_ISUPPORT_MAX)
                    } else {
                        None
                    },
                )
                .await?;
                continue;
            }

            Command::Ping { token } => {
                match token {
                    Some(t) => {
                        writer
                            .write_all(
                                format!(":{server_name} PONG {server_name} :{t}\r\n").as_bytes(),
                            )
                            .await?;
                    }
                    None => {
                        // Modern / irctest: bare PING → ERR_NOORIGIN (or 461).
                        writer
                            .write_all(
                                numeric(
                                    server_name,
                                    409,
                                    nick.as_deref().unwrap_or("*"),
                                    &["No origin specified"],
                                )
                                .as_bytes(),
                            )
                            .await?;
                    }
                }
                continue;
            }

            Command::Quit { reason } => {
                quit_reason = reason.clone().filter(|r| !r.is_empty());
                // Announce to channels + ERROR before close so irctest can sync-PING.
                if let Some(n) = nick.as_deref() {
                    let mut g = shared.lock().await;
                    let user_s = user.as_deref().unwrap_or("user");
                    let why = quit_reason.as_deref().unwrap_or("Client Quit");
                    let line = format!(":{n}!{user_s}@dsc.local QUIT :{why}\r\n");
                    let payload: std::sync::Arc<str> = std::sync::Arc::from(line.as_str());
                    for chan in &channels {
                        let ids = g.channel_member_ids(chan, conn_id);
                        let senders = g.clone_outboxes_for(&ids);
                        for tx in senders {
                            let _ = tx.try_send(std::sync::Arc::clone(&payload));
                        }
                        if let Some(ch) = g.channel_mut(chan) {
                            ch.remove_member(conn_id);
                        }
                        g.remove_channel_if_empty(chan);
                    }
                    let _ = g.clear_nick(conn_id);
                }
                channels.clear();
                let why = quit_reason.as_deref().unwrap_or("Client Quit");
                let n = nick.as_deref().unwrap_or("*");
                writer
                    .write_all(
                        format!("ERROR :Closing Link: {n} (Quit: {why})\r\n").as_bytes(),
                    )
                    .await?;
                writer.flush().await?;
                quit_announced = true;
                break;
            }
            _ => {
                // post-registration commands
                if let Some(code) = cmd_precheck::require_registered(registered) {
                    writer
                        .write_all(
                            numeric(
                                server_name,
                                code,
                                nick.as_deref().unwrap_or("*"),
                                &["You have not registered"],
                            )
                            .as_bytes(),
                        )
                        .await?;
                    continue;
                }

                let nick_s = nick.as_deref().unwrap_or("*");
                let user_s = user.as_deref().unwrap_or("user");
                let host = "dsc.local";
                let prefix = format!("{nick_s}!{user_s}@{host}");

                match &cmd {
                    Command::Admin => {
                        writer
                            .write_all(
                                numeric(
                                    server_name,
                                    256,
                                    nick_s,
                                    &[&format!("Administrative info about {server_name}")],
                                )
                                .as_bytes(),
                            )
                            .await?;
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
                        writer
                            .write_all(numeric(server_name, 257, nick_s, &[admin]).as_bytes())
                            .await?;
                        writer
                            .write_all(
                                numeric(server_name, 258, nick_s, &["Decision Science Corp"])
                                    .as_bytes(),
                            )
                            .await?;
                        writer
                            .write_all(numeric(server_name, 259, nick_s, &[email]).as_bytes())
                            .await?;
                        continue;
                    }

                    Command::Oper { .. } => {
                        if !secure && cfg.security.tls_required_for_auth() {
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        464,
                                        nick_s,
                                        &["TLS required for authentication"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            continue;
                        }
                        let (Some(name), Some(pass)) = (msg.params.first(), msg.params.get(1))
                        else {
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        461,
                                        nick_s,
                                        &["OPER", "Not enough parameters"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            continue;
                        };
                        let oper = &cfg.oper;
                        if !oper.enabled || oper.name.is_empty() {
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        491,
                                        nick_s,
                                        &["No O-lines for your host"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            continue;
                        }
                        if name == &oper.name && pass == &oper.password {
                            is_oper = true;
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        381,
                                        nick_s,
                                        &["You are now an IRC operator"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            info!(%nick_s, "client obtained OPER");
                        } else {
                            writer
                                .write_all(
                                    numeric(server_name, 464, nick_s, &["Password incorrect"])
                                        .as_bytes(),
                                )
                                .await?;
                        }
                        continue;
                    }

                    Command::Join { .. } => {
                        let Some(chan_list) = msg.params.first() else {
                            continue;
                        };
                        for chan in chan_list.split(',') {
                            let chan = chan.trim();
                            if !valid_channel_name(chan, cfg.server.max_channel_length) {
                                writer
                                    .write_all(
                                        numeric(
                                            server_name,
                                            403,
                                            nick_s,
                                            &[chan, "No such channel"],
                                        )
                                        .as_bytes(),
                                    )
                                    .await?;
                                continue;
                            }
                            // Client-local channel quota (checked before taking the lock for create).
                            if !channels.contains(chan)
                                && channels.len() >= cfg.limits.max_channels_per_client
                            {
                                writer
                                    .write_all(
                                        numeric(
                                            server_name,
                                            405,
                                            nick_s,
                                            &[chan, "You have joined too many channels"],
                                        )
                                        .as_bytes(),
                                    )
                                    .await?;
                                continue;
                            }
                            let join_result = {
                                let mut g = shared.lock().await;
                                // Ensure display nick is indexed before NAMES (first NICK may race).
                                if let Some(ref n) = nick {
                                    g.ensure_nick_indexed(conn_id, n);
                                }
                                let exists = g.has_channel(chan);
                                if !exists && g.channel_count() >= cfg.limits.max_channels {
                                    Err(405)
                                } else if g
                                    .channel(chan)
                                    .map(|c| c.members.contains(&conn_id))
                                    .unwrap_or(false)
                                {
                                    Ok(None)
                                } else {
                                    let nick_chk = nick.as_deref().unwrap_or("*");
                                    let user_chk = user.as_deref().unwrap_or("user");
                                    let blocked = if let Some(ch) = g.channel(chan) {
                                        if ch.is_banned(nick_chk, user_chk, "dsc.local") {
                                            Some(474)
                                        } else if ch.mode_i
                                            && !ch.invites.contains(&conn_id)
                                            && !is_oper
                                        {
                                            Some(473)
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    };
                                    if let Some(code) = blocked {
                                        Err(code)
                                    } else {
                                        let (topic, over) = {
                                            let lim = g.config().limits.max_members_per_channel;
                                            let ch = g.channel_or_default(chan.to_string());
                                            if ch.members.len() >= lim {
                                                (None, true)
                                            } else {
                                                let first = ch.members.is_empty();
                                                ch.members.insert(conn_id);
                                                ch.invites.remove(&conn_id);
                                                if first {
                                                    ch.ops.insert(conn_id);
                                                }
                                                (ch.topic.clone(), false)
                                            }
                                        };
                                        if over {
                                            Err(471)
                                        } else {
                                            // Thin copy only — sort/@ format after lock drop (C10).
                                            let thin = g
                                                .names_thin(chan)
                                                .expect("channel just joined");
                                            Ok(Some((thin, topic)))
                                        }
                                    }
                                }
                            };
                            let (names_thin, topic) = match join_result {
                                Ok(None) => continue, // already a member
                                Ok(Some(pair)) => pair,
                                Err(405) => {
                                    writer
                                        .write_all(
                                            numeric(
                                                server_name,
                                                405,
                                                nick_s,
                                                &[chan, "Too many channels on this server"],
                                            )
                                            .as_bytes(),
                                        )
                                        .await?;
                                    continue;
                                }
                                Err(471) => {
                                    writer
                                        .write_all(
                                            numeric(
                                                server_name,
                                                471,
                                                nick_s,
                                                &[chan, "Cannot join channel (+l)"],
                                            )
                                            .as_bytes(),
                                        )
                                        .await?;
                                    continue;
                                }
                                Err(473) => {
                                    writer
                                        .write_all(
                                            numeric(
                                                server_name,
                                                473,
                                                nick_s,
                                                &[chan, "Cannot join channel (+i)"],
                                            )
                                            .as_bytes(),
                                        )
                                        .await?;
                                    continue;
                                }
                                Err(474) => {
                                    writer
                                        .write_all(
                                            numeric(
                                                server_name,
                                                474,
                                                nick_s,
                                                &[chan, "Cannot join channel (+b)"],
                                            )
                                            .as_bytes(),
                                        )
                                        .await?;
                                    continue;
                                }
                                Err(_) => continue,
                            };
                            channels.insert(chan.to_string());
                            let join_line = format!(":{prefix} JOIN :{chan}\r\n");
                            writer.write_all(join_line.as_bytes()).await?;
                            let _ = shared
                                .lock()
                                .await
                                .fanout_channel(chan, &join_line, conn_id);
                            // away-notify: notify channel peers when joiner is already away.
                            {
                                let g = shared.lock().await;
                                if let Some(away_msg) = g.away_message(conn_id) {
                                    let peers = g.away_notify_in_channel(chan, conn_id);
                                    let away_line = format!(":{prefix} AWAY :{away_msg}\r\n");
                                    let _ = g.fanout_ids(&peers, &away_line);
                                }
                            }
                            match &topic {
                                Some(t) => {
                                    writer
                                        .write_all(
                                            numeric(server_name, 332, nick_s, &[chan, t.as_str()])
                                                .as_bytes(),
                                        )
                                        .await?;
                                }
                                None => {
                                    writer
                                        .write_all(
                                            numeric(
                                                server_name,
                                                331,
                                                nick_s,
                                                &[chan, "No topic is set"],
                                            )
                                            .as_bytes(),
                                        )
                                        .await?;
                                }
                            }
                            // Split 353 payloads to wire size (ties C2 / max_line_bytes).
                            // Sort/@ format happens here — Shared lock already dropped (C10).
                            let names_snap = names_thin.into_names_snapshot();
                            let overhead = server_name.len() + nick_s.len() + chan.len() + 24; // " 353  =  :\r\n" plus margins
                            let max_payload =
                                cfg.limits.max_line_bytes.saturating_sub(overhead).max(16);
                            for chunk in names_snap.split_for_wire(max_payload) {
                                writer
                                    .write_all(
                                        numeric(
                                            server_name,
                                            353,
                                            nick_s,
                                            &["=", chan, chunk.as_str()],
                                        )
                                        .as_bytes(),
                                    )
                                    .await?;
                            }
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        366,
                                        nick_s,
                                        &[chan, "End of /NAMES list"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            // Auto-replay recent channel history (Ergo-style convenience).
                            let (hist, limit) = {
                                let g = shared.lock().await;
                                (g.history_store(), g.config().history.auto_replay_on_join)
                            };
                            if let (Some(store), lim) = (hist, limit) {
                                if lim > 0 {
                                    if let Ok(rows) =
                                        store.latest_async(chan.to_string(), lim).await
                                    {
                                        for h in rows {
                                            let line =
                                                adapt_bus_line(&h.tagged_privmsg(), &enabled_caps);
                                            writer.write_all(line.as_bytes()).await?;
                                        }
                                    }
                                }
                            }
                        }
                        continue;
                    }

                    Command::Chathistory { .. } => {
                        let sub = msg.params.first().map(String::as_str).unwrap_or("");
                        if !sub.eq_ignore_ascii_case("LATEST") {
                            writer
                                .write_all(
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
                            writer
                                .write_all(
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
                        let store = { shared.lock().await.history_store() };
                        let Some(store) = store else {
                            writer
                                .write_all(
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
                        let use_batch = has_cap(&enabled_caps, "batch");
                        if use_batch {
                            writer
                                .write_all(
                                    format!(
                                        ":{} BATCH +chathist draft/chathistory
",
                                        server_name
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                        }
                        for h in rows {
                            let mut line = adapt_bus_line(&h.tagged_privmsg(), &enabled_caps);
                            if use_batch {
                                line = prepend_tag(&line, "batch", "chathist");
                            }
                            writer.write_all(line.as_bytes()).await?;
                        }
                        if use_batch {
                            writer
                                .write_all(
                                    format!(
                                        ":{} BATCH -chathist
",
                                        server_name
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                        }
                        continue;
                    }

                    Command::Privmsg { .. } => {
                        let (Some(target), Some(text)) = (msg.params.first(), msg.params.get(1))
                        else {
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        461,
                                        nick_s,
                                        &["PRIVMSG", "Not enough parameters"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            continue;
                        };
                        if target.starts_with('#') {
                            let (allowed, hist) = {
                                let g = shared.lock().await;
                                let allowed = match g.channel(target.as_str()) {
                                    Some(ch) if ch.members.contains(&conn_id) => true,
                                    Some(ch) if !ch.mode_n => true,
                                    Some(_) => false,
                                    None => false,
                                };
                                (allowed, g.history_store())
                            };
                            if !allowed {
                                writer
                                    .write_all(
                                        numeric(
                                            server_name,
                                            404,
                                            nick_s,
                                            &[target.as_str(), "Cannot send to channel"],
                                        )
                                        .as_bytes(),
                                    )
                                    .await?;
                                continue;
                            }
                            let mut line = if let Some(store) = hist {
                                match store
                                    .append_async(target.clone(), prefix.clone(), text.clone())
                                    .await
                                {
                                    Ok(h) => h.tagged_privmsg(),
                                    Err(_) => format!(
                                        ":{prefix} PRIVMSG {target} :{text}
"
                                    ),
                                }
                            } else {
                                format!(
                                    ":{prefix} PRIVMSG {target} :{text}
"
                                )
                            };
                            if let Some(acc) = account.as_deref() {
                                line = prepend_tag(&line, "account", acc);
                            }
                            line = relay_client_tags(line, &msg, &enabled_caps);
                            let _ =
                                shared
                                    .lock()
                                    .await
                                    .fanout_channel(target.as_str(), &line, conn_id);
                        } else {
                            let tid = {
                                let g = shared.lock().await;
                                g.nick_id(&ascii_casefold(target))
                            };
                            let Some(tid) = tid else {
                                writer
                                    .write_all(
                                        numeric(
                                            server_name,
                                            401,
                                            nick_s,
                                            &[target.as_str(), "No such nick/channel"],
                                        )
                                        .as_bytes(),
                                    )
                                    .await?;
                                continue;
                            };
                            let mut line = format!(":{prefix} PRIVMSG {target} :{text}\r\n");
                            if let Some(acc) = account.as_deref() {
                                line = prepend_tag(&line, "account", acc);
                            }
                            line = relay_client_tags(line, &msg, &enabled_caps);
                            let away_reply = {
                                let g = shared.lock().await;
                                let away = g.away_message(tid).map(str::to_string);
                                let _ = g.fanout_ids(&[tid], &line);
                                away
                            };
                            if let Some(msg) = away_reply.as_deref() {
                                writer
                                    .write_all(
                                        numeric(server_name, 301, nick_s, &[target.as_str(), msg])
                                            .as_bytes(),
                                    )
                                    .await?;
                            }
                        }
                        continue;
                    }

                    Command::Tagmsg { .. } => {
                        if !has_cap(&enabled_caps, "message-tags") {
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        421,
                                        nick_s,
                                        &["TAGMSG", "Unknown command"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            continue;
                        }
                        let Some(target) = msg.params.first() else {
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        461,
                                        nick_s,
                                        &["TAGMSG", "Not enough parameters"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            continue;
                        };
                        let text = msg.params.get(1).map(String::as_str).unwrap_or("");
                        if target.starts_with('#') {
                            let allowed = {
                                let g = shared.lock().await;
                                match g.channel(target.as_str()) {
                                    Some(ch) if ch.members.contains(&conn_id) => true,
                                    Some(ch) if !ch.mode_n => true,
                                    _ => false,
                                }
                            };
                            if !allowed {
                                writer
                                    .write_all(
                                        numeric(
                                            server_name,
                                            404,
                                            nick_s,
                                            &[target.as_str(), "Cannot send to channel"],
                                        )
                                        .as_bytes(),
                                    )
                                    .await?;
                                continue;
                            }
                            let mut line = format!(":{prefix} TAGMSG {target} :{text}\r\n");
                            if let Some(acc) = account.as_deref() {
                                line = prepend_tag(&line, "account", acc);
                            }
                            line = relay_client_tags(line, &msg, &enabled_caps);
                            let _ = shared.lock().await.fanout_channel(
                                target.as_str(),
                                &line,
                                conn_id,
                            );
                        } else {
                            let tid = {
                                let g = shared.lock().await;
                                g.nick_id(&ascii_casefold(target))
                            };
                            let Some(tid) = tid else {
                                writer
                                    .write_all(
                                        numeric(
                                            server_name,
                                            401,
                                            nick_s,
                                            &[target.as_str(), "No such nick/channel"],
                                        )
                                        .as_bytes(),
                                    )
                                    .await?;
                                continue;
                            };
                            let mut line = format!(":{prefix} TAGMSG {target} :{text}\r\n");
                            if let Some(acc) = account.as_deref() {
                                line = prepend_tag(&line, "account", acc);
                            }
                            line = relay_client_tags(line, &msg, &enabled_caps);
                            let _ = shared.lock().await.fanout_ids(&[tid], &line);
                        }
                        continue;
                    }

                    Command::Part { .. } => {
                        let Some(chan) = msg.params.first() else {
                            continue;
                        };
                        if !channels.contains(chan) {
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        442,
                                        nick_s,
                                        &[chan.as_str(), "You're not on that channel"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            continue;
                        }
                        let is_member = {
                            let g = shared.lock().await;
                            g.channel(chan.as_str())
                                .map(|c| c.members.contains(&conn_id))
                                .unwrap_or(false)
                        };
                        if !is_member {
                            channels.remove(chan);
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        442,
                                        nick_s,
                                        &[chan.as_str(), "You're not on that channel"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            continue;
                        }
                        channels.remove(chan);
                        let part_reason = msg.params.get(1).map(String::as_str).unwrap_or("");
                        let line = if part_reason.is_empty() {
                            format!(":{prefix} PART {chan}\r\n")
                        } else {
                            format!(":{prefix} PART {chan} :{part_reason}\r\n")
                        };
                        writer.write_all(line.as_bytes()).await?;
                        {
                            let mut g = shared.lock().await;
                            let _ = g.fanout_channel(chan.as_str(), &line, conn_id);
                            if let Some(ch) = g.channel_mut(chan) {
                                ch.remove_member(conn_id);
                                if ch.members.is_empty() {
                                    g.remove_channel(chan);
                                }
                            }
                        }
                        continue;
                    }

                    Command::Topic { .. } => {
                        let Some(chan) = msg.params.first() else {
                            continue;
                        };
                        if !channels.contains(chan) {
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        442,
                                        nick_s,
                                        &[chan.as_str(), "You're not on that channel"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            continue;
                        }
                        if msg.params.len() == 1 {
                            let topic = {
                                let g = shared.lock().await;
                                g.channel(chan.as_str()).and_then(|c| c.topic.clone())
                            };
                            match topic {
                                Some(t) => {
                                    writer
                                        .write_all(
                                            numeric(
                                                server_name,
                                                332,
                                                nick_s,
                                                &[chan.as_str(), t.as_str()],
                                            )
                                            .as_bytes(),
                                        )
                                        .await?;
                                }
                                None => {
                                    writer
                                        .write_all(
                                            numeric(
                                                server_name,
                                                331,
                                                nick_s,
                                                &[chan.as_str(), "No topic is set"],
                                            )
                                            .as_bytes(),
                                        )
                                        .await?;
                                }
                            }
                            continue;
                        }
                        let new_topic = msg.params.get(1).cloned().unwrap_or_default();
                        if field_has_control(&new_topic) {
                            writer
                                .write_all(
                                    numeric(server_name, 461, nick_s, &["TOPIC", "Invalid topic"])
                                        .as_bytes(),
                                )
                                .await?;
                            continue;
                        }
                        if new_topic.len() > cfg.limits.max_topic_bytes {
                            writer
                                .write_all(
                                    numeric(server_name, 461, nick_s, &["TOPIC", "Topic too long"])
                                        .as_bytes(),
                                )
                                .await?;
                            continue;
                        }
                        let allowed = {
                            let g = shared.lock().await;
                            match g.channel(chan.as_str()) {
                                Some(ch) if is_oper || ch.is_op(conn_id) => true,
                                Some(ch) if !ch.mode_t => true,
                                _ => false,
                            }
                        };
                        if !allowed {
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        482,
                                        nick_s,
                                        &[chan.as_str(), "You're not channel operator"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            continue;
                        }
                        {
                            let mut g = shared.lock().await;
                            if let Some(ch) = g.channel_mut(chan.as_str()) {
                                if new_topic.is_empty() {
                                    ch.topic = None;
                                } else {
                                    ch.topic = Some(new_topic.clone());
                                }
                            }
                        }
                        let line = format!(":{prefix} TOPIC {chan} :{new_topic}\r\n");
                        writer.write_all(line.as_bytes()).await?;
                        let _ = shared
                            .lock()
                            .await
                            .fanout_channel(chan.as_str(), &line, conn_id);
                        continue;
                    }

                    Command::Kick { .. } => {
                        let (Some(chan), Some(target_nick)) =
                            (msg.params.first(), msg.params.get(1))
                        else {
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        461,
                                        nick_s,
                                        &["KICK", "Not enough parameters"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            continue;
                        };
                        let reason = msg.params.get(2).map(String::as_str).unwrap_or(nick_s);
                        let (allowed, target_id) = {
                            let g = shared.lock().await;
                            let target_id = g.nick_id(&ascii_casefold(target_nick.as_str()));
                            match (g.channel(chan.as_str()), target_id) {
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
                                    .and_then(|tid| {
                                        g.channel(chan.as_str()).map(|c| c.members.contains(&tid))
                                    })
                                    .unwrap_or(false)
                            };
                            if !on_chan {
                                writer
                                    .write_all(
                                        numeric(
                                            server_name,
                                            441,
                                            nick_s,
                                            &[
                                                target_nick.as_str(),
                                                chan.as_str(),
                                                "They aren't on that channel",
                                            ],
                                        )
                                        .as_bytes(),
                                    )
                                    .await?;
                            } else {
                                writer
                                    .write_all(
                                        numeric(
                                            server_name,
                                            482,
                                            nick_s,
                                            &[chan.as_str(), "You're not channel operator"],
                                        )
                                        .as_bytes(),
                                    )
                                    .await?;
                            }
                            continue;
                        }
                        let line = format!(":{prefix} KICK {chan} {target_nick} :{reason}\r\n");
                        // C10: snapshot member ids under lock, fanout via cloned senders off lock,
                        // then mutate membership — authz already decided above.
                        if let Some(tid) = target_id {
                            let ids = {
                                let g = shared.lock().await;
                                let mut ids = g.channel_member_ids(chan.as_str(), conn_id);
                                if !ids.contains(&tid) {
                                    ids.push(tid);
                                }
                                ids
                            };
                            let senders = {
                                let g = shared.lock().await;
                                g.clone_outboxes_for(&ids)
                            };
                            let payload: std::sync::Arc<str> = std::sync::Arc::from(line.as_str());
                            for tx in senders {
                                let _ = tx.try_send(std::sync::Arc::clone(&payload));
                            }
                            {
                                let mut g = shared.lock().await;
                                if let Some(ch) = g.channel_mut(chan.as_str()) {
                                    ch.remove_member(tid);
                                    if ch.members.is_empty() {
                                        g.remove_channel(chan.as_str());
                                    }
                                }
                            }
                        }
                        // Deliver to kicker too (standard).
                        writer.write_all(line.as_bytes()).await?;
                        continue;
                    }

                    Command::Mode { .. } => {
                        let Some(target) = msg.params.first() else {
                            continue;
                        };
                        if !target.starts_with('#') {
                            // user modes: ignore for v0 except oper might set +o later
                            continue;
                        }
                        if msg.params.len() == 1 {
                            let modes = {
                                let g = shared.lock().await;
                                g.channel(target.as_str())
                                    .map(|c| c.mode_chars())
                                    .unwrap_or_else(|| "+nt".into())
                            };
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        324,
                                        nick_s,
                                        &[target.as_str(), modes.as_str()],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            continue;
                        }
                        let mode_str = msg.params.get(1).map(String::as_str).unwrap_or("");
                        // Mode letters that take a parameter consume successive params
                        // starting at index 2 (`MODE #c +oo a b` → a then b). (C8)
                        let mut mode_argi = 2usize;
                        let privileged = {
                            let g = shared.lock().await;
                            match g.channel(target.as_str()) {
                                Some(ch) => is_oper || ch.is_op(conn_id),
                                None => false,
                            }
                        };
                        if !privileged {
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        482,
                                        nick_s,
                                        &[target.as_str(), "You're not channel operator"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            continue;
                        }

                        let mut adding = true;
                        let mut applied = String::new();
                        let mut applied_args: Vec<String> = Vec::new();
                        let mut last_sign: Option<char> = None;
                        let mut push_letter = |applied: &mut String, adding: bool, letter: char| {
                            let sign = if adding { '+' } else { '-' };
                            if last_sign != Some(sign) {
                                applied.push(sign);
                                last_sign = Some(sign);
                            }
                            applied.push(letter);
                        };
                        for ch in mode_str.chars() {
                            match ch {
                                '+' => adding = true,
                                '-' => adding = false,
                                'o' => {
                                    let Some(who) = msg.params.get(mode_argi).cloned() else {
                                        continue;
                                    };
                                    mode_argi += 1;
                                    let mut g = shared.lock().await;
                                    let Some(tid) = g.nick_id(&ascii_casefold(&who)) else {
                                        drop(g);
                                        writer
                                            .write_all(
                                                numeric(
                                                    server_name,
                                                    441,
                                                    nick_s,
                                                    &[
                                                        who.as_str(),
                                                        target.as_str(),
                                                        "They aren't on that channel",
                                                    ],
                                                )
                                                .as_bytes(),
                                            )
                                            .await?;
                                        continue;
                                    };
                                    let Some(chan) = g.channel_mut(target.as_str()) else {
                                        continue;
                                    };
                                    if !chan.members.contains(&tid) {
                                        drop(g);
                                        writer
                                            .write_all(
                                                numeric(
                                                    server_name,
                                                    441,
                                                    nick_s,
                                                    &[
                                                        who.as_str(),
                                                        target.as_str(),
                                                        "They aren't on that channel",
                                                    ],
                                                )
                                                .as_bytes(),
                                            )
                                            .await?;
                                        continue;
                                    }
                                    if adding {
                                        chan.ops.insert(tid);
                                    } else {
                                        chan.ops.remove(&tid);
                                    }
                                    push_letter(&mut applied, adding, 'o');
                                    applied_args.push(who);
                                }
                                't' => {
                                    let mut g = shared.lock().await;
                                    if let Some(chan) = g.channel_mut(target.as_str()) {
                                        chan.mode_t = adding;
                                        push_letter(&mut applied, adding, 't');
                                    }
                                }
                                'n' => {
                                    let mut g = shared.lock().await;
                                    if let Some(chan) = g.channel_mut(target.as_str()) {
                                        chan.mode_n = adding;
                                        push_letter(&mut applied, adding, 'n');
                                    }
                                }
                                'i' => {
                                    let mut g = shared.lock().await;
                                    if let Some(chan) = g.channel_mut(target.as_str()) {
                                        chan.mode_i = adding;
                                        push_letter(&mut applied, adding, 'i');
                                    }
                                }
                                'b' => {
                                    let mask = msg.params.get(mode_argi).cloned();
                                    let Some(mask) = mask else {
                                        // List bans when +b/-b has no parameter.
                                        let bans = {
                                            let g = shared.lock().await;
                                            g.channel(target.as_str())
                                                .map(|c| c.bans.iter().cloned().collect::<Vec<_>>())
                                                .unwrap_or_default()
                                        };
                                        for b in bans {
                                            writer
                                                .write_all(
                                                    numeric(
                                                        server_name,
                                                        367,
                                                        nick_s,
                                                        &[target.as_str(), b.as_str()],
                                                    )
                                                    .as_bytes(),
                                                )
                                                .await?;
                                        }
                                        writer
                                            .write_all(
                                                numeric(
                                                    server_name,
                                                    368,
                                                    nick_s,
                                                    &[target.as_str(), "End of Channel Ban List"],
                                                )
                                                .as_bytes(),
                                            )
                                            .await?;
                                        continue;
                                    };
                                    mode_argi += 1;
                                    let mut g = shared.lock().await;
                                    if let Some(chan) = g.channel_mut(target.as_str()) {
                                        if adding {
                                            chan.bans.insert(mask.clone());
                                        } else {
                                            chan.bans.remove(&mask);
                                        }
                                        push_letter(&mut applied, adding, 'b');
                                        applied_args.push(mask);
                                    }
                                }
                                _ => {
                                    writer
                                        .write_all(
                                            numeric(
                                                server_name,
                                                472,
                                                nick_s,
                                                &[&ch.to_string(), "is unknown mode char to me"],
                                            )
                                            .as_bytes(),
                                        )
                                        .await?;
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
                            let _ =
                                shared
                                    .lock()
                                    .await
                                    .fanout_channel(target.as_str(), &line, conn_id);
                        }
                        continue;
                    }

                    Command::Invite { .. } => {
                        let (Some(who), Some(chan)) = (msg.params.first(), msg.params.get(1))
                        else {
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        461,
                                        nick_s,
                                        &["INVITE", "Not enough parameters"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            continue;
                        };
                        let (allowed, tid) = {
                            let g = shared.lock().await;
                            let tid = g.nick_id(&ascii_casefold(who));
                            let allowed = match g.channel(chan.as_str()) {
                                Some(ch) if ch.members.contains(&conn_id) => {
                                    is_oper || ch.is_op(conn_id) || !ch.mode_i
                                }
                                Some(_) => false,
                                None => false,
                            };
                            (allowed, tid)
                        };
                        if !allowed {
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        482,
                                        nick_s,
                                        &[chan.as_str(), "You're not channel operator"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            continue;
                        }
                        let Some(tid) = tid else {
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        401,
                                        nick_s,
                                        &[who.as_str(), "No such nick/channel"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            continue;
                        };
                        {
                            let mut g = shared.lock().await;
                            if let Some(ch) = g.channel_mut(chan.as_str()) {
                                ch.invites.insert(tid);
                            }
                        }
                        writer
                            .write_all(
                                numeric(server_name, 341, nick_s, &[who.as_str(), chan.as_str()])
                                    .as_bytes(),
                            )
                            .await?;
                        let line = format!(":{prefix} INVITE {who} :{chan}\r\n");
                        let _ = shared.lock().await.fanout_ids(&[tid], &line);
                        continue;
                    }

                    Command::Notice { .. } => {
                        let (Some(target), Some(text)) = (msg.params.first(), msg.params.get(1))
                        else {
                            continue; // NOTICE: no error replies
                        };
                        if target.starts_with('#') {
                            let allowed = {
                                let g = shared.lock().await;
                                match g.channel(target.as_str()) {
                                    Some(ch) if ch.members.contains(&conn_id) => true,
                                    Some(ch) if !ch.mode_n => true,
                                    _ => false,
                                }
                            };
                            if allowed {
                                let mut line = format!(":{prefix} NOTICE {target} :{text}\r\n");
                                line = relay_client_tags(line, &msg, &enabled_caps);
                                let _ = shared.lock().await.fanout_channel(
                                    target.as_str(),
                                    &line,
                                    conn_id,
                                );
                            }
                        } else {
                            let tid = {
                                let g = shared.lock().await;
                                g.nick_id(&ascii_casefold(target))
                            };
                            if let Some(tid) = tid {
                                let mut line = format!(":{prefix} NOTICE {target} :{text}\r\n");
                                line = relay_client_tags(line, &msg, &enabled_caps);
                                let _ = shared.lock().await.fanout_ids(&[tid], &line);
                            }
                        }
                        continue;
                    }

                    Command::Names { .. } => {
                        let chan_list = msg.params.first().cloned().unwrap_or_default();
                        let targets: Vec<String> = if chan_list.is_empty() {
                            let g = shared.lock().await;
                            g.channel_names()
                        } else {
                            chan_list
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect()
                        };
                        for chan in targets {
                            let thin = {
                                let g = shared.lock().await;
                                g.names_thin(chan.as_str())
                            };
                            if let Some(thin) = thin {
                                // Format/split after lock drop (C10).
                                let snap = thin.into_names_snapshot();
                                let overhead = server_name.len() + nick_s.len() + chan.len() + 24;
                                let max_payload =
                                    cfg.limits.max_line_bytes.saturating_sub(overhead).max(16);
                                for chunk in snap.split_for_wire(max_payload) {
                                    writer
                                        .write_all(
                                            numeric(
                                                server_name,
                                                353,
                                                nick_s,
                                                &["=", chan.as_str(), chunk.as_str()],
                                            )
                                            .as_bytes(),
                                        )
                                        .await?;
                                }
                            }
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        366,
                                        nick_s,
                                        &[chan.as_str(), "End of /NAMES list"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                        }
                        if chan_list.is_empty() {
                            // RFC: end with 366 * when listing all — already emitted per channel.
                        }
                        continue;
                    }

                    Command::List { .. } => {
                        writer
                            .write_all(
                                numeric(server_name, 321, nick_s, &["Channel", "Users Name"])
                                    .as_bytes(),
                            )
                            .await?;
                        let rows = {
                            let g = shared.lock().await;
                            let filter = msg.params.first().cloned();
                            g.list_rows(filter.as_deref())
                        };
                        for (name, n, topic) in rows {
                            let n_s = n.to_string();
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        322,
                                        nick_s,
                                        &[name.as_str(), n_s.as_str(), topic.as_str()],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                        }
                        writer
                            .write_all(
                                numeric(server_name, 323, nick_s, &["End of /LIST"]).as_bytes(),
                            )
                            .await?;
                        continue;
                    }

                    Command::Who { .. } => {
                        let mask = msg.params.first().map(String::as_str).unwrap_or("0");
                        let rows = {
                            let g = shared.lock().await;
                            let mut out = Vec::new();
                            if mask.starts_with('#') {
                                if let Some(ch) = g.channel(mask) {
                                    for id in ch.members.clone() {
                                        if let Some(n) = g.display_nick(id) {
                                            let away = g.away_message(id).is_some();
                                            let flags = match (away, ch.ops.contains(&id)) {
                                                (true, true) => "G@",
                                                (true, false) => "G",
                                                (false, true) => "H@",
                                                (false, false) => "H",
                                            };
                                            out.push((
                                                mask.to_string(),
                                                n.to_string(),
                                                flags.to_string(),
                                            ));
                                        }
                                    }
                                }
                            } else {
                                for (display, id) in g.nick_entries() {
                                    let key = ascii_casefold(&display);
                                    if mask == "0" || mask == "*" || key.eq_ignore_ascii_case(mask)
                                    {
                                        let flags = if g.away_message(id).is_some() {
                                            "G"
                                        } else {
                                            "H"
                                        };
                                        out.push(("*".into(), display, flags.into()));
                                    }
                                }
                            }
                            out
                        };
                        for (chan, n, flags) in rows {
                            // 352: <channel> <user> <host> <server> <nick> <H|G>[*][@|+] :<hopcount> <real>
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        352,
                                        nick_s,
                                        &[
                                            chan.as_str(),
                                            "user",
                                            "dsc.local",
                                            server_name,
                                            n.as_str(),
                                            flags.as_str(),
                                            "0 realname",
                                        ],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                        }
                        writer
                            .write_all(
                                numeric(server_name, 315, nick_s, &[mask, "End of /WHO list"])
                                    .as_bytes(),
                            )
                            .await?;
                        continue;
                    }

                    Command::Whois { .. } => {
                        let Some(target) = msg.params.first() else {
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        461,
                                        nick_s,
                                        &["WHOIS", "Not enough parameters"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            continue;
                        };
                        let info = {
                            let g = shared.lock().await;
                            let tid = g.nick_id(&ascii_casefold(target));
                            tid.map(|id| {
                                let nick =
                                    g.display_nick(id).unwrap_or(target.as_str()).to_string();
                                let chans = g.whois_channels(id);
                                let away = g.away_message(id).map(str::to_string);
                                (nick, chans, away)
                            })
                        };
                        let Some((who_nick, chans, away)) = info else {
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        401,
                                        nick_s,
                                        &[target.as_str(), "No such nick/channel"],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                            continue;
                        };
                        writer
                            .write_all(
                                numeric(
                                    server_name,
                                    311,
                                    nick_s,
                                    &[who_nick.as_str(), "user", "dsc.local", "*", "realname"],
                                )
                                .as_bytes(),
                            )
                            .await?;
                        if let Some(msg) = away.as_deref() {
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        301,
                                        nick_s,
                                        &[who_nick.as_str(), msg],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                        }
                        writer
                            .write_all(
                                numeric(
                                    server_name,
                                    312,
                                    nick_s,
                                    &[who_nick.as_str(), server_name, "DSC ircd"],
                                )
                                .as_bytes(),
                            )
                            .await?;
                        if !chans.is_empty() {
                            let list = chans.join(" ");
                            writer
                                .write_all(
                                    numeric(
                                        server_name,
                                        319,
                                        nick_s,
                                        &[who_nick.as_str(), list.as_str()],
                                    )
                                    .as_bytes(),
                                )
                                .await?;
                        }
                        writer
                            .write_all(
                                numeric(
                                    server_name,
                                    318,
                                    nick_s,
                                    &[who_nick.as_str(), "End of /WHOIS list"],
                                )
                                .as_bytes(),
                            )
                            .await?;
                        continue;
                    }

                    Command::Motd => {
                        let motd_start = format!("- {server_name} Message of the day -");
                        writer
                            .write_all(
                                numeric(server_name, 375, nick_s, &[motd_start.as_str()])
                                    .as_bytes(),
                            )
                            .await?;
                        for line in cfg.server.motd.lines() {
                            let body = format!("- {line}");
                            writer
                                .write_all(
                                    numeric(server_name, 372, nick_s, &[body.as_str()]).as_bytes(),
                                )
                                .await?;
                        }
                        writer
                            .write_all(
                                numeric(server_name, 376, nick_s, &["End of /MOTD command"])
                                    .as_bytes(),
                            )
                            .await?;
                        continue;
                    }

                    Command::Version => {
                        writer
                            .write_all(
                                numeric(
                                    server_name,
                                    351,
                                    nick_s,
                                    &[
                                        &format!("dsc-ircd-{VERSION}"),
                                        server_name,
                                        "Mark x Cody IRC",
                                    ],
                                )
                                .as_bytes(),
                            )
                            .await?;
                        continue;
                    }

                    Command::Lusers => {
                        let (clients, channels_n) = {
                            let g = shared.lock().await;
                            (g.client_count(), g.channel_count())
                        };
                        let c_s = clients.to_string();
                        let ch_s = channels_n.to_string();
                        writer
                            .write_all(
                                numeric(
                                    server_name,
                                    251,
                                    nick_s,
                                    &[&format!(
                                        "There are {clients} users and 0 invisible on 1 servers"
                                    )],
                                )
                                .as_bytes(),
                            )
                            .await?;
                        writer
                            .write_all(
                                numeric(server_name, 252, nick_s, &["0", "operator(s) online"])
                                    .as_bytes(),
                            )
                            .await?;
                        writer
                            .write_all(
                                numeric(
                                    server_name,
                                    254,
                                    nick_s,
                                    &[ch_s.as_str(), "channels formed"],
                                )
                                .as_bytes(),
                            )
                            .await?;
                        writer
                            .write_all(
                                numeric(
                                    server_name,
                                    255,
                                    nick_s,
                                    &[&format!("I have {c_s} clients and 1 servers")],
                                )
                                .as_bytes(),
                            )
                            .await?;
                        continue;
                    }

                    Command::Away { message } => {
                        match message {
                            Some(text) => {
                                {
                                    let mut g = shared.lock().await;
                                    g.set_away(conn_id, text.clone());
                                    let peers = g.away_notify_peers(conn_id);
                                    let line = format!(":{prefix} AWAY :{text}\r\n");
                                    let _ = g.fanout_ids(&peers, &line);
                                }
                                writer
                                    .write_all(
                                        numeric(
                                            server_name,
                                            306,
                                            nick_s,
                                            &["You have been marked as being away"],
                                        )
                                        .as_bytes(),
                                    )
                                    .await?;
                            }
                            None => {
                                {
                                    let mut g = shared.lock().await;
                                    g.clear_away(conn_id);
                                    let peers = g.away_notify_peers(conn_id);
                                    let line = format!(":{prefix} AWAY\r\n");
                                    let _ = g.fanout_ids(&peers, &line);
                                }
                                writer
                                    .write_all(
                                        numeric(
                                            server_name,
                                            305,
                                            nick_s,
                                            &["You are no longer marked as being away"],
                                        )
                                        .as_bytes(),
                                    )
                                    .await?;
                            }
                        }
                        continue;
                    }

                    Command::Unknown { verb } => {
                        writer
                            .write_all(
                                numeric(
                                    server_name,
                                    421,
                                    nick_s,
                                    &[verb.as_str(), "Unknown command"],
                                )
                                .as_bytes(),
                            )
                            .await?;
                        continue;
                    }
                    _ => {
                        writer
                            .write_all(
                                numeric(server_name, 421, nick_s, &[cmd.verb(), "Unknown command"])
                                    .as_bytes(),
                            )
                            .await?;
                        continue;
                    }
                } // inner post-register match
            }
        } // outer match
    }

    if let Some(n) = nick {
        let mut g = shared.lock().await;
        let _ = g.clear_nick(conn_id);
        if !quit_announced {
            let user_s = user.as_deref().unwrap_or("user");
            let reason = quit_reason.as_deref().unwrap_or("Connection closed");
            let line = format!(":{n}!{user_s}@dsc.local QUIT :{reason}\r\n");
            let payload: std::sync::Arc<str> = std::sync::Arc::from(line.as_str());
            for chan in &channels {
                let ids = g.channel_member_ids(chan, conn_id);
                let senders = g.clone_outboxes_for(&ids);
                for tx in senders {
                    let _ = tx.try_send(std::sync::Arc::clone(&payload));
                }
                if let Some(ch) = g.channel_mut(chan) {
                    ch.remove_member(conn_id);
                }
                g.remove_channel_if_empty(chan);
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
            let _ = g.clear_nick(conn_id);
            let affected = g.channels_containing(conn_id);
            for chan in affected {
                if let Some(ch) = g.channel_mut(&chan) {
                    ch.remove_member(conn_id);
                }
                g.remove_channel_if_empty(&chan);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::cap::{apply_cap_req, cap_name_matches};
    use super::*;

    #[test]
    fn cap_name_matches_ignores_value_suffix() {
        assert!(cap_name_matches("sasl", "sasl=PLAIN,EXTERNAL"));
        assert!(cap_name_matches("SASL=PLAIN", "sasl"));
        assert!(!cap_name_matches("message-tags", "server-time"));
    }

    #[test]
    fn apply_cap_req_ack_nak_enable_disable() {
        let advertised = advertised_caps(true, false);
        let mut enabled = HashSet::new();
        // Mixed unknown → atomic NAK, no enable
        let (ack, nak) = apply_cap_req("sasl bogon", &advertised, &mut enabled);
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
        let with = advertised_caps(true, true);
        assert!(with.iter().any(|c| c.starts_with("sasl")));
        assert!(with.iter().any(|c| c == "away-notify"));
        assert!(with.iter().any(|c| c == "message-tags"));
        assert!(with.iter().any(|c| c == "batch"));
        assert!(with.iter().any(|c| c == "draft/chathistory"));
        assert!(with.iter().any(|c| c == "account-tag"));
        assert!(with.iter().any(|c| c == "server-time"));
        assert!(!with.iter().any(|c| c == "multi-prefix"));
        let with_no_hist = advertised_caps(true, false);
        assert!(!with_no_hist.iter().any(|c| c == "draft/chathistory"));
        let without = advertised_caps(false, false);
        assert!(without.iter().any(|c| c == "cap-notify"));
        assert!(without.iter().any(|c| c == "message-tags"));
        assert!(!without.iter().any(|c| c.starts_with("sasl")));
    }

    #[test]
    fn has_cap_is_case_insensitive() {
        let mut set = HashSet::new();
        set.insert("server-time".into());
        assert!(has_cap(&set, "SERVER-TIME"));
        assert!(!has_cap(&set, "message-tags"));
    }

    #[test]
    fn session_deadline_at_reg_vs_idle() {
        let start = Instant::now();
        let act = start + Duration::from_secs(5);
        assert!(session_deadline_at(false, start, act, None, Some(Duration::from_secs(1))).is_none());
        let reg = session_deadline_at(
            false,
            start,
            act,
            Some(Duration::from_secs(10)),
            Some(Duration::from_secs(1)),
        );
        assert_eq!(reg, Some(start + Duration::from_secs(10)));
        let idle = session_deadline_at(
            true,
            start,
            act,
            Some(Duration::from_secs(10)),
            Some(Duration::from_secs(30)),
        );
        assert_eq!(idle, Some(act + Duration::from_secs(30)));
    }

    #[tokio::test]
    async fn read_line_outcome_complete_oversized_eof() {
        use tokio::io::BufReader;
        // Complete line
        let mut r = BufReader::new(std::io::Cursor::new(b"PING :x\r\n".to_vec()));
        let mut buf = Vec::new();
        assert!(matches!(
            read_line_outcome(&mut r, &mut buf, 64).await.unwrap(),
            LineOutcome::Complete
        ));
        assert!(buf.ends_with(b"\n"));
        buf.clear();
        assert!(matches!(
            read_line_outcome(&mut r, &mut buf, 64).await.unwrap(),
            LineOutcome::Eof
        ));

        // Oversized without newline
        let huge = vec![b'A'; 80];
        let mut r = BufReader::new(std::io::Cursor::new(huge));
        let mut buf = Vec::new();
        assert!(matches!(
            read_line_outcome(&mut r, &mut buf, 64).await.unwrap(),
            LineOutcome::Oversized
        ));
        assert!(buf.len() >= 64);
    }
}
