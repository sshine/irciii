//! The UI worker's update and render logic, operating on [`UiState`].

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use irciii_proto::history::{HistoryItem, HistoryKind};
use irciii_proto::ui_state::{QuitKind, UiState};
use irciii_proto::wire::{ClientCommand, ClientToDaemon, DaemonToClient};
use promptui_core::input::Input;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::hooks::{self, HookCtx};
use crate::input::{InputAction, parse};
use crate::{history_view, status_bar};

/// Append a client-local line to the history.
fn emit_local(state: &mut UiState, kind: HistoryKind) {
    state.history.push(HistoryItem::now(kind));
    state.heights.push(None);
}

/// Page size for PgUp/PgDn: one viewport minus a line of overlap.
fn page(state: &UiState) -> usize {
    state.viewport_height.saturating_sub(1).max(1) as usize
}

/// Handle a terminal event.
pub fn handle_event(state: &mut UiState, event: &Event) {
    let Event::Key(key) = event else { return };
    if key.kind == KeyEventKind::Release {
        return;
    }

    // ^X cycles the active target; promptui's Input does not map Ctrl+X, so
    // match the raw event before converting.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('x') {
        state.cycle_target();
        return;
    }

    match Input::from(*key) {
        Input::Char(c) => {
            state.input.insert(state.cursor, c);
            state.cursor += c.len_utf8();
        }
        Input::Backspace => {
            if let Some(prev) = prev_boundary(&state.input, state.cursor) {
                state.input.remove(prev);
                state.cursor = prev;
            }
        }
        Input::Delete => {
            if state.cursor < state.input.len() {
                state.input.remove(state.cursor);
            }
        }
        Input::Left => {
            if let Some(prev) = prev_boundary(&state.input, state.cursor) {
                state.cursor = prev;
            }
        }
        Input::Right => {
            if let Some(next) = next_boundary(&state.input, state.cursor) {
                state.cursor = next;
            }
        }
        Input::Home => state.cursor = 0,
        Input::End => state.cursor = state.input.len(),
        Input::ClearAll => {
            state.input.clear();
            state.cursor = 0;
        }
        Input::PageUp => state.scroll_offset += page(state),
        Input::PageDown => {
            state.scroll_offset = state.scroll_offset.saturating_sub(page(state));
        }
        Input::Enter => submit(state),
        _ => {}
    }
}

fn prev_boundary(s: &str, cursor: usize) -> Option<usize> {
    s[..cursor].char_indices().last().map(|(i, _)| i)
}

fn next_boundary(s: &str, cursor: usize) -> Option<usize> {
    s[cursor..].chars().next().map(|c| cursor + c.len_utf8())
}

/// Submit the current input line.
fn submit(state: &mut UiState) {
    let line = std::mem::take(&mut state.input);
    state.cursor = 0;
    if line.trim().is_empty() {
        return;
    }
    match parse(&line) {
        InputAction::Say(text) => {
            if state.active_target == 0 {
                emit_local(
                    state,
                    HistoryKind::Error {
                        text: "no active channel or query (^X to cycle, /join to join)".into(),
                    },
                );
            } else {
                let target = state.active_target_name().to_string();
                state
                    .pending
                    .push(ClientToDaemon::Command(ClientCommand::Msg { target, text }));
            }
        }
        InputAction::Command(cmd) => {
            state.pending.push(ClientToDaemon::Command(cmd));
        }
        InputAction::Detach => {
            state.pending.push(ClientToDaemon::Detach);
            state.should_quit = QuitKind::Detach;
        }
        InputAction::Exit(_reason) => {
            state.pending.push(ClientToDaemon::Exit);
            state.should_quit = QuitKind::Exit;
        }
        InputAction::Error(text) => emit_local(state, HistoryKind::Error { text }),
    }
}

/// Apply a message from the daemon, dispatching events through the hooks.
pub fn apply_msg(state: &mut UiState, msg: &DaemonToClient) {
    match msg {
        DaemonToClient::Hello { .. } => {}
        DaemonToClient::Snapshot(snapshot) | DaemonToClient::State(snapshot) => {
            state.snapshot = snapshot.clone();
            // Keep the active target index in range if targets disappeared.
            if state.active_target > state.snapshot.targets.len() {
                state.active_target = 0;
            }
            // Convenience: when the first target appears and we're on
            // (status), follow it (irssi switches to a channel on join).
            if state.active_target == 0 && state.snapshot.targets.len() == 1 {
                state.active_target = 1;
            }
        }
        DaemonToClient::History { items } => {
            for (seq, item) in items {
                state.history.push(item.clone());
                state.heights.push(None);
                state.last_seq = *seq;
            }
        }
        DaemonToClient::Event { seq, item } => {
            state.history.push(item.clone());
            state.heights.push(None);
            state.last_seq = *seq;
            run_hooks(state, &item.clone());
        }
        DaemonToClient::Bye { exiting } => {
            state.should_quit = if *exiting {
                QuitKind::Exit
            } else {
                QuitKind::Detach
            };
        }
    }
}

