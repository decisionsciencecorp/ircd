//! TOML config — Rust-native shape, Unreal-familiar knobs (see docs/REFERENCE.md).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub server: ServerSection,
    #[serde(default)]
    pub listen: Vec<ListenSection>,
    #[serde(default)]
    pub oper: OperSection,
    #[serde(default)]
    pub history: HistorySection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerSection {
    /// Me::name equivalent
    #[serde(default = "default_server_name")]
    pub name: String,
    #[serde(default = "default_motd")]
    pub motd: String,
    #[serde(default)]
    pub admin_name: String,
    #[serde(default)]
    pub admin_email: String,
    #[serde(default = "default_nick_len")]
    pub max_nick_length: usize,
    #[serde(default = "default_chan_len")]
    pub max_channel_length: usize,
}

fn default_server_name() -> String {
    "ircd.dsc.local".into()
}
fn default_motd() -> String {
    "Welcome to dsc-ircd — Unreal-inspired clean-room Rust IRCd".into()
}
fn default_nick_len() -> usize {
    30
}
fn default_chan_len() -> usize {
    50
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListenSection {
    pub bind: String,
    #[serde(default)]
    pub tls: bool,
    /// IRC-over-WebSocket (text frames) instead of raw TCP IRC.
    #[serde(default)]
    pub websocket: bool,
    pub cert: Option<PathBuf>,
    pub key: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HistorySection {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_history_path")]
    pub path: PathBuf,
    #[serde(default = "default_hist_max")]
    pub max_per_channel: usize,
    #[serde(default = "default_hist_replay")]
    pub auto_replay_on_join: usize,
}

fn default_true() -> bool {
    true
}
fn default_history_path() -> PathBuf {
    PathBuf::from("./data/history.sqlite3")
}
fn default_hist_max() -> usize {
    1000
}
fn default_hist_replay() -> usize {
    50
}

impl Default for HistorySection {
    fn default() -> Self {
        Self {
            enabled: true,
            path: default_history_path(),
            max_per_channel: default_hist_max(),
            auto_replay_on_join: default_hist_replay(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct OperSection {
    /// When false, OPER always fails (no O-lines).
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub name: String,
    /// Lab plaintext match only — replace with hash before any public deploy.
    #[serde(default)]
    pub password: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerSection {
                name: default_server_name(),
                motd: default_motd(),
                admin_name: String::new(),
                admin_email: String::new(),
                max_nick_length: default_nick_len(),
                max_channel_length: default_chan_len(),
            },
            listen: vec![ListenSection {
                bind: "127.0.0.1:6667".into(),
                tls: false,
                websocket: false,
                cert: None,
                key: None,
            }],
            oper: OperSection::default(),
            history: HistorySection::default(),
        }
    }
}

impl Config {
    pub fn load_file(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        let cfg: Config = toml::from_str(&text)
            .with_context(|| format!("parse config {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<()> {
        if self.server.name.is_empty() {
            bail!("server.name must not be empty");
        }
        if self.server.max_nick_length < 1 || self.server.max_nick_length > 50 {
            bail!("server.max_nick_length out of range");
        }
        for (i, l) in self.listen.iter().enumerate() {
            if l.bind.is_empty() {
                bail!("listen[{i}].bind empty");
            }
            if l.tls && (l.cert.is_none() || l.key.is_none()) {
                bail!("listen[{i}] tls=true requires cert and key");
            }
        }
        Ok(())
    }
}
