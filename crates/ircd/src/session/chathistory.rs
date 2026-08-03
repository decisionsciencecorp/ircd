//! CHATHISTORY reply framing (F3 / F2b batch invariants).

use std::collections::HashSet;

use anyhow::Result;
use ircd_core::tags::{adapt_bus_line, prepend_tag, unix_ms_to_rfc3339};
use tokio::io::AsyncWriteExt;

use super::cap::has_cap;
use crate::history::{HistBound, HistMsg, HistQuery, HistoryStore};

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
        // Same ref family only (msgid↔msgid or timestamp↔timestamp).
        let ok = matches!(
            (a, b),
            (HistBound::MsgId(_), HistBound::MsgId(_)) | (HistBound::TsMs(_), HistBound::TsMs(_))
        );
        if !ok {
            return None;
        }
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
        // AROUND requires a real pivot (* / None is not a coherent window).
        if matches!(sel, HistBound::None) {
            return None;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{HistBound, HistQuery};
    use tokio::io::{AsyncBufReadExt, BufReader};

    #[test]
    fn parse_sub_verbs() {
        assert!(is_targets(&["TARGETS".into(), "*".into(), "10".into()]));
        let (q, ch, lim) = parse_sub(&["TARGETS".into(), "*".into(), "10".into()]).unwrap();
        assert!(matches!(q, HistQuery::Latest));
        assert!(ch.is_empty());
        assert_eq!(lim, 10);
        let (q, ch, lim) = parse_sub(&[
            "BETWEEN".into(),
            "#c".into(),
            "msgid=dsc1".into(),
            "msgid=dsc4".into(),
            "8".into(),
        ])
        .unwrap();
        assert!(matches!(
            q,
            HistQuery::Between(HistBound::MsgId(1), HistBound::MsgId(4))
        ));
        assert_eq!(ch, "#c");
        assert_eq!(lim, 8);
        assert!(
            parse_sub(&["AROUND".into(), "#c".into(), "*".into(), "3".into()]).is_none(),
            "AROUND * rejected"
        );
        let (q, _, _) = parse_sub(&[
            "AROUND".into(),
            "#c".into(),
            "msgid=dsc2".into(),
            "3".into(),
        ])
        .unwrap();
        assert!(matches!(q, HistQuery::Around(HistBound::MsgId(2))));
        let (q, _, _) =
            parse_sub(&["AFTER".into(), "#c".into(), "dsc2".into(), "5".into()]).unwrap();
        assert!(matches!(q, HistQuery::After(HistBound::MsgId(2))));
        let (q, _, _) = parse_sub(&["BEFORE".into(), "#c".into(), "*".into()]).unwrap();
        assert!(matches!(q, HistQuery::Before(_)));
        let (q, _, _) = parse_sub(&["LATEST".into(), "#c".into(), "*".into(), "1".into()]).unwrap();
        assert!(matches!(q, HistQuery::Latest));
        assert!(parse_sub(&["NOPE".into(), "#c".into()]).is_none());
        assert!(
            parse_sub(&[
                "BETWEEN".into(),
                "#c".into(),
                "msgid=dsc1".into(),
                "*".into(),
                "5".into()
            ])
            .is_none(),
            "BETWEEN mixed/unbound rejected"
        );
    }

    #[tokio::test]
    async fn emit_targets_and_history_batch() {
        let mut caps = HashSet::new();
        caps.insert("batch".into());
        caps.insert("server-time".into());
        caps.insert("message-tags".into());
        let (client, server) = tokio::io::duplex(8192);
        let mut writer = server;
        let rows = [HistMsg {
            id: 7,
            channel: "#lab".into(),
            ts_ms: 1_700_000_000_000,
            prefix: "n!u@h".into(),
            text: "hi".into(),
        }];
        emit_history_batch(&mut writer, "srv", &rows, &caps)
            .await
            .unwrap();
        emit_targets_batch(
            &mut writer,
            "srv",
            &[("#lab".into(), 7, 1_700_000_000_000)],
            &caps,
        )
        .await
        .unwrap();
        drop(writer);
        let mut r = BufReader::new(client);
        let mut all = String::new();
        let mut buf = String::new();
        while r.read_line(&mut buf).await.unwrap() > 0 {
            all.push_str(&buf);
            buf.clear();
        }
        assert!(all.contains("BATCH +chathist"));
        assert!(all.contains("PRIVMSG #lab"));
        assert!(all.contains("CHATHISTORY TARGETS #lab"));
        assert!(all.contains("BATCH -chathist"));
    }
}
