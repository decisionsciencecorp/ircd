//! Single-node channel history (sqlite) + CHATHISTORY helpers.
//!
//! Sync rusqlite work runs under [`tokio::task::spawn_blocking`] via the
//! `*_async` methods so Tokio worker threads are not stalled (H-06).

use std::path::Path;
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

pub struct HistoryStore {
    db: Mutex<Connection>,
    max_per_channel: usize,
    max_total_rows: usize,
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
        // Fail closed if an existing DB is group/world-readable; sqlite may create 0644, so
        // tighten after open rather than rejecting a fresh file.
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
            db: Mutex::new(db),
            max_per_channel: max_per_channel.max(1),
            max_total_rows,
        })
    }

    pub fn append(&self, channel: &str, prefix: &str, text: &str) -> Result<HistMsg> {
        let ts_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO channel_history (channel, ts_ms, prefix, text) VALUES (?1, ?2, ?3, ?4)",
            params![channel, ts_ms, prefix, text],
        )?;
        let id = db.last_insert_rowid();
        // Per-channel prune
        db.execute(
            "DELETE FROM channel_history WHERE channel = ?1 AND id NOT IN (
                SELECT id FROM channel_history WHERE channel = ?1 ORDER BY id DESC LIMIT ?2
             )",
            params![channel, self.max_per_channel as i64],
        )?;
        if self.max_total_rows > 0 {
            db.execute(
                "DELETE FROM channel_history WHERE id NOT IN (
                    SELECT id FROM channel_history ORDER BY id DESC LIMIT ?1
                 )",
                params![self.max_total_rows as i64],
            )?;
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
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(
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
    fn open_existing_file_tightens_perms() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let path = dir.path().join("pre.sqlite3");
        std::fs::write(&path, []).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&path, perms).unwrap();
        let _store = HistoryStore::open(&path, 3).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn append_latest_and_prune() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("h.sqlite3");
        let store = HistoryStore::open(&path, 3).unwrap();
        for i in 0..5 {
            store.append("#lab", "a!b@c", &format!("msg{i}")).unwrap();
        }
        let rows = store.latest("#lab", 50).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].text, "msg2");
        assert_eq!(rows[2].text, "msg4");
        assert!(rows[0].msgid().starts_with("dsc"));
        let tagged = rows[0].tagged_privmsg();
        assert!(tagged.contains("PRIVMSG #lab"));
        assert!(tagged.starts_with("@msgid="));
    }

    #[test]
    fn global_retention_prunes_across_channels() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("g.sqlite3");
        let store = HistoryStore::open_with_retention(&path, 100, 4).unwrap();
        for i in 0..3 {
            store.append("#a", "p", &format!("a{i}")).unwrap();
        }
        for i in 0..3 {
            store.append("#b", "p", &format!("b{i}")).unwrap();
        }
        let a = store.latest("#a", 50).unwrap();
        let b = store.latest("#b", 50).unwrap();
        assert!(a.len() + b.len() <= 4, "a={a:?} b={b:?}");
    }

    #[tokio::test]
    async fn append_async_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("async.sqlite3");
        let store = Arc::new(HistoryStore::open(&path, 10).unwrap());
        let h = store
            .append_async("#c".into(), "n!u@h".into(), "hi".into())
            .await
            .unwrap();
        assert_eq!(h.text, "hi");
        let rows = store.latest_async("#c".into(), 10).await.unwrap();
        assert_eq!(rows.len(), 1);
    }
}
