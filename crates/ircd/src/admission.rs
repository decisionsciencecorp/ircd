//! Connection admission helpers (H-09) — callable before costly TLS/WS work.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Semaphore, SemaphorePermit};

use crate::config::Config;
use crate::state::Shared;
use std::net::SocketAddr;

/// Duration allowed for a TLS/WS handshake before the socket is dropped.
pub fn handshake_timeout(cfg: &Config) -> Duration {
    Duration::from_secs(cfg.security.handshake_timeout_secs.max(1))
}

/// Registration deadline (NICK+USER). `None` when disabled (0).
pub fn registration_deadline(cfg: &Config) -> Option<Duration> {
    let secs = cfg.security.registration_timeout_secs;
    if secs == 0 {
        None
    } else {
        Some(Duration::from_secs(secs))
    }
}

/// Idle deadline for registered clients. `None` when disabled.
pub fn idle_deadline(cfg: &Config) -> Option<Duration> {
    let secs = cfg.security.idle_timeout_secs;
    if secs == 0 {
        None
    } else {
        Some(Duration::from_secs(secs))
    }
}

/// Build the in-flight handshake/session semaphore from config.
pub fn handshake_semaphore(cfg: &Config) -> Arc<Semaphore> {
    Arc::new(Semaphore::new(cfg.security.max_handshake_inflight.max(1)))
}

/// Try to take a handshake slot; returns `None` if the cap is saturated.
pub async fn try_acquire_handshake(sem: &Semaphore) -> Option<SemaphorePermit<'_>> {
    sem.try_acquire().ok()
}

/// Admit under `[limits]` *before* TLS/WS handshake. Caller must [`Shared::release`]
/// if the handshake fails or the session never starts.
pub async fn admit_early(
    shared: &Arc<Mutex<Shared>>,
    peer: SocketAddr,
) -> Result<(), &'static str> {
    let mut g = shared.lock().await;
    g.try_admit(peer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, SecuritySection};
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn timeouts_honour_zero_disable() {
        let mut cfg = Config::default();
        cfg.security.registration_timeout_secs = 0;
        cfg.security.idle_timeout_secs = 0;
        assert!(registration_deadline(&cfg).is_none());
        assert!(idle_deadline(&cfg).is_none());
        assert_eq!(handshake_timeout(&cfg), Duration::from_secs(15));
    }

    #[test]
    fn idle_deadline_enabled() {
        let mut cfg = Config::default();
        cfg.security.idle_timeout_secs = 30;
        assert_eq!(idle_deadline(&cfg), Some(Duration::from_secs(30)));
    }

    #[tokio::test]
    async fn admit_early_and_release() {
        let mut cfg = Config::default();
        cfg.limits.max_clients = 1;
        let shared = Arc::new(Mutex::new(Shared::new(Arc::new(cfg), None)));
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(9, 9, 9, 9)), 1);
        assert!(admit_early(&shared, peer).await.is_ok());
        assert_eq!(
            admit_early(&shared, peer).await,
            Err("Too many connections")
        );
        shared.lock().await.release(peer);
        assert!(admit_early(&shared, peer).await.is_ok());
    }

    #[tokio::test]
    async fn handshake_semaphore_bounds() {
        let cfg = Config {
            security: SecuritySection {
                max_handshake_inflight: 1,
                ..SecuritySection::default()
            },
            ..Config::default()
        };
        let sem = handshake_semaphore(&cfg);
        let p1 = try_acquire_handshake(&sem).await;
        assert!(p1.is_some());
        assert!(try_acquire_handshake(&sem).await.is_none());
        drop(p1);
        assert!(try_acquire_handshake(&sem).await.is_some());
    }
}
