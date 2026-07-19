//! A minimal irssi-like terminal IRC client.
//!
//! Running `irciii` starts a daemon (which owns the IRC connections and chat
//! history) and a client (the TUI) as separate processes. `/detach` leaves the
//! daemon running and prints a session id; `irciii --attach <session-id>`
//! reattaches with full scrollback. `/exit` (or `/quit`) terminates both.
//!
//! ![irciii-logo.png](./irciii-logo.png)
