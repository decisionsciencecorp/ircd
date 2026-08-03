//! Shared server state: channels, nicks, admission, member-targeted outboxes.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};

use crate::config::Config;
use crate::history::HistoryStore;

/// Stable connection identity for authorization and membership (not display nick).
pub type ClientId = u64;

#[derive(Clone, Debug)]
pub struct BusMsg {
    /// Channel name including `#`, or `*` for server-wide (unused for now).
    pub target: String,
    pub line: String,
    /// Skip delivering back to this connection id.
    pub skip_conn: ClientId,
}

/// Per-channel membership and simple modes (+n/+t) plus channel ops.
#[derive(Debug, Clone)]
pub struct ChannelState {
    pub members: HashSet<ClientId>,
    pub ops: HashSet<ClientId>,
    pub topic: Option<String>,
    /// +n — no messages from outside
    pub mode_n: bool,
    /// +t — only ops may set topic
    pub mode_t: bool,
}

impl Default for ChannelState {
    fn default() -> Self {
        Self {
            members: HashSet::new(),
            ops: HashSet::new(),
            topic: None,
            mode_n: true,
            mode_t: true,
        }
    }
}

/// Membership nick snapshot for NAMES — format off the global lock (H-16).
#[derive(Debug, Clone)]
pub struct NamesSnapshot {
    /// (sort_key, display) — display includes `@` for ops.
    entries: Vec<(String, String)>,
}

impl NamesSnapshot {
    /// Single space-joined NAMES list (may exceed wire size — prefer `split_for_wire`).
    pub fn from_sets(
        members: &HashSet<ClientId>,
        ops: &HashSet<ClientId>,
        id_to_nick: &HashMap<ClientId, String>,
    ) -> Self {
        let mut entries: Vec<(String, String)> = members
            .iter()
            .filter_map(|id| {
                let nick = id_to_nick.get(id)?;
                let display = if ops.contains(id) {
                    format!("@{nick}")
                } else {
                    nick.clone()
                };
                Some((nick.to_ascii_lowercase(), display))
            })
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Self { entries }
    }

