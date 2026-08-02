mod common;

use common::{read_until, shared_plain, with_client, with_two_clients};
use ircd::config::OperSection;
use ircd::state::Shared;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

#[tokio::test(flavor = "multi_thread")]
async fn mode_minus_o_and_unknown_and_nick_change() {
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"NICK op\r\nUSER o 0 * :O\r\nJOIN #m\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("366"))).await;
        w2.write_all(b"NICK other\r\nUSER x 0 * :X\r\nJOIN #m\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("366"))).await;
        w1.write_all(b"MODE #m +o other\r\nMODE #m -o other\r\nMODE #m +z\r\nNICK op2\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r1, |l| l.iter().any(|x| x.contains("472"))).await;
        assert!(lines.iter().any(|l| l.contains("MODE")));
        assert!(lines.iter().any(|l| l.contains("472")));
        w2.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn oper_disabled_and_authenticate_abort() {
    let mut cfg = common::base_cfg(None);
    cfg.oper = OperSection {
        enabled: false,
        name: String::new(),
        password: String::new(),
    };
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
    with_client(shared, 11, |mut w, mut r| async move {
        w.write_all(b"CAP LS\r\nNICK n\r\nUSER u 0 * :U\r\nCAP REQ :sasl\r\nAUTHENTICATE PLAIN\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r, |l| l.iter().any(|x| x == "AUTHENTICATE +")).await;
        w.write_all(b"AUTHENTICATE *\r\nCAP END\r\nOPER admin x\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter().any(|x| x.contains("906")) || l.iter().any(|x| x.contains("491"))
        })
        .await;
        assert!(lines.iter().any(|l| l.contains("906") || l.contains("491")));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rejoin_and_topic_get_set() {
    let shared = shared_plain();
    with_client(shared, 12, |mut w, mut r| async move {
        w.write_all(
            b"NICK t\r\nUSER t 0 * :T\r\nJOIN #top\r\nJOIN #top\r\nTOPIC #top :hi\r\nTOPIC #top\r\nQUIT :x\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("332"))).await;
        assert!(lines.iter().any(|l| l.contains("hi")));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mode_tn_cap_disable_bad_b64() {
    let shared = shared_plain();
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        w1.write_all(b"CAP LS\r\nNICK op\r\nUSER o 0 * :O\r\nCAP REQ :server-time\r\nCAP END\r\nJOIN #z\r\nMODE #z -t\r\nMODE #z -n\r\nMODE #z +t\r\nMODE #z +n\r\nCAP REQ :-server-time\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("ACK") && x.contains("-server-time"))).await;
        w2.write_all(b"CAP LS\r\nNICK out\r\nUSER u 0 * :U\r\nCAP REQ :sasl\r\nAUTHENTICATE PLAIN\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x == "AUTHENTICATE +")).await;
        w2.write_all(b"AUTHENTICATE !!!bad!!\r\nCAP END\r\nJOIN #z\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r2, |l| l.iter().any(|x| x.contains("904"))).await;
        assert!(lines.iter().any(|l| l.contains("904")));
        w1.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn kick_non_member_and_topic_no_chan() {
    let shared = shared_plain();
    with_client(shared, 13, |mut w, mut r| async move {
        w.write_all(b"NICK k\r\nUSER k 0 * :K\r\nJOIN #k\r\nKICK #k nobody :x\r\nTOPIC #missing\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter().any(|x| x.contains("441") || x.contains("442") || x.contains("403"))
        })
        .await;
        assert!(!lines.is_empty());
    })
    .await;
}
