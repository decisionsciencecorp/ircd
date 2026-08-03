//! F1b — INVITE / +i / +b numeric honesty.

mod common;

use common::{read_until, shared_plain, with_client, with_two_clients};
use tokio::io::AsyncWriteExt;

#[tokio::test(flavor = "multi_thread")]
async fn invite_nonmember_gets_442() {
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK a\r\nUSER a 0 * :A\r\nJOIN #x\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;
        w2.write_all(b"NICK b\r\nUSER b 0 * :B\r\nINVITE a #x\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r2, |l| l.iter().any(|x| x.contains("442"))).await;
        assert!(
            lines.iter().any(|l| l.contains("442")),
            "non-member INVITE must be 442 not 482: {lines:?}"
        );
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn invite_already_on_channel_gets_443() {
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK a\r\nUSER a 0 * :A\r\nJOIN #y\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;
        w2.write_all(b"NICK b\r\nUSER b 0 * :B\r\nJOIN #y\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("366"))).await;
        w1.write_all(b"INVITE b #y\r\nQUIT :x\r\n").await.unwrap();
        let lines = read_until(&mut r1, |l| l.iter().any(|x| x.contains("443"))).await;
        assert!(
            lines.iter().any(|l| l.contains("443")),
            "already-on-channel INVITE → 443: {lines:?}"
        );
        w2.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn invite_nosuch_channel_gets_403() {
    let shared = shared_plain();
    with_client(shared, 55, |mut w, mut r| async move {
        w.write_all(b"NICK n\r\nUSER u 0 * :U\r\nINVITE n #nosuch\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("403"))).await;
        assert!(
            lines.iter().any(|l| l.contains("403")),
            "missing channel INVITE → 403: {lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn isupport_chanmodes_lists_b_and_i() {
    let shared = shared_plain();
    with_client(shared, 56, |mut w, mut r| async move {
        w.write_all(b"NICK n\r\nUSER u 0 * :U\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("CHANMODES="))).await;
        let five = lines
            .iter()
            .find(|l| l.contains("CHANMODES="))
            .expect("005 CHANMODES");
        assert!(
            five.contains("CHANMODES=b,,,nti"),
            "005 must advertise type-A +b and +i: {five}"
        );
    })
    .await;
}
