//! DSC IRCd bootstrap listener.
//!
//! Enough protocol for a client to register, join channels, and chat.
//! Feature growth tracks UnrealIRCd as the ops/reference model (clean-room).

mod config;
mod history;
mod session;
mod tls;
mod ws;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Mutex};
use tracing::{info, warn};

use config::Config;
use history::HistoryStore;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
struct BusMsg {
    /// Channel name including `#`, or `*` for server-wide (unused for now).
    target: String,
    line: String,
    /// Skip delivering back to this connection id.
    skip_conn: u64,
}

/// Per-channel membership and simple modes (+n/+t) plus channel ops.
#[derive(Debug, Clone)]
pub(crate) struct ChannelState {
    pub members: HashSet<String>,
    pub ops: HashSet<String>,
    pub topic: Option<String>,
    /// +n — no messages from outside
    pub mode_n: bool,
    /// +t — only ops may set topic
    pub mode_t: bool,
}

impl Default for ChannelState {
    fn default() -> Self {
        Self {
            members: HashSet::new(),
            ops: HashSet::new(),
            topic: None,
            mode_n: true,
            mode_t: true,
        }
    }
}

impl ChannelState {
    pub fn names_prefixed(&self) -> String {
        let mut names: Vec<(String, String)> = self
            .members
            .iter()
            .map(|n| {
                let display = if self.ops.contains(n) {
                    format!("@{n}")
                } else {
                    n.clone()
                };
                (n.to_ascii_lowercase(), display)
            })
            .collect();
        names.sort_by(|a, b| a.0.cmp(&b.0));
        names
            .into_iter()
            .map(|(_, d)| d)
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn mode_chars(&self) -> String {
        let mut s = String::from("+");
        if self.mode_n {
            s.push('n');
        }
        if self.mode_t {
            s.push('t');
        }
        s
    }

    pub fn is_op(&self, nick: &str) -> bool {
        self.ops.contains(nick)
    }

    pub fn remove_nick(&mut self, nick: &str) {
        self.members.remove(nick);
        self.ops.remove(nick);
    }
}

pub(crate) struct Shared {
    pub next_id: u64,
    /// nick -> connection id (single nick registration for v0)
    pub nicks: HashMap<String, u64>,
    /// channel -> state
    pub channels: HashMap<String, ChannelState>,
    pub bus: broadcast::Sender<BusMsg>,
    pub config: Arc<Config>,
    pub history: Option<Arc<HistoryStore>>,
    /// peer IP → active connection count
    pub ip_counts: HashMap<String, usize>,
    pub client_count: usize,
}

impl Shared {
    fn new(config: Arc<Config>, history: Option<Arc<HistoryStore>>) -> Self {
        let (bus, _) = broadcast::channel(256);
        Self {
            next_id: 1,
            nicks: HashMap::new(),
            channels: HashMap::new(),
            bus,
            config,
            history,
            ip_counts: HashMap::new(),
            client_count: 0,
        }
    }

    /// Admit a new connection under `[limits]`, or return a rejection reason.
    pub fn try_admit(&mut self, peer: SocketAddr) -> Result<(), &'static str> {
        let ip = peer.ip().to_string();
        let lim = &self.config.limits;
        if self.client_count >= lim.max_clients {
            return Err("Too many connections");
        }
        let n = self.ip_counts.get(&ip).copied().unwrap_or(0);
        if n >= lim.max_clients_per_ip {
            return Err("Too many connections from your host");
        }
        self.client_count += 1;
        *self.ip_counts.entry(ip).or_default() += 1;
        Ok(())
    }

    pub fn release(&mut self, peer: SocketAddr) {
        let ip = peer.ip().to_string();
        self.client_count = self.client_count.saturating_sub(1);
        if let Some(n) = self.ip_counts.get_mut(&ip) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.ip_counts.remove(&ip);
            }
        }
    }
}

