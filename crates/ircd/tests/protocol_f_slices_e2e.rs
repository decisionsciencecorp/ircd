//! F1c–F4 protocol slices — WHO/WHOIS honesty, query verbs, tags, SASL edges,
//! CHATHISTORY, KILL/WALLOPS.

mod common;

use std::sync::Arc;

use common::{
    read_until, shared_plain, shared_with_history, with_client, with_client_secure,
    with_two_clients,
};
use ircd::state::Shared;
use tempfile::tempdir;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

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
        w.write_all(b"AUTHENTICATE AGFsaWNlAHNlY3JldA==\r\n")
            .await
            .unwrap();
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
async fn f1d_pass_rejects_wrong_password() {
    let mut cfg = common::base_cfg(None);
    cfg.server.password = "s3cret".into();
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 74, |mut w, mut r| async move {
        w.write_all(b"PASS wrong\r\nNICK n\r\nUSER u 0 * :U\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("464"))).await;
        assert!(
            lines.iter().any(|l| l.contains("464")),
            "bad PASS → 464: {lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("001 ")),
            "must not register after bad PASS: {lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn f3_chathistory_after_around_between_targets() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f3b.db");
    let shared = shared_with_history(path);
    with_client(shared, 75, |mut w, mut r| async move {
        w.write_all(
            b"CAP LS\r\nCAP REQ :draft/chathistory batch\r\nNICK a\r\nUSER a 0 * :A\r\nCAP END\r\nJOIN #z\r\n",
        )
        .await
        .unwrap();
        let _ = read_until(&mut r, |l| l.iter().any(|x| x.contains("366"))).await;
        for i in 0..6 {
            w.write_all(format!("PRIVMSG #z :n{i}\r\n").as_bytes())
                .await
                .unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        w.write_all(
            b"CHATHISTORY AFTER #z * 2\r\nCHATHISTORY AROUND #z msgid=dsc3 3\r\nCHATHISTORY BETWEEN #z msgid=dsc1 msgid=dsc4 10\r\nCHATHISTORY TARGETS * 10\r\nQUIT :x\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter().filter(|x| x.contains("BATCH -chathist")).count() >= 3
                || l.iter().any(|x| x.contains("CHATHISTORY TARGETS"))
        })
        .await;
        assert!(
            lines.iter().any(|l| l.contains("PRIVMSG #z") || l.contains("TARGETS")),
            "AFTER/AROUND/BETWEEN/TARGETS produce history: {lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn f2c_sasl_chunk_too_long_905() {
    let shared = shared_plain();
    with_client_secure(shared, 76, |mut w, mut r| async move {
        w.write_all(b"CAP LS\r\nCAP REQ :sasl\r\nAUTHENTICATE PLAIN\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r, |l| l.iter().any(|x| x == "AUTHENTICATE +")).await;
        let long = format!("AUTHENTICATE {}\r\n", "A".repeat(401));
        w.write_all(long.as_bytes()).await.unwrap();
        w.write_all(b"NICK n\r\nUSER u 0 * :U\r\nCAP END\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("905"))).await;
        assert!(
            lines.iter().any(|l| l.contains("905")),
            "AUTHENTICATE >400 → 905: {lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn f4_nonoper_kill_wallops_denied() {
    let shared = shared_plain();
    with_client(shared, 77, |mut w, mut r| async move {
        w.write_all(b"NICK n\r\nUSER u 0 * :U\r\nKILL x :nope\r\nWALLOPS :nope\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter().filter(|x| x.contains("481")).count() >= 2
        })
        .await;
        assert!(
            lines.iter().filter(|l| l.contains("481")).count() >= 2,
            "non-oper KILL/WALLOPS → 481: {lines:?}"
        );
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
        w2.write_all(b"NICK victim\r\nUSER v 0 * :V\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("376"))).await;
        w1.write_all(b"WALLOPS :heads up\r\nKILL victim :bye\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let vlines = read_until(&mut r2, |l| {
            l.iter()
                .any(|x| x.starts_with("ERROR ") || x.contains("KILL"))
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
            olines
                .iter()
                .any(|l| l.contains("WALLOPS") && l.contains("heads up")),
            "oper sees WALLOPS: {olines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn f3_chathistory_errors_and_targets() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f3e.db");
    let shared = shared_with_history(path);
    with_client(shared.clone(), 78, |mut w, mut r| async move {
        w.write_all(
            b"CAP LS\r\nCAP REQ :batch draft/chathistory\r\nNICK a\r\nUSER a 0 * :A\r\nCAP END\r\nJOIN #z\r\n",
        )
        .await
        .unwrap();
        let _ = read_until(&mut r, |l| l.iter().any(|x| x.contains("366"))).await;
        w.write_all(b"PRIVMSG #z :seed\r\n").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        w.write_all(
            b"CHATHISTORY NOPE #z * 1\r\nCHATHISTORY LATEST #other * 1\r\nCHATHISTORY TARGETS * 10\r\nQUIT :x\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter().any(|x| x.contains("CHATHISTORY TARGETS"))
                && l.iter().filter(|x| x.contains("400")).count() >= 1
        })
        .await;
        assert!(
            lines.iter().any(|l| l.contains("400") && l.contains("Invalid")),
            "bad verb → 400: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("442")),
            "not on channel → 442: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("CHATHISTORY TARGETS #z")),
            "TARGETS lists joined channel: {lines:?}"
        );
    })
    .await;

    // History disabled → 400
    let shared = shared_plain();
    with_client(shared, 79, |mut w, mut r| async move {
        w.write_all(b"NICK n\r\nUSER u 0 * :U\r\nCHATHISTORY LATEST #z * 1\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("400"))).await;
        assert!(
            lines.iter().any(|l| l.contains("History is disabled")),
            "no store → 400: {lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn f3_join_auto_replay_without_chathistory_cap() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f3j.db");
    let shared = shared_with_history(path);
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK seed\r\nUSER s 0 * :S\r\nJOIN #r\r\nPRIVMSG #r :old\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("PRIVMSG #r"))).await;
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        // No draft/chathistory CAP → JOIN auto-replays history lines.
        w2.write_all(b"NICK joiner\r\nUSER j 0 * :J\r\nJOIN #r\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r2, |l| {
            l.iter()
                .any(|x| x.contains("PRIVMSG #r") && x.contains("old"))
        })
        .await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("PRIVMSG #r") && l.contains("old")),
            "JOIN auto-replay: {lines:?}"
        );
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn f2c_sasl_abort_and_chunk_continue() {
    let shared = shared_plain();
    with_client_secure(shared.clone(), 80, |mut w, mut r| async move {
        w.write_all(
            b"CAP LS\r\nCAP REQ :sasl\r\nAUTHENTICATE EXTERNAL\r\nAUTHENTICATE PLAIN\r\nAUTHENTICATE *\r\nNICK n\r\nUSER u 0 * :U\r\nCAP END\r\nQUIT :x\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter().any(|x| x.contains("908")) && l.iter().any(|x| x.contains("906"))
        })
        .await;
        assert!(
            lines.iter().any(|l| l.contains("908")),
            "bad mech → 908: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("906")),
            "abort → 906: {lines:?}"
        );
    })
    .await;

    with_client_secure(shared, 81, |mut w, mut r| async move {
        w.write_all(b"CAP LS\r\nCAP REQ :sasl\r\nAUTHENTICATE PLAIN\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r, |l| l.iter().any(|x| x == "AUTHENTICATE +")).await;
        // Exact 400-char chunk (continue) then a final chunk — buf becomes garbage → 904.
        let chunk = "A".repeat(400);
        w.write_all(format!("AUTHENTICATE {chunk}\r\n").as_bytes())
            .await
            .unwrap();
        w.write_all(
            b"AUTHENTICATE AGFsaWNlAHNlY3JldA==\r\nNICK n\r\nUSER u 0 * :U\r\nCAP END\r\nQUIT :x\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter().any(|x| x.contains("903") || x.contains("904"))
        })
        .await;
        assert!(
            lines.iter().any(|l| l.contains("904")),
            "400-chunk then final → 904: {lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn f1d_pass_accepts_correct_password() {
    let mut cfg = common::base_cfg(None);
    cfg.server.password = "s3cret".into();
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 82, |mut w, mut r| async move {
        w.write_all(b"PASS s3cret\r\nNICK n\r\nUSER u 0 * :U\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("001 "))).await;
        assert!(
            lines.iter().any(|l| l.contains("001 ")),
            "good PASS → register: {lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn audit_privmsg_rejects_control_chars() {
    let shared = shared_plain();
    with_client(shared, 83, |mut w, mut r| async move {
        w.write_all(b"NICK n\r\nUSER u 0 * :U\r\nJOIN #c\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r, |l| l.iter().any(|x| x.contains("366"))).await;
        // Embedded CR in trailing — must 461, not fan out / store.
        w.write_all(b"PRIVMSG #c :hi\rinjected\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("461"))).await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("461") && l.contains("Invalid message")),
            "control in PRIVMSG → 461: {lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn audit_history_ads_require_live_store() {
    // cfg.history.enabled defaults true, but no store attached → no CAP/005 history ads.
    let mut cfg = common::base_cfg(None);
    cfg.history.enabled = true;
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 84, |mut w, mut r| async move {
        w.write_all(b"CAP LS\r\nNICK n\r\nUSER u 0 * :U\r\nCAP END\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("001 "))).await;
        let cap_ls = lines
            .iter()
            .find(|l| l.contains("CAP") && l.contains("LS"))
            .cloned();
        assert!(
            cap_ls
                .as_ref()
                .map(|l| !l.contains("draft/chathistory"))
                .unwrap_or(false),
            "no live store → no draft/chathistory: {cap_ls:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("CHATHISTORY=")),
            "no live store → no 005 CHATHISTORY=: {lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn audit_around_star_and_userhost_away() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("aud.db");
    let shared = shared_with_history(path);
    with_client(shared.clone(), 85, |mut w, mut r| async move {
        w.write_all(
            b"CAP LS\r\nCAP REQ :draft/chathistory batch\r\nNICK a\r\nUSER a 0 * :A\r\nCAP END\r\nJOIN #z\r\nPRIVMSG #z :x\r\nCHATHISTORY AROUND #z * 3\r\nQUIT :x\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("400"))).await;
        assert!(
            lines.iter().any(|l| l.contains("400") && l.contains("Invalid")),
            "AROUND * → 400: {lines:?}"
        );
    })
    .await;

    let shared = shared_plain();
    with_client(shared, 86, |mut w, mut r| async move {
        w.write_all(b"NICK n\r\nUSER u 0 * :U\r\nAWAY :brb\r\nUSERHOST n\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("302"))).await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("302") && l.contains("n=-u@")),
            "away USERHOST → =-: {lines:?}"
        );
    })
    .await;
}
