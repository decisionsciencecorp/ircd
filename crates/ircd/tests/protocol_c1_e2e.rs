//! C1 — core client usability: queries, NOTICE, direct PRIVMSG.
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
async fn version_motd_lusers_list_whois_who_names() {
    let cfg = Arc::new(base_cfg(None));
    let shared = Arc::new(Mutex::new(Shared::new(Arc::clone(&cfg), None)));
    with_client(shared, 1, |mut w, mut r| async move {
        let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
        register(&mut w, "q").await;
        let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 001 "))).await;
        w.write_all(
            b"VERSION\r\nMOTD\r\nLUSERS\r\nJOIN #c\r\nLIST\r\nLIST #c\r\nWHO #c\r\nWHO q\r\nWHOIS q\r\nNAMES #c\r\nNAMES\r\nPRIVMSG\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r, |ls| {
            ls.iter().any(|l| l.contains(" 318 "))
                && ls.iter().any(|l| l.contains(" 351 "))
                && ls.iter().any(|l| l.contains(" 323 "))
                && ls.iter().any(|l| l.contains(" 251 "))
                && ls.iter().any(|l| l.contains(" 315 "))
                && ls.iter().any(|l| l.contains(" 461 "))
        })
        .await;
        assert!(lines.iter().any(|l| l.contains(" 351 ")));
        assert!(lines.iter().any(|l| l.contains(" 376 ") || l.contains(" 372 ")));
        assert!(lines.iter().any(|l| l.contains(" 322 ")));
        assert!(lines.iter().any(|l| l.contains(" 352 ")));
        assert!(lines.iter().any(|l| l.contains(" 311 ")));
        assert!(lines.iter().any(|l| l.contains(" 353 ")));
        assert!(lines.iter().any(|l| l.contains(" 461 ")));
    })
    .await;
}

#[tokio::test]
async fn direct_privmsg_and_notice() {
    let cfg = Arc::new(base_cfg(None));
    let shared = Arc::new(Mutex::new(Shared::new(Arc::clone(&cfg), None)));

    let shared_a = Arc::clone(&shared);
    let a = tokio::spawn(async move {
        with_client(shared_a, 2, |mut w, mut r| async move {
            let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
            register(&mut w, "alice").await;
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 001 "))).await;
            let lines =
                read_until(&mut r, |ls| ls.iter().any(|l| l.contains("PRIVMSG alice"))).await;
            assert!(lines.iter().any(|l| l.contains("direct-hi")));
            let lines =
                read_until(&mut r, |ls| ls.iter().any(|l| l.contains("NOTICE alice"))).await;
            assert!(lines.iter().any(|l| l.contains("psst")));
        })
        .await;
    });

    let shared_b = Arc::clone(&shared);
    let b = tokio::spawn(async move {
        with_client(shared_b, 3, |mut w, mut r| async move {
            let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
            register(&mut w, "bob").await;
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 001 "))).await;
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            w.write_all(b"PRIVMSG alice :direct-hi\r\nNOTICE alice :psst\r\nPRIVMSG nobody :x\r\n")
                .await
                .unwrap();
            let lines = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 401 "))).await;
            assert!(lines.iter().any(|l| l.contains(" 401 ")));
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        })
        .await;
    });

    let _ = tokio::join!(a, b);
}

#[tokio::test]
async fn channel_notice_and_whois_unknown() {
    let cfg = Arc::new(base_cfg(None));
    let shared = Arc::new(Mutex::new(Shared::new(Arc::clone(&cfg), None)));

    let shared_a = Arc::clone(&shared);
    let a = tokio::spawn(async move {
        with_client(shared_a, 4, |mut w, mut r| async move {
            let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
            register(&mut w, "n1").await;
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 001 "))).await;
            w.write_all(b"JOIN #n\r\n").await.unwrap();
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 366 "))).await;
            let lines = read_until(&mut r, |ls| ls.iter().any(|l| l.contains("NOTICE #n"))).await;
            assert!(lines.iter().any(|l| l.contains("chan-note")));
        })
        .await;
    });

    let shared_b = Arc::clone(&shared);
    let b = tokio::spawn(async move {
        with_client(shared_b, 5, |mut w, mut r| async move {
            let _ = read_until(&mut r, |ls| ls.len() >= 3).await;
            register(&mut w, "n2").await;
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 001 "))).await;
            w.write_all(b"JOIN #n\r\n").await.unwrap();
            let _ = read_until(&mut r, |ls| ls.iter().any(|l| l.contains(" 366 "))).await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            w.write_all(b"NOTICE #n :chan-note\r\nWHOIS missing\r\nWHOIS\r\n")
                .await
                .unwrap();
            let lines = read_until(&mut r, |ls| {
                ls.iter().any(|l| l.contains(" 401 ")) && ls.iter().any(|l| l.contains(" 461 "))
            })
            .await;
            assert!(lines.iter().any(|l| l.contains(" 401 ")));
            assert!(lines.iter().any(|l| l.contains(" 461 ")));
        })
        .await;
    });

    let _ = tokio::join!(a, b);
}
