//! The irciii daemon: owns IRC connections, the canonical chat history, and
//! serves attached clients over TCP with a postcard-framed protocol.

pub mod connection;
pub mod session;
pub mod translate;
