//! A12 — PART membership + control-char field validation (H-16A).

mod common;

use common::{read_until, shared_plain, with_client, with_two_clients};
use tokio::io::AsyncWriteExt;

#[tokio::test(flavor = "multi_thread")]
async fn part_when_not_member_returns_442_no_broadcast() {
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK alice\r\nUSER a 0 * :A\r\nJOIN #lab\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;
        w2.write_all(b"NICK bob\r\nUSER b 0 * :B\r\nPART #lab\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let bob = read_until(&mut r2, |l| l.iter().any(|x| x.contains("442"))).await;
        assert!(
            bob.iter().any(|l| l.contains("442")),
            "non-member PART must 442: {bob:?}"
        );
        assert!(
            !bob.iter().any(|l| l.contains("PART #lab") && !l.contains("442")),
            "bob must not get successful PART echo: {bob:?}"
        );
        // alice should not see a bob PART while waiting briefly
        w1.write_all(b"PRIVMSG #lab :ping\r\nQUIT :x\r\n").await.unwrap();
        let alice = read_until(&mut r1, |l| l.iter().any(|x| x.contains("ERROR") || x.contains("QUIT") || l.len() > 8)).await;
        assert!(
            !alice.iter().any(|l| l.contains("PART") && l.contains("bob")),
            "alice must not see PART from non-member: {alice:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn topic_with_control_char_rejected() {
    let shared = shared_plain();
    with_client(shared, 21, |mut w, mut r| async move {
        w.write_all(b"NICK alice\r\nUSER a 0 * :A\r\nJOIN #lab\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r, |l| l.iter().any(|x| x.contains("366"))).await;
        w.write_all(b"TOPIC #lab :bad\x07topic\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("461"))).await;
        assert!(
            lines.iter().any(|l| l.contains("461")),
            "control char in topic must 461: {lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("TOPIC #lab :bad")),
            "must not broadcast bad topic: {lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn join_channel_with_control_rejected() {
    let shared = shared_plain();
    with_client(shared, 23, |mut w, mut r| async move {
        w.write_all(b"NICK alice\r\nUSER a 0 * :A\r\nJOIN #bad\x07chan\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("403"))).await;
        assert!(
            lines.iter().any(|l| l.contains("403")),
            "control in channel name must 403: {lines:?}"
        );
    })
    .await;
}
