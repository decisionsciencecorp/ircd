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
    /// +i — invite-only
    pub mode_i: bool,
    /// Simple ban masks (nick!user@host or nick) — baseline +b.
    pub bans: HashSet<String>,
    /// Pending invites (ClientId) cleared on JOIN or channel destroy.
    pub invites: HashSet<ClientId>,
}

impl Default for ChannelState {
    fn default() -> Self {
        Self {
            members: HashSet::new(),
            ops: HashSet::new(),
            topic: None,
            mode_n: true,
            mode_t: true,
            mode_i: false,
            bans: HashSet::new(),
            invites: HashSet::new(),
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
        if self.mode_i {
            s.push('i');
        }
        s
    }

    /// Match nick!user@host against stored ban masks (exact or nick-only).
    pub fn is_banned(&self, nick: &str, user: &str, host: &str) -> bool {
        let full = format!("{nick}!{user}@{host}");
        let nick_l = nick.to_ascii_lowercase();
        self.bans.iter().any(|m| {
            let ml = m.to_ascii_lowercase();
            ml == full.to_ascii_lowercase() || ml == nick_l || ml == format!("{nick_l}!*@*")
        })
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

/// Global server state. Fields are private (H-14); mutate via invariant methods.
pub struct Shared {
    next_id: ClientId,
    /// Display nick (casefolded key) → connection id
    nicks: HashMap<String, ClientId>,
    /// Connection id → current display nick
    id_to_nick: HashMap<ClientId, String>,
    channels: HashMap<String, ChannelState>,
    /// Legacy broadcast retained for microbench comparison only — sessions use outboxes.
    bus: broadcast::Sender<BusMsg>,
    outboxes: HashMap<ClientId, mpsc::Sender<String>>,
    config: Arc<Config>,
    history: Option<Arc<HistoryStore>>,
    ip_counts: HashMap<String, usize>,
    client_count: usize,
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

    pub fn alloc_conn_id(&mut self) -> ClientId {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    pub fn config(&self) -> Arc<Config> {
        Arc::clone(&self.config)
    }

    pub fn history_store(&self) -> Option<Arc<HistoryStore>> {
        self.history.clone()
    }

    pub fn subscribe_bus(&self) -> broadcast::Receiver<BusMsg> {
        self.bus.subscribe()
    }

    /// Expose the legacy bus sender for microbench comparison only.
    pub fn bus_sender(&self) -> broadcast::Sender<BusMsg> {
        self.bus.clone()
    }

    pub fn client_count(&self) -> usize {
        self.client_count
    }

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    pub fn has_channel(&self, name: &str) -> bool {
        self.channels.contains_key(name)
    }

    pub fn channel(&self, name: &str) -> Option<&ChannelState> {
        self.channels.get(name)
    }

    pub fn channel_mut(&mut self, name: &str) -> Option<&mut ChannelState> {
        self.channels.get_mut(name)
    }

    pub fn channel_or_default(&mut self, name: String) -> &mut ChannelState {
        self.channels.entry(name).or_default()
    }

    pub fn remove_channel(&mut self, name: &str) {
        self.channels.remove(name);
    }

    pub fn channel_names(&self) -> Vec<String> {
        self.channels.keys().cloned().collect()
    }

    pub fn remove_channel_if_empty(&mut self, name: &str) {
        if self
            .channels
            .get(name)
            .map(|c| c.members.is_empty())
            .unwrap_or(false)
        {
            self.channels.remove(name);
        }
    }

    pub fn channels_containing(&self, id: ClientId) -> Vec<String> {
        self.channels
            .iter()
            .filter(|(_, ch)| ch.members.contains(&id))
            .map(|(name, _)| name.clone())
            .collect()
    }

    pub fn nick_id(&self, folded_key: &str) -> Option<ClientId> {
        self.nicks.get(folded_key).copied()
    }

    pub fn display_nick(&self, id: ClientId) -> Option<&str> {
        self.id_to_nick.get(&id).map(String::as_str)
    }

    pub fn set_nick(&mut self, id: ClientId, nick: String) {
        let key = ircd_core::ascii_casefold(&nick);
        self.nicks.insert(key, id);
        self.id_to_nick.insert(id, nick);
    }

    /// Drop previous nick key for `id`, then bind `nick`.
    pub fn replace_nick(&mut self, id: ClientId, old_nick: Option<&str>, nick: String) {
        if let Some(old) = old_nick {
            self.nicks.remove(&ircd_core::ascii_casefold(old));
        }
        self.set_nick(id, nick);
    }

    pub fn ensure_nick_indexed(&mut self, id: ClientId, nick: &str) {
        self.id_to_nick.insert(id, nick.to_string());
        self.nicks
            .entry(ircd_core::ascii_casefold(nick))
            .or_insert(id);
    }

    pub fn clear_nick(&mut self, id: ClientId) -> Option<String> {
        let nick = self.id_to_nick.remove(&id)?;
        self.nicks.remove(&ircd_core::ascii_casefold(&nick));
        Some(nick)
    }

    pub fn remove_nick_key(&mut self, folded_key: &str) {
        self.nicks.remove(folded_key);
    }

    /// Snapshot NAMES for a channel (formats off the lock via returned snapshot).
    pub fn names_snapshot(&self, channel: &str) -> Option<NamesSnapshot> {
        let ch = self.channels.get(channel)?;
        Some(NamesSnapshot::from_sets(
            &ch.members,
            &ch.ops,
            &self.id_to_nick,
        ))
    }

    /// WHO/list helpers: (display_nick, ClientId) for all registered nicks.
    pub fn nick_entries(&self) -> Vec<(String, ClientId)> {
        self.nicks
            .iter()
            .map(|(key, id)| {
                let display = self
                    .id_to_nick
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| key.clone());
                (display, *id)
            })
            .collect()
    }

    pub fn has_nick_key(&self, folded_key: &str) -> bool {
        self.nicks.contains_key(folded_key)
    }

    /// Channel member ids excluding `skip` (for KICK fanout prep).
    pub fn channel_member_ids(&self, channel: &str, skip: ClientId) -> Vec<ClientId> {
        self.channels
            .get(channel)
            .map(|c| c.members.iter().copied().filter(|id| *id != skip).collect())
            .unwrap_or_default()
    }

    /// LIST rows: (name, member_count, topic), optionally filtered by comma list.
    pub fn list_rows(&self, filter: Option<&str>) -> Vec<(String, usize, String)> {
        let mut v: Vec<(String, usize, String)> = self
            .channels
            .iter()
            .filter(|(name, _)| {
                filter
                    .map(|f| f.split(',').any(|c| c.trim() == name.as_str()))
                    .unwrap_or(true)
            })
            .map(|(name, ch)| {
                (
                    name.clone(),
                    ch.members.len(),
                    ch.topic.clone().unwrap_or_default(),
                )
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// WHOIS channel list with `@` for ops.
    pub fn whois_channels(&self, id: ClientId) -> Vec<String> {
        self.channels
            .iter()
            .filter(|(_, ch)| ch.members.contains(&id))
            .map(|(name, ch)| {
                if ch.ops.contains(&id) {
                    format!("@{name}")
                } else {
                    name.clone()
                }
            })
            .collect()
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
        *s.channel_or_default("#a".into()) = ChannelState::default();
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
        ch.mode_i = true;
        assert_eq!(ch.mode_chars(), "+ti");
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
        let ch = s.channel_or_default("#c".into());
        ch.members.insert(1);
        ch.members.insert(2);
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
        s.channel_or_default("#c".into()).members.insert(9);
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
        let st = s.fanout_ids(&[5, 99], "X\r\n");
        assert_eq!(st.missing_outbox, 1);
        assert_eq!(st.delivered, 1);
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

    #[test]
    fn is_banned_matches_nick() {
        let mut ch = ChannelState::default();
        ch.bans.insert("banned".into());
        assert!(ch.is_banned("banned", "user", "dsc.local"));
        assert!(!ch.is_banned("other", "user", "dsc.local"));
        ch.bans.insert("x!*@*".into());
        assert!(ch.is_banned("x", "a", "b"));
    }

    #[test]
    fn names_split_pathological_and_empty() {
        let snap = NamesSnapshot {
            entries: vec![("long".into(), "abcdefghij".into())],
        };
        let parts = snap.split_for_wire(4);
        assert!(!parts.is_empty());
        let empty = NamesSnapshot { entries: vec![] };
        assert_eq!(empty.split_for_wire(8), vec![String::new()]);
    }

    #[test]
    fn fanout_missing_channel_is_noop() {
        let s = Shared::new(Arc::new(Config::default()), None);
        let st = s.fanout_channel("#nope", "x\r\n", 0);
        assert_eq!(st.attempted, 0);
    }

    #[test]
    fn shared_accessors_cover_invariants() {
        let mut s = Shared::new(Arc::new(Config::default()), None);
        let id = s.alloc_conn_id();
        assert!(s.history_store().is_none());
        let _ = s.subscribe_bus();
        let _ = s.bus_sender();
        assert_eq!(s.config().server.name.is_empty(), false);
        s.set_nick(id, "Zed".into());
        assert!(s.has_nick_key(&ircd_core::ascii_casefold("Zed")));
        s.replace_nick(id, Some("Zed"), "Zoe".into());
        s.ensure_nick_indexed(id, "Zoe");
        assert_eq!(s.display_nick(id), Some("Zoe"));
        assert!(!s.nick_entries().is_empty());
        s.channel_or_default("#z".into()).members.insert(id);
        assert!(s.has_channel("#z"));
        assert_eq!(s.channel_count(), 1);
        assert!(!s.channel_names().is_empty());
        assert_eq!(s.channels_containing(id), vec!["#z".to_string()]);
        assert!(!s.channel_member_ids("#z", 99).is_empty());
        assert!(!s.whois_channels(id).is_empty());
        assert!(!s.list_rows(None).is_empty());
        assert!(!s.list_rows(Some("#z")).is_empty());
        assert!(s.names_snapshot("#z").is_some());
        assert!(s.channel("#missing").is_none());
        assert!(s.channel_mut("#missing").is_none());
        s.channel_mut("#z").unwrap().remove_member(id);
        s.remove_channel_if_empty("#z");
        s.remove_channel("#nope");
        s.remove_nick_key(&ircd_core::ascii_casefold("Zoe"));
        assert!(s.clear_nick(id).is_some());
        assert!(s.clear_nick(id).is_none());
    }
}
