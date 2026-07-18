//! TCP transport to the daemon, exposed as the same mpsc channel pair the
//! host loop uses for an in-process session.

use std::io;

use irciii_proto::framing::{self, FrameBuffer};
use irciii_proto::wire::{ClientToDaemon, DaemonToClient};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Connect to a daemon on loopback and pump frames both ways.
///
/// Returns the channel pair for [`host::run`](crate::host::run) and the pump
/// task handle; await the handle after the host returns so queued outbound
/// messages (Detach/Exit) are flushed before the process ends.
pub async fn connect(
    port: u16,
) -> io::Result<(
    mpsc::Sender<ClientToDaemon>,
    mpsc::Receiver<DaemonToClient>,
    JoinHandle<()>,
)> {
    let stream = TcpStream::connect(("127.0.0.1", port)).await?;
    let (tx_out, rx_out) = mpsc::channel::<ClientToDaemon>(64);
    let (tx_in, rx_in) = mpsc::channel::<DaemonToClient>(64);
    let pump = tokio::spawn(pump(stream, rx_out, tx_in));
    Ok((tx_out, rx_in, pump))
}

async fn pump(
    mut stream: TcpStream,
    mut rx_out: mpsc::Receiver<ClientToDaemon>,
    tx_in: mpsc::Sender<DaemonToClient>,
) {
    let mut frames = FrameBuffer::new();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        tokio::select! {
            outbound = rx_out.recv() => {
                let Some(msg) = outbound else {
                    // Host is gone: flush and stop.
                    let _ = stream.shutdown().await;
                    return;
                };
                let Ok(frame) = framing::encode(&msg) else { return };
                if stream.write_all(&frame).await.is_err() {
                    return;
                }
            }
            read = stream.read(&mut buf) => {
                let Ok(n) = read else { return };
                if n == 0 {
                    return; // daemon closed the connection
                }
                frames.extend(&buf[..n]);
                loop {
                    match frames.pop_decode::<DaemonToClient>() {
                        Ok(Some(msg)) => {
                            if tx_in.send(msg).await.is_err() {
                                return;
                            }
                        }
                        Ok(None) => break,
                        Err(_) => return,
                    }
                }
            }
        }
    }
}
