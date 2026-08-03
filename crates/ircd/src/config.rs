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
    /// Built-in SASL PLAIN accounts (no external services in v0).
    #[serde(default)]
    pub accounts: Vec<AccountSection>,
    #[serde(default)]
    pub limits: LimitsSection,
    #[serde(default)]
    pub security: SecuritySection,
    #[serde(default)]
    pub websocket: WebSocketSection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AccountSection {
    pub name: String,
    pub password: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LimitsSection {
    #[serde(default = "default_max_clients")]
    pub max_clients: usize,
    #[serde(default = "default_max_per_ip")]
    pub max_clients_per_ip: usize,
    #[serde(default = "default_flood_lines")]
    pub flood_lines_per_window: u32,
    #[serde(default = "default_flood_window")]
    pub flood_window_secs: u64,
    /// Max IRC line octets including CR/LF (disconnect if exceeded). Default 8192 (IRCv3-tagged budget).
    #[serde(default = "default_max_line_bytes")]
    pub max_line_bytes: usize,
    /// Max channels that may exist server-wide.
    #[serde(default = "default_max_channels")]
    pub max_channels: usize,
    /// Max channels a single client may join.
    #[serde(default = "default_max_channels_per_client")]
    pub max_channels_per_client: usize,
    /// Max members in one channel.
    #[serde(default = "default_max_members")]
    pub max_members_per_channel: usize,
    /// Max topic octets (excluding framing).
    #[serde(default = "default_max_topic")]
    pub max_topic_bytes: usize,
}

fn default_max_clients() -> usize {
    256
}
fn default_max_per_ip() -> usize {
    32
}
fn default_flood_lines() -> u32 {
    30
}
fn default_flood_window() -> u64 {
    10
}
fn default_max_line_bytes() -> usize {
    8192
}
fn default_max_channels() -> usize {
    1024
}
fn default_max_channels_per_client() -> usize {
    64
}
fn default_max_members() -> usize {
    512
}
fn default_max_topic() -> usize {
    390
}

impl Default for LimitsSection {
    fn default() -> Self {
        Self {
            max_clients: default_max_clients(),
            max_clients_per_ip: default_max_per_ip(),
            flood_lines_per_window: default_flood_lines(),
            flood_window_secs: default_flood_window(),
            max_line_bytes: default_max_line_bytes(),
            max_channels: default_max_channels(),
            max_channels_per_client: default_max_channels_per_client(),
            max_members_per_channel: default_max_members(),
            max_topic_bytes: default_max_topic(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebSocketSection {
    /// Exact Origin values allowed for browser clients. Empty = deny all Origins.
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    /// Allow handshakes with no Origin header (native clients).
    #[serde(default = "default_true_ws")]
    pub allow_missing_origin: bool,
    /// Require `Sec-WebSocket-Protocol: irc`.
    #[serde(default = "default_true_ws")]
    pub require_irc_subprotocol: bool,
}

fn default_true_ws() -> bool {
    true
}

impl Default for WebSocketSection {
    fn default() -> Self {
        Self {
            allowed_origins: Vec::new(),
            allow_missing_origin: true,
            require_irc_subprotocol: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SecuritySection {
    /// Production posture: forbid plaintext listens; force TLS for SASL/OPER.
    #[serde(default)]
    pub production: bool,
    /// Refuse AUTHENTICATE/OPER on non-TLS connections (implied by `production`).
    #[serde(default)]
    pub require_tls_for_auth: bool,
    /// Disconnect if registration (NICK+USER) not completed in time. 0 = disabled.
    #[serde(default = "default_registration_timeout")]
    pub registration_timeout_secs: u64,
    /// Disconnect idle registered clients. 0 = disabled (lab default).
    #[serde(default)]
    pub idle_timeout_secs: u64,
    /// TLS/WS handshake deadline before the session is admitted to protocol.
    #[serde(default = "default_handshake_timeout")]
    pub handshake_timeout_secs: u64,
    /// Cap concurrent TLS/WS handshakes (and optionally plaintext admits).
    #[serde(default = "default_max_handshake")]
    pub max_handshake_inflight: usize,
}

fn default_registration_timeout() -> u64 {
    60
}
fn default_handshake_timeout() -> u64 {
    15
}
fn default_max_handshake() -> usize {
    64
}

impl Default for SecuritySection {
    fn default() -> Self {
        Self {
            production: false,
            require_tls_for_auth: false,
            registration_timeout_secs: default_registration_timeout(),
            idle_timeout_secs: 0,
            handshake_timeout_secs: default_handshake_timeout(),
            max_handshake_inflight: default_max_handshake(),
        }
    }
}

impl SecuritySection {
    /// Whether SASL/OPER must ride a TLS (or WSS) connection.
    pub fn tls_required_for_auth(&self) -> bool {
        self.production || self.require_tls_for_auth
    }
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
    /// Optional connection password (`PASS`). Empty = not required.
    #[serde(default)]
    pub password: String,
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
    /// Global retention across all channels (0 = disabled).
    #[serde(default)]
    pub max_total_rows: usize,
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

/// JOIN auto-replay is clamped to CHATHISTORY's per-query max (200).
pub fn clamp_auto_replay_on_join(n: usize) -> usize {
    n.min(200)
}

impl Default for HistorySection {
    fn default() -> Self {
        Self {
            enabled: true,
            path: default_history_path(),
            max_per_channel: default_hist_max(),
            max_total_rows: 0,
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
                password: String::new(),
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
            accounts: Vec::new(),
            limits: LimitsSection::default(),
            security: SecuritySection::default(),
            websocket: WebSocketSection::default(),
        }
    }
}

impl Config {
    pub fn load_file(path: &Path) -> Result<Self> {
        let text =
            fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?;
        let cfg: Config =
            toml::from_str(&text).with_context(|| format!("parse config {}", path.display()))?;
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
        if self.security.production {
            if self.listen.is_empty() {
                bail!("security.production requires at least one TLS listen");
            }
            if self.listen.iter().any(|l| !l.tls) {
                bail!("security.production forbids plaintext listen blocks");
            }
        }
        if self.security.max_handshake_inflight == 0 {
            bail!("security.max_handshake_inflight must be >= 1");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn default_valid() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn quotas_defaults_sane() {
        let c = Config::default();
        assert!(c.limits.max_channels >= 1);
        assert!(c.limits.max_channels_per_client >= 1);
        assert!(c.limits.max_members_per_channel >= 1);
        assert!(c.limits.max_topic_bytes >= 1);
        assert_eq!(clamp_auto_replay_on_join(500), 200);
        assert_eq!(clamp_auto_replay_on_join(50), 50);
    }

    #[test]
    fn load_and_reject_bad_tls() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            r#"
[server]
name = "t"

[[listen]]
bind = "127.0.0.1:1"
tls = true
"#
        )
        .unwrap();
        assert!(Config::load_file(f.path()).is_err());
    }

    #[test]
    fn empty_name_fails() {
        let mut c = Config::default();
        c.server.name.clear();
        assert!(c.validate().is_err());
    }

    #[test]
    fn nick_len_and_empty_bind() {
        let mut c = Config::default();
        c.server.max_nick_length = 0;
        assert!(c.validate().is_err());
        c.server.max_nick_length = 30;
        c.listen[0].bind.clear();
        assert!(c.validate().is_err());
    }

    #[test]
    fn load_ok_toml() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            r#"
[server]
name = "ok.test"
motd = "hi"

[[listen]]
bind = "127.0.0.1:9"
tls = false

[history]
enabled = false
"#
        )
        .unwrap();
        let c = Config::load_file(f.path()).unwrap();
        assert_eq!(c.server.name, "ok.test");
    }

    #[test]
    fn production_forbids_plaintext() {
        let mut c = Config::default();
        c.security.production = true;
        c.listen = vec![ListenSection {
            bind: "127.0.0.1:6667".into(),
            tls: false,
            websocket: false,
            cert: None,
            key: None,
        }];
        assert!(c.validate().is_err());
    }

    #[test]
    fn production_ok_with_tls_listen() {
        let mut c = Config::default();
        c.security.production = true;
        c.listen = vec![ListenSection {
            bind: "127.0.0.1:6697".into(),
            tls: true,
            websocket: false,
            cert: Some(PathBuf::from("c.pem")),
            key: Some(PathBuf::from("k.pem")),
        }];
        c.validate().unwrap();
        assert!(c.security.tls_required_for_auth());
    }

    #[test]
    fn production_empty_listen_fails() {
        let mut c = Config::default();
        c.security.production = true;
        c.listen.clear();
        assert!(c.validate().is_err());
    }

    #[test]
    fn max_handshake_zero_fails() {
        let mut c = Config::default();
        c.security.max_handshake_inflight = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn tls_required_for_auth_flag() {
        let mut s = SecuritySection::default();
        assert!(!s.tls_required_for_auth());
        s.require_tls_for_auth = true;
        assert!(s.tls_required_for_auth());
        s.require_tls_for_auth = false;
        s.production = true;
        assert!(s.tls_required_for_auth());
    }
}
