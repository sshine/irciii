//! CLI entry point for irciii.

use clap::Parser;
use irciii_daemon::session::Session;
use irciii_proto::ui_state::QuitKind;
use tokio::sync::mpsc;
use uuid::Uuid;

/// A minimal irssi-like terminal IRC client.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Attach to a running session by id (printed on /detach).
    #[arg(long, value_name = "SESSION_ID")]
    attach: Option<Uuid>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    if args.attach.is_some() {
        // M4 wires this to the TCP daemon; the in-process client cannot attach.
        eprintln!("--attach is not implemented yet");
        std::process::exit(1);
    }

    // In-process daemon (the M4 split runs this in a separate process).
    let (client_tx, daemon_rx) = mpsc::channel(64);
    let (daemon_tx, client_rx) = mpsc::channel(64);
    let session = tokio::spawn(async move {
        Session::new(daemon_tx).run(daemon_rx).await;
    });

    let quit = irciii_client::host::run(client_tx, client_rx).await?;
    session.abort();
    if quit == QuitKind::Detach {
        eprintln!("note: detach requires the daemon/client split; session ended");
    }
    Ok(())
}
