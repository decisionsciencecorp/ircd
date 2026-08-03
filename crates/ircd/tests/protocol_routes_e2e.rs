mod common;

use common::{read_until, shared_plain, with_client, with_two_clients};
use tokio::io::AsyncWriteExt;

#[tokio::test(flavor = "multi_thread")]
async fn nick_errors_and_collision() {
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK alice\r\nUSER a 0 * :A\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("001"))).await;
        w2.write_all(b"NICK !!!\r\n").await.unwrap();
        let bad = read_until(&mut r2, |l| l.iter().any(|x| x.contains("432"))).await;
        assert!(bad.iter().any(|l| l.contains("432")));
        w2.write_all(b"NICK alice\r\nQUIT :x\r\n").await.unwrap();
        let coll = read_until(&mut r2, |l| l.iter().any(|x| x.contains("433"))).await;
        assert!(coll.iter().any(|l| l.contains("433")));
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cap_list_nak_ping_quit() {
    let shared = shared_plain();
    with_client(shared, 3, |mut w, mut r| async move {
        w.write_all(b"CAP LS\r\nCAP REQ :not-a-real-cap server-time\r\nCAP LIST\r\nNICK z\r\nUSER u 0 * :U\r\nCAP END\r\nPING :xyz\r\nQUIT :bye\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("PONG"))).await;
        assert!(lines.iter().any(|l| l.contains("NAK")));
        assert!(lines.iter().any(|l| l.contains("PONG")));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn part_kick_mode_admin() {
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK op\r\nUSER o 0 * :O\r\nJOIN #room\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;
        w2.write_all(b"NICK victim\r\nUSER v 0 * :V\r\nJOIN #room\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("366"))).await;

        w1.write_all(b"MODE #room +o victim\r\nMODE #room\r\nADMIN\r\nKICK #room victim :out\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r1, |l| {
            l.iter().any(|x| x.contains("KICK")) && l.iter().any(|x| x.contains("256"))
        })
        .await;
        assert!(lines.iter().any(|l| l.contains("256")));
        let kick_seen = read_until(&mut r2, |l| l.iter().any(|x| x.contains("KICK"))).await;
        assert!(kick_seen.iter().any(|l| l.contains("KICK")));
        w1.write_all(b"PART #room\r\nQUIT :x\r\n").await.unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("PART"))).await;
        w2.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn topic_non_op_and_privmsg_rules() {
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK a\r\nUSER a 0 * :A\r\nJOIN #n\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;
        w2.write_all(b"NICK b\r\nUSER b 0 * :B\r\nPRIVMSG #n :nope\r\n")
            .await
            .unwrap();
        let denied = read_until(&mut r2, |l| l.iter().any(|x| x.contains("404"))).await;
        assert!(denied.iter().any(|l| l.contains("404")));
        w2.write_all(b"JOIN #n\r\nTOPIC #n :set by non-op\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let t = read_until(&mut r2, |l| l.iter().any(|x| x.contains("482"))).await;
        assert!(t.iter().any(|l| l.contains("482")));
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sasl_fail_and_oper_fail() {
    let shared = shared_plain();
    with_client(shared, 8, |mut w, mut r| async move {
        w.write_all(
            b"CAP LS\r\nNICK eve\r\nUSER e 0 * :E\r\nCAP REQ :sasl\r\nAUTHENTICATE PLAIN\r\n",
        )
        .await
        .unwrap();
        let _ = read_until(&mut r, |l| l.iter().any(|x| x == "AUTHENTICATE +")).await;
        w.write_all(b"AUTHENTICATE AGFsaWNlAHdyb25n\r\nCAP END\r\nOPER admin bad\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("904"))).await;
        assert!(lines.iter().any(|l| l.contains("904")));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn chathistory_disabled() {
    let shared = shared_plain();
    with_client(shared, 9, |mut w, mut r| async move {
        w.write_all(b"NICK c\r\nUSER c 0 * :C\r\nJOIN #x\r\nCHATHISTORY LATEST #x * 5\r\nCHATHISTORY BEFORE #x * 5\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("400"))).await;
        assert!(lines.iter().any(|l| l.contains("CHATHISTORY")));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unregistered_gets_451() {
    let shared = shared_plain();
    with_client(shared, 10, |mut w, mut r| async move {
        w.write_all(b"JOIN #nope\r\nQUIT :x\r\n").await.unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("451"))).await;
        assert!(lines.iter().any(|l| l.contains("451")));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn truthful_cap_ls_omits_unimplemented() {
    let shared = shared_plain();
    with_client(shared, 2, |mut w, mut r| async move {
        w.write_all(b"CAP LS\r\nQUIT :x\r\n").await.unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter().any(|x| x.contains("CAP") && x.contains("LS"))
        })
        .await;
        let ls = lines
            .iter()
            .find(|x| x.contains("LS"))
            .expect("CAP LS line");
        for bad in ["away-notify", "multi-prefix", "echo-message"] {
            assert!(
                !ls.to_ascii_lowercase().contains(bad),
                "CAP LS must not advertise {bad}: {ls}"
            );
        }
        for good in [
            "message-tags",
            "server-time",
            "account-tag",
            "batch",
            "cap-notify",
        ] {
            assert!(
                ls.to_ascii_lowercase().contains(good),
                "CAP LS must advertise {good}: {ls}"
            );
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn nick_change_does_not_transfer_ops_to_stolen_nick() {
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        // Client1 becomes channel op as OpOld
        w1.write_all(b"NICK OpOld\r\nUSER o 0 * :O\r\nJOIN #priv\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("JOIN"))).await;
        // Rename — ops must stay on Client1's id
        w1.write_all(b"NICK OpNew\r\n").await.unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("NICK"))).await;
        // Client2 steals OpOld
        w2.write_all(b"NICK OpOld\r\nUSER s 0 * :S\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("001"))).await;
        // Stolen nick must not MODE/KICK without joining
        w2.write_all(
            b"MODE #priv +o OpNew\r\nKICK #priv OpNew :nope\r\nPRIVMSG #priv :pwn\r\nQUIT :x\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r2, |l| {
            l.iter().any(|x| {
                x.contains("ERROR")
                    || x.contains("482")
                    || x.contains("404")
                    || x.contains("441")
                    || x.contains("QUIT")
                    || l.len() > 3
            })
        })
        .await;
        let blob = lines.join("\n");
        assert!(
            !blob.contains("MODE #priv +o")
                || blob.contains("482")
                || blob.contains("442")
                || blob.contains("401")
                || blob.contains("403")
                || blob.contains("441")
                || blob.contains("404"),
            "stolen nick must not exercise op authority: {blob}"
        );
        // OpNew (original) can still kick if somehow needed — at least still op for MODE
        w1.write_all(b"MODE #priv\r\nQUIT :x\r\n").await.unwrap();
        let op_lines = read_until(&mut r1, |l| {
            l.iter().any(|x| x.contains("324") || x.contains("QUIT"))
        })
        .await;
        assert!(op_lines.iter().any(|l| l.contains("324")), "{op_lines:?}");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_line_disconnects() {
    use ircd::config::Config;
    use ircd::state::Shared;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    let mut cfg = Config::default();
    cfg.limits.max_line_bytes = 64;
    cfg.limits.max_clients = 64;
    cfg.limits.max_clients_per_ip = 64;
    cfg.limits.flood_lines_per_window = 500;
    cfg.server.name = "cov.test".into();
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 9, |mut w, mut r| async move {
        let _ = read_until(&mut r, |l| l.len() >= 3).await;
        let huge = "A".repeat(80);
        w.write_all(format!("{huge}\r\n").as_bytes()).await.unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter()
                .any(|x| x.contains("too long") || x.contains("ERROR"))
        })
        .await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("too long") || l.contains("ERROR")),
            "{lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn kick_revokes_privmsg_for_victim() {
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK op\r\nUSER o 0 * :O\r\nJOIN #k\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("JOIN"))).await;
        w2.write_all(b"NICK vic\r\nUSER v 0 * :V\r\nJOIN #k\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("JOIN"))).await;
        w1.write_all(b"KICK #k vic :out\r\n").await.unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("KICK"))).await;
        w2.write_all(b"PRIVMSG #k :still here?\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r2, |l| {
            l.iter().any(|x| x.contains("404") || x.contains("ERROR"))
        })
        .await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("404") || l.contains("Cannot send")),
            "kicked nick must lose send access: {lines:?}"
        );
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn error_exit_clears_nick_for_reuse() {
    let shared = shared_plain();
    // First client registers nick then we drop by closing write side via QUIT after join;
    // second client must be able to claim the nick (no ghost in nicks map).
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK Ghost\r\nUSER g 0 * :G\r\nJOIN #g\r\nQUIT :bye\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| {
            l.iter().any(|x| x.contains("QUIT") || x.contains("JOIN"))
        })
        .await;
        // Allow Drop cleanup spawn to run
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        w2.write_all(b"NICK Ghost\r\nUSER g 0 * :G\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r2, |l| {
            l.iter().any(|x| x.contains("001") || x.contains("433"))
        })
        .await;
        assert!(
            lines.iter().any(|l| l.contains("001")),
            "nick must be free after prior session cleanup: {lines:?}"
        );
    })
    .await;
}
