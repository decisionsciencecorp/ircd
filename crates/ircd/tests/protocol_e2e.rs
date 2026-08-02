mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{read_until, shared_plain, shared_with_history, with_client, with_two_clients};
use ircd::config::LimitsSection;
use ircd::state::Shared;
use tempfile::tempdir;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

#[tokio::test(flavor = "multi_thread")]
async fn register_join_privmsg_chathistory() {
    let dir = tempdir().unwrap();
    let hist = dir.path().join("h.sqlite3");
    let shared = shared_with_history(hist);

    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK alice\r\nUSER a 0 * :A\r\nJOIN #lab\r\nPRIVMSG #lab :hello hist\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("001 alice"))).await;
        tokio::time::sleep(Duration::from_millis(30)).await;

        w2.write_all(
            b"CAP LS\r\nNICK bob\r\nUSER b 0 * :B\r\nCAP END\r\nJOIN #lab\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r2, |l| {
            l.iter().any(|x| x.contains("hello hist")) && l.iter().any(|x| x.contains("366 bob"))
        })
        .await;
        assert!(lines.iter().any(|l| l.contains("hello hist")), "{lines:?}");

        w2.write_all(b"CHATHISTORY LATEST #lab * 10\r\nQUIT :done\r\n")
            .await
            .unwrap();
        // Without advertising `batch`/`chathistory`, history still replays as PRIVMSG lines.
        let more = read_until(&mut r2, |l| l.iter().any(|x| x.contains("hello hist"))).await;
        assert!(more.iter().any(|l| l.contains("hello hist")), "{more:?}");
        w1.write_all(b"QUIT :done\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sasl_plain_success() {
    let shared = shared_plain();
    with_client(shared, 3, |mut w, mut r| async move {
        w.write_all(b"CAP LS\r\nNICK alice\r\nUSER a 0 * :A\r\nCAP REQ :sasl\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r, |l| l.iter().any(|x| x.contains("ACK"))).await;
        w.write_all(b"AUTHENTICATE PLAIN\r\n").await.unwrap();
        let _ = read_until(&mut r, |l| l.iter().any(|x| x == "AUTHENTICATE +")).await;
        w.write_all(b"AUTHENTICATE AGFsaWNlAHNlY3JldA==\r\nCAP END\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("001 alice"))).await;
        assert!(lines.iter().any(|l| l.contains("903")));
        assert!(lines.iter().any(|l| l.contains("900")));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn flood_closes() {
    let mut cfg = common::base_cfg(None);
    cfg.limits = LimitsSection {
        max_clients: 32,
        max_clients_per_ip: 32,
        flood_lines_per_window: 5,
        flood_window_secs: 30,
        max_line_bytes: 8192,
        max_channels: 1024,
        max_channels_per_client: 64,
        max_members_per_channel: 512,
        max_topic_bytes: 390,
    };
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 4, |mut w, mut r| async move {
        for i in 0..20 {
            w.write_all(format!("PING :x{i}\r\n").as_bytes())
                .await
                .unwrap();
        }
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("Excess Flood"))).await;
        assert!(lines.iter().any(|l| l.contains("Excess Flood")));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn admission_rejects() {
    let mut cfg = common::base_cfg(None);
    cfg.limits.max_clients = 1;
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_two_clients(shared, |(mut w1, mut r1), (_w2, mut r2)| async move {
        w1.write_all(b"NICK a\r\nUSER a 0 * :A\r\n").await.unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("Connected"))).await;
        let lines = read_until(&mut r2, |l| l.iter().any(|x| x.contains("Too many connections")))
            .await;
        assert!(lines.iter().any(|l| l.contains("Too many connections")));
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn oper_and_topic() {
    let shared = shared_plain();
    with_client(shared, 7, |mut w, mut r| async move {
        w.write_all(
            b"NICK op\r\nUSER o 0 * :O\r\nJOIN #t\r\nOPER admin operpass\r\nTOPIC #t :hello topic\r\nQUIT :x\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter().any(|x| x.contains("381")) && l.iter().any(|x| x.contains("332"))
        })
        .await;
        assert!(lines.iter().any(|l| l.contains("hello topic")));
    })
    .await;
}
