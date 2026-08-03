//! B1 — member-targeted routing: non-members must not see channel PRIVMSG; kick reaches victim.
mod common;

use std::sync::Arc;

use common::{base_cfg, read_until, with_client};
use ircd::state::Shared;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

#[tokio::test]
async fn privmsg_only_to_members() {
    let cfg = Arc::new(base_cfg(None));
    let shared = Arc::new(Mutex::new(Shared::new(Arc::clone(&cfg), None)));

    let shared_a = Arc::clone(&shared);
    let a = tokio::spawn(async move {
        with_client(shared_a, 1, |mut w, mut r| async move {
            let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
            w.write_all(b"NICK a\r\nUSER a 0 * :A\r\nJOIN #c\r\n")
                .await
                .unwrap();
            let _ = read_until(&mut r, |ls| {
                ls.iter()
                    .any(|l| l.contains("JOIN :#c") || l.contains(" 366 "))
            })
            .await;
            let lines = read_until(&mut r, |ls| ls.iter().any(|l| l.contains("PRIVMSG #c"))).await;
            assert!(lines.iter().any(|l| l.contains("hello-members")));
        })
        .await;
    });

    let shared_b = Arc::clone(&shared);
    let b = tokio::spawn(async move {
        with_client(shared_b, 2, |mut w, mut r| async move {
            let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
            w.write_all(b"NICK b\r\nUSER b 0 * :B\r\nJOIN #c\r\n")
                .await
                .unwrap();
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 366 "))).await;
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            w.write_all(b"PRIVMSG #c :hello-members\r\n").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        })
        .await;
    });

    let shared_c = Arc::clone(&shared);
    let c = tokio::spawn(async move {
        with_client(shared_c, 3, |mut w, mut r| async move {
            let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
            w.write_all(b"NICK c\r\nUSER c 0 * :C\r\n").await.unwrap();
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 001 "))).await;
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            let lines = read_until(&mut r, |_| false).await;
            assert!(
                !lines.iter().any(|l| l.contains("PRIVMSG #c")),
                "non-member saw channel traffic: {lines:?}"
            );
        })
        .await;
    });

    let _ = tokio::join!(a, b, c);
}

#[tokio::test]
async fn kick_reaches_victim_via_outbox() {
    let cfg = Arc::new(base_cfg(None));
    let shared = Arc::new(Mutex::new(Shared::new(Arc::clone(&cfg), None)));

    let shared_op = Arc::clone(&shared);
    let op = tokio::spawn(async move {
        with_client(shared_op, 10, |mut w, mut r| async move {
            let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
            w.write_all(b"NICK op\r\nUSER op 0 * :Op\r\nJOIN #k\r\n")
                .await
                .unwrap();
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 366 "))).await;
            let _ = read_until(&mut r, |ls| {
                ls.iter().any(|l| l.contains("JOIN") && l.contains("vic"))
            })
            .await;
            w.write_all(b"KICK #k vic :out\r\n").await.unwrap();
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains("KICK #k vic"))).await;
        })
        .await;
    });

    let shared_vic = Arc::clone(&shared);
    let vic = tokio::spawn(async move {
        with_client(shared_vic, 11, |mut w, mut r| async move {
            let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            w.write_all(b"NICK vic\r\nUSER vic 0 * :V\r\nJOIN #k\r\n")
                .await
                .unwrap();
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 366 "))).await;
            let lines = read_until(&mut r, |ls| ls.iter().any(|l| l.contains("KICK #k vic"))).await;
            assert!(
                lines.iter().any(|l| l.contains("KICK #k vic")),
                "victim missed KICK: {lines:?}"
            );
        })
        .await;
    });

    let _ = tokio::join!(op, vic);
}
