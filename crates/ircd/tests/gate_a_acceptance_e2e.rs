//! A9 — Gate A adversarial acceptance pack (Doc #974 public-host blockers).
//!
//! Run: `cargo test -p ircd --test gate_a_acceptance_e2e`
//!
//! Maps to slices A1–A8 + A10–A12. Individual protocol_*_e2e suites remain the
//! deep coverage; this pack is the single green light before public bind.

mod common;

use common::{read_until, shared_plain, with_client, with_client_secure, with_two_clients};
use ircd::config::{AccountSection, Config, WebSocketSection};
use ircd::fs_perms::{ensure_private_file, is_world_or_group_accessible};
use ircd::lock_discipline::find_lock_across_await;
use ircd::state::Shared;
use ircd::ws::evaluate_ws_handshake;
use ircd::ws_policy::origin_allowed;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

#[tokio::test(flavor = "multi_thread")]
async fn a1_cap_ls_has_no_false_ads() {
    let shared = shared_plain();
    with_client(shared, 90, |mut w, mut r| async move {
        let _ = read_until(&mut r, |l| l.len() >= 3).await;
        w.write_all(b"CAP LS\r\nQUIT :x\r\n").await.unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter().any(|x| x.contains("CAP") && x.contains("LS"))
        })
        .await;
        let blob = lines.join("\n");
        for forbidden in ["multi-prefix", "away-notify", "echo-message"] {
            assert!(
                !blob.contains(forbidden),
                "false CAP ad {forbidden}: {blob}"
            );
        }
        // C3: these are advertised only with wire tests.
        for required in [
            "cap-notify",
            "message-tags",
            "server-time",
            "account-tag",
            "batch",
        ] {
            assert!(blob.contains(required), "missing CAP {required}: {blob}");
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a2_nick_steal_does_not_transfer_ops() {
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        let _ = read_until(&mut r1, |l| l.len() >= 3).await;
        let _ = read_until(&mut r2, |l| l.len() >= 3).await;
        w1.write_all(b"NICK OpOld\r\nUSER o 0 * :O\r\nJOIN #priv\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;
        w1.write_all(b"NICK OpNew\r\n").await.unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("NICK"))).await;
        w2.write_all(b"NICK OpOld\r\nUSER s 0 * :S\r\nMODE #priv +o OpNew\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r2, |l| {
            l.iter().any(|x| {
                x.contains("482") || x.contains("442") || x.contains("441") || x.contains("404")
            })
        })
        .await;
        let blob = lines.join("\n");
        assert!(
            blob.contains("482")
                || blob.contains("442")
                || blob.contains("441")
                || blob.contains("404")
                || blob.contains("401"),
            "stolen nick must not op: {blob}"
        );
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a3_oversized_line_417() {
    let mut cfg = Config::default();
    cfg.limits.max_line_bytes = 64;
    cfg.security.registration_timeout_secs = 0;
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 91, |mut w, mut r| async move {
        let _ = read_until(&mut r, |l| l.len() >= 3).await;
        let huge = "A".repeat(80);
        w.write_all(format!("{huge}\r\n").as_bytes()).await.unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter()
                .any(|x| x.contains("417") || x.contains("too long") || x.contains("ERROR"))
        })
        .await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("417") || l.contains("too long") || l.contains("ERROR")),
            "{lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a4_kick_revokes_channel_send() {
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        let _ = read_until(&mut r1, |l| l.len() >= 3).await;
        let _ = read_until(&mut r2, |l| l.len() >= 3).await;
        w1.write_all(b"NICK op\r\nUSER o 0 * :O\r\nJOIN #k\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;
        w2.write_all(b"NICK vic\r\nUSER v 0 * :V\r\nJOIN #k\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("366"))).await;
        w1.write_all(b"KICK #k vic :out\r\n").await.unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("KICK"))).await;
        w2.write_all(b"PRIVMSG #k :nope\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r2, |l| l.iter().any(|x| x.contains("404"))).await;
        assert!(lines.iter().any(|l| l.contains("404")), "{lines:?}");
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a5_plaintext_sasl_blocked_when_tls_required() {
    let mut cfg = Config::default();
    cfg.security.require_tls_for_auth = true;
    cfg.security.registration_timeout_secs = 0;
    cfg.accounts = vec![AccountSection {
        name: "alice".into(),
        password: "secret".into(),
    }];
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 92, |mut w, mut r| async move {
        let _ = read_until(&mut r, |l| l.len() >= 3).await;
        w.write_all(
            b"CAP LS\r\nNICK n\r\nUSER u 0 * :U\r\nCAP REQ :sasl\r\nAUTHENTICATE PLAIN\r\nQUIT :x\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("904"))).await;
        assert!(lines.iter().any(|l| l.contains("TLS required")), "{lines:?}");
    })
    .await;
    // Secure path still works
    let mut cfg = Config::default();
    cfg.security.require_tls_for_auth = true;
    cfg.security.registration_timeout_secs = 0;
    cfg.accounts = vec![AccountSection {
        name: "alice".into(),
        password: "secret".into(),
    }];
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client_secure(shared, 93, |mut w, mut r| async move {
        let _ = read_until(&mut r, |l| l.len() >= 3).await;
        w.write_all(b"CAP LS\r\nCAP REQ :sasl\r\nAUTHENTICATE PLAIN\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x == "AUTHENTICATE +")).await;
        assert!(lines.iter().any(|l| l == "AUTHENTICATE +"), "{lines:?}");
    })
    .await;
}

#[test]
fn a6_session_has_no_lock_across_await() {
    let src = include_str!("../src/session.rs");
    let bad = find_lock_across_await(src);
    assert!(bad.is_empty(), "lock-across-await at {bad:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a7_channel_quotas() {
    let mut cfg = Config::default();
    cfg.limits.max_channels_per_client = 1;
    cfg.security.registration_timeout_secs = 0;
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 94, |mut w, mut r| async move {
        let _ = read_until(&mut r, |l| l.len() >= 3).await;
        w.write_all(b"NICK q\r\nUSER q 0 * :Q\r\nJOIN #a\r\nJOIN #b\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("405"))).await;
        assert!(lines.iter().any(|l| l.contains("405")), "{lines:?}");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a8_casemap_and_cap_end() {
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        let _ = read_until(&mut r1, |l| l.len() >= 3).await;
        let _ = read_until(&mut r2, |l| l.len() >= 3).await;
        w1.write_all(b"CAP LS\r\nNICK Alice\r\nUSER a 0 * :A\r\n")
            .await
            .unwrap();
        let mid = read_until(&mut r1, |l| {
            l.iter().any(|x| x.contains("CAP") && x.contains("LS"))
        })
        .await;
        assert!(!mid.iter().any(|l| l.contains("001")), "{mid:?}");
        w1.write_all(b"CAP END\r\n").await.unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("001"))).await;
        w2.write_all(b"NICK alice\r\nUSER b 0 * :B\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r2, |l| l.iter().any(|x| x.contains("433"))).await;
        assert!(lines.iter().any(|l| l.contains("433")), "{lines:?}");
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[test]
fn a10_ws_origin_policy() {
    let mut cfg = WebSocketSection::default();
    cfg.allowed_origins = vec!["https://ok.test".into()];
    cfg.require_irc_subprotocol = true;
    assert!(evaluate_ws_handshake(None, &["irc".into()], &cfg).is_ok());
    assert!(evaluate_ws_handshake(Some("https://evil"), &["irc".into()], &cfg).is_err());
    assert!(!origin_allowed(
        Some("https://evil"),
        &cfg.allowed_origins,
        true
    ));
}

#[test]
fn a11_refuse_world_readable_secret() {
    let dir = tempdir().unwrap();
    let key = dir.path().join("key.pem");
    fs::write(&key, b"secret").unwrap();
    let mut perms = fs::metadata(&key).unwrap().permissions();
    perms.set_mode(0o644);
    fs::set_permissions(&key, perms).unwrap();
    assert!(is_world_or_group_accessible(0o644));
    assert!(ensure_private_file(&key).is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a12_part_nonmember_442() {
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        let _ = read_until(&mut r1, |l| l.len() >= 3).await;
        let _ = read_until(&mut r2, |l| l.len() >= 3).await;
        w1.write_all(b"NICK alice\r\nUSER a 0 * :A\r\nJOIN #lab\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;
        w2.write_all(b"NICK bob\r\nUSER b 0 * :B\r\nPART #lab\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let bob = read_until(&mut r2, |l| l.iter().any(|x| x.contains("442"))).await;
        assert!(bob.iter().any(|l| l.contains("442")), "{bob:?}");
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}
