//! IRC-over-WebSocket bridge (text frames ↔ CRLF IRC lines).

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;
use tracing::warn;

use crate::admission;
use crate::session;
use crate::state::Shared;
use crate::ws_lines::normalize_ws_irc_lines;
use crate::ws_policy::{origin_allowed, subprotocol_acceptable};

/// Run IRC session over an already-accepted WebSocket (plain or after TLS).
pub async fn handle_websocket<S>(
    stream: S,
    peer: SocketAddr,
    shared: Arc<Mutex<Shared>>,
    secure: bool,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let mut ws_cfg = WebSocketConfig::default();
    // Bound hostile browser frames (H-02). Classic IRC line budget is smaller; allow tagged headroom.
    ws_cfg.max_message_size = Some(64 * 1024);
    ws_cfg.max_frame_size = Some(64 * 1024);
    let ws = tokio_tungstenite::accept_async_with_config(stream, Some(ws_cfg)).await?;
    let (mut sink, mut ws_stream) = ws.split();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (duplex_r, mut duplex_w) = tokio::io::split(client);

    // WebSocket → duplex (normalize to CRLF lines)
    let inbound = tokio::spawn(async move {
        while let Some(item) = ws_stream.next().await {
            match item {
                Ok(Message::Text(text)) => {
                    if text.len() > 64 * 1024 {
                        return;
                    }
                    for line in normalize_ws_irc_lines(&text) {
                        if line.len() > 8192 {
                            return;
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
                        if text.len() > 64 * 1024 {
                            return;
                        }
                        for line in normalize_ws_irc_lines(text) {
                            if line.len() > 8192 {
                                return;
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
    let result = session::handle_client(reader, writer, peer, shared, secure, true).await;
    inbound.abort();
    outbound.abort();
    result
}

pub async fn accept_plain_ws(
    listener: tokio::net::TcpListener,
    shared: Arc<Mutex<Shared>>,
) -> Result<()> {
    loop {
        let (socket, peer) = listener.accept().await?;
        let shared = Arc::clone(&shared);
        // Admit before WS upgrade work (H-09/H-12).
        if let Err(reason) = admission::admit_early(&shared, peer).await {
            warn!(%peer, %reason, "ws pre-admit rejected");
            continue;
        }
        tokio::spawn(async move {
            let result = handle_websocket(socket, peer, Arc::clone(&shared), false).await;
            if let Err(e) = result {
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
        if let Err(reason) = admission::admit_early(&shared, peer).await {
            warn!(%peer, %reason, "wss pre-admit rejected");
            continue;
        }
        tokio::spawn(async move {
            match acceptor.accept(socket).await {
                Ok(tls) => {
                    if let Err(e) = handle_websocket(tls, peer, Arc::clone(&shared), true).await {
                        warn!(%peer, error = %e, "wss client session ended");
                    }
                }
                Err(e) => {
                    shared.lock().await.release(peer);
                    warn!(%peer, error = %e, "wss handshake failed");
                }
            }
        });
    }
}

/// Evaluate Origin + subprotocol policy from config (unit-testable entry for acceptors).
pub fn evaluate_ws_handshake(
    origin: Option<&str>,
    offered_protocols: &[String],
    cfg: &crate::config::WebSocketSection,
) -> Result<(), &'static str> {
    if !origin_allowed(origin, &cfg.allowed_origins, cfg.allow_missing_origin) {
        return Err("origin not allowed");
    }
    if !subprotocol_acceptable(offered_protocols, cfg.require_irc_subprotocol) {
        return Err("irc subprotocol required");
    }
    Ok(())
}

#[cfg(test)]
mod eval_tests {
    use super::*;
    use crate::config::WebSocketSection;

    #[test]
    fn evaluate_ws_handshake_respects_policy() {
        let cfg = WebSocketSection {
            allowed_origins: vec!["https://ok.test".into()],
            allow_missing_origin: true,
            require_irc_subprotocol: true,
        };
        assert!(evaluate_ws_handshake(None, &["irc".into()], &cfg).is_ok());
        assert!(evaluate_ws_handshake(Some("https://evil"), &["irc".into()], &cfg).is_err());
        assert!(evaluate_ws_handshake(None, &["chat".into()], &cfg).is_err());
    }
}
