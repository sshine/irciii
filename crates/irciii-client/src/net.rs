//! TLS transport to the daemon, exposed as the same mpsc channel pair the
//! host loop uses for an in-process session.
//!
//! The daemon uses a per-session self-signed certificate; instead of PKI
//! verification the client pins the certificate's SHA-256 fingerprint (from
//! the session file, or `--fingerprint` for remote attach).

use std::io;
use std::sync::Arc;

use irciii_proto::framing::{self, FrameBuffer};
use irciii_proto::wire::{ClientToDaemon, DaemonToClient};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use sha2::Digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

/// Verifies the server certificate by SHA-256 fingerprint only.
#[derive(Debug)]
struct PinnedFingerprint {
    fingerprint: String,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedFingerprint {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let seen: String = sha2::Sha256::digest(end_entity.as_ref())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if seen == self.fingerprint.to_lowercase() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "daemon certificate fingerprint mismatch (got {seen})"
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Connect to a daemon over TLS (pinned by fingerprint) and pump frames both
/// ways.
///
/// Returns the channel pair for [`host::run`](crate::host::run) and the pump
/// task handle; await the handle after the host returns so queued outbound
/// messages (Detach/Exit) are flushed before the process ends.
pub async fn connect(
    host: &str,
    port: u16,
    fingerprint: &str,
) -> io::Result<(
    mpsc::Sender<ClientToDaemon>,
    mpsc::Receiver<DaemonToClient>,
    JoinHandle<()>,
)> {
    let provider = CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::aws_lc_rs::default_provider()));
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedFingerprint {
            fingerprint: fingerprint.to_string(),
            provider,
        }))
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));

    let tcp = TcpStream::connect((host, port)).await?;
    let server_name = ServerName::try_from(host.to_string())
        .unwrap_or_else(|_| ServerName::try_from("irciii").expect("static name"));
    let stream = connector.connect(server_name, tcp).await?;

    let (tx_out, rx_out) = mpsc::channel::<ClientToDaemon>(64);
    let (tx_in, rx_in) = mpsc::channel::<DaemonToClient>(64);
    let pump = tokio::spawn(pump(stream, rx_out, tx_in));
    Ok((tx_out, rx_in, pump))
}

async fn pump(
    mut stream: TlsStream<TcpStream>,
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
