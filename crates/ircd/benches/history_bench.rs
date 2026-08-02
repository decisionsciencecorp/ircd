//! Criterion benches for HistoryStore append/latest (CHATHISTORY hot path).

use criterion::{criterion_group, criterion_main, Criterion};
use ircd::history::HistoryStore;
use tempfile::tempdir;

fn history_append_latest(c: &mut Criterion) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bench.sqlite3");
    let store = HistoryStore::open(&path, 1000).unwrap();
    for i in 0..200 {
        store
            .append("#bench", "n!u@h", &format!("seed {i}"))
            .unwrap();
    }

    c.bench_function("history_append", |b| {
        let mut i = 0u64;
        b.iter(|| {
            i += 1;
            store
                .append("#bench", "n!u@h", &format!("msg {i}"))
                .unwrap();
        });
    });

    c.bench_function("history_latest_50", |b| {
        b.iter(|| store.latest("#bench", 50).unwrap());
    });
}

criterion_group!(benches, history_append_latest);
criterion_main!(benches);