struct CliOverlay {
    config_path: Option<PathBuf>,
    bind: Option<String>,
    tls_bind: Option<String>,
    ws_bind: Option<String>,
    wss_bind: Option<String>,
    tls_cert: Option<PathBuf>,
    tls_key: Option<PathBuf>,
}

fn print_help() {
    println!(
        "\
dsc-ircd {VERSION}

Usage:
  ircd [--config FILE] [--bind HOST:PORT] [--tls-bind HOST:PORT]
       [--ws-bind HOST:PORT] [--wss-bind HOST:PORT]
       [--tls-cert FILE --tls-key FILE]
  ircd gen-cert [--out DIR] [--cn NAME]

CLI listen flags override config file listens when present.
See config.example.toml and docs/REFERENCE.md."
    );
}

fn run_gen_cert(argv: &[String]) -> Result<()> {
    let mut out = PathBuf::from("./certs");
    let mut cn = "ircd.dsc.local".to_string();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--out" => {
                i += 1;
                out = PathBuf::from(argv.get(i).context("--out needs value")?);
            }
            "--cn" => {
                i += 1;
                cn = argv.get(i).context("--cn needs value")?.clone();
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => bail!("unknown gen-cert arg {other}"),
        }
        i += 1;
    }
    let (cert, key) = tls::gen_self_signed(&out, &cn)?;
    println!("wrote {}", cert.display());
    println!("wrote {}", key.display());
    Ok(())
}

fn parse_cli(argv: &[String]) -> Result<CliOverlay> {
    let mut c = CliOverlay {
        config_path: None,
        bind: None,
        tls_bind: None,
        ws_bind: None,
        wss_bind: None,
        tls_cert: None,
        tls_key: None,
    };
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--config" => {
                i += 1;
                c.config_path = Some(PathBuf::from(argv.get(i).context("--config needs value")?));
            }
            "--bind" => {
                i += 1;
                c.bind = Some(argv.get(i).context("--bind needs value")?.clone());
            }
            "--tls-bind" => {
                i += 1;
                c.tls_bind = Some(argv.get(i).context("--tls-bind needs value")?.clone());
            }
            "--ws-bind" => {
                i += 1;
                c.ws_bind = Some(argv.get(i).context("--ws-bind needs value")?.clone());
            }
            "--wss-bind" => {
                i += 1;
                c.wss_bind = Some(argv.get(i).context("--wss-bind needs value")?.clone());
            }
            "--tls-cert" => {
                i += 1;
                c.tls_cert = Some(PathBuf::from(argv.get(i).context("--tls-cert needs value")?));
            }
            "--tls-key" => {
                i += 1;
                c.tls_key = Some(PathBuf::from(argv.get(i).context("--tls-key needs value")?));
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => bail!("unknown arg {other}"),
        }
        i += 1;
    }
    Ok(c)
}

