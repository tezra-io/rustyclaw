pub mod agent;
pub mod bus;
pub mod channels;
pub mod cli;
pub mod config;
pub mod cron;
pub mod embeddings;
pub mod error;
pub mod heartbeat;
pub mod logging;
pub mod providers;
pub mod scheduler;
pub mod session;
pub mod tools;
pub mod triggers;
pub mod utils;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
