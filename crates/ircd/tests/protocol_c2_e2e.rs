//! C2 — INVITE, +b/+i, PART/QUIT reasons.
mod common;

use std::sync::Arc;

use common::{base_cfg, read_until, with_client};
use ircd::state::Shared;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

async fn register(w: &mut tokio::io::WriteHalf<tokio::io::DuplexStream>, nick: &str) {
    w.write_all(format!("NICK {nick}\r\nUSER {nick} 0 * :{nick}\r\n").as_bytes())
        .await
        .unwrap();
}

#[tokio::test]
async fn invite_only_and_invite() {
    let cfg = Arc::new(base_cfg(None));
    let shared = Arc::new(Mutex::new(Shared::new(Arc::clone(&cfg), None)));

    let shared_op = Arc::clone(&shared);
    let op = tokio::spawn(async move {
        with_client(shared_op, 20, |mut w, mut r| async move {
            let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
            register(&mut w, "op").await;
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 001 "))).await;
            w.write_all(b"JOIN #i\r\nMODE #i +i\r\n").await.unwrap();
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains("MODE #i +i"))).await;
            // wait for guest attempt then invite
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            w.write_all(b"INVITE guest #i\r\n").await.unwrap();
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 341 "))).await;
        })
        .await;
    });

    let shared_g = Arc::clone(&shared);
    let guest = tokio::spawn(async move {
        with_client(shared_g, 21, |mut w, mut r| async move {
            let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
            register(&mut w, "guest").await;
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 001 "))).await;
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            w.write_all(b"JOIN #i\r\n").await.unwrap();
            let lines = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 473 "))).await;
            assert!(lines.iter().any(|l| l.contains(" 473 ")));
            let lines =
                read_until(&mut r, |ls| ls.iter().any(|l| l.contains("INVITE guest"))).await;
            assert!(lines.iter().any(|l| l.contains("INVITE")));
            w.write_all(b"JOIN #i\r\n").await.unwrap();
            let lines = read_until(&mut r, |ls| ls.iter().any(|l| l.contains("JOIN :#i"))).await;
            assert!(lines.iter().any(|l| l.contains("JOIN :#i")));
        })
        .await;
    });

    let _ = tokio::join!(op, guest);
}

#[tokio::test]
async fn ban_blocks_join_and_part_reason() {
    let cfg = Arc::new(base_cfg(None));
    let shared = Arc::new(Mutex::new(Shared::new(Arc::clone(&cfg), None)));

    let shared_op = Arc::clone(&shared);
    let op = tokio::spawn(async move {
        with_client(shared_op, 22, |mut w, mut r| async move {
            let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
            register(&mut w, "chop").await;
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 001 "))).await;
            w.write_all(b"JOIN #b\r\nMODE #b +b banned\r\n")
                .await
                .unwrap();
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains("MODE #b +b"))).await;
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            // see PART with reason from peer
            let lines = read_until(&mut r, |ls| ls.iter().any(|l| l.contains("PART #b"))).await;
            assert!(lines.iter().any(|l| l.contains("see-ya")));
        })
        .await;
    });

    let shared_ban = Arc::clone(&shared);
    let banned = tokio::spawn(async move {
        with_client(shared_ban, 23, |mut w, mut r| async move {
            let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
            register(&mut w, "banned").await;
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 001 "))).await;
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            w.write_all(b"JOIN #b\r\n").await.unwrap();
            let lines = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 474 "))).await;
            assert!(lines.iter().any(|l| l.contains(" 474 ")));
        })
        .await;
    });

    let shared_p = Arc::clone(&shared);
    let parter = tokio::spawn(async move {
        with_client(shared_p, 24, |mut w, mut r| async move {
            let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
            register(&mut w, "parter").await;
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 001 "))).await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            w.write_all(b"JOIN #b\r\n").await.unwrap();
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 366 "))).await;
            w.write_all(b"PART #b :see-ya\r\n").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await;
    });

    let _ = tokio::join!(op, banned, parter);
}
