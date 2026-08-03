//! Criterion benches for member-targeted fanout (H-07/H-08).
//!
//! Reports delivery cost for 1 / 10 / 100 / 1000 recipients with bounded outboxes.

use std::sync::Arc;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use ircd::config::Config;
use ircd::state::{ChannelState, Shared, OUTBOX_CAP};
use tokio::sync::mpsc;

fn setup_channel(n: usize) -> (Shared, Vec<mpsc::Receiver<std::sync::Arc<str>>>) {
    let mut s = Shared::new(Arc::new(Config::default()), None);
    let mut rxs = Vec::with_capacity(n);
    let mut ch = ChannelState::default();
    for i in 1..=n as u64 {
        let (tx, rx) = mpsc::channel::<std::sync::Arc<str>>(OUTBOX_CAP);
        s.register_outbox(i, tx);
        ch.members.insert(i);
        rxs.push(rx);
    }
    *s.channel_or_default("#bench".into()) = ch;
    (s, rxs)
}

fn drain_all(rxs: &mut [mpsc::Receiver<std::sync::Arc<str>>]) {
    for rx in rxs.iter_mut() {
        while rx.try_recv().is_ok() {}
    }
}

fn fanout_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("fanout_channel");
    group.warm_up_time(Duration::from_millis(300));
    group.measurement_time(Duration::from_secs(2));
    let line = ":a!b@c PRIVMSG #bench :hello fanout\r\n";

    for n in [1usize, 10, 100, 1000] {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let (shared, mut rxs) = setup_channel(n);
            b.iter(|| {
                let st = shared.fanout_channel("#bench", line, 0);
                assert_eq!(st.delivered, n);
                drain_all(&mut rxs);
            });
        });
    }
    group.finish();

    c.bench_function("fanout_slow_consumer_policy", |b| {
        let mut s = Shared::new(Arc::new(Config::default()), None);
        let (tx_fast, mut rx_fast) = mpsc::channel::<std::sync::Arc<str>>(64);
        let (tx_slow, mut rx_slow) = mpsc::channel::<std::sync::Arc<str>>(1);
        s.register_outbox(1, tx_fast);
        s.register_outbox(2, tx_slow);
        let mut ch = ChannelState::default();
        ch.members.insert(1);
        ch.members.insert(2);
        *s.channel_or_default("#bench".into()) = ch;
        b.iter(|| {
            while rx_slow.try_recv().is_ok() {}
            while rx_fast.try_recv().is_ok() {}
            let _ = s.fanout_channel("#bench", "fill\r\n", 0);
            let st = s.fanout_channel("#bench", "probe\r\n", 0);
            assert!(st.dropped >= 1 || st.delivered >= 1);
            let _ = (st, &mut rx_fast, &mut rx_slow);
        });
    });

    c.bench_function("legacy_bus_send", |b| {
        let shared = Shared::new(Arc::new(Config::default()), None);
        let line = line.to_string();
        b.iter(|| {
            let _ = shared.bus_sender().send(ircd::BusMsg {
                target: "#bench".into(),
                line: line.clone(),
                skip_conn: 0,
            });
        });
    });
}

criterion_group!(benches, fanout_scaling);
criterion_main!(benches);
