//! dsc-ircd library surface — sessions, config, history, TLS/WS accepts.
//!
//! The `ircd` binary is a thin CLI wrapper around this crate.

pub mod admission;
pub mod cmd_precheck;
pub mod config;
pub mod fs_perms;
pub mod history;
pub mod lock_discipline;
pub mod session;
pub mod state;
pub mod tls;
pub mod ws;
pub mod ws_lines;
pub mod ws_policy;

pub use config::Config;
pub use history::HistoryStore;
pub use session::handle_client;
pub use state::{BusMsg, ChannelState, ClientId, NamesSnapshot, NamesThin, Shared};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
