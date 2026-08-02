//! DSC IRCd bootstrap listener.
//!
//! Enough protocol for a client to register, join channels, and chat.
//! Feature growth tracks UnrealIRCd as the ops/reference model (clean-room).

mod config;
mod session;
mod tls;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Mutex};
use tracing::{info, warn};

use config::Config;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
struct BusMsg {
    /// Channel name including `#`, or `*` for server-wide (unused for now).
    target: String,
    line: String,
    /// Skip delivering back to this connection id.
    skip_conn: u64,
}

struct Shared {
    next_id: u64,
    /// nick -> connection id (single nick registration for v0)
    nicks: HashMap<String, u64>,
    /// channel -> member nicks
    channels: HashMap<String, HashSet<String>>,
    bus: broadcast::Sender<BusMsg>,
    config: Arc<Config>,
}

impl Shared {
    fn new(config: Arc<Config>) -> Self {
        let (bus, _) = broadcast::channel(256);
        Self {
            next_id: 1,
            nicks: HashMap::new(),
            channels: HashMap::new(),
            bus,
            config,
        }
    }
}

struct CliOverlay {
    config_path: Option<PathBuf>,
    bind: Option<String>,
    tls_bind: Option<String>,
    tls_cert: Option<PathBuf>,
    tls_key: Option<PathBuf>,
}

fn print_help() {
    println!(
        "\
dsc-ircd {VERSION}

Usage:
  ircd [--config FILE] [--bind HOST:PORT] [--tls-bind HOST:PORT --tls-cert FILE --tls-key FILE]
  ircd gen-cert [--out DIR] [--cn NAME]

CLI listen flags override/extend config file listens.
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
            cert: None,
            key: None,
        });
    }
    if let Some(bind) = &cli.tls_bind {
        listens.push(config::ListenSection {
            bind: bind.clone(),
            tls: true,
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

    let shared = Arc::new(Mutex::new(Shared::new(Arc::clone(&cfg))));
    let mut joins = tokio::task::JoinSet::new();

    info!(
        "dsc-ircd {VERSION} starting name={} listens={}",
        cfg.server.name,
        cfg.listen.len()
    );

    for listen in &cfg.listen {
        let addr = listen.bind.clone();
        let shared = Arc::clone(&shared);
        if listen.tls {
            let cert = listen.cert.clone().context("tls cert")?;
            let key = listen.key.clone().context("tls key")?;
            let acceptor = tls::load_acceptor(&cert, &key)?;
            let listener = TcpListener::bind(&addr)
                .await
                .with_context(|| format!("bind tls {addr}"))?;
            info!("TLS on {addr} (cert {})", cert.display());
            joins.spawn(async move { accept_tls(listener, acceptor, shared).await });
        } else {
            let listener = TcpListener::bind(&addr)
                .await
                .with_context(|| format!("bind plaintext {addr}"))?;
            info!("plaintext on {addr}");
            joins.spawn(async move { accept_plaintext(listener, shared).await });
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
