//! CHATHISTORY reply framing (F3 / F2b batch invariants).

use std::collections::HashSet;

use anyhow::Result;
use ircd_core::tags::{adapt_bus_line, prepend_tag, unix_ms_to_rfc3339};
use tokio::io::AsyncWriteExt;

use crate::history::{HistMsg, HistQuery, HistoryStore};
use super::cap::has_cap;

pub(super) fn is_targets(params: &[String]) -> bool {
    params
        .first()
        .map(|s| s.eq_ignore_ascii_case("TARGETS"))
        .unwrap_or(false)
}

/// Parse CHATHISTORY params into (query, channel, limit).
/// For TARGETS, channel is empty.
pub(super) fn parse_sub(params: &[String]) -> Option<(HistQuery, String, usize)> {
    let sub = params.first()?.as_str();
    if sub.eq_ignore_ascii_case("TARGETS") {
        let limit = params
            .get(2)
            .or_else(|| params.get(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(50)
            .clamp(1, 200);
        return Some((HistQuery::Latest, String::new(), limit));
    }
    let channel = params.get(1)?.clone();
    if sub.eq_ignore_ascii_case("BETWEEN") {
        let a = HistoryStore::parse_selector(params.get(2).map(String::as_str).unwrap_or("*"));
        let b = HistoryStore::parse_selector(params.get(3).map(String::as_str).unwrap_or("*"));
        let limit = params
            .get(4)
            .and_then(|s| s.parse().ok())
            .unwrap_or(50)
            .clamp(1, 200);
        return Some((HistQuery::Between(a, b), channel, limit));
    }
    let sel = HistoryStore::parse_selector(params.get(2).map(String::as_str).unwrap_or("*"));
    let limit = params
        .get(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(50)
        .clamp(1, 200);
    let kind = if sub.eq_ignore_ascii_case("LATEST") {
        HistQuery::Latest
    } else if sub.eq_ignore_ascii_case("BEFORE") {
        HistQuery::Before(sel)
    } else if sub.eq_ignore_ascii_case("AFTER") {
        HistQuery::After(sel)
    } else if sub.eq_ignore_ascii_case("AROUND") {
        HistQuery::Around(sel)
    } else {
        return None;
    };
    Some((kind, channel, limit))
}

pub(super) async fn emit_history_batch<W>(
    writer: &mut W,
    server_name: &str,
    rows: &[HistMsg],
    enabled_caps: &HashSet<String>,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let use_batch = has_cap(enabled_caps, "batch");
    if use_batch {
        writer
            .write_all(format!(":{server_name} BATCH +chathist draft/chathistory\r\n").as_bytes())
            .await?;
    }
    for h in rows {
        let mut line = adapt_bus_line(&h.tagged_privmsg(), enabled_caps);
        if use_batch {
            line = prepend_tag(&line, "batch", "chathist");
        }
        writer.write_all(line.as_bytes()).await?;
    }
    if use_batch {
        writer
            .write_all(format!(":{server_name} BATCH -chathist\r\n").as_bytes())
            .await?;
    }
    Ok(())
}

pub(super) async fn emit_targets_batch<W>(
    writer: &mut W,
    server_name: &str,
    targets: &[(String, i64, i64)],
    enabled_caps: &HashSet<String>,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let use_batch = has_cap(enabled_caps, "batch");
    if use_batch {
        writer
            .write_all(format!(":{server_name} BATCH +chathist draft/chathistory\r\n").as_bytes())
            .await?;
    }
    for (chan, id, ts) in targets {
        let msgid = format!("dsc{id}");
        let stamp = unix_ms_to_rfc3339(*ts);
        let mut line =
            format!("@msgid={msgid};time={stamp} :{server_name} CHATHISTORY TARGETS {chan}\r\n");
        if use_batch {
            line = prepend_tag(&line, "batch", "chathist");
        }
        let line = adapt_bus_line(&line, enabled_caps);
        writer.write_all(line.as_bytes()).await?;
    }
    if use_batch {
        writer
            .write_all(format!(":{server_name} BATCH -chathist\r\n").as_bytes())
            .await?;
    }
    Ok(())
}
