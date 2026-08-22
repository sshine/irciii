//! CLI entry point for irciii.

use std::process::Stdio;
use std::time::Duration;

use clap::{Parser, Subcommand};
use irciii_daemon::server::DEFAULT_LISTEN;
use irciii_proto::session_file;
use irciii_proto::ui_state::QuitKind;
use irciii_proto::wire::ClientCommand;
use uuid::Uuid;

/// A minimal irssi-like terminal IRC client.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Server to connect to on startup.
    #[arg(value_name = "SERVER", requires = "port", conflicts_with = "attach")]
    server: Option<String>,

    /// Port of the server (6697 is the usual TLS port).
    #[arg(value_name = "PORT")]
    port: Option<u16>,

    /// Channel to join once connected.
    #[arg(value_name = "CHANNEL", requires = "server")]
    channel: Option<String>,

    /// Connect without TLS.
    #[arg(long, requires = "server")]
    no_tls: bool,

    /// Nick to use instead of $USER.
    #[arg(long, value_name = "NICK", requires = "server")]
    nick: Option<String>,

    /// Attach to a running session by id (printed on /detach).
    #[arg(long, value_name = "SESSION_ID")]
    attach: Option<Uuid>,

    /// Daemon address to attach to (for remote sessions without a local
    /// session file); requires --fingerprint.
    #[arg(
        long,
        value_name = "HOST:PORT",
        requires = "attach",
        requires = "fingerprint"
    )]
    daemon: Option<String>,

    /// SHA-256 fingerprint of the daemon's TLS certificate (printed on
    /// /detach; required with --daemon).
    #[arg(long, value_name = "SHA256_HEX")]
    fingerprint: Option<String>,

    /// Address the daemon listens on when starting a new session.
    #[arg(long, value_name = "HOST:PORT", default_value = DEFAULT_LISTEN)]
    listen: String,

    /// List local sessions and exit.
    #[arg(long)]
    list: bool,

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
        /// The address to listen on.
        #[arg(long, default_value = DEFAULT_LISTEN)]
        listen: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if args.list {
        return list_sessions();
    }
    match args.command {
        Some(Command::Daemon { session, listen }) => {
            irciii_daemon::server::run(session, &listen).await?;
            Ok(())
        }
        None => match args.attach {
            Some(session) => run_client(session, args.daemon, args.fingerprint, None).await,
            None => {
                let startup = args.server.map(|server| ClientCommand::Connect {
                    server,
                    port: args.port.unwrap_or(if args.no_tls { 6667 } else { 6697 }),
                    tls: !args.no_tls,
                    nick: args.nick,
                    channels: args.channel.into_iter().collect(),
                });
                let session = Uuid::new_v4();
                spawn_daemon(session, &args.listen)?;
                wait_for_session_file(session).await?;
                run_client(session, None, None, startup).await
            }
        },
    }
}

/// `irciii --list`: print local sessions.
fn list_sessions() -> Result<(), Box<dyn std::error::Error>> {
    let sessions = session_file::list()?;
    if sessions.is_empty() {
        println!("no sessions");
        return Ok(());
    }
    for (id, info) in sessions {
        println!(
            "{id}  {}:{}  pid {}  started {}",
            info.host, info.port, info.pid, info.started
        );
    }
    Ok(())
}

/// Spawn the daemon as a detached child of this binary.
fn spawn_daemon(session: Uuid, listen: &str) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let log = session_file::session_dir().join(format!("{session}.log"));
    std::fs::create_dir_all(session_file::session_dir())?;
    let log = std::fs::File::create(log)?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon")
        .arg("--session")
        .arg(session.to_string())
        .arg("--listen")
        .arg(listen)
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
    Err(format!(
        "daemon did not start (no session file for {session}; see {})",
        session_file::session_dir()
            .join(format!("{session}.log"))
            .display()
    )
    .into())
}

/// The client half: connect over TLS, run the TUI, report how it ended.
async fn run_client(
    session: Uuid,
    daemon: Option<String>,
    fingerprint: Option<String>,
    startup: Option<ClientCommand>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Remote attach: explicit address + pinned fingerprint. Local attach:
    // both come from the session file.
    let (host, port, fingerprint) = match (daemon, fingerprint) {
        (Some(addr), Some(fingerprint)) => {
            let (host, port) = addr
                .rsplit_once(':')
                .ok_or_else(|| format!("bad --daemon address: {addr}"))?;
            (host.to_string(), port.parse::<u16>()?, fingerprint)
        }
        _ => {
            let info = session_file::read(session)
                .map_err(|err| format!("cannot find session {session}: {err}"))?;
            (info.host, info.port, info.fingerprint)
        }
    };
    let (tx, rx, pump) = irciii_client::net::connect(&host, port, &fingerprint).await?;
    let quit = irciii_client::host::run(tx, rx, startup).await?;
    // Let the pump flush queued Detach/Exit frames to the daemon.
    let _ = tokio::time::timeout(Duration::from_secs(2), pump).await;
    match quit {
        QuitKind::Detach => {
            println!("detached from {host}:{port}");
            println!("reattach locally:   irciii --attach {session}");
            println!(
                "reattach remotely:  irciii --attach {session} --daemon {host}:{port} --fingerprint {fingerprint}"
            );
        }
        QuitKind::Exit | QuitKind::None => {}
    }
    Ok(())
}
