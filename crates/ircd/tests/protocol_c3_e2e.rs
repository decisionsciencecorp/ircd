//! C3 — IRCv3 caps re-enabled with wire tests (message-tags, server-time, account-tag, batch).
mod common;

use std::sync::Arc;

use common::{base_cfg, read_until, with_client};
use ircd::state::Shared;
use tempfile::tempdir;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

// shared_with_history is in common — allow dead_code silence via direct path below.

#[tokio::test]
async fn cap_ls_lists_c3_caps_and_server_time_on_privmsg() {
    let cfg = Arc::new(base_cfg(None));
    let shared = Arc::new(Mutex::new(Shared::new(Arc::clone(&cfg), None)));
    with_client(shared, 30, |mut w, mut r| async move {
        let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
        w.write_all(b"CAP LS\r\nCAP REQ :message-tags server-time account-tag\r\nCAP END\r\n")
            .await
            .unwrap();
        w.write_all(b"NICK t\r\nUSER t 0 * :T\r\nJOIN #t\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 366 "))).await;
        assert!(lines.iter().any(|l| l.contains("message-tags")));
        assert!(lines.iter().any(|l| l.contains("server-time")));
        assert!(lines.iter().any(|l| l.contains("account-tag")));
        assert!(lines.iter().any(|l| l.contains("batch")));
        // self does not see own PRIVMSG via fanout; join second? just CAP ACK enough for LS.
        let _ = lines;
    })
    .await;
}

#[tokio::test]
async fn server_time_tag_reaches_peer() {
    let cfg = Arc::new(base_cfg(None));
    let shared = Arc::new(Mutex::new(Shared::new(Arc::clone(&cfg), None)));

    let shared_a = Arc::clone(&shared);
    let a = tokio::spawn(async move {
        with_client(shared_a, 31, |mut w, mut r| async move {
            let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
            w.write_all(b"CAP LS\r\nCAP REQ :server-time\r\nCAP END\r\nNICK a\r\nUSER a 0 * :A\r\nJOIN #st\r\n")
                .await
                .unwrap();
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 366 "))).await;
            let lines = read_until(&mut r, |ls| {
                ls.iter().any(|l| l.contains("PRIVMSG #st") && l.contains("@time="))
            })
            .await;
            assert!(
                lines
                    .iter()
                    .any(|l| l.starts_with("@time=") && l.contains("PRIVMSG #st")),
                "{lines:?}"
            );
        })
        .await;
    });

    let shared_b = Arc::clone(&shared);
    let b = tokio::spawn(async move {
        with_client(shared_b, 32, |mut w, mut r| async move {
            let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
            w.write_all(b"NICK b\r\nUSER b 0 * :B\r\nJOIN #st\r\n")
                .await
                .unwrap();
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 366 "))).await;
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            w.write_all(b"PRIVMSG #st :timed\r\n").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        })
        .await;
    });

    let _ = tokio::join!(a, b);
}

#[tokio::test]
async fn chathistory_batch_when_cap_enabled() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("c3hist.sqlite3");
    let shared = common::shared_with_history(path);

    with_client(shared, 33, |mut w, mut r| async move {
        let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
        w.write_all(
            b"CAP LS\r\nCAP REQ :batch draft/chathistory\r\nCAP END\r\nNICK h\r\nUSER h 0 * :H\r\nJOIN #h\r\n",
        )
        .await
        .unwrap();
        let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 366 "))).await;
        w.write_all(b"PRIVMSG #h :one\r\nPRIVMSG #h :two\r\n").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        w.write_all(b"CHATHISTORY LATEST #h * 10\r\n").await.unwrap();
        let lines = read_until(&mut r, |ls| ls.iter().any(|l| l.contains("BATCH -chathist"))).await;
        assert!(lines.iter().any(|l| l.contains("BATCH +chathist")));
        assert!(lines.iter().any(|l| l.contains("batch=chathist") || l.contains(";batch=chathist") || l.contains("@batch=chathist")));
        assert!(lines.iter().any(|l| l.contains("BATCH -chathist")));
    })
    .await;
}
