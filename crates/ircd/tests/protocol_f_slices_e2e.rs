//! F1c–F4 protocol slices — WHO/WHOIS honesty, query verbs, tags, SASL edges,
//! CHATHISTORY, KILL/WALLOPS.

mod common;

use common::{
    read_until, shared_plain, shared_with_history, with_client, with_client_secure, with_two_clients,
};
use tempfile::tempdir;
use tokio::io::AsyncWriteExt;

#[tokio::test(flavor = "multi_thread")]
async fn f1c_no_whox_in_isupport() {
    let shared = shared_plain();
    with_client(shared, 70, |mut w, mut r| async move {
        w.write_all(b"NICK n\r\nUSER u 0 * :U\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("CHANMODES="))).await;
        let five = lines
            .iter()
            .find(|l| l.contains("CHANMODES="))
            .expect("005");
        assert!(
            !five.split_whitespace().any(|t| t == "WHOX"),
            "WHOX must not be advertised: {five}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn f1c_whois_oper_and_list_names() {
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK op\r\nUSER op 0 * :O\r\nOPER admin operpass\r\nJOIN #q\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("381"))).await;
        w2.write_all(b"NICK peer\r\nUSER peer 0 * :P\r\nJOIN #q\r\nWHOIS op\r\nLIST #q\r\nNAMES #q\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r2, |l| {
            l.iter().any(|x| x.contains("318")) && l.iter().any(|x| x.contains("323"))
        })
        .await;
        assert!(
            lines.iter().any(|l| l.contains("313")),
            "WHOIS oper → 313: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("322") && l.contains("#q")),
            "LIST 322: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("353")),
            "NAMES 353: {lines:?}"
        );
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn f1d_userhost_ison_time_info() {
    let shared = shared_plain();
    with_client(shared, 71, |mut w, mut r| async move {
        w.write_all(
            b"NICK n\r\nUSER u 0 * :U\r\nUSERHOST n\r\nISON n missing\r\nTIME\r\nINFO\r\nQUIT :x\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter().any(|x| x.contains("302"))
                && l.iter().any(|x| x.contains("303"))
                && l.iter().any(|x| x.contains("391"))
                && l.iter().any(|x| x.contains("374"))
        })
        .await;
        assert!(lines.iter().any(|l| l.contains("302") && l.contains("n=+u@")));
        assert!(lines.iter().any(|l| l.contains("303") && l.contains("n")));
        assert!(lines.iter().any(|l| l.contains("391")));
        assert!(lines.iter().any(|l| l.contains("371")));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn f2a_tag_block_too_large_gets_417() {
    let shared = shared_plain();
    with_client(shared, 72, |mut w, mut r| async move {
        w.write_all(b"NICK n\r\nUSER u 0 * :U\r\n").await.unwrap();
        let _ = read_until(&mut r, |l| l.iter().any(|x| x.contains("376"))).await;
        let huge = format!("@{} PRIVMSG n :x\r\n", "a".repeat(5000));
        w.write_all(huge.as_bytes()).await.unwrap();
        w.write_all(b"QUIT :x\r\n").await.unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("417"))).await;
        assert!(
            lines.iter().any(|l| l.contains("417")),
            "oversized tag block → 417: {lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn f2c_sasl_reauth_gets_907() {
    let shared = shared_plain();
    with_client_secure(shared, 73, |mut w, mut r| async move {
        w.write_all(b"CAP LS\r\nCAP REQ :sasl\r\nAUTHENTICATE PLAIN\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r, |l| l.iter().any(|x| x == "AUTHENTICATE +")).await;
        // alice\0alice\0secret
        w.write_all(b"AUTHENTICATE AGFsaWNlAHNlY3JldA==\r\n").await.unwrap();
        let _ = read_until(&mut r, |l| l.iter().any(|x| x.contains("903"))).await;
        w.write_all(b"AUTHENTICATE PLAIN\r\nNICK n\r\nUSER u 0 * :U\r\nCAP END\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("907"))).await;
        assert!(
            lines.iter().any(|l| l.contains("907")),
            "already authenticated → 907: {lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn f3_chathistory_before_and_msgreftypes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f3.db");
    let shared = shared_with_history(path);
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(
            b"CAP LS\r\nCAP REQ :draft/chathistory batch\r\nNICK a\r\nUSER a 0 * :A\r\nCAP END\r\nJOIN #h\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r1, |l| l.iter().any(|x| x.contains("MSGREFTYPES"))).await;
        assert!(
            lines.iter().any(|l| l.contains("MSGREFTYPES=msgid,timestamp")),
            "005 MSGREFTYPES: {lines:?}"
        );
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;
        for i in 0..5 {
            w1.write_all(format!("PRIVMSG #h :m{i}\r\n").as_bytes())
                .await
                .unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        w2.write_all(
            b"CAP LS\r\nCAP REQ :draft/chathistory batch\r\nNICK b\r\nUSER b 0 * :B\r\nCAP END\r\nJOIN #h\r\n",
        )
        .await
        .unwrap();
        let join_lines = read_until(&mut r2, |l| l.iter().any(|x| x.contains("366"))).await;
        // With draft/chathistory, JOIN must not auto-replay PRIVMSG history.
        assert!(
            !join_lines.iter().any(|l| l.contains("PRIVMSG #h")),
            "JOIN auto-replay suppressed: {join_lines:?}"
        );
        w2.write_all(b"CHATHISTORY LATEST #h * 3\r\n").await.unwrap();
        let hist = read_until(&mut r2, |l| {
            l.iter().any(|x| x.contains("BATCH -chathist"))
                || l.iter().filter(|x| x.contains("PRIVMSG #h")).count() >= 1
        })
        .await;
        assert!(
            hist.iter().any(|l| l.contains("PRIVMSG #h")),
            "LATEST returns history: {hist:?}"
        );
        w2.write_all(b"CHATHISTORY BEFORE #h * 2\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("BATCH -") || x.contains("PRIVMSG"))).await;
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn f4_kill_and_wallops() {
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK op\r\nUSER op 0 * :O\r\nOPER admin operpass\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("381"))).await;
        w2.write_all(b"NICK victim\r\nUSER v 0 * :V\r\n").await.unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("376"))).await;
        w1.write_all(b"WALLOPS :heads up\r\nKILL victim :bye\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let vlines = read_until(&mut r2, |l| {
            l.iter().any(|x| x.starts_with("ERROR ") || x.contains("KILL"))
        })
        .await;
        assert!(
            vlines
                .iter()
                .any(|l| l.contains("KILL") || l.starts_with("ERROR ")),
            "victim sees KILL/ERROR: {vlines:?}"
        );
        let olines = read_until(&mut r1, |l| l.iter().any(|x| x.contains("WALLOPS"))).await;
        assert!(
            olines.iter().any(|l| l.contains("WALLOPS") && l.contains("heads up")),
            "oper sees WALLOPS: {olines:?}"
        );
    })
    .await;
}
