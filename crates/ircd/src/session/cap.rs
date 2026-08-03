//! CAP / SASL negotiation handlers (H-14).

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use ircd_core::{numeric, RawLine};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tracing::info;

use crate::state::{ClientId, Shared};

/// Caps advertised without per-cap conformance tests must stay empty (A1 / Doc #974).
/// SASL is added conditionally in `advertised_caps` when accounts exist.
pub(crate) const BASE_CAPS: &[&str] = &[
    "cap-notify",
    "message-tags",
    "server-time",
    "account-tag",
    "batch",
    "away-notify",
];

pub(crate) fn advertised_caps(has_accounts: bool, has_history: bool) -> Vec<String> {
    let mut caps: Vec<String> = BASE_CAPS.iter().map(|s| (*s).to_string()).collect();
    if has_accounts {
        caps.push("sasl=PLAIN".to_string());
    }
    if has_history {
        caps.push("draft/chathistory".to_string());
    }
    caps
}

pub(crate) fn cap_name_matches(requested: &str, advertised: &str) -> bool {
    let req = requested.split('=').next().unwrap_or(requested);
    let adv = advertised.split('=').next().unwrap_or(advertised);
    req.eq_ignore_ascii_case(adv)
}

/// Apply a CAP REQ token list; returns (ACK tokens, NAK tokens).
pub(crate) fn apply_cap_req(
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

pub(crate) fn has_cap(enabled: &HashSet<String>, name: &str) -> bool {
    enabled
        .iter()
        .any(|c| c.split('=').next().unwrap_or(c).eq_ignore_ascii_case(name))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SaslState {
    Idle,
    /// Accumulating AUTHENTICATE base64 chunks (IRCv3 ≤400 octets per line).
    AwaitPlain { buf: String },
}

const SASL_CHUNK_MAX: usize = 400;
const SASL_TOTAL_MAX: usize = 1024;

pub(crate) async fn handle_cap<W>(
    writer: &mut W,
    server_name: &str,
    msg: &RawLine,
    nick: Option<&str>,
    cap_negotiating: &mut bool,
    enabled_caps: &mut HashSet<String>,
    advertised: &[String],
    shared: &Arc<Mutex<Shared>>,
    conn_id: ClientId,
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
        let has_away = has_cap(enabled_caps, "away-notify");
        shared.lock().await.set_away_notify(conn_id, has_away);
        return Ok(());
    }
    if sub.eq_ignore_ascii_case("END") {
        *cap_negotiating = false;
        return Ok(());
    }
    writer
        .write_all(numeric(server_name, 410, nick_s, &[sub, "Invalid CAP subcommand"]).as_bytes())
        .await?;
    Ok(())
}

pub(crate) async fn handle_authenticate<W>(
    writer: &mut W,
    server_name: &str,
    msg: &RawLine,
    nick: Option<&str>,
    user: Option<&str>,
    accounts: &[crate::config::AccountSection],
    enabled_caps: &HashSet<String>,
    sasl_state: &mut SaslState,
    account: &mut Option<String>,
    shared: &Arc<Mutex<Shared>>,
    conn_id: ClientId,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    use base64::Engine;

    let nick_s = nick.unwrap_or("*");
    if !has_cap(enabled_caps, "sasl") {
        writer
            .write_all(
                numeric(server_name, 904, nick_s, &["SASL authentication failed"]).as_bytes(),
            )
            .await?;
        return Ok(());
    }
    let param = msg.params.first().map(String::as_str).unwrap_or("");
    match sasl_state.clone() {
        SaslState::Idle => {
            if account.is_some() {
                writer
                    .write_all(
                        numeric(
                            server_name,
                            907,
                            nick_s,
                            &["You have already authenticated using SASL"],
                        )
                        .as_bytes(),
                    )
                    .await?;
                return Ok(());
            }
            if param.eq_ignore_ascii_case("PLAIN") {
                *sasl_state = SaslState::AwaitPlain { buf: String::new() };
                writer.write_all(b"AUTHENTICATE +\r\n").await?;
            } else if param == "*" {
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
        SaslState::AwaitPlain { mut buf } => {
            if param == "*" {
                *sasl_state = SaslState::Idle;
                writer
                    .write_all(
                        numeric(server_name, 906, nick_s, &["SASL authentication aborted"])
                            .as_bytes(),
                    )
                    .await?;
                return Ok(());
            }
            if param == "+" {
                // Empty final chunk — finalize with what we have.
            } else {
                if param.len() > SASL_CHUNK_MAX {
                    *sasl_state = SaslState::Idle;
                    writer
                        .write_all(
                            numeric(
                                server_name,
                                905,
                                nick_s,
                                &["SASL message too long"],
                            )
                            .as_bytes(),
                        )
                        .await?;
                    return Ok(());
                }
                buf.push_str(param);
                if buf.len() > SASL_TOTAL_MAX {
                    *sasl_state = SaslState::Idle;
                    writer
                        .write_all(
                            numeric(
                                server_name,
                                905,
                                nick_s,
                                &["SASL message too long"],
                            )
                            .as_bytes(),
                        )
                        .await?;
                    return Ok(());
                }
                // Exact 400 ⇒ more chunks expected.
                if param.len() == SASL_CHUNK_MAX {
                    *sasl_state = SaslState::AwaitPlain { buf };
                    return Ok(());
                }
            }
            *sasl_state = SaslState::Idle;
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(buf.as_bytes()) else {
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
            shared
                .lock()
                .await
                .set_account(conn_id, acc.name.clone());
            let user_s = user.unwrap_or("user");
            let host = "dsc.local";
            let full = format!("{nick_s}!{user_s}@{host}");
            writer
                .write_all(
                    numeric(
                        server_name,
                        900,
                        nick_s,
                        &[
                            full.as_str(),
                            acc.name.as_str(),
                            &format!("You are now logged in as {}", acc.name),
                        ],
                    )
                    .as_bytes(),
                )
                .await?;
            writer
                .write_all(
                    numeric(
                        server_name,
                        903,
                        nick_s,
                        &["SASL authentication successful"],
                    )
                    .as_bytes(),
                )
                .await?;
            info!(account = %acc.name, nick = %nick_s, "SASL PLAIN success");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AccountSection, Config};
    use crate::state::Shared;
    use ircd_core::RawLine;
    use tokio::io::{AsyncBufReadExt, BufReader};

    #[test]
    fn apply_cap_req_empty_and_disable() {
        let adv = advertised_caps(true, true);
        let mut enabled = HashSet::new();
        let (ack, nak) = apply_cap_req("", &adv, &mut enabled);
        assert!(ack.is_empty() && nak.is_empty());
        let (ack, nak) = apply_cap_req("batch", &adv, &mut enabled);
        assert!(ack.contains(&"batch".into()));
        assert!(nak.is_empty());
        let (ack, nak) = apply_cap_req("-batch", &adv, &mut enabled);
        assert_eq!(ack, vec!["-batch".to_string()]);
        assert!(nak.is_empty());
        assert!(!has_cap(&enabled, "batch"));
    }

    #[tokio::test]
    async fn authenticate_success_abort_total_limit() {
        let cfg = Arc::new(Config::default());
        let shared = Arc::new(Mutex::new(Shared::new(cfg, None)));
        let accounts = [AccountSection {
            name: "alice".into(),
            password: "secret".into(),
        }];
        let mut caps = HashSet::new();
        caps.insert("sasl".into());
        let mut state = SaslState::Idle;
        let mut account = None;

        // Success path (900/903).
        let (c, s) = tokio::io::duplex(4096);
        let mut w = s;
        let mut msg = RawLine::parse("AUTHENTICATE PLAIN").unwrap();
        handle_authenticate(
            &mut w,
            "t",
            &msg,
            Some("n"),
            Some("u"),
            &accounts,
            &caps,
            &mut state,
            &mut account,
            &shared,
            1,
        )
        .await
        .unwrap();
        msg = RawLine::parse("AUTHENTICATE AGFsaWNlAHNlY3JldA==").unwrap();
        handle_authenticate(
            &mut w,
            "t",
            &msg,
            Some("n"),
            Some("u"),
            &accounts,
            &caps,
            &mut state,
            &mut account,
            &shared,
            1,
        )
        .await
        .unwrap();
        drop(w);
        let mut body = String::new();
        let mut r = BufReader::new(c);
        let mut line = String::new();
        while r.read_line(&mut line).await.unwrap() > 0 {
            body.push_str(&line);
            line.clear();
        }
        assert!(body.contains("903"));
        assert_eq!(account.as_deref(), Some("alice"));

        // Idle abort via *
        state = SaslState::Idle;
        account = None;
        let (c, s) = tokio::io::duplex(1024);
        let mut w = s;
        msg = RawLine::parse("AUTHENTICATE *").unwrap();
        handle_authenticate(
            &mut w,
            "t",
            &msg,
            Some("n"),
            Some("u"),
            &accounts,
            &caps,
            &mut state,
            &mut account,
            &shared,
            1,
        )
        .await
        .unwrap();
        drop(w);
        let mut body = String::new();
        let mut r = BufReader::new(c);
        let mut line = String::new();
        while r.read_line(&mut line).await.unwrap() > 0 {
            body.push_str(&line);
            line.clear();
        }
        assert!(body.contains("906"));

        // Total buffer > SASL_TOTAL_MAX → 905
        state = SaslState::Idle;
        let (c, s) = tokio::io::duplex(2048);
        let mut w = s;
        msg = RawLine::parse("AUTHENTICATE PLAIN").unwrap();
        handle_authenticate(
            &mut w,
            "t",
            &msg,
            Some("n"),
            Some("u"),
            &accounts,
            &caps,
            &mut state,
            &mut account,
            &shared,
            1,
        )
        .await
        .unwrap();
        let chunk = "A".repeat(400);
        for _ in 0..3 {
            msg = RawLine::parse(&format!("AUTHENTICATE {chunk}")).unwrap();
            handle_authenticate(
                &mut w,
                "t",
                &msg,
                Some("n"),
                Some("u"),
                &accounts,
                &caps,
                &mut state,
                &mut account,
                &shared,
                1,
            )
            .await
            .unwrap();
        }
        drop(w);
        let mut body = String::new();
        let mut r = BufReader::new(c);
        let mut line = String::new();
        while r.read_line(&mut line).await.unwrap() > 0 {
            body.push_str(&line);
            line.clear();
        }
        assert!(body.contains("905"), "total too long: {body}");
    }
}