    pub fn format_prefixed(&self) -> String {
        self.entries
            .iter()
            .map(|(_, d)| d.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Split into trailing payloads that fit `max_payload_bytes` (353 `:names` body).
    pub fn split_for_wire(&self, max_payload_bytes: usize) -> Vec<String> {
        let max = max_payload_bytes.max(1);
        let mut out = Vec::new();
        let mut cur = String::new();
        for (_, display) in &self.entries {
            let need = if cur.is_empty() {
                display.len()
            } else {
                cur.len() + 1 + display.len()
            };
            if !cur.is_empty() && need > max {
                out.push(std::mem::take(&mut cur));
                cur.push_str(display);
            } else if cur.is_empty() {
                cur.push_str(display);
            } else {
                cur.push(' ');
                cur.push_str(display);
            }
            // Pathological single nick longer than max: emit alone.
            if cur.len() > max && cur == *display {
                out.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        if out.is_empty() {
            out.push(String::new());
        }
        out
    }
}

impl ChannelState {
    /// Clone only member nick/ops needed for NAMES (cheap under lock).
    pub fn names_snapshot(&self, id_to_nick: &HashMap<ClientId, String>) -> NamesSnapshot {
        let mut entries: Vec<(String, String)> = self
            .members
            .iter()
            .filter_map(|id| {
                let nick = id_to_nick.get(id)?;
                let display = if self.ops.contains(id) {
                    format!("@{nick}")
                } else {
                    nick.clone()
                };
                Some((nick.to_ascii_lowercase(), display))
            })
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        NamesSnapshot { entries }
    }

    pub fn names_prefixed(&self, id_to_nick: &HashMap<ClientId, String>) -> String {
        self.names_snapshot(id_to_nick).format_prefixed()
    }

    pub fn mode_chars(&self) -> String {
        let mut s = String::from("+");
        if self.mode_n {
            s.push('n');
        }
        if self.mode_t {
            s.push('t');
        }
        s
    }

    pub fn is_op(&self, id: ClientId) -> bool {
        self.ops.contains(&id)
    }

    pub fn remove_member(&mut self, id: ClientId) {
        self.members.remove(&id);
        self.ops.remove(&id);
    }
}

/// Result of a member-targeted fanout attempt (H-07/H-08).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FanoutStats {
    pub attempted: usize,
    pub delivered: usize,
    pub dropped: usize,
    pub missing_outbox: usize,
}

/// Default per-client outbound queue depth (slow-consumer: try_send drop).
pub const OUTBOX_CAP: usize = 64;

pub struct Shared {
    pub next_id: ClientId,
    /// Display nick → connection id (single nick registration for v0)
    pub nicks: HashMap<String, ClientId>,
    /// Connection id → current display nick
    pub id_to_nick: HashMap<ClientId, String>,
    /// channel → state
    pub channels: HashMap<String, ChannelState>,
    /// Legacy broadcast retained for microbench comparison only — sessions use outboxes.
    pub bus: broadcast::Sender<BusMsg>,
    /// Per-connection bounded outbound queues (member-targeted routing).
    pub outboxes: HashMap<ClientId, mpsc::Sender<String>>,
    pub config: Arc<Config>,
    pub history: Option<Arc<HistoryStore>>,
    /// peer IP → active connection count
    pub ip_counts: HashMap<String, usize>,
    pub client_count: usize,
}

impl Shared {
    pub fn new(config: Arc<Config>, history: Option<Arc<HistoryStore>>) -> Self {
        let (bus, _) = broadcast::channel(256);
        Self {
            next_id: 1,
            nicks: HashMap::new(),
            id_to_nick: HashMap::new(),
            channels: HashMap::new(),
            bus,
            outboxes: HashMap::new(),
            config,
            history,
            ip_counts: HashMap::new(),
            client_count: 0,
        }
    }

    pub fn register_outbox(&mut self, id: ClientId, tx: mpsc::Sender<String>) {
        self.outboxes.insert(id, tx);
    }

    pub fn unregister_outbox(&mut self, id: ClientId) {
        self.outboxes.remove(&id);
    }

    /// Deliver `line` to every current channel member except `skip`.
    /// Slow consumers: bounded `try_send` — full queues count as `dropped`.
    pub fn fanout_channel(&self, channel: &str, line: &str, skip: ClientId) -> FanoutStats {
        let Some(ch) = self.channels.get(channel) else {
            return FanoutStats::default();
        };
        let ids: Vec<ClientId> = ch
            .members
            .iter()
            .copied()
            .filter(|id| *id != skip)
            .collect();
        self.fanout_ids(&ids, line)
    }

    /// Deliver to an explicit id list (e.g. KICK before membership remove).
    pub fn fanout_ids(&self, ids: &[ClientId], line: &str) -> FanoutStats {
        let mut stats = FanoutStats {
            attempted: ids.len(),
            ..Default::default()
        };
        for id in ids {
            match self.outboxes.get(id) {
                None => stats.missing_outbox += 1,
                Some(tx) => match tx.try_send(line.to_string()) {
                    Ok(()) => stats.delivered += 1,
                    Err(_) => stats.dropped += 1,
                },
            }
        }
        stats
    }

    /// Admit a new connection under `[limits]`, or return a rejection reason.
    pub fn try_admit(&mut self, peer: SocketAddr) -> Result<(), &'static str> {
        let ip = peer.ip().to_string();
        let lim = &self.config.limits;
        if self.client_count >= lim.max_clients {
            return Err("Too many connections");
        }
        let n = self.ip_counts.get(&ip).copied().unwrap_or(0);
        if n >= lim.max_clients_per_ip {
            return Err("Too many connections from your host");
        }
        self.client_count += 1;
        *self.ip_counts.entry(ip).or_default() += 1;
        Ok(())
    }

    /// Whether a new channel name may be created under `[limits].max_channels`.
    pub fn can_create_channel(&self) -> bool {
        self.channels.len() < self.config.limits.max_channels
    }

    pub fn release(&mut self, peer: SocketAddr) {
        let ip = peer.ip().to_string();
        self.client_count = self.client_count.saturating_sub(1);
        if let Some(n) = self.ip_counts.get_mut(&ip) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.ip_counts.remove(&ip);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn peer(octet: u8) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, octet)), 12345)
    }

    #[test]
    fn names_prefixed_ops_sorted() {
        let mut ch = ChannelState::default();
        ch.members.insert(1);
        ch.members.insert(2);
        ch.ops.insert(2);
        let mut map: HashMap<ClientId, String> = HashMap::new();
        map.insert(1, "alice".into());
        map.insert(2, "bob".into());
        assert_eq!(ch.names_prefixed(&map), "alice @bob");
    }

    #[test]
    fn nick_change_keeps_ops_on_client_id() {
        let mut ch = ChannelState::default();
        ch.members.insert(7);
        ch.ops.insert(7);
        let mut nicks: HashMap<String, ClientId> = HashMap::new();
        let mut id_to_nick: HashMap<ClientId, String> = HashMap::new();
        nicks.insert("OpOld".into(), 7u64);
        id_to_nick.insert(7u64, "OpOld".into());
        // NICK OpOld -> OpNew: membership stays on id 7
        nicks.remove("OpOld");
        nicks.insert("OpNew".into(), 7);
        id_to_nick.insert(7, "OpNew".into());
        assert!(ch.is_op(7));
        assert!(ch.members.contains(&7));
        // Stolen old nick is a different connection
        nicks.insert("OpOld".into(), 99);
        assert!(!ch.members.contains(&99));
        assert!(!ch.is_op(99));
    }

    #[test]
    fn can_create_channel_respects_cap() {
        let mut cfg = Config::default();
        cfg.limits.max_channels = 1;
        let mut s = Shared::new(Arc::new(cfg), None);
        assert!(s.can_create_channel());
        s.channels.insert("#a".into(), ChannelState::default());
        assert!(!s.can_create_channel());
    }

    #[test]
    fn admit_and_release() {
        let mut cfg = Config::default();
        cfg.limits.max_clients = 2;
        cfg.limits.max_clients_per_ip = 2;
        let mut s = Shared::new(Arc::new(cfg), None);
        assert!(s.try_admit(peer(1)).is_ok());
        assert!(s.try_admit(peer(1)).is_ok());
        assert_eq!(s.try_admit(peer(1)), Err("Too many connections"));
        s.release(peer(1));
        assert!(s.try_admit(peer(2)).is_ok());
    }

    #[test]
    fn per_ip_limit() {
        let mut cfg = Config::default();
        cfg.limits.max_clients = 100;
        cfg.limits.max_clients_per_ip = 1;
        let mut s = Shared::new(Arc::new(cfg), None);
        assert!(s.try_admit(peer(1)).is_ok());
        assert_eq!(
            s.try_admit(peer(1)),
            Err("Too many connections from your host")
        );
        assert!(s.try_admit(peer(2)).is_ok());
    }

    #[test]
    fn modes_ops_and_remove() {
        let mut ch = ChannelState::default();
        assert_eq!(ch.mode_chars(), "+nt");
        ch.mode_n = false;
        assert_eq!(ch.mode_chars(), "+t");
        ch.members.insert(3);
        ch.ops.insert(3);
        assert!(ch.is_op(3));
        ch.remove_member(3);
        assert!(ch.members.is_empty());
        assert!(!ch.is_op(3));
    }

    #[tokio::test]
    async fn fanout_only_channel_members() {
        let cfg = Config::default();
        let mut s = Shared::new(Arc::new(cfg), None);
        let (tx1, mut rx1) = mpsc::channel::<String>(4);
        let (tx2, mut rx2) = mpsc::channel::<String>(4);
        let (tx3, mut rx3) = mpsc::channel::<String>(4);
        s.register_outbox(1, tx1);
        s.register_outbox(2, tx2);
        s.register_outbox(3, tx3);
        let mut ch = ChannelState::default();
        ch.members.insert(1);
        ch.members.insert(2);
        s.channels.insert("#c".into(), ch);
        let st = s.fanout_channel("#c", "LINE\r\n", 1);
        assert_eq!(st.attempted, 1);
        assert_eq!(st.delivered, 1);
        assert!(rx1.try_recv().is_err());
        assert_eq!(rx2.try_recv().unwrap(), "LINE\r\n");
        assert!(rx3.try_recv().is_err());
    }

    #[tokio::test]
    async fn fanout_slow_consumer_drops() {
        let cfg = Config::default();
        let mut s = Shared::new(Arc::new(cfg), None);
        let (tx, mut rx) = mpsc::channel::<String>(1);
        s.register_outbox(9, tx);
        let mut ch = ChannelState::default();
        ch.members.insert(9);
        s.channels.insert("#c".into(), ch);
        assert_eq!(s.fanout_channel("#c", "a\r\n", 0).delivered, 1);
        let st = s.fanout_channel("#c", "b\r\n", 0);
        assert_eq!(st.dropped, 1);
        assert_eq!(rx.try_recv().unwrap(), "a\r\n");
    }

    #[tokio::test]
    async fn fanout_ids_includes_explicit_victim() {
        let cfg = Config::default();
        let mut s = Shared::new(Arc::new(cfg), None);
        let (tx, mut rx) = mpsc::channel::<String>(4);
        s.register_outbox(5, tx);
        let st = s.fanout_ids(&[5], "KICK\r\n");
        assert_eq!(st.delivered, 1);
        assert_eq!(rx.try_recv().unwrap(), "KICK\r\n");
    }


    #[test]
    fn names_snapshot_formats_off_channel() {
        let mut ch = ChannelState::default();
        ch.members.insert(1);
        ch.members.insert(2);
        ch.ops.insert(2);
        let mut map: HashMap<ClientId, String> = HashMap::new();
        map.insert(1, "alice".into());
        map.insert(2, "bob".into());
        let snap = ch.names_snapshot(&map);
        assert_eq!(snap.format_prefixed(), "alice @bob");
        let parts = snap.split_for_wire(8); // "alice" = 5, " @bob" needs more
        let joined = parts.join(" ");
        assert!(joined.contains("alice"));
        assert!(joined.contains("@bob"));
    }

    #[test]
    fn names_split_for_wire_chunks() {
        let snap = NamesSnapshot {
            entries: (0..20)
                .map(|i| {
                    let n = format!("u{i:02}");
                    (n.clone(), n)
                })
                .collect(),
        };
        let parts = snap.split_for_wire(12);
        assert!(parts.len() > 1);
        for p in &parts {
            assert!(p.len() <= 12, "chunk too long: {p:?}");
        }
        assert_eq!(parts.join(" ").split_whitespace().count(), 20);
    }

}
