//! C8 — MODE multi-arg consumption (`+oo nick1 nick2`).

mod common;

use common::{read_until, shared_plain, with_three_clients};
use tokio::io::AsyncWriteExt;

#[tokio::test(flavor = "multi_thread")]
async fn mode_plus_oo_applies_distinct_nicks() {
    let shared = shared_plain();
    with_three_clients(
        shared,
        |(mut w_op, mut r_op), (mut w_a, mut r_a), (mut w_b, mut r_b)| async move {
            w_op.write_all(b"NICK op\r\nUSER o 0 * :O\r\nJOIN #m\r\n")
                .await
                .unwrap();
            let _ = read_until(&mut r_op, |l| l.iter().any(|x| x.contains("366"))).await;

            w_a.write_all(b"NICK a\r\nUSER u 0 * :U\r\nJOIN #m\r\n")
                .await
                .unwrap();
            let _ = read_until(&mut r_a, |l| l.iter().any(|x| x.contains("366"))).await;
            let _ = read_until(&mut r_op, |l| l.iter().any(|x| x.contains("JOIN"))).await;

            w_b.write_all(b"NICK b\r\nUSER u 0 * :U\r\nJOIN #m\r\n")
                .await
                .unwrap();
            let _ = read_until(&mut r_b, |l| l.iter().any(|x| x.contains("366"))).await;
            let _ = read_until(&mut r_op, |l| l.iter().any(|x| x.contains("JOIN"))).await;

            // Q1: +oo a b must op both distinct nicks (not reuse first arg).
            w_op.write_all(b"MODE #m +oo a b\r\n").await.unwrap();
            let op_lines = read_until(&mut r_op, |l| {
                l.iter()
                    .any(|x| x.contains("MODE #m") && x.contains('a') && x.contains('b'))
            })
            .await;
            assert!(
                op_lines.iter().any(|l| {
                    l.contains("MODE #m")
                        && l.contains("+oo")
                        && l.contains(" a")
                        && l.contains(" b")
                        && !l.contains(" a a")
                }),
                "want MODE +oo a b, got {op_lines:?}"
            );
            let a_see = read_until(&mut r_a, |l| {
                l.iter().any(|x| x.contains("MODE #m") && x.contains("+oo"))
            })
            .await;
            assert!(
                a_see.iter().any(|l| l.contains(" a") && l.contains(" b")),
                "peer must see both targets: {a_see:?}"
            );

            // Q2: +o-o advances across sign change — grant a (already op), remove b.
            w_op.write_all(b"MODE #m +o-o a b\r\n").await.unwrap();
            let mix = read_until(&mut r_op, |l| {
                l.iter()
                    .any(|x| x.contains("MODE #m") && x.contains("-o") && x.contains('b'))
            })
            .await;
            assert!(
                mix.iter().any(|l| {
                    l.contains("MODE #m")
                        && (l.contains("+o-o") || (l.contains("+o") && l.contains("-o")))
                        && l.contains('a')
                        && l.contains('b')
                }),
                "want +o-o a b style MODE, got {mix:?}"
            );

            // Q3: de-opped target must not MODE; single-arg +o must not invent a
            // second nick (arg-reuse privilege bug).
            w_b.write_all(b"MODE #m +n\r\n").await.unwrap();
            let denied = read_until(&mut r_b, |l| l.iter().any(|x| x.contains("482"))).await;
            assert!(
                denied.iter().any(|l| l.contains("482")),
                "de-opped b must not change modes: {denied:?}"
            );

            w_op.write_all(b"MODE #m +o a\r\n").await.unwrap();
            let single = read_until(&mut r_op, |l| {
                l.iter()
                    .any(|x| x.contains("MODE #m") && x.contains("+o") && x.contains('a'))
            })
            .await;
            assert!(
                single
                    .iter()
                    .any(|l| l.contains("MODE #m +o a") && !l.contains(" b")),
                "single +o a must not also name b: {single:?}"
            );

            w_op.write_all(b"QUIT :x\r\n").await.unwrap();
            w_a.write_all(b"QUIT :x\r\n").await.unwrap();
            w_b.write_all(b"QUIT :x\r\n").await.unwrap();
        },
    )
    .await;
}
