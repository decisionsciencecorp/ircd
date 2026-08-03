//! C10 — short global-lock snapshots for NAMES/KICK.

mod common;

use common::{read_until, shared_plain, with_two_clients};
use tokio::io::AsyncWriteExt;

#[tokio::test(flavor = "multi_thread")]
async fn kick_fanout_and_authz_unchanged() {
    // Q2: op can kick; victim sees KICK; de-opped cannot kick; membership revoked.
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK op\r\nUSER o 0 * :O\r\nJOIN #c10\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;
        w2.write_all(b"NICK vic\r\nUSER v 0 * :V\r\nJOIN #c10\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("366"))).await;
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("JOIN"))).await;

        w1.write_all(b"KICK #c10 vic :out\r\n").await.unwrap();
        let v = read_until(&mut r2, |l| l.iter().any(|x| x.contains("KICK"))).await;
        assert!(
            v.iter().any(|l| l.contains("KICK #c10 vic")),
            "victim must see KICK fanout: {v:?}"
        );

        w2.write_all(b"PRIVMSG #c10 :nope\r\n").await.unwrap();
        let denied = read_until(&mut r2, |l| l.iter().any(|x| x.contains("404"))).await;
        assert!(denied.iter().any(|l| l.contains("404")), "{denied:?}");

        // Non-op cannot kick (rejoin as new nick without ops).
        w2.write_all(b"NICK vic2\r\nJOIN #c10\r\n").await.unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("366"))).await;
        w2.write_all(b"KICK #c10 op :no\r\n").await.unwrap();
        let nop = read_until(&mut r2, |l| l.iter().any(|x| x.contains("482"))).await;
        assert!(nop.iter().any(|l| l.contains("482")), "{nop:?}");

        w1.write_all(b"QUIT :x\r\n").await.unwrap();
        w2.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn names_after_join_lists_both() {
    // Q1: NAMES wire still correct after thin-snapshot refactor.
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK a\r\nUSER a 0 * :A\r\nJOIN #n\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;
        w2.write_all(b"NICK b\r\nUSER b 0 * :B\r\nJOIN #n\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r2, |l| l.iter().any(|x| x.contains("366"))).await;
        let names = lines
            .iter()
            .filter(|l| l.contains("353"))
            .cloned()
            .collect::<Vec<_>>();
        let blob = names.join(" ");
        assert!(blob.contains('a') && blob.contains('b'), "{names:?}");

        w1.write_all(b"NAMES #n\r\n").await.unwrap();
        let n = read_until(&mut r1, |l| {
            l.iter().any(|x| x.contains("366") && x.contains("#n"))
        })
        .await;
        assert!(
            n.iter().any(|l| l.contains("353") && l.contains('b')),
            "{n:?}"
        );
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
        w2.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[test]
fn c10_call_sites_format_after_lock() {
    // Q1/Q2 code-path proof: session must convert NamesThin → snapshot / fanout
    // off the Shared guard (not names_snapshot inside the JOIN lock block).
    let src = include_str!("../src/session/mod.rs");
    assert!(
        src.contains("into_names_snapshot()"),
        "JOIN/NAMES must format via into_names_snapshot after thin copy"
    );
    assert!(
        src.contains("clone_outboxes_for"),
        "KICK must clone outboxes for off-lock fanout"
    );
    assert!(
        src.contains("names_thin("),
        "must use names_thin under lock"
    );
    // After the JOIN lock closes (`};` on join_result), into_names_snapshot runs.
    let marker = "let (names_thin, topic, away_join) = match join_result";
    let idx = src.find(marker).expect("join_result match");
    let after = &src[idx..idx + 8000];
    assert!(
        after.contains("into_names_snapshot()"),
        "format must follow join_result match (lock already dropped)"
    );
    assert!(
        !after[..after.find("into_names_snapshot()").unwrap()].contains("shared.lock()"),
        "no re-lock between join_result and into_names_snapshot"
    );
}
