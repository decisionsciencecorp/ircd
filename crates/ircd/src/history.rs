//! Single-node channel history (sqlite) + CHATHISTORY helpers.

use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
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

fn unix_ms_to_rfc3339(ts_ms: i64) -> String {
    let secs = (ts_ms / 1000) as u64;
    let millis = (ts_ms % 1000) as u32;
    let days = secs / 86400;
    let tod = secs % 86400;
    let hour = tod / 3600;
    let min = (tod % 3600) / 60;
    let sec = tod % 60;
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
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}.{millis:03}Z")
}

pub struct HistoryStore {
    db: Mutex<Connection>,
    max_per_channel: usize,
}

impl HistoryStore {
    pub fn open(path: &Path, max_per_channel: usize) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("mkdir {}", parent.display()))?;
        }
        let db = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS channel_history (
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
        // prune
        db.execute(
            "DELETE FROM channel_history WHERE channel = ?1 AND id NOT IN (
                SELECT id FROM channel_history WHERE channel = ?1 ORDER BY id DESC LIMIT ?2
             )",
            params![channel, self.max_per_channel as i64],
        )?;
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
}
