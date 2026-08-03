//! F1a — AWAY + away-notify.

mod common;

use common::{read_until, shared_plain, with_client, with_two_clients};
use tokio::io::AsyncWriteExt;

#[tokio::test(flavor = "multi_thread")]
async fn away_set_clear_self_numerics() {
    let shared = shared_plain();
    with_client(shared, 40, |mut w, mut r| async move {
        w.write_all(b"NICK a\r\nUSER a 0 * :A\r\nAWAY :gone\r\nAWAY\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter().any(|x| x.contains("305")) && l.iter().any(|x| x.contains("306"))
        })
        .await;
        assert!(
            lines.iter().any(|l| l.contains("306")),
            "missing RPL_NOWAWAY: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("305")),
            "missing RPL_UNAWAY: {lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn away_notify_peer_on_set_and_clear() {
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"CAP REQ :away-notify\r\nCAP END\r\nNICK a\r\nUSER a 0 * :A\r\nJOIN #aw\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;
        w2.write_all(b"CAP REQ :away-notify\r\nCAP END\r\nNICK b\r\nUSER b 0 * :B\r\nJOIN #aw\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("366"))).await;
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("JOIN"))).await;

        w2.write_all(b"AWAY :brb\r\n").await.unwrap();
        let set_lines = read_until(&mut r1, |l| {
            l.iter()
                .any(|x| x.contains("AWAY") && x.contains("brb") && !x.contains("306"))
        })
        .await;
        assert!(
            set_lines
                .iter()
                .any(|l| l.contains("AWAY") && l.contains("brb")),
            "peer missing away-notify set: {set_lines:?}"
        );

        w2.write_all(b"AWAY\r\n").await.unwrap();
        let clear_lines = read_until(&mut r1, |l| {
            l.iter().any(|x| {
                x.contains(" AWAY") && !x.contains(':') && !x.contains("306") && !x.contains("305")
            }) || l
                .iter()
                .any(|x| x.ends_with("AWAY") || x.contains("AWAY\r"))
        })
        .await;
        assert!(
            clear_lines.iter().any(|l| {
                let t = l.trim_end_matches(['\r', '\n']);
                t.contains("AWAY") && !t.contains("brb") && !t.contains("306") && !t.contains("305")
            }),
            "peer missing away-notify clear: {clear_lines:?}"
        );

        w1.write_all(b"QUIT :x\r\n").await.unwrap();
        w2.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn away_notify_on_join_when_already_away() {
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(
            b"CAP REQ :away-notify\r\nCAP END\r\nNICK a\r\nUSER a 0 * :A\r\nJOIN #j\r\n",
        )
        .await
        .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;

        w2.write_all(
            b"CAP REQ :away-notify\r\nCAP END\r\nNICK b\r\nUSER b 0 * :B\r\nAWAY :out\r\nJOIN #j\r\n",
        )
        .await
        .unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("366"))).await;

        let lines = read_until(&mut r1, |l| {
            l.iter()
                .any(|x| x.contains("JOIN"))
                && l.iter().any(|x| x.contains("AWAY") && x.contains("out"))
        })
        .await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("AWAY") && l.contains("out")),
            "missing away-notify on join: {lines:?}"
        );
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
        w2.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn whois_and_privmsg_surface_away_without_cap() {
    // Q3: WHOIS 301 + PRIVMSG 301 without away-notify on the querier.
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK a\r\nUSER a 0 * :A\r\n").await.unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("001"))).await;
        w2.write_all(b"NICK b\r\nUSER b 0 * :B\r\nAWAY :zzz\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("306"))).await;

        w1.write_all(b"WHOIS b\r\nPRIVMSG b :hi\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r1, |l| {
            l.iter().filter(|x| x.contains("301")).count() >= 2
        })
        .await;
        let threes = lines
            .iter()
            .filter(|l| l.contains("301") && l.contains("zzz"))
            .count();
        assert!(
            threes >= 2,
            "expected WHOIS+PRIVMSG 301 with away text, got {threes}: {lines:?}"
        );
        w2.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}
