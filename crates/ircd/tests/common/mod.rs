//! Shared duplex harness — runs `handle_client` on the same task tree (no spawn)
//! so llvm/tarpaulin attributes session coverage.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use ircd::config::{AccountSection, Config, HistorySection, LimitsSection, OperSection};
use ircd::history::HistoryStore;
use ircd::session::handle_client;
use ircd::state::Shared;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::Mutex;
use tokio::time::timeout;

pub fn peer(n: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 30000 + n)
}

pub fn base_cfg(hist: Option<std::path::PathBuf>) -> Config {
    let mut c = Config::default();
    c.server.name = "cov.test".into();
    c.server.motd = "motd".into();
    c.server.admin_name = "Admin".into();
    c.server.admin_email = "a@b.c".into();
    c.history = HistorySection {
        enabled: hist.is_some(),
        path: hist.unwrap_or_else(|| "./unused.sqlite3".into()),
        max_per_channel: 100,
        max_total_rows: 0,
        auto_replay_on_join: 20,
    };
    c.limits = LimitsSection {
        max_clients: 64,
        max_clients_per_ip: 64,
        flood_lines_per_window: 500,
        flood_window_secs: 30,
        max_line_bytes: 8192,
        max_channels: 1024,
        max_channels_per_client: 64,
        max_members_per_channel: 512,
        max_topic_bytes: 390,
    };
    c.accounts = vec![AccountSection {
        name: "alice".into(),
        password: "secret".into(),
    }];
    c.oper = OperSection {
        enabled: true,
        name: "admin".into(),
        password: "operpass".into(),
    };
    c
}

pub async fn read_until(
    reader: &mut BufReader<tokio::io::ReadHalf<DuplexStream>>,
    pred: impl Fn(&[String]) -> bool,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut buf = String::new();
    let _ = timeout(Duration::from_secs(4), async {
        loop {
            buf.clear();
            if reader.read_line(&mut buf).await.unwrap_or(0) == 0 {
                break;
            }
            let line = buf.trim_end_matches(['\r', '\n']).to_string();
            if !line.is_empty() {
                lines.push(line);
            }
            if pred(&lines) {
                break;
            }
        }
    })
    .await;
    lines
}

/// Run one client script against a fresh session (handle_client joined, not spawned).
pub async fn with_client<F, Fut>(
    shared: Arc<Mutex<Shared>>,
    port: u16,
    client: F,
) -> Fut::Output
where
    F: FnOnce(
        tokio::io::WriteHalf<DuplexStream>,
        BufReader<tokio::io::ReadHalf<DuplexStream>>,
    ) -> Fut,
    Fut: std::future::Future,
{
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    let (sr, sw) = tokio::io::split(server_end);
    let (cr, cw) = tokio::io::split(client_end);
    let reader = BufReader::new(cr);
    let server = handle_client(sr, sw, peer(port), Arc::clone(&shared), false, false);
    let client_fut = client(cw, reader);
    let (server_res, out) = tokio::join!(server, client_fut);
    let _ = server_res;
    out
}

pub async fn with_two_clients<F, Fut>(
    shared: Arc<Mutex<Shared>>,
    client: F,
) -> Fut::Output
where
    F: FnOnce(
        (
            tokio::io::WriteHalf<DuplexStream>,
            BufReader<tokio::io::ReadHalf<DuplexStream>>,
        ),
        (
            tokio::io::WriteHalf<DuplexStream>,
            BufReader<tokio::io::ReadHalf<DuplexStream>>,
        ),
    ) -> Fut,
    Fut: std::future::Future,
{
    let (c1, s1) = tokio::io::duplex(64 * 1024);
    let (c2, s2) = tokio::io::duplex(64 * 1024);
    let (s1r, s1w) = tokio::io::split(s1);
    let (s2r, s2w) = tokio::io::split(s2);
    let (c1r, c1w) = tokio::io::split(c1);
    let (c2r, c2w) = tokio::io::split(c2);
    let srv1 = handle_client(s1r, s1w, peer(1), Arc::clone(&shared), false, false);
    let srv2 = handle_client(s2r, s2w, peer(2), Arc::clone(&shared), false, false);
    let client_fut = client((c1w, BufReader::new(c1r)), (c2w, BufReader::new(c2r)));
    let (r1, r2, out) = tokio::join!(srv1, srv2, client_fut);
    let _ = (r1, r2);
    out
}

pub fn shared_with_history(path: std::path::PathBuf) -> Arc<Mutex<Shared>> {
    let cfg = Arc::new(base_cfg(Some(path.clone())));
    let store = Arc::new(HistoryStore::open(&path, 100).unwrap());
    Arc::new(Mutex::new(Shared::new(cfg, Some(store))))
}

pub fn shared_plain() -> Arc<Mutex<Shared>> {
    Arc::new(Mutex::new(Shared::new(Arc::new(base_cfg(None)), None)))
}


/// Like [`with_client`] but marks the session as TLS-secured (auth allowed under require_tls_for_auth).
pub async fn with_client_secure<F, Fut>(
    shared: Arc<Mutex<Shared>>,
    port: u16,
    client: F,
) -> Fut::Output
where
    F: FnOnce(
        tokio::io::WriteHalf<DuplexStream>,
        BufReader<tokio::io::ReadHalf<DuplexStream>>,
    ) -> Fut,
    Fut: std::future::Future,
{
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    let (sr, sw) = tokio::io::split(server_end);
    let (cr, cw) = tokio::io::split(client_end);
    let reader = BufReader::new(cr);
    let server = handle_client(sr, sw, peer(port), Arc::clone(&shared), true, false);
    let client_fut = client(cw, reader);
    let (server_res, out) = tokio::join!(server, client_fut);
    let _ = server_res;
    out
}
