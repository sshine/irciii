//! The client host loop: terminal in, daemon channel out.
//!
//! Transport-agnostic: the daemon side is a pair of mpsc channels, wired to an
//! in-process session or to a TCP socket by the caller.

use crossterm::event::EventStream;
use futures::StreamExt;
use irciii_proto::ui_state::{QuitKind, UiState};
use irciii_proto::wire::{ClientToDaemon, DaemonToClient, PROTO_VERSION};
use tokio::sync::mpsc;

use crate::ui::Ui;

/// Errors from the client host.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// Terminal I/O failed.
    #[error("terminal: {0}")]
    Io(#[from] std::io::Error),
    /// The daemon channel closed unexpectedly.
    #[error("daemon connection lost")]
    DaemonGone,
}

/// Run the TUI against a daemon channel pair until the user detaches or exits.
pub async fn run(
    tx: mpsc::Sender<ClientToDaemon>,
    mut rx: mpsc::Receiver<DaemonToClient>,
) -> Result<QuitKind, HostError> {
    let mut state = UiState::new();
    let mut ui = Ui::new();

    let _ = tx
        .send(ClientToDaemon::Hello {
            proto_version: PROTO_VERSION,
        })
        .await;
    let _ = tx.send(ClientToDaemon::Attach { replay_from_seq: 0 }).await;

    let mut terminal = ratatui::init();
    let result = event_loop(&mut state, &mut ui, &tx, &mut rx, &mut terminal).await;
    ratatui::restore();
    result
}

async fn event_loop(
    state: &mut UiState,
    ui: &mut Ui,
    tx: &mpsc::Sender<ClientToDaemon>,
    rx: &mut mpsc::Receiver<DaemonToClient>,
    terminal: &mut ratatui::DefaultTerminal,
) -> Result<QuitKind, HostError> {
    let mut events = EventStream::new();
    // The tick redraws the status bar clock and polls for hot reloads.
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));

    loop {
        terminal.draw(|frame| ui.render(state, frame))?;

        tokio::select! {
            event = events.next() => {
                if let Some(Ok(event)) = event {
                    ui.handle_event(state, &event);
                }
            }
            msg = rx.recv() => {
                let Some(msg) = msg else {
                    return Err(HostError::DaemonGone);
                };
                ui.apply_msg(state, &msg);
            }
            _ = tick.tick() => {
                if ui.maybe_reload() {
                    // A new worker may render differently: re-measure everything.
                    state.opts_gen += 1;
                }
            }
        }

        for msg in state.pending.drain(..) {
            let _ = tx.send(msg).await;
        }
        match state.should_quit {
            QuitKind::None => {}
            quit => return Ok(quit),
        }
    }
}
