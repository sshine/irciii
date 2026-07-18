//! CLI entry point for irciii.

use std::process::Stdio;
use std::time::Duration;

use clap::{Parser, Subcommand};
use irciii_proto::session_file;
use irciii_proto::ui_state::QuitKind;
use uuid::Uuid;

/// A minimal irssi-like terminal IRC client.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Attach to a running session by id (printed on /detach).
    #[arg(long, value_name = "SESSION_ID")]
    attach: Option<Uuid>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the daemon process for a session (internal; spawned by `irciii`).
    #[command(hide = true)]
    Daemon {
        /// The session id to serve.
        #[arg(long)]
        session: Uuid,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    match args.command {
        Some(Command::Daemon { session }) => run_daemon(session).await,
        None => match args.attach {
            Some(session) => run_client(session).await,
            None => {
                let session = Uuid::new_v4();
                spawn_daemon(session)?;
                wait_for_session_file(session).await?;
                run_client(session).await
            }
        },
    }
}

/// The daemon half: logs to a file, serves until /exit.
async fn run_daemon(session: Uuid) -> Result<(), Box<dyn std::error::Error>> {
    irciii_daemon::server::run(session).await?;
    Ok(())
}

/// Spawn the daemon as a detached child of this binary.
fn spawn_daemon(session: Uuid) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let log = session_file::session_dir().join(format!("{session}.log"));
    std::fs::create_dir_all(session_file::session_dir())?;
    let log = std::fs::File::create(log)?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon")
        .arg("--session")
        .arg(session.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Own process group: the daemon survives the client's terminal/SIGHUP.
        cmd.process_group(0);
    }
    cmd.spawn()?;
    Ok(())
}

/// Poll for the daemon's session file (it appears once the socket is bound).
async fn wait_for_session_file(session: Uuid) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..100 {
        if session_file::read(session).is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(format!("daemon did not start (no session file for {session})").into())
}

/// The client half: connect over TCP, run the TUI, report how it ended.
async fn run_client(session: Uuid) -> Result<(), Box<dyn std::error::Error>> {
    let info = session_file::read(session)
        .map_err(|err| format!("cannot find session {session}: {err}"))?;
    let (tx, rx, pump) = irciii_client::net::connect(info.port).await?;
    let quit = irciii_client::host::run(tx, rx).await?;
    // Let the pump flush queued Detach/Exit frames to the daemon.
    let _ = tokio::time::timeout(Duration::from_secs(2), pump).await;
    match quit {
        QuitKind::Detach => {
            println!("detached; reattach with: irciii --attach {session}");
        }
        QuitKind::Exit | QuitKind::None => {}
    }
    Ok(())
}
