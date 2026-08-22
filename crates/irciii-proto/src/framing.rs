//! Postcard frames over a byte stream, delimited by a u32-LE length prefix.
//!
//! The functions here are transport-agnostic: [`encode`] produces a complete
//! frame to write, and [`FrameBuffer`] incrementally consumes received bytes
//! and yields complete payloads. Both the tokio client and daemon build on
//! these, and they are trivially testable without I/O.

use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

/// Maximum accepted frame payload size (16 MiB); guards against garbage
/// length prefixes allocating unbounded memory.
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

/// Errors from encoding or decoding frames.
#[derive(Debug, Error)]
pub enum FrameError {
    /// Postcard (de)serialization failed.
    #[error("postcard: {0}")]
    Postcard(#[from] postcard::Error),
    /// A frame length prefix exceeded [`MAX_FRAME_LEN`].
    #[error("frame of {0} bytes exceeds maximum of {MAX_FRAME_LEN}")]
    TooLarge(u32),
}

/// Serialize a value into a complete frame: u32-LE length prefix + postcard payload.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, FrameError> {
    let payload = postcard::to_stdvec(value)?;
    let len = u32::try_from(payload.len()).map_err(|_| FrameError::TooLarge(u32::MAX))?;
    if len > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(len));
    }
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Deserialize a complete frame payload (without the length prefix).
pub fn decode<T: DeserializeOwned>(payload: &[u8]) -> Result<T, FrameError> {
    Ok(postcard::from_bytes(payload)?)
}

/// Incremental frame reassembly buffer: feed it received bytes, pop payloads.
#[derive(Debug, Default)]
pub struct FrameBuffer {
    buf: Vec<u8>,
}

impl FrameBuffer {
    /// Create an empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append received bytes.
    pub fn extend(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Pop the next complete frame payload, if one has fully arrived.
    pub fn pop(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]);
        if len > MAX_FRAME_LEN {
            return Err(FrameError::TooLarge(len));
        }
        let end = 4 + len as usize;
        if self.buf.len() < end {
            return Ok(None);
        }
        let payload = self.buf[4..end].to_vec();
        self.buf.drain(..end);
        Ok(Some(payload))
    }

    /// Pop and decode the next complete frame, if one has fully arrived.
    pub fn pop_decode<T: DeserializeOwned>(&mut self) -> Result<Option<T>, FrameError> {
        match self.pop()? {
            Some(payload) => Ok(Some(decode(&payload)?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{HistoryItem, HistoryKind};
    use crate::wire::{ClientCommand, ClientToDaemon, DaemonToClient};

    #[test]
    fn roundtrip_client_to_daemon() {
        let msg = ClientToDaemon::Command(ClientCommand::Msg {
            target: "#diku".into(),
            text: "hello, world — ✓ unicode".into(),
        });
        let frame = encode(&msg).expect("encode");
        let mut fb = FrameBuffer::new();
        fb.extend(&frame);
        let back: ClientToDaemon = fb.pop_decode().expect("pop").expect("complete frame");
        assert_eq!(back, msg);
        assert_eq!(fb.pop().expect("pop"), None);
    }

    #[test]
    fn roundtrip_daemon_to_client_split_delivery() {
        let msg = DaemonToClient::Event {
            seq: 42,
            item: HistoryItem {
                ts_ms: 1_700_000_000_000,
                kind: HistoryKind::Privmsg {
                    nick: "sshine".into(),
                    userhost: Some("~s@host".into()),
                    target: "#diku".into(),
                    text: "so did Oppenheimer!".into(),
                },
            },
        };
        let frame = encode(&msg).expect("encode");
        let mut fb = FrameBuffer::new();
        // Deliver byte by byte: no frame until the last byte arrives.
        for (i, byte) in frame.iter().enumerate() {
            fb.extend(&[*byte]);
            let popped = fb.pop_decode::<DaemonToClient>().expect("pop");
            if i + 1 < frame.len() {
                assert_eq!(popped, None);
            } else {
                assert_eq!(popped, Some(msg.clone()));
            }
        }
    }

    #[test]
    fn two_frames_in_one_delivery() {
        let a = ClientToDaemon::Detach;
        let b = ClientToDaemon::Exit;
        let mut bytes = encode(&a).expect("encode");
        bytes.extend(encode(&b).expect("encode"));
        let mut fb = FrameBuffer::new();
        fb.extend(&bytes);
        assert_eq!(fb.pop_decode::<ClientToDaemon>().expect("pop"), Some(a));
        assert_eq!(fb.pop_decode::<ClientToDaemon>().expect("pop"), Some(b));
        assert_eq!(fb.pop().expect("pop"), None);
    }

    #[test]
    fn oversized_length_prefix_is_rejected() {
        let mut fb = FrameBuffer::new();
        fb.extend(&(MAX_FRAME_LEN + 1).to_le_bytes());
        assert!(matches!(fb.pop(), Err(FrameError::TooLarge(_))));
    }
}
