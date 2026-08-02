//! Criterion benches for channel name list + tag adapt (routing-adjacent).

use std::collections::HashSet;
use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion};
use ircd::config::Config;
use ircd::state::{ChannelState, Shared};
use ircd_core::tags::adapt_bus_line;

fn routing_hot_paths(c: &mut Criterion) {
    let mut ch = ChannelState::default();
    for i in 0..200 {
        let nick = format!("user{i}");
        if i % 10 == 0 {
            ch.ops.insert(nick.clone());
        }
        ch.members.insert(nick);
    }

    c.bench_function("names_prefixed_200", |b| {
        b.iter(|| ch.names_prefixed());
    });

    let mut caps = HashSet::new();
    caps.insert("message-tags".into());
    caps.insert("server-time".into());
    let line = "@msgid=dsc1;time=2026-01-01T00:00:00.000Z;account=alice :a!b@c PRIVMSG #x :hello world\r\n";

    c.bench_function("adapt_bus_line_tagged", |b| {
        b.iter(|| adapt_bus_line(line, &caps));
    });

    let cfg = Arc::new(Config::default());
    let shared = Shared::new(cfg, None);
    c.bench_function("bus_send_privmsg", |b| {
        b.iter(|| {
            let _ = shared.bus.send(ircd::BusMsg {
                target: "#x".into(),
                line: line.to_string(),
                skip_conn: 0,
            });
        });
    });
}

criterion_group!(benches, routing_hot_paths);
criterion_main!(benches);
