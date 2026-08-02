//! DSC IRCd bootstrap listener.
//!
//! Enough protocol for a client to register, join `#test`, and chat.
//! Feature growth tracks UnrealIRCd as the ops/reference model (clean-room).

mod session;
mod tls;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Mutex};
use tracing::{info, warn};

const SERVER_NAME: &str = "ircd.dsc.local";
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
}

impl Shared {
    fn new() -> Self {
        let (bus, _) = broadcast::channel(256);
        Self {
            next_id: 1,
            nicks: HashMap::new(),
            channels: HashMap::new(),
            bus,
        }
    }
}

struct RunArgs {
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
  ircd [--bind HOST:PORT] [--tls-bind HOST:PORT --tls-cert FILE --tls-key FILE]
  ircd gen-cert [--out DIR] [--cn NAME]

Defaults: plaintext 127.0.0.1:6667 when no bind flags are given.
TLS lab: generate certs, then pass --tls-bind 127.0.0.1:6697 with cert/key."
    );
}

fn run_gen_cert(argv: &[String]) -> Result<()> {
    let mut out = PathBuf::from("./certs");
    let mut cn = SERVER_NAME.to_string();
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
    println!(
        "example: cargo run -p ircd -- --bind 127.0.0.1:6667 --tls-bind 127.0.0.1:6697 --tls-cert {} --tls-key {}",
        cert.display(),
        key.display()
    );
    Ok(())
}

fn parse_run_args(argv: &[String]) -> Result<RunArgs> {
    let mut bind = None;
    let mut tls_bind = None;
    let mut tls_cert = None;
    let mut tls_key = None;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--bind" => {
                i += 1;
                bind = Some(argv.get(i).context("--bind needs value")?.clone());
            }
            "--tls-bind" => {
                i += 1;
                tls_bind = Some(argv.get(i).context("--tls-bind needs value")?.clone());
            }
            "--tls-cert" => {
                i += 1;
                tls_cert = Some(PathBuf::from(argv.get(i).context("--tls-cert needs value")?));
            }
            "--tls-key" => {
                i += 1;
                tls_key = Some(PathBuf::from(argv.get(i).context("--tls-key needs value")?));
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => bail!("unknown arg {other}"),
        }
        i += 1;
    }
    Ok(RunArgs {
        bind,
        tls_bind,
        tls_cert,
        tls_key,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    // rustls 0.23: pick an explicit provider when both ring and aws-lc-rs are in the graph.
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

    let args = parse_run_args(&argv)?;
    let tls_bind = args.tls_bind.clone();
    let bind = match (&args.bind, &tls_bind) {
        (Some(b), _) => Some(b.clone()),
        (None, None) => Some("127.0.0.1:6667".to_string()),
        (None, Some(_)) => None, // TLS-only ok
    };

    if bind.is_none() && tls_bind.is_none() {
        bail!("nothing to listen on; pass --bind and/or --tls-bind");
    }

    let shared = Arc::new(Mutex::new(Shared::new()));
    let mut joins = tokio::task::JoinSet::new();

    if let Some(addr) = bind {
        let listener = TcpListener::bind(&addr)
            .await
            .with_context(|| format!("bind plaintext {addr}"))?;
        info!("dsc-ircd {VERSION} plaintext on {addr} (server name {SERVER_NAME})");
        let shared = Arc::clone(&shared);
        joins.spawn(async move { accept_plaintext(listener, shared).await });
    }

    if let Some(addr) = tls_bind {
        let cert = args
            .tls_cert
            .clone()
            .context("--tls-bind requires --tls-cert")?;
        let key = args
            .tls_key
            .clone()
            .context("--tls-bind requires --tls-key")?;
        let acceptor = tls::load_acceptor(&cert, &key)?;
        let listener = TcpListener::bind(&addr)
            .await
            .with_context(|| format!("bind tls {addr}"))?;
        info!(
            "dsc-ircd {VERSION} TLS on {addr} (cert {} key {})",
            cert.display(),
            key.display()
        );
        let shared = Arc::clone(&shared);
        joins.spawn(async move { accept_tls(listener, acceptor, shared).await });
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