/// Dispatch one event through the hook registry.
///
/// The registry is rebuilt per event: hooks are stateless by contract (they
/// cannot outlive a dylib reload), and this keeps trait objects out of
/// [`UiState`].
fn run_hooks(state: &mut UiState, event: &HistoryItem) {
    let mut ctx = HookCtx::new(&state.snapshot.nick);
    for hook in &mut hooks::registry() {
        hook.on_event(event, &mut ctx);
    }
    let HookCtx { emitted, sent, .. } = ctx;
    for kind in emitted {
        emit_local(state, kind);
    }
    state.pending.extend(sent);
}

/// Render the full client: history, status bar, input line.
pub fn render(state: &mut UiState, frame: &mut Frame<'_>) {
    let chunks = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(frame.area());

    state.viewport_height = chunks[0].height;
    history_view::clamp_scroll(state, chunks[0].width, chunks[0].height);
    history_view::render(state, chunks[0], frame.buffer_mut());
    status_bar::render(state, chunks[1], frame.buffer_mut());

    // Input line: "[#target] input".
    let prompt = format!("[{}] ", state.active_target_name());
    let line = Line::from(vec![
        Span::styled(prompt.clone(), Style::default().fg(Color::Cyan)),
        Span::raw(state.input.clone()),
    ]);
    line.render(chunks[2], frame.buffer_mut());
    let cursor_x =
        chunks[2].x + (prompt.chars().count() + state.input[..state.cursor].chars().count()) as u16;
    frame.set_cursor_position((
        cursor_x.min(chunks[2].right().saturating_sub(1)),
        chunks[2].y,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyEventState};
    use irciii_proto::wire::{SessionSnapshot, TargetInfo, TargetKind};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    fn state_with_channel() -> UiState {
        let mut state = UiState::new();
        state.snapshot = SessionSnapshot {
            connected_to: Some("irc.libera.chat".into()),
            nick: "sshine".into(),
            user_modes: "+Ziw".into(),
            targets: vec![TargetInfo {
                name: "#diku".into(),
                kind: TargetKind::Channel,
                modes: "+n".into(),
                topic: None,
            }],
        };
        state.active_target = 1;
        state
    }

    #[test]
    fn typing_and_submit_says_in_active_target() {
        let mut state = state_with_channel();
        for c in "hello".chars() {
            handle_event(&mut state, &key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        handle_event(&mut state, &key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            state.pending,
            vec![ClientToDaemon::Command(ClientCommand::Msg {
                target: "#diku".into(),
                text: "hello".into(),
            })]
        );
        assert!(state.input.is_empty());
    }

    #[test]
    fn ctrl_x_cycles_target() {
        let mut state = state_with_channel();
        assert_eq!(state.active_target_name(), "#diku");
        handle_event(&mut state, &key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert_eq!(state.active_target_name(), "(status)");
        handle_event(&mut state, &key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert_eq!(state.active_target_name(), "#diku");
    }

    #[test]
    fn quit_command_queues_exit() {
        let mut state = state_with_channel();
        for c in "/quit".chars() {
            handle_event(&mut state, &key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        handle_event(&mut state, &key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(state.pending, vec![ClientToDaemon::Exit]);
        assert_eq!(state.should_quit, QuitKind::Exit);
    }

    #[test]
    fn event_appends_history_and_runs_hooks() {
        let mut state = state_with_channel();
        let item = HistoryItem::now(HistoryKind::Privmsg {
            nick: "n_ko".into(),
            target: "#diku".into(),
            text: "look at https://example.org".into(),
        });
        apply_msg(
            &mut state,
            &DaemonToClient::Event {
                seq: 7,
                item: item.clone(),
            },
        );
        assert_eq!(state.last_seq, 7);
        // The event itself plus the url-logger hook's status line.
        assert_eq!(state.history.len(), 2);
        assert_eq!(state.heights.len(), 2);
    }

    #[test]
    fn snapshot_follows_first_target() {
        let mut state = UiState::new();
        assert_eq!(state.active_target, 0);
        let snapshot = state_with_channel().snapshot;
        apply_msg(&mut state, &DaemonToClient::Snapshot(snapshot));
        assert_eq!(state.active_target_name(), "#diku");
    }
}
