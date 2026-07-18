//! The irciii client host: owns the terminal, the TCP connection to the
//! daemon, and the (optionally hot-reloaded) UI worker.

pub mod host;
#[cfg(feature = "hot-reload")]
pub mod reload;
pub mod ui;
