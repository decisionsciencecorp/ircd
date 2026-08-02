//! dsc-ircd binary — CLI + listener spawn.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use ircd::config::{self, Config};
use ircd::history::HistoryStore;
use ircd::session;
use ircd::state::Shared;
use ircd::tls;
use ircd::ws;
use ircd::VERSION;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tracing::{info, warn};

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
