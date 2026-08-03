//! Single-node channel history (sqlite) + CHATHISTORY helpers.
//!
//! Sync rusqlite work runs under [`tokio::task::spawn_blocking`] via the
//! `*_async` methods so Tokio worker threads are not stalled (H-06).
//!
//! Prune cadence (C12): per-channel DELETE runs only when the in-memory count
//! exceeds `max_per_channel` (not on every INSERT). Global retention DELETE runs
//! only when approximate total rows exceed `max_total_rows` (not on every INSERT
//! under the cap).

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};

use crate::fs_perms::{ensure_private_dir, ensure_private_file};
use ircd_core::tags::unix_ms_to_rfc3339;
use rusqlite::{params, Connection};

#[derive(Debug, Clone)]
pub struct HistMsg {
    pub id: i64,
    pub channel: String,
    pub ts_ms: i64,
    pub prefix: String,
    pub text: String,
}

impl HistMsg {
    pub fn msgid(&self) -> String {
        format!("dsc{}", self.id)
    }

    pub fn tagged_privmsg(&self) -> String {
        let stamp = unix_ms_to_rfc3339(self.ts_ms);
        format!(
            "@msgid={};time={} :{} PRIVMSG {} :{}\r\n",
            self.msgid(),
            stamp,
            self.prefix,
            self.channel,
            self.text
        )
    }
}

struct HistInner {
    db: Connection,
    /// Approximate live row counts per channel (reset on prune).
    chan_counts: HashMap<String, usize>,
}

pub struct HistoryStore {
    inner: Mutex<HistInner>,
    max_per_channel: usize,
    max_total_rows: usize,
    /// Approximate live row count for global retention (C12).
    approx_total: AtomicU64,
}

impl HistoryStore {
    pub fn open(path: &Path, max_per_channel: usize) -> Result<Self> {
        Self::open_with_retention(path, max_per_channel, 0)
    }