fn merge_config(mut cfg: Config, cli: &CliOverlay) -> Result<Config> {
    // If CLI specifies binds, replace listen list with CLI-derived entries.
    let mut listens = Vec::new();
    if let Some(bind) = &cli.bind {
        listens.push(config::ListenSection {
            bind: bind.clone(),
            tls: false,
            websocket: false,
            cert: None,
            key: None,
        });
    }
    if let Some(bind) = &cli.tls_bind {
        listens.push(config::ListenSection {
            bind: bind.clone(),
            tls: true,
            websocket: false,
            cert: cli.tls_cert.clone(),
            key: cli.tls_key.clone(),
        });
    }
    if let Some(bind) = &cli.ws_bind {
        listens.push(config::ListenSection {
            bind: bind.clone(),
            tls: false,
            websocket: true,
            cert: None,
            key: None,
        });
    }
    if let Some(bind) = &cli.wss_bind {
        listens.push(config::ListenSection {
            bind: bind.clone(),
            tls: true,
            websocket: true,
            cert: cli.tls_cert.clone(),
            key: cli.tls_key.clone(),
        });
    }
    if !listens.is_empty() {
        cfg.listen = listens;
    } else if cfg.listen.is_empty() {
        cfg.listen = Config::default().listen;
    }
    cfg.validate()?;
    Ok(cfg)
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ircd=info".into()),
        )
        .init();

    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(|s| s.as_str()) == Some("gen-cert") {
        return run_gen_cert(&argv[1..]);
    }

    let cli = parse_cli(&argv)?;
    let base = if let Some(path) = &cli.config_path {
        Config::load_file(path)?
    } else {
        Config::default()
    };
    let cfg = Arc::new(merge_config(base, &cli)?);

    let history = if cfg.history.enabled {
        match HistoryStore::open(&cfg.history.path, cfg.history.max_per_channel) {
            Ok(h) => {
                info!("history sqlite {}", cfg.history.path.display());
                Some(Arc::new(h))
            }
            Err(e) => {
                warn!("history disabled; open failed: {e:#}");
                None
            }
        }
    } else {
        None
    };

    let shared = Arc::new(Mutex::new(Shared::new(Arc::clone(&cfg), history)));
    let mut joins = tokio::task::JoinSet::new();

    info!(
        "dsc-ircd {VERSION} starting name={} listens={}",
        cfg.server.name,
        cfg.listen.len()
    );

    for listen in &cfg.listen {
        let addr = listen.bind.clone();
        let shared = Arc::clone(&shared);
        let listener = TcpListener::bind(&addr)
            .await
            .with_context(|| format!("bind {addr}"))?;
        match (listen.websocket, listen.tls) {
            (false, false) => {
                info!("plaintext IRC on {addr}");
                joins.spawn(async move { accept_plaintext(listener, shared).await });
            }
            (false, true) => {
                let cert = listen.cert.clone().context("tls cert")?;
                let key = listen.key.clone().context("tls key")?;
                let acceptor = tls::load_acceptor(&cert, &key)?;
                info!("TLS IRC on {addr} (cert {})", cert.display());
                joins.spawn(async move { accept_tls(listener, acceptor, shared).await });
            }
            (true, false) => {
                info!("WebSocket IRC on {addr}");
                joins.spawn(async move { ws::accept_plain_ws(listener, shared).await });
            }
            (true, true) => {
                let cert = listen.cert.clone().context("tls cert")?;
                let key = listen.key.clone().context("tls key")?;
                let acceptor = tls::load_acceptor(&cert, &key)?;
                info!("WSS IRC on {addr} (cert {})", cert.display());
                joins.spawn(async move { ws::accept_tls_ws(listener, acceptor, shared).await });
            }
        }
    }

    if joins.is_empty() {
        bail!("no listen blocks configured");
    }

    while let Some(res) = joins.join_next().await {
        res.context("listener task join")??;
    }
    Ok(())
}

async fn accept_plaintext(listener: TcpListener, shared: Arc<Mutex<Shared>>) -> Result<()> {
    loop {
        let (socket, peer) = listener.accept().await?;
        let shared = Arc::clone(&shared);
        tokio::spawn(async move {
            let (reader, writer) = socket.into_split();
            if let Err(e) = session::handle_client(reader, writer, peer, shared).await {
                warn!(%peer, error = %e, "plaintext client session ended");
            }
        });
    }
}

async fn accept_tls(
    listener: TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    shared: Arc<Mutex<Shared>>,
) -> Result<()> {
    loop {
        let (socket, peer) = listener.accept().await?;
        let shared = Arc::clone(&shared);
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            match acceptor.accept(socket).await {
                Ok(tls_stream) => {
                    let (reader, writer) = tokio::io::split(tls_stream);
                    if let Err(e) = session::handle_client(reader, writer, peer, shared).await {
                        warn!(%peer, error = %e, "tls client session ended");
                    }
                }
                Err(e) => warn!(%peer, error = %e, "tls handshake failed"),
            }
        });
    }
}
