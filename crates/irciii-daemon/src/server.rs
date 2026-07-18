//! The daemon's TLS server: bridges one attached client at a time to the
//! [`Session`](crate::session::Session) over postcard frames.
//!
//! TLS is on by default with a per-session self-signed certificate; clients
//! verify by pinning the certificate's SHA-256 fingerprint (published in the
//! session file and printed on detach) rather than via PKI.

use std::io;
use std::sync::Arc;

use irciii_proto::framing::{self, FrameBuffer};
use irciii_proto::session_file::{self, SessionFile};
use irciii_proto::wire::{ClientToDaemon, DaemonToClient, PROTO_VERSION};
use sha2::Digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use uuid::Uuid;

use crate::session::{Session, SessionEnd};

/// The default address the daemon listens on.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:6678";

type Client = TlsStream<TcpStream>;

/// Errors from the daemon server.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// Socket or session file I/O failed.
    #[error("io: {0}")]
    Io(#[from] io::Error),
    /// Certificate generation failed.
    #[error("tls certificate: {0}")]
    Cert(#[from] rcgen::Error),
    /// TLS configuration failed.
    #[error("tls: {0}")]
    Tls(#[from] rustls::Error),
    /// The listen address did not parse.
    #[error("bad listen address '{0}' (expected host:port)")]
    BadAddr(String),
}

/// Format a SHA-256 digest as lowercase hex.
pub fn fingerprint_hex(cert_der: &[u8]) -> String {
    sha2::Sha256::digest(cert_der)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Run the daemon: bind `listen` (host:port, default [`DEFAULT_LISTEN`]),
/// write the session file, serve clients until the session exits. The session
/// file is removed on the way out.
pub async fn run(session_id: Uuid, listen: &str) -> Result<(), ServerError> {
    let (host, base_port) = listen
        .rsplit_once(':')
        .ok_or_else(|| ServerError::BadAddr(listen.to_string()))?;
    let base_port: u16 = base_port
        .parse()
        .map_err(|_| ServerError::BadAddr(listen.to_string()))?;

    // Per-session self-signed certificate; clients pin its fingerprint.
    let cert = rcgen::generate_simple_self_signed(vec![host.to_string(), "irciii".to_string()])?;
    let cert_der = cert.cert.der().to_vec();
    let fingerprint = fingerprint_hex(&cert_der);
    let key = rustls::pki_types::PrivateKeyDer::try_from(cert.key_pair.serialize_der())
        .map_err(|err| ServerError::BadAddr(err.to_string()))?;
    let tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der.into()], key)?;
    let acceptor = TlsAcceptor::from(Arc::new(tls_config));

    // The requested port may be taken by another session: try successive
    // ports (6678, 6679, ...) before giving up.
    let mut listener = None;
    for port in base_port..base_port.saturating_add(50) {
        match TcpListener::bind((host, port)).await {
            Ok(bound) => {
                listener = Some(bound);
                break;
            }
            Err(err) if err.kind() == io::ErrorKind::AddrInUse => continue,
            Err(err) => return Err(err.into()),
        }
    }
    let listener = listener.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrInUse,
            format!(
                "no free port in {base_port}..{}",
                base_port.saturating_add(50)
            ),
        )
    })?;
    let addr = listener.local_addr()?;
    session_file::write(
        session_id,
        &SessionFile {
            host: host.to_string(),
            port: addr.port(),
            fingerprint: fingerprint.clone(),
            pid: std::process::id(),
            started: chrono::Utc::now().to_rfc3339(),
        },
    )?;
    tracing::info!(%session_id, %addr, %fingerprint, "daemon listening (tls)");

    let (in_tx, in_rx) = mpsc::channel::<ClientToDaemon>(64);
    let (out_tx, mut out_rx) = mpsc::channel::<DaemonToClient>(64);
    let mut session_task = tokio::spawn(async move { Session::new(out_tx).run(in_rx).await });

    let mut client: Option<Client> = None;
    let mut frames = FrameBuffer::new();
    let mut read_buf = vec![0u8; 16 * 1024];

    let end = loop {
        tokio::select! {
            end = &mut session_task => {
                break end.unwrap_or(SessionEnd::ClientGone);
            }
            accepted = listener.accept() => {
                let Ok((stream, _addr)) = accepted else { continue };
                let Ok(stream) = acceptor.accept(stream).await else {
                    continue; // failed handshake; keep serving
                };
                // A new client displaces the old one.
                if let Some(mut old) = client.take() {
                    let _ = write_msg(&mut old, &DaemonToClient::Bye { exiting: false }).await;
                }
                frames = FrameBuffer::new();
                client = Some(stream);
            }
            outbound = out_rx.recv() => {
                let Some(msg) = outbound else { continue };
                // No client attached: drop; replay-on-attach catches them up.
                if let Some(stream) = client.as_mut()
                    && write_msg(stream, &msg).await.is_err()
                {
                    client = None;
                }
            }
            read = read_client(client.as_mut(), &mut read_buf) => {
                match read {
                    Ok(0) => client = None, // connection closed
                    Ok(n) => {
                        frames.extend(&read_buf[..n]);
                        if !dispatch(&mut frames, &mut client, session_id, &in_tx).await {
                            client = None;
                        }
                    }
                    Err(_) => client = None,
                }
            }
        }
    };

    session_file::remove(session_id)?;
    tracing::info!(%session_id, ?end, "daemon exiting");
    Ok(())
}

/// Decode and route buffered frames; false when the connection should drop.
async fn dispatch(
    frames: &mut FrameBuffer,
    client: &mut Option<Client>,
    session_id: Uuid,
    in_tx: &mpsc::Sender<ClientToDaemon>,
) -> bool {
    loop {
        let msg = match frames.pop_decode::<ClientToDaemon>() {
            Ok(Some(msg)) => msg,
            Ok(None) => return true,
            Err(err) => {
                tracing::warn!(%err, "bad frame from client");
                return false;
            }
        };
        match msg {
            // The handshake is answered here; the session doesn't know
            // wire-protocol details.
            ClientToDaemon::Hello { proto_version } => {
                let reply = DaemonToClient::Hello {
                    proto_version: PROTO_VERSION,
                    session_id,
                };
                if proto_version != PROTO_VERSION {
                    tracing::warn!(proto_version, "client protocol mismatch");
                    if let Some(stream) = client.as_mut() {
                        let _ = write_msg(stream, &reply).await;
                        let _ = write_msg(stream, &DaemonToClient::Bye { exiting: false }).await;
                    }
                    return false;
                }
                if let Some(stream) = client.as_mut()
                    && write_msg(stream, &reply).await.is_err()
                {
                    return false;
                }
            }
            msg => {
                if in_tx.send(msg).await.is_err() {
                    return false;
                }
            }
        }
    }
}

async fn write_msg(stream: &mut Client, msg: &DaemonToClient) -> io::Result<()> {
    let frame = framing::encode(msg).map_err(io::Error::other)?;
    stream.write_all(&frame).await
}

/// Read from the client if there is one; pend forever otherwise (the select!
/// arm is effectively disabled while detached).
async fn read_client(stream: Option<&mut Client>, buf: &mut [u8]) -> io::Result<usize> {
    match stream {
        Some(stream) => stream.read(buf).await,
        None => std::future::pending().await,
    }
}
