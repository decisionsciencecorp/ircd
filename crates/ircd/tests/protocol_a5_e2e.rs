//! A5 — TLS-required auth + registration/idle deadlines (H-04/H-09).

mod common;

use common::{read_until, with_client, with_client_secure};
use ircd::config::{AccountSection, Config};
use ircd::state::Shared;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

#[tokio::test(flavor = "multi_thread")]
async fn plaintext_sasl_refused_when_tls_required() {
    let mut cfg = Config::default();
    cfg.security.require_tls_for_auth = true;
    cfg.accounts = vec![AccountSection {
        name: "alice".into(),
        password: "secret".into(),
    }];
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 31, |mut w, mut r| async move {
        w.write_all(
            b"CAP LS\r\nNICK n\r\nUSER u 0 * :U\r\nCAP REQ :sasl\r\nAUTHENTICATE PLAIN\r\nQUIT :x\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("904"))).await;
        assert!(
            lines.iter().any(|l| l.contains("TLS required")),
            "plaintext SASL must fail with TLS required: {lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l == "AUTHENTICATE +"),
            "must not continue SASL challenge on plaintext: {lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn secure_session_allows_sasl_challenge() {
    let mut cfg = Config::default();
    cfg.security.require_tls_for_auth = true;
    cfg.accounts = vec![AccountSection {
        name: "alice".into(),
        password: "secret".into(),
    }];
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client_secure(shared, 32, |mut w, mut r| async move {
        w.write_all(
            b"CAP LS\r\nNICK n\r\nUSER u 0 * :U\r\nCAP REQ :sasl\r\nAUTHENTICATE PLAIN\r\nQUIT :x\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x == "AUTHENTICATE +")).await;
        assert!(
            lines.iter().any(|l| l == "AUTHENTICATE +"),
            "TLS-marked session may start SASL: {lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn registration_timeout_disconnects() {
    let mut cfg = Config::default();
    cfg.security.registration_timeout_secs = 1;
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 33, |_w, mut r| async move {
        let lines = read_until(&mut r, |l| {
            l.iter().any(|x| x.contains("Registration timeout"))
        })
        .await;
        assert!(
            lines.iter().any(|l| l.contains("Registration timeout")),
            "{lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn idle_timeout_disconnects() {
    let mut cfg = Config::default();
    cfg.security.registration_timeout_secs = 0;
    cfg.security.idle_timeout_secs = 1;
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 34, |mut w, mut r| async move {
        w.write_all(b"NICK idle\r\nUSER i 0 * :I\r\n").await.unwrap();
        let _ = read_until(&mut r, |l| l.iter().any(|x| x.contains("001"))).await;
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("Idle timeout"))).await;
        assert!(
            lines.iter().any(|l| l.contains("Idle timeout")),
            "{lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn plaintext_oper_refused_when_tls_required() {
    let mut cfg = Config::default();
    cfg.security.require_tls_for_auth = true;
    cfg.security.registration_timeout_secs = 0;
    cfg.oper.enabled = true;
    cfg.oper.name = "admin".into();
    cfg.oper.password = "operpass".into();
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 35, |mut w, mut r| async move {
        w.write_all(b"NICK n\r\nUSER u 0 * :U\r\nOPER admin operpass\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("464"))).await;
        assert!(
            lines.iter().any(|l| l.contains("464") && l.contains("TLS required")),
            "{lines:?}"
        );
    })
    .await;
}
