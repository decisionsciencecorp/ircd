//! Shared server state: channels, nicks, admission, broadcast bus.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::sync::broadcast;

use crate::config::Config;
use crate::history::HistoryStore;

#[derive(Clone, Debug)]
pub struct BusMsg {
    /// Channel name including `#`, or `*` for server-wide (unused for now).
    pub target: String,
    pub line: String,
    /// Skip delivering back to this connection id.
    pub skip_conn: u64,
}

/// Per-channel membership and simple modes (+n/+t) plus channel ops.
#[derive(Debug, Clone)]
pub struct ChannelState {
    pub members: HashSet<String>,
    pub ops: HashSet<String>,
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
    pub fn names_prefixed(&self) -> String {
        let mut names: Vec<(String, String)> = self
            .members
            .iter()
            .map(|n| {
                let display = if self.ops.contains(n) {
                    format!("@{n}")
                } else {
                    n.clone()
                };
                (n.to_ascii_lowercase(), display)
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

    pub fn is_op(&self, nick: &str) -> bool {
        self.ops.contains(nick)
    }

    pub fn remove_nick(&mut self, nick: &str) {
        self.members.remove(nick);
        self.ops.remove(nick);
    }
}

pub struct Shared {
    pub next_id: u64,
    /// nick -> connection id (single nick registration for v0)
    pub nicks: HashMap<String, u64>,
    /// channel -> state
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
        ch.members.insert("bob".into());
        ch.members.insert("alice".into());
        ch.ops.insert("bob".into());
        // Sorted by nick (case-insensitive); @op is display-only.
        assert_eq!(ch.names_prefixed(), "alice @bob");
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
        ch.members.insert("x".into());
        ch.ops.insert("x".into());
        assert!(ch.is_op("x"));
        ch.remove_nick("x");
        assert!(ch.members.is_empty());
        assert!(!ch.is_op("x"));
    }
}