    pub fn open_with_retention(
        path: &Path,
        max_per_channel: usize,
        max_total_rows: usize,
    ) -> Result<Self> {
        if let Some(parent) = path.parent() {
            ensure_private_dir(parent)?;
        }
        if path.exists() {
            ensure_private_file(path)?;
        }
        let db = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        {
            use std::os::unix::fs::PermissionsExt;
            let meta =
                std::fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            std::fs::set_permissions(path, perms)
                .with_context(|| format!("chmod 0600 {}", path.display()))?;
        }
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS channel_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel TEXT NOT NULL,
                ts_ms INTEGER NOT NULL,
                prefix TEXT NOT NULL,
                text TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_hist_chan_id ON channel_history(channel, id);
             CREATE INDEX IF NOT EXISTS idx_hist_chan_ts ON channel_history(channel, ts_ms);",
        )?;
        Ok(Self {
            inner: Mutex::new(HistInner {
                db,
                chan_counts: HashMap::new(),
            }),
            max_per_channel: max_per_channel.max(1),
            max_total_rows,
            approx_total: AtomicU64::new(0),
        })
    }

    pub fn append(&self, channel: &str, prefix: &str, text: &str) -> Result<HistMsg> {
        let ts_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let mut g = self.inner.lock().unwrap();
        {
            let mut stmt = g.db.prepare_cached(
                "INSERT INTO channel_history (channel, ts_ms, prefix, text) VALUES (?1, ?2, ?3, ?4)",
            )?;
            stmt.execute(params![channel, ts_ms, prefix, text])?;
        }
        let id = g.db.last_insert_rowid();
        let count = {
            let e = g.chan_counts.entry(channel.to_string()).or_insert(0);
            *e = e.saturating_add(1);
            *e
        };
        // Per-channel prune only when over the cap (C12) — not every INSERT.
        if count > self.max_per_channel {
            let deleted = count - self.max_per_channel;
            g.db.execute(
                "DELETE FROM channel_history WHERE channel = ?1 AND id NOT IN (
                    SELECT id FROM channel_history WHERE channel = ?1 ORDER BY id DESC LIMIT ?2
                 )",
                params![channel, self.max_per_channel as i64],
            )?;
            g.chan_counts
                .insert(channel.to_string(), self.max_per_channel);
            let _ = self
                .approx_total
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
                    Some(cur.saturating_sub(deleted as u64))
                });
        }
        let approx = self.approx_total.fetch_add(1, Ordering::Relaxed) + 1;
        // Global prune only when over the total cap (not every INSERT under the cap).
        if self.max_total_rows > 0 && approx > self.max_total_rows as u64 {
            g.db.execute(
                "DELETE FROM channel_history WHERE id NOT IN (
                    SELECT id FROM channel_history ORDER BY id DESC LIMIT ?1
                 )",
                params![self.max_total_rows as i64],
            )?;
            self.approx_total
                .store(self.max_total_rows as u64, Ordering::Relaxed);
            g.chan_counts.clear();
        }
        Ok(HistMsg {
            id,
            channel: channel.to_string(),
            ts_ms,
            prefix: prefix.to_string(),
            text: text.to_string(),
        })
    }

    pub fn latest(&self, channel: &str, limit: usize) -> Result<Vec<HistMsg>> {
        let limit = limit.clamp(1, 200) as i64;
        let g = self.inner.lock().unwrap();
        let mut stmt = g.db.prepare_cached(
            "SELECT id, channel, ts_ms, prefix, text FROM channel_history
             WHERE channel = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![channel, limit], |row| {
            Ok(HistMsg {
                id: row.get(0)?,
                channel: row.get(1)?,
                ts_ms: row.get(2)?,
                prefix: row.get(3)?,
                text: row.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        out.reverse(); // chronological
        Ok(out)
    }

    /// Parse `dsc<id>` msgid → row id.
    pub fn parse_msgid(s: &str) -> Option<i64> {
        s.strip_prefix("dsc")?.parse().ok()
    }

    /// Parse IRCv3 timestamp=… or msgid=… selector; `*` → None bound.
    pub fn parse_selector(sel: &str) -> HistBound {
        if sel == "*" {
            return HistBound::None;
        }
        if let Some(rest) = sel.strip_prefix("msgid=") {
            return HistBound::MsgId(Self::parse_msgid(rest).unwrap_or(-1));
        }
        if let Some(rest) = sel
            .strip_prefix("timestamp=")
            .or_else(|| sel.strip_prefix("time="))
        {
            return HistBound::TsMs(rfc3339_to_unix_ms(rest).unwrap_or(-1));
        }
        // Bare msgid without prefix (lenient).
        if let Some(id) = Self::parse_msgid(sel) {
            return HistBound::MsgId(id);
        }
        HistBound::None
    }

    pub fn before(&self, channel: &str, bound: HistBound, limit: usize) -> Result<Vec<HistMsg>> {
        let limit = limit.clamp(1, 200) as i64;
        let g = self.inner.lock().unwrap();
        let mut out = Vec::new();
        match bound {
            HistBound::None => {
                let mut stmt = g.db.prepare_cached(
                    "SELECT id, channel, ts_ms, prefix, text FROM channel_history
                     WHERE channel = ?1 ORDER BY id DESC LIMIT ?2",
                )?;
                for r in stmt.query_map(params![channel, limit], row_hist)? {
                    out.push(r?);
                }
            }
            HistBound::MsgId(id) => {
                let mut stmt = g.db.prepare_cached(
                    "SELECT id, channel, ts_ms, prefix, text FROM channel_history
                     WHERE channel = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3",
                )?;
                for r in stmt.query_map(params![channel, id, limit], row_hist)? {
                    out.push(r?);
                }
            }
            HistBound::TsMs(ts) => {
                let mut stmt = g.db.prepare_cached(
                    "SELECT id, channel, ts_ms, prefix, text FROM channel_history
                     WHERE channel = ?1 AND ts_ms < ?2 ORDER BY id DESC LIMIT ?3",
                )?;
                for r in stmt.query_map(params![channel, ts, limit], row_hist)? {
                    out.push(r?);
                }
            }
        }
        out.reverse();
        Ok(out)
    }

    pub fn after(&self, channel: &str, bound: HistBound, limit: usize) -> Result<Vec<HistMsg>> {
        let limit = limit.clamp(1, 200) as i64;
        let g = self.inner.lock().unwrap();
        let mut out = Vec::new();
        match bound {
            HistBound::None => {
                let mut stmt = g.db.prepare_cached(
                    "SELECT id, channel, ts_ms, prefix, text FROM channel_history
                     WHERE channel = ?1 ORDER BY id ASC LIMIT ?2",
                )?;
                for r in stmt.query_map(params![channel, limit], row_hist)? {
                    out.push(r?);
                }
            }
            HistBound::MsgId(id) => {
                let mut stmt = g.db.prepare_cached(
                    "SELECT id, channel, ts_ms, prefix, text FROM channel_history
                     WHERE channel = ?1 AND id > ?2 ORDER BY id ASC LIMIT ?3",
                )?;
                for r in stmt.query_map(params![channel, id, limit], row_hist)? {
                    out.push(r?);
                }
            }
            HistBound::TsMs(ts) => {
                let mut stmt = g.db.prepare_cached(
                    "SELECT id, channel, ts_ms, prefix, text FROM channel_history
                     WHERE channel = ?1 AND ts_ms > ?2 ORDER BY id ASC LIMIT ?3",
                )?;
                for r in stmt.query_map(params![channel, ts, limit], row_hist)? {
                    out.push(r?);
                }
            }
        }
        Ok(out)
    }

    pub fn around(&self, channel: &str, bound: HistBound, limit: usize) -> Result<Vec<HistMsg>> {
        let limit = limit.clamp(1, 200);
        let before_n = limit / 2;
        let after_n = limit.saturating_sub(before_n);
        let before = self.before(channel, bound, before_n.max(1))?;
        let mut after = self.after(channel, bound, after_n.max(1))?;
        let pivot = match bound {
            HistBound::MsgId(id) if id > 0 => self.by_id(channel, id)?,
            HistBound::TsMs(ts) if ts >= 0 => self.nearest_ts(channel, ts)?,
            _ => None,
        };
        let mut out = before;
        if let Some(p) = pivot {
            if out.last().map(|h| h.id) != Some(p.id) && after.first().map(|h| h.id) != Some(p.id) {
                out.push(p);
            }
        }
        out.append(&mut after);
        out.truncate(limit);
        Ok(out)
    }

    pub fn between(
        &self,
        channel: &str,
        a: HistBound,
        b: HistBound,
        limit: usize,
    ) -> Result<Vec<HistMsg>> {
        let limit = limit.clamp(1, 200) as i64;
        match (a, b) {
            (HistBound::MsgId(x), HistBound::MsgId(y)) => {
                let lo = x.min(y);
                let hi = x.max(y);
                let g = self.inner.lock().unwrap();
                let mut stmt = g.db.prepare_cached(
                    "SELECT id, channel, ts_ms, prefix, text FROM channel_history
                     WHERE channel = ?1 AND id >= ?2 AND id <= ?3
                     ORDER BY id ASC LIMIT ?4",
                )?;
                let mut out = Vec::new();
                for r in stmt.query_map(params![channel, lo, hi, limit], row_hist)? {
                    out.push(r?);
                }
                Ok(out)
            }
            (HistBound::TsMs(x), HistBound::TsMs(y)) => {
                let lo_ts = x.min(y);
                let hi_ts = x.max(y);
                let g = self.inner.lock().unwrap();
                let mut stmt = g.db.prepare_cached(
                    "SELECT id, channel, ts_ms, prefix, text FROM channel_history
                     WHERE channel = ?1 AND ts_ms >= ?2 AND ts_ms <= ?3
                     ORDER BY id ASC LIMIT ?4",
                )?;
                let mut out = Vec::new();
                for r in stmt.query_map(params![channel, lo_ts, hi_ts, limit], row_hist)? {
                    out.push(r?);
                }
                Ok(out)
            }
            // Mixed / unbound selectors are invalid for BETWEEN (MSGREFTYPES honesty).
            _ => Ok(Vec::new()),
        }
    }

    fn by_id(&self, channel: &str, id: i64) -> Result<Option<HistMsg>> {
        let g = self.inner.lock().unwrap();
        let mut stmt = g.db.prepare_cached(
            "SELECT id, channel, ts_ms, prefix, text FROM channel_history
             WHERE channel = ?1 AND id = ?2 LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![channel, id], row_hist)?;
        Ok(rows.next().transpose()?)
    }

    /// Nearest row by timestamp using two bounded `(channel, ts_ms)` index seeks
    /// (floor ≤ ts and ceil ≥ ts) — not `ORDER BY ABS(...)` full-channel scan.
    fn nearest_ts(&self, channel: &str, ts: i64) -> Result<Option<HistMsg>> {
        let g = self.inner.lock().unwrap();
        let mut floor_stmt = g.db.prepare_cached(
            "SELECT id, channel, ts_ms, prefix, text FROM channel_history
             WHERE channel = ?1 AND ts_ms <= ?2
             ORDER BY ts_ms DESC, id DESC LIMIT 1",
        )?;
        let floor = floor_stmt
            .query_map(params![channel, ts], row_hist)?
            .next()
            .transpose()?;
        let mut ceil_stmt = g.db.prepare_cached(
            "SELECT id, channel, ts_ms, prefix, text FROM channel_history
             WHERE channel = ?1 AND ts_ms >= ?2
             ORDER BY ts_ms ASC, id ASC LIMIT 1",
        )?;
        let ceil = ceil_stmt
            .query_map(params![channel, ts], row_hist)?
            .next()
            .transpose()?;
        Ok(match (floor, ceil) {
            (Some(a), Some(b)) => {
                let da = (a.ts_ms - ts).abs();
                let db = (b.ts_ms - ts).abs();
                if da < db || (da == db && a.id <= b.id) {
                    Some(a)
                } else {
                    Some(b)
                }
            }
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        })
    }

    /// Channels that have at least one history row (for TARGETS).
    pub fn channels_with_history(&self, limit: usize) -> Result<Vec<(String, i64, i64)>> {
        let limit = limit.clamp(1, 200) as i64;
        let g = self.inner.lock().unwrap();
        let mut stmt = g.db.prepare_cached(
            "SELECT channel, MAX(id), MAX(ts_ms) FROM channel_history
             GROUP BY channel ORDER BY MAX(id) DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Append on a blocking pool thread (safe from async contexts).
    pub async fn append_async(
        self: &Arc<Self>,
        channel: String,
        prefix: String,
        text: String,
    ) -> Result<HistMsg> {
        let store = Arc::clone(self);
        tokio::task::spawn_blocking(move || store.append(&channel, &prefix, &text))
            .await
            .context("history append join")?
    }

    /// Latest on a blocking pool thread (safe from async contexts).
    pub async fn latest_async(
        self: &Arc<Self>,
        channel: String,
        limit: usize,
    ) -> Result<Vec<HistMsg>> {
        let store = Arc::clone(self);
        tokio::task::spawn_blocking(move || store.latest(&channel, limit))
            .await
            .context("history latest join")?
    }

    pub async fn query_async(
        self: &Arc<Self>,
        kind: HistQuery,
        channel: String,
        limit: usize,
    ) -> Result<Vec<HistMsg>> {
        let store = Arc::clone(self);
        tokio::task::spawn_blocking(move || match kind {
            HistQuery::Latest => store.latest(&channel, limit),
            HistQuery::Before(b) => store.before(&channel, b, limit),
            HistQuery::After(b) => store.after(&channel, b, limit),
            HistQuery::Around(b) => store.around(&channel, b, limit),
            HistQuery::Between(a, b) => store.between(&channel, a, b, limit),
        })
        .await
        .context("history query join")?
    }

    pub async fn targets_async(self: &Arc<Self>, limit: usize) -> Result<Vec<(String, i64, i64)>> {
        let store = Arc::clone(self);
        tokio::task::spawn_blocking(move || store.channels_with_history(limit))
            .await
            .context("history targets join")?
    }
}

fn row_hist(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistMsg> {
    Ok(HistMsg {
        id: row.get(0)?,
        channel: row.get(1)?,
        ts_ms: row.get(2)?,
        prefix: row.get(3)?,
        text: row.get(4)?,
    })
}

/// CHATHISTORY selector bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistBound {
    None,
    MsgId(i64),
    TsMs(i64),
}

/// CHATHISTORY query kind (channel-scoped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistQuery {
    Latest,
    Before(HistBound),
    After(HistBound),
    Around(HistBound),
    Between(HistBound, HistBound),
}

/// Best-effort RFC3339 → unix ms (accepts `…Z` with optional fractional seconds).
pub fn rfc3339_to_unix_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    // YYYY-MM-DDTHH:MM:SS(.mmm)Z
    if s.len() < 20 || !s.ends_with('Z') {
        return None;
    }
    let body = &s[..s.len() - 1];
    let (date, time) = body.split_once('T')?;
    let mut d = date.split('-');
    let y: i64 = d.next()?.parse().ok()?;
    let mo: u32 = d.next()?.parse().ok()?;
    let day: u32 = d.next()?.parse().ok()?;
    let (hms, frac) = match time.split_once('.') {
        Some((h, f)) => (h, f),
        None => (time, "0"),
    };
    let mut t = hms.split(':');
    let hour: u64 = t.next()?.parse().ok()?;
    let min: u64 = t.next()?.parse().ok()?;
    let sec: u64 = t.next()?.parse().ok()?;
    let mut millis: u32 = 0;
    if !frac.is_empty() {
        let padded = format!("{frac:0<3}");
        millis = padded[..3].parse().ok()?;
    }
    // Days from civil date (Howard Hinnant).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if mo > 2 { mo - 3 } else { mo + 9 };
    let doy = (153 * mp as u64 + 2) / 5 + day as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = (era * 146097 + doe as i64) - 719468;
    let secs = days * 86400 + (hour * 3600 + min * 60 + sec) as i64;
    Some(secs * 1000 + millis as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn append_latest_and_prune() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("h.db");
        let store = HistoryStore::open(&path, 5).unwrap();
        for i in 0..20 {
            store.append("#lab", "a!b@c", &format!("msg{i}")).unwrap();
        }
        let rows = store.latest("#lab", 100).unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].text, "msg15");
        assert_eq!(rows[4].text, "msg19");
    }

    #[test]
    fn before_after_between() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("q.db");
        let store = HistoryStore::open(&path, 100).unwrap();
        let mut ids = Vec::new();
        for i in 0..10 {
            ids.push(store.append("#c", "p", &format!("{i}")).unwrap().id);
        }
        let mid = ids[4];
        let before = store.before("#c", HistBound::MsgId(mid), 3).unwrap();
        assert_eq!(before.len(), 3);
        assert_eq!(before[2].id, ids[3]);
        let after = store.after("#c", HistBound::MsgId(mid), 2).unwrap();
        assert_eq!(after.len(), 2);
        assert_eq!(after[0].id, ids[5]);
        let between = store
            .between("#c", HistBound::MsgId(ids[2]), HistBound::MsgId(ids[5]), 10)
            .unwrap();
        assert_eq!(between.len(), 4);
        assert_eq!(
            HistoryStore::parse_selector("msgid=dsc7"),
            HistBound::MsgId(7)
        );
        assert_eq!(HistoryStore::parse_selector("*"), HistBound::None);
        let ts = unix_ms_to_rfc3339(store.latest("#c", 1).unwrap()[0].ts_ms);
        assert!(matches!(
            HistoryStore::parse_selector(&format!("timestamp={ts}")),
            HistBound::TsMs(_)
        ));
    }

    #[test]
    fn global_retention_prunes_across_channels() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("g.db");
        let store = HistoryStore::open_with_retention(&path, 100, 10).unwrap();
        for i in 0..100 {
            let chan = format!("#c{}", i % 5);
            store.append(&chan, "p", &format!("{i}")).unwrap();
        }
        let mut total = 0usize;
        for i in 0..5 {
            total += store.latest(&format!("#c{i}"), 200).unwrap().len();
        }
        assert!(total <= 10, "total={total}");
    }

    #[tokio::test]
    async fn append_async_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.db");
        let store = Arc::new(HistoryStore::open(&path, 50).unwrap());
        let h = store
            .append_async("#c".into(), "n!u@h".into(), "hi".into())
            .await
            .unwrap();
        assert_eq!(h.text, "hi");
        let rows = store.latest_async("#c".into(), 10).await.unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn prune_skips_when_under_cap() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("u.db");
        let store = HistoryStore::open(&path, 100).unwrap();
        for i in 0..10 {
            store.append("#x", "p", &format!("{i}")).unwrap();
        }
        // Under cap: channel count tracked, no need to assert SQL — latest still 10.
        assert_eq!(store.latest("#x", 200).unwrap().len(), 10);
    }

    #[test]
    fn reopen_existing_db_runs_private_file_check() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("reopen.db");
        {
            let store = HistoryStore::open(&path, 10).unwrap();
            store.append("#c", "p", "a").unwrap();
        }
        let store = HistoryStore::open_with_retention(&path, 10, 50).unwrap();
        assert_eq!(store.latest("#c", 10).unwrap().len(), 1);
        assert_eq!(store.channels_with_history(10).unwrap().len(), 1);
    }

    #[test]
    fn around_and_targets() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("ar.db");
        let store = HistoryStore::open(&path, 100).unwrap();
        let mut ids = Vec::new();
        for i in 0..10 {
            ids.push(store.append("#c", "p", &format!("{i}")).unwrap().id);
        }
        let around = store.around("#c", HistBound::MsgId(ids[5]), 4).unwrap();
        assert!(!around.is_empty());
        assert!(around.len() <= 4);
        let targets = store.channels_with_history(10).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].0, "#c");
        let after_ts = store.after("#c", HistBound::TsMs(0), 3).unwrap();
        assert_eq!(after_ts.len(), 3);
        let before_none = store.before("#c", HistBound::None, 2).unwrap();
        assert_eq!(before_none.len(), 2);
    }

    #[test]
    fn selectors_ts_between_and_lenient_msgid() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sel.db");
        let store = HistoryStore::open(&path, 100).unwrap();
        for i in 0..6 {
            store.append("#c", "p", &format!("{i}")).unwrap();
        }
        let rows = store.latest("#c", 6).unwrap();
        let ts0 = rows[0].ts_ms;
        let ts3 = rows[3].ts_ms;
        assert_eq!(HistoryStore::parse_selector("dsc9"), HistBound::MsgId(9));
        assert_eq!(HistoryStore::parse_selector("nope"), HistBound::None);
        assert!(matches!(
            HistoryStore::parse_selector(&format!("time={}", unix_ms_to_rfc3339(ts0))),
            HistBound::TsMs(_)
        ));
        assert!(matches!(
            HistoryStore::parse_selector("timestamp=bogus"),
            HistBound::TsMs(-1)
        ));
        let before_ts = store.before("#c", HistBound::TsMs(ts3 + 1), 10).unwrap();
        assert!(before_ts.len() >= 3);
        let around_ts = store.around("#c", HistBound::TsMs(ts3), 4).unwrap();
        assert!(!around_ts.is_empty());
        let between_ts = store
            .between("#c", HistBound::TsMs(ts0), HistBound::TsMs(ts3), 10)
            .unwrap();
        assert!(!between_ts.is_empty());
        let mixed = store
            .between("#c", HistBound::MsgId(1), HistBound::TsMs(ts3), 3)
            .unwrap();
        assert!(mixed.is_empty(), "mixed BETWEEN refs → empty");
        let after_none = store.after("#c", HistBound::None, 2).unwrap();
        assert_eq!(after_none.len(), 2);
        // nearest_ts via AROUND timestamp (index seeks, not ABS scan).
        let around_mid = store
            .around("#c", HistBound::TsMs((ts0 + ts3) / 2), 4)
            .unwrap();
        assert!(!around_mid.is_empty());
    }

    #[test]
    fn rfc3339_epoch() {
        assert_eq!(rfc3339_to_unix_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(rfc3339_to_unix_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339_to_unix_ms("short"), None);
        assert_eq!(rfc3339_to_unix_ms("1970-01-01T00:00:00"), None);
    }

    #[test]
    fn nearest_ts_empty_and_around_none_bound() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("near.db");
        let store = HistoryStore::open(&path, 50).unwrap();
        assert!(store
            .around("#missing", HistBound::TsMs(1), 4)
            .unwrap()
            .is_empty());
        store.append("#c", "p", "only").unwrap();
        let ts = store.latest("#c", 1).unwrap()[0].ts_ms;
        let hit = store.around("#c", HistBound::TsMs(ts + 10_000), 2).unwrap();
        assert_eq!(hit.len(), 1);
        assert!(store
            .between("#c", HistBound::None, HistBound::MsgId(1), 5)
            .unwrap()
            .is_empty());
        // Pivot arm `_ => None` (MsgId ≤ 0).
        let _ = store.around("#c", HistBound::MsgId(0), 2).unwrap();
    }

    #[test]
    fn nearest_ts_floor_ceil_and_closer_branch() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("fc.db");
        let store = HistoryStore::open(&path, 50).unwrap();
        let a = store.append("#c", "p", "a").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = store.append("#c", "p", "b").unwrap();
        assert!(b.ts_ms >= a.ts_ms);
        let toward_b = a.ts_ms + (b.ts_ms - a.ts_ms).max(1) * 3 / 4;
        let toward_a = a.ts_ms + (b.ts_ms - a.ts_ms).max(1) / 4;
        assert!(!store
            .around("#c", HistBound::TsMs(toward_b), 3)
            .unwrap()
            .is_empty());
        assert!(!store
            .around("#c", HistBound::TsMs(toward_a), 3)
            .unwrap()
            .is_empty());
        assert!(!store
            .around("#c", HistBound::TsMs(b.ts_ms + 1_000_000), 2)
            .unwrap()
            .is_empty());
        assert!(!store
            .around("#c", HistBound::TsMs(a.ts_ms - 1_000_000), 2)
            .unwrap()
            .is_empty());
    }
}
