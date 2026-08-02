//! IRC-over-WebSocket bridge (text frames ↔ CRLF IRC lines).

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tracing::warn;

use crate::session;
use crate::state::Shared;

/// Run IRC session over an already-accepted WebSocket (plain or after TLS).
pub async fn handle_websocket<S>(stream: S, peer: SocketAddr, shared: Arc<Mutex<Shared>>) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (mut sink, mut ws_stream) = ws.split();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (duplex_r, mut duplex_w) = tokio::io::split(client);

    // WebSocket → duplex (normalize to CRLF lines)
    let inbound = tokio::spawn(async move {
        while let Some(item) = ws_stream.next().await {
            match item {
                Ok(Message::Text(text)) => {
                    for part in text.split('\n') {
                        let line = part.trim_end_matches('\r');
                        if line.is_empty() {
                            continue;
                        }
                        let mut out = line.to_string();
                        out.push_str("\r\n");
                        if duplex_w.write_all(out.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                }
                Ok(Message::Binary(bin)) => {
                    if let Ok(text) = std::str::from_utf8(&bin) {
                        for part in text.split('\n') {
                            let line = part.trim_end_matches('\r');
                            if line.is_empty() {
                                continue;
                            }
                            let mut out = line.to_string();
                            out.push_str("\r\n");
                            if duplex_w.write_all(out.as_bytes()).await.is_err() {
                                return;
                            }
                        }
                    }
                }
                Ok(Message::Close(_)) | Err(_) => return,
                // Ping/Pong: tungstenite may surface these; IRC clients usually idle on IRC PING.
                _ => {}
            }
        }
    });

    // duplex → WebSocket (one IRC line per text frame)
    let outbound = tokio::spawn(async move {
        let mut reader = BufReader::new(duplex_r);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let text = line.trim_end_matches(['\r', '\n']).to_string();
                    if sink.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let (reader, writer) = tokio::io::split(server);
    let result = session::handle_client(reader, writer, peer, shared).await;
    inbound.abort();
    outbound.abort();
    result
}

pub async fn accept_plain_ws(listener: tokio::net::TcpListener, shared: Arc<Mutex<Shared>>) -> Result<()> {
    loop {
        let (socket, peer) = listener.accept().await?;
        let shared = Arc::clone(&shared);
        tokio::spawn(async move {
            if let Err(e) = handle_websocket(socket, peer, shared).await {
                warn!(%peer, error = %e, "websocket client session ended");
            }
        });
    }
}

pub async fn accept_tls_ws(
    listener: tokio::net::TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    shared: Arc<Mutex<Shared>>,
) -> Result<()> {
    loop {
        let (socket, peer) = listener.accept().await?;
        let shared = Arc::clone(&shared);
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            match acceptor.accept(socket).await {
                Ok(tls) => {
                    if let Err(e) = handle_websocket(tls, peer, shared).await {
                        warn!(%peer, error = %e, "wss client session ended");
                    }
                }
                Err(e) => warn!(%peer, error = %e, "wss handshake failed"),
            }
        });
    }
}
