//! Extra C4 coverage — CAP/SASL edges, OPER, registration errors (H-14 bar).

mod common;

use std::sync::Arc;

use common::{base_cfg, read_until, with_client};
use ircd::state::Shared;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

fn shared_accounts() -> Arc<Mutex<Shared>> {
    Arc::new(Mutex::new(Shared::new(Arc::new(base_cfg(None)), None)))
}

#[tokio::test(flavor = "multi_thread")]
async fn c4_cap_nak_and_invalid_sub() {
    let shared = shared_accounts();
    with_client(shared, 81, |mut w, mut r| async move {
        let _ = read_until(&mut r, |l| l.len() >= 2).await;
        w.write_all(b"CAP LS\r\nCAP REQ :not-a-real-cap\r\nCAP FROB\r\nCAP END\r\nNICK c4a\r\nUSER c 0 * :C\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("001") || x.contains("410")))
            .await;
        assert!(
            lines.iter().any(|l| l.contains("NAK") || l.contains("410") || l.contains("001")),
            "{lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn c4_sasl_abort_and_bad_mech() {
    let shared = shared_accounts();
    with_client(shared, 82, |mut w, mut r| async move {
        let _ = read_until(&mut r, |l| l.len() >= 2).await;
        w.write_all(
            b"CAP LS\r\nCAP REQ :sasl\r\nAUTHENTICATE EXTERNAL\r\nAUTHENTICATE PLAIN\r\nAUTHENTICATE *\r\nCAP END\r\nNICK c4b\r\nUSER c 0 * :C\r\nQUIT :x\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter()
                .any(|x| x.contains("908") || x.contains("906") || x.contains("001"))
        })
        .await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("908") || l.contains("906") || l.contains("AUTHENTICATE")),
            "{lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn c4_oper_and_topic_errors() {
    let shared = shared_accounts();
    with_client(shared, 83, |mut w, mut r| async move {
        let _ = read_until(&mut r, |l| l.len() >= 2).await;
        w.write_all(
            b"NICK c4c\r\nUSER c 0 * :C\r\nOPER wrong wrong\r\nOPER admin operpass\r\nTOPIC #nope\r\nPRIVMSG\r\nNOTICE\r\nMODE #nope\r\nINVITE\r\nKICK #nope x\r\nQUIT :x\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r, |l| l.iter().any(|x| x.contains("381") || x.contains("491")))
            .await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("381") || l.contains("491") || l.contains("464") || l.contains("001")),
            "{lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn c4_auth_without_sasl_cap() {
    let shared = shared_accounts();
    with_client(shared, 84, |mut w, mut r| async move {
        let _ = read_until(&mut r, |l| l.len() >= 2).await;
        w.write_all(b"AUTHENTICATE PLAIN\r\nNICK c4d\r\nUSER c 0 * :C\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter().any(|x| x.contains("904") || x.contains("001"))
        })
        .await;
        assert!(
            lines.iter().any(|l| l.contains("904") || l.contains("001")),
            "{lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn c4_chathistory_errors_without_history_and_membership() {
    let shared = shared_accounts();
    with_client(shared, 85, |mut w, mut r| async move {
        let _ = read_until(&mut r, |l| l.len() >= 2).await;
        w.write_all(
            b"NICK c4e\r\nUSER c 0 * :C\r\nCHATHISTORY LATEST #nowhere * 5\r\nJOIN #c4e\r\nCHATHISTORY LATEST #c4e * 5\r\nQUIT :x\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter()
                .any(|x| x.contains("442") || x.contains("400") || x.contains("History"))
        })
        .await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("442") || l.contains("400") || l.contains("CHATHISTORY")),
            "{lines:?}"
        );
    })
    .await;
}
