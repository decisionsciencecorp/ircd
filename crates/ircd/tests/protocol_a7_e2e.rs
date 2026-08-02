//! A7 — channel / membership quotas (H-11).

mod common;

use common::{read_until, with_client, with_two_clients};
use ircd::config::Config;
use ircd::state::Shared;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

#[tokio::test(flavor = "multi_thread")]
async fn max_channels_per_client_returns_405() {
    let mut cfg = Config::default();
    cfg.limits.max_channels_per_client = 1;
    cfg.security.registration_timeout_secs = 0;
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 41, |mut w, mut r| async move {
        w.write_all(b"NICK q\r\nUSER q 0 * :Q\r\nJOIN #a\r\nJOIN #b\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("405"))).await;
        assert!(
            lines.iter().any(|l| l.contains("405") && l.contains("#b")),
            "second JOIN must 405: {lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn max_channels_server_wide_returns_405() {
    let mut cfg = Config::default();
    cfg.limits.max_channels = 1;
    cfg.security.registration_timeout_secs = 0;
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK a\r\nUSER a 0 * :A\r\nJOIN #only\r\n").await.unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;
        w2.write_all(b"NICK b\r\nUSER b 0 * :B\r\nJOIN #other\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r2, |l| l.iter().any(|x| x.contains("405"))).await;
        assert!(
            lines.iter().any(|l| l.contains("405")),
            "new channel beyond server cap must 405: {lines:?}"
        );
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn max_members_returns_471() {
    let mut cfg = Config::default();
    cfg.limits.max_members_per_channel = 1;
    cfg.security.registration_timeout_secs = 0;
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK a\r\nUSER a 0 * :A\r\nJOIN #full\r\n").await.unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;
        w2.write_all(b"NICK b\r\nUSER b 0 * :B\r\nJOIN #full\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r2, |l| l.iter().any(|x| x.contains("471"))).await;
        assert!(
            lines.iter().any(|l| l.contains("471")),
            "member cap must 471: {lines:?}"
        );
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn topic_too_long_rejected() {
    let mut cfg = Config::default();
    cfg.limits.max_topic_bytes = 8;
    cfg.security.registration_timeout_secs = 0;
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 42, |mut w, mut r| async move {
        w.write_all(b"NICK t\r\nUSER t 0 * :T\r\nJOIN #t\r\nTOPIC #t :123456789\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("461"))).await;
        assert!(
            lines.iter().any(|l| l.contains("461") && l.contains("TOPIC")),
            "{lines:?}"
        );
    })
    .await;
}
