//! C7 — TAGMSG, client-only +tag relay, CHATHISTORY ISUPPORT.

mod common;

use common::{read_until, shared_plain, with_client, with_two_clients};
use tokio::io::AsyncWriteExt;

#[tokio::test(flavor = "multi_thread")]
async fn tagmsg_relays_client_plus_tag_with_message_tags() {
    // Q1: both ends with message-tags → TAGMSG + client tag preserved.
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"CAP REQ :message-tags\r\nCAP END\r\nNICK a\r\nUSER a 0 * :A\r\nJOIN #t\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;
        w2.write_all(b"CAP REQ :message-tags\r\nCAP END\r\nNICK b\r\nUSER b 0 * :B\r\nJOIN #t\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("366"))).await;
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("JOIN"))).await;

        w1.write_all(b"@+foo=bar TAGMSG #t :\r\n").await.unwrap();
        let lines = read_until(&mut r2, |l| {
            l.iter()
                .any(|x| x.contains("TAGMSG") && x.contains("+foo=bar"))
        })
        .await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("TAGMSG") && l.contains("+foo=bar")),
            "{lines:?}"
        );
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
        w2.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tagmsg_without_message_tags_is_421() {
    // Q2: no message-tags → standard error, not silent drop.
    let shared = shared_plain();
    with_client(shared, 70, |mut w, mut r| async move {
        w.write_all(b"NICK n\r\nUSER u 0 * :U\r\nTAGMSG #x :\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("421"))).await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("421") && l.contains("TAGMSG")),
            "{lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn client_plus_tag_on_privmsg_preserved() {
    // Q3: @+… PRIVMSG fanout keeps client tag when both have message-tags.
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"CAP REQ :message-tags\r\nCAP END\r\nNICK a\r\nUSER a 0 * :A\r\nJOIN #p\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;
        w2.write_all(b"CAP REQ :message-tags\r\nCAP END\r\nNICK b\r\nUSER b 0 * :B\r\nJOIN #p\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("366"))).await;
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("JOIN"))).await;

        w1.write_all(b"@+draft/reply=xyz PRIVMSG #p :hi\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r2, |l| {
            l.iter()
                .any(|x| x.contains("PRIVMSG") && x.contains("+draft/reply=xyz"))
        })
        .await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("PRIVMSG") && l.contains("+draft/reply")),
            "{lines:?}"
        );
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
        w2.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn isupport_advertises_chathistory_when_enabled() {
    // Q4: 005 CHATHISTORY=<limit> + MSGREFTYPES when history on (F3).
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let path = dir.path().join("h.sqlite3");
    let shared = common::shared_with_history(path);
    with_client(shared, 71, |mut w, mut r| async move {
        w.write_all(b"NICK h\r\nUSER u 0 * :U\r\n").await.unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("005"))).await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("005") && l.contains("CHATHISTORY=200")),
            "want CHATHISTORY=200 matching latest clamp: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("MSGREFTYPES=msgid,timestamp")),
            "MSGREFTYPES required when history on: {lines:?}"
        );
        w.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}
