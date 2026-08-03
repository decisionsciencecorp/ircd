//! C4 — typed Command dispatch + Shared invariants (H-14 coverage / regression).

mod common;

use std::sync::Arc;

use common::{read_until, with_client, with_two_clients};
use ircd::config::Config;
use ircd::state::Shared;
use ircd_core::{ascii_casefold, Command, RawLine};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

#[test]
fn command_classifies_all_known_verbs() {
    let samples = [
        ("CAP LS", "CAP"),
        ("AUTHENTICATE PLAIN", "AUTHENTICATE"),
        ("NICK bob", "NICK"),
        ("USER u 0 * :Real", "USER"),
        ("PING :x", "PING"),
        ("QUIT :bye", "QUIT"),
        ("ADMIN", "ADMIN"),
        ("OPER x y", "OPER"),
        ("JOIN #c", "JOIN"),
        ("PART #c :x", "PART"),
        ("PRIVMSG #c :hi", "PRIVMSG"),
        ("NOTICE nick :hi", "NOTICE"),
        ("TOPIC #c :t", "TOPIC"),
        ("KICK #c nick :r", "KICK"),
        ("MODE #c +n", "MODE"),
        ("INVITE n #c", "INVITE"),
        ("NAMES #c", "NAMES"),
        ("LIST #c", "LIST"),
        ("WHO #c", "WHO"),
        ("WHOIS bob", "WHOIS"),
        ("MOTD", "MOTD"),
        ("VERSION", "VERSION"),
        ("LUSERS", "LUSERS"),
        ("CHATHISTORY LATEST #c * 10", "CHATHISTORY"),
        ("NOSUCH", "NOSUCH"),
    ];
    for (raw, verb) in samples {
        let line = RawLine::parse(raw).unwrap();
        let cmd = Command::from_raw(&line);
        assert_eq!(cmd.verb(), verb, "raw={raw}");
    }
}

#[test]
fn shared_nick_and_channel_invariants() {
    let mut s = Shared::new(Arc::new(Config::default()), None);
    let id = s.alloc_conn_id();
    assert_eq!(id, 1);
    assert!(s.history_store().is_none());
    let _rx = s.subscribe_bus();
    let _tx = s.bus_sender();
    s.set_nick(id, "Alice".into());
    assert!(s.has_nick_key(&ascii_casefold("Alice")));
    assert_eq!(s.nick_id(&ascii_casefold("Alice")), Some(id));
    assert_eq!(s.display_nick(id), Some("Alice"));
    s.replace_nick(id, Some("Alice"), "Alicia".into());
    assert!(!s.has_nick_key(&ascii_casefold("Alice")));
    assert!(s.has_nick_key(&ascii_casefold("Alicia")));
    s.ensure_nick_indexed(id, "Alicia");
    s.channel_or_default("#lab".into()).members.insert(id);
    s.channel_or_default("#lab".into()).ops.insert(id);
    assert!(s.has_channel("#lab"));
    assert_eq!(s.channel_count(), 1);
    assert_eq!(s.channel_names(), vec!["#lab".to_string()]);
    assert_eq!(s.channels_containing(id), vec!["#lab".to_string()]);
    assert_eq!(s.channel_member_ids("#lab", 0), vec![id]);
    assert_eq!(s.whois_channels(id), vec!["@#lab".to_string()]);
    let rows = s.list_rows(Some("#lab"));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "#lab");
    let snap = s.names_snapshot("#lab").unwrap();
    assert!(snap.format_prefixed().contains("Alicia") || snap.format_prefixed().contains("@"));
    s.channel_mut("#lab").unwrap().remove_member(id);
    s.remove_channel_if_empty("#lab");
    assert!(!s.has_channel("#lab"));
    s.remove_nick_key(&ascii_casefold("Alicia"));
    let _ = s.clear_nick(id);
    assert_eq!(s.client_count(), 0);
    assert!(s.config().server.name.len() > 0);
    let entries = s.nick_entries();
    assert!(entries.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn c4_queries_and_unknown_via_typed_dispatch() {
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(Config::default()), None)));
    with_client(shared, 77, |mut w, mut r| async move {
        let _ = read_until(&mut r, |l| l.len() >= 2).await;
        w.write_all(
            b"NICK c4user\r\nUSER c 0 * :C4\r\nADMIN\r\nVERSION\r\nMOTD\r\nLUSERS\r\nWHOIS c4user\r\nLIST\r\nNAMES\r\nWHO 0\r\nFROBNOZ\r\nQUIT :done\r\n",
        )
        .await
        .unwrap();
        let lines = read_until(&mut r, |l| {
            l.iter().any(|x| x.contains("421") && x.contains("FROBNOZ"))
        })
        .await;
        assert!(
            lines.iter().any(|l| l.contains("256") || l.contains("ADMIN")),
            "ADMIN numeric: {lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("351") || l.contains("VERSION")));
        assert!(lines.iter().any(|l| l.contains("376") || l.contains("MOTD")));
        assert!(lines.iter().any(|l| l.contains("251") || l.contains("LUSERS") || l.contains("clients")));
        assert!(
            lines.iter().any(|l| l.contains("421") && l.contains("FROBNOZ")),
            "unknown 421: {lines:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn c4_notice_and_part_reason() {
    let shared = Arc::new(Mutex::new(Shared::new(Arc::new(Config::default()), None)));
    with_two_clients(shared, |(mut w1, mut r1), (mut w2, mut r2)| async move {
        let _ = read_until(&mut r1, |l| l.len() >= 2).await;
        let _ = read_until(&mut r2, |l| l.len() >= 2).await;
        w1.write_all(b"NICK a\r\nUSER a 0 * :A\r\nJOIN #c4\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r1, |l| l.iter().any(|x| x.contains("JOIN"))).await;
        w2.write_all(b"NICK b\r\nUSER b 0 * :B\r\nJOIN #c4\r\n")
            .await
            .unwrap();
        let _ = read_until(&mut r2, |l| l.iter().any(|x| x.contains("JOIN"))).await;
        w1.write_all(b"NOTICE b :psst\r\nPART #c4 :leaving\r\nQUIT :x\r\n")
            .await
            .unwrap();
        let lines = read_until(&mut r2, |l| {
            l.iter().any(|x| x.contains("NOTICE") || x.contains("PART"))
        })
        .await;
        assert!(
            lines
                .iter()
                .any(|l| l.contains("NOTICE") || l.contains("PART")),
            "{lines:?}"
        );
        w2.write_all(b"QUIT :x\r\n").await.unwrap();
    })
    .await;
}
