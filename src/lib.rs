pub mod agent;
pub mod bus;
pub mod channels;
pub mod cli;
pub mod config;
pub mod cron;
pub mod error;
pub mod heartbeat;
pub mod providers;
pub mod session;
pub mod tools;
pub mod utils;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const LOGO: &str = "\u{1F408}"; // cat emoji
