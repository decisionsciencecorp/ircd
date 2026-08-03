//! C9 — buffered session line I/O (no 50ms read_u8 tick).

mod common;

use common::{read_until, shared_plain, with_client, with_two_clients};
use ircd::config::Config;
use ircd::state::Shared;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

#[tokio::test(flavor = "multi_thread")]
async fn buffered_io_privmsg_and_outbox_interleave() {
    // Q2: inbound PRIVMSG and peer outbox delivery both progress under load —
    // fairness is select! (biased outbox), not a 50ms poll tick.
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK a\r\nUSER a 0 * :A\r\nJOIN #c9\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;
        w2.write_all(b"NICK b\r\nUSER b 0 * :B\r\nJOIN #c9\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("366"))).await;
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("JOIN"))).await;

        for i in 0..40 {
            w1.write_all(format!("PRIVMSG #c9 :burst-{i}\r\n").as_bytes())
                .await
                .unwrap();
        }
        w2.write_all(b"PRIVMSG #c9 :from-b\r\n").await.unwrap();

        let a_lines = read_until(&mut r1, |l| {
            l.iter()
                .any(|x| x.contains("PRIVMSG #c9") && x.contains("from-b"))
        })
        .await;
        assert!(
            a_lines
                .iter()
                .any(|l| l.contains("PRIVMSG #c9") && l.contains("from-b")),
            "a must receive b's message via outbox while a was flooding inbound: {a_lines:?}"
        );

        let b_lines = read_until(&mut r2, |l| {
            l.iter()
                .filter(|x| x.contains("PRIVMSG #c9") && x.contains("burst-"))
                .count()
                >= 20
        })
        .await;
        let bursts = b_lines
            .iter()
            .filter(|x| x.contains("PRIVMSG #c9") && x.contains("burst-"))
            .count();
        assert!(
            bursts >= 20,
            "b must receive a's fanout (got {bursts}): {b_lines:?}"
        );

        w1.write_all(b"QUIT :x\r\n").await.unwrap();
        w2.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn buffered_io_oversized_flood_and_deadlines() {
    // Q3: oversized / flood / reg+idle deadlines still fire after BufReader change.
    let mut cfg = Config::default();
    cfg.limits.max_line_bytes = 64;
    cfg.limits.flood_lines_per_window = 500;
    cfg.security.registration_timeout_secs = 0;
    cfg.security.idle_timeout_secs = 0;
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 190, |mut w, mut r| async move {
        let _ = read_until(&mut r, |l| l.len() >= 3).await;
        let huge = "A".repeat(80);
        w.write_all(format!("{huge}\r\n").as_bytes()).await.unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter()
                .any(|x| x.contains("too long") || x.contains("ERROR"))
        })
        .await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("too long") || l.contains("ERROR")),
            "{lines:?}"
        );
    })
    .await;

    let mut cfg = Config::default();
    cfg.limits.flood_lines_per_window = 5;
    cfg.limits.flood_window_secs = 30;
    cfg.security.registration_timeout_secs = 0;
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 191, |mut w, mut r| async move {
        for i in 0..20 {
            w.write_all(format!("PING :c9{i}\r\n").as_bytes())
                .await
                .unwrap();
        }
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("Excess Flood"))).await;
        assert!(
            lines.iter().any(|l| l.contains("Excess Flood")),
            "{lines:?}"
        );
    })
    .await;

    let mut cfg = Config::default();
    cfg.security.registration_timeout_secs = 1;
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 192, |_w, mut r| async move {
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

    let mut cfg = Config::default();
    cfg.security.registration_timeout_secs = 0;
    cfg.security.idle_timeout_secs = 1;
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 193, |mut w, mut r| async move {
        w.write_all(b"NICK c9idle\r\nUSER i 0 * :I\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r, |l| l.iter().any(|x| x.contains("001"))).await;
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("Idle timeout"))).await;
        assert!(
            lines.iter().any(|l| l.contains("Idle timeout")),
            "{lines:?}"
        );
    })
    .await;
}
