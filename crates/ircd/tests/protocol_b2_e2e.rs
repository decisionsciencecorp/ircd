//! B2 — NAMES snapshot off lock + 353 wire split (H-16).
mod common;

use std::collections::HashMap;
use std::sync::Arc;

use common::{base_cfg, read_until, with_client};
use ircd::state::{ChannelState, ClientId, Shared};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

#[tokio::test]
async fn join_emits_366_after_names() {
    let cfg = Arc::new(base_cfg(None));
    let shared = Arc::new(Mutex::new(Shared::new(Arc::clone(&cfg), None)));
    with_client(shared, 1, |mut w, mut r| async move {
        let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
        w.write_all(b"NICK n\r\nUSER n 0 * :N\r\nJOIN #names\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 366 "))).await;
        assert!(lines.iter().any(|l| l.contains(" 353 ")));
        assert!(lines.iter().any(|l| l.contains(" 366 ")));
    })
    .await;
}

#[test]
fn many_members_split_into_multiple_353_payloads() {
    let mut ch = ChannelState::default();
    let mut map: HashMap<ClientId, String> = HashMap::new();
    for i in 0..40u64 {
        ch.members.insert(i);
        map.insert(i, format!("user{i:02}"));
    }
    let snap = ch.names_snapshot(&map);
    let parts = snap.split_for_wire(32);
    assert!(
        parts.len() > 1,
        "expected multiple chunks, got {}",
        parts.len()
    );
    for p in &parts {
        assert!(p.len() <= 32, "chunk {p:?} len {}", p.len());
    }
    assert_eq!(parts.join(" ").split_whitespace().count(), 40);
}
