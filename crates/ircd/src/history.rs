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
             CREATE INDEX IF NOT EXISTS idx_hist_chan_id ON channel_history(channel, id);",
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
    }
}
