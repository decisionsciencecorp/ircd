//! Shared server state: channels, nicks, admission, broadcast bus.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::sync::broadcast;

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

impl ChannelState {
    pub fn names_prefixed(&self, id_to_nick: &HashMap<ClientId, String>) -> String {
        let mut names: Vec<(String, String)> = self
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
        names.sort_by(|a, b| a.0.cmp(&b.0));
        names
            .into_iter()
            .map(|(_, d)| d)
            .collect::<Vec<_>>()
            .join(" ")
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

pub struct Shared {
    pub next_id: ClientId,
    /// Display nick → connection id (single nick registration for v0)
    pub nicks: HashMap<String, ClientId>,
    /// Connection id → current display nick
    pub id_to_nick: HashMap<ClientId, String>,
    /// channel → state
    pub channels: HashMap<String, ChannelState>,
    pub bus: broadcast::Sender<BusMsg>,
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
            config,
            history,
            ip_counts: HashMap::new(),
            client_count: 0,
        }
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
}
