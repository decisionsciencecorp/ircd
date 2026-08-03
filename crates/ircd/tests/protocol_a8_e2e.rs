//! A8 — wire foundation: CAP atomicity, registration hold, casemap, ISUPPORT.

mod common;

use common::{read_until, shared_plain, with_client, with_two_clients};
use ircd_core::ascii_casefold;
use tokio::io::AsyncWriteExt;

#[tokio::test(flavor = "multi_thread")]
async fn mixed_cap_req_single_nak_no_partial_enable() {
    let shared = shared_plain();
    with_client(shared, 51, |mut w, mut r| async move {
        w.write_all(
            b"CAP LS\r\nCAP REQ :sasl bogon\r\nNICK n\r\nUSER u 0 * :U\r\nCAP END\r\nQUIT :x\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("NAK"))).await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("NAK") && l.contains("sasl") && l.contains("bogon")),
            "{lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|l| l.contains(" ACK ") || l.contains("ACK :")),
            "{lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cap_end_releases_registration() {
    let shared = shared_plain();
    with_client(shared, 52, |mut w, mut r| async move {
        // Drain greetings first
        let _ = read_until(&mut r, |l| l.len() >= 3).await;
        w.write_all(b"CAP LS\r\nNICK n\r\nUSER u 0 * :U\r\n")
            .await
            .unwrap();
        let mid = read_until(&mut r, |l| {
            l.iter().any(|x| x.contains("CAP") && x.contains("LS"))
        })
        .await;
        assert!(
            !mid.iter().any(|l| l.contains("001")),
            "must hold registration until CAP END: {mid:?}"
        );
        w.write_all(b"CAP END\r\nQUIT :x\r\n").await.unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter()
                .any(|x| x.contains("005") && x.contains("CASEMAPPING"))
        })
        .await;
        assert!(lines.iter().any(|l| l.contains("001")), "{lines:?}");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("005") && l.contains("CASEMAPPING=ascii")),
            "{lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn nick_casemap_collision_alice_alice() {
    let shared = shared_plain();
    with_two_clients(
        shared.clone(),
        |(mut w1, mut r1), (mut w2, mut r2)| async move {
            let _ = read_until(&mut r1, |l| l.len() >= 3).await;
            let _ = read_until(&mut r2, |l| l.len() >= 3).await;
            w1.write_all(b"NICK Alice\r\nUSER a 0 * :A\r\n")
                .await
                .unwrap();
            let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("001"))).await;
            // Prove map has folded key before second NICK
            {
                let g = shared.lock().await;
                assert!(
                    g.nicks.contains_key(&ascii_casefold("Alice")),
                    "map keys: {:?}",
                    g.nicks.keys().collect::<Vec<_>>()
                );
            }
            w2.write_all(b"NICK alice\r\nUSER b 0 * :B\r\nQUIT :x\r\n")
                .await
                .unwrap();
            let lines = read_until(&mut r2, |l| {
                l.iter().any(|x| x.contains("433") || x.contains("001"))
            })
            .await;
            assert!(
                lines.iter().any(|l| l.contains("433")),
                "expected 433, map keys after: check lines {lines:?}"
            );
            assert!(
                !lines.iter().any(|l| l.contains("001")),
                "must not register stolen nick: {lines:?}"
            );
            w1.write_all(b"QUIT :x\r\n").await.unwrap();
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_command_returns_421() {
    let shared = shared_plain();
    with_client(shared, 53, |mut w, mut r| async move {
        w.write_all(b"NICK n\r\nUSER u 0 * :U\r\nFOOBAR baz\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("421"))).await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("421") && l.contains("FOOBAR")),
            "{lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn nick_without_param_431() {
    let shared = shared_plain();
    with_client(shared, 54, |mut w, mut r| async move {
        let _ = read_until(&mut r, |l| l.len() >= 3).await;
        w.write_all(b"NICK\r\nQUIT :x\r\n").await.unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("431"))).await;
        assert!(lines.iter().any(|l| l.contains("431")), "{lines:?}");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cannot_disable_cap_notify() {
    let shared = shared_plain();
    with_client(shared, 55, |mut w, mut r| async move {
        let _ = read_until(&mut r, |l| l.len() >= 3).await;
        w.write_all(b"CAP LS\r\nCAP REQ :-cap-notify\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("NAK"))).await;
        assert!(lines.iter().any(|l| l.contains("NAK")), "{lines:?}");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pong_has_server_prefix() {
    let shared = shared_plain();
    with_client(shared, 56, |mut w, mut r| async move {
        let _ = read_until(&mut r, |l| l.len() >= 3).await;
        w.write_all(b"NICK n\r\nUSER u 0 * :U\r\nPING :xyz\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("PONG"))).await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("PONG") && l.contains("cov.test") && l.contains("xyz")),
            "{lines:?}"
        );
    })
    .await;
}
