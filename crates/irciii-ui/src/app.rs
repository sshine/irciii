//! The UI worker's update and render logic, operating on [`UiState`].

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use irciii_proto::history::{HistoryItem, HistoryKind};
use irciii_proto::ui_state::{QuitKind, UiState};
use irciii_proto::wire::{ClientCommand, ClientToDaemon, DaemonToClient};
use promptui_core::input::Input;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::Widget;

use promptui_core::widgets::text_field::TextFieldAction;

use crate::hooks::{self, HookCtx};
use crate::input::{InputAction, parse};
use crate::{history_view, status_bar, text_input};

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
        Input::PageUp => state.scroll_offset += page(state),
        Input::PageDown => {
            state.scroll_offset = state.scroll_offset.saturating_sub(page(state));
        }
        // Up/Down recall input history on a single-line draft; once the field
        // is multi-line they navigate between its lines instead.
        Input::Up if !state.input.contains('\n') => history_recall(state, Direction::Back),
        Input::Down if !state.input.contains('\n') => history_recall(state, Direction::Forward),
        Input::Tab => {
            if let Some(completed) = crate::input::complete_command(&state.input) {
                state.input = completed;
                // with_field sees the mismatch and rebuilds, cursor at end.
            }
        }
        Input::Enter => {
            let action = text_input::with_field(state, |field| field.input(Input::Enter));
            if let TextFieldAction::Submit(content) = action {
                submit(state, content);
            }
        }
        input => {
            text_input::with_field(state, |field| field.input(input));
        }
    }
}

enum Direction {
    Back,
    Forward,
}

/// Readline-style input history: Up walks back through submitted lines
/// (stashing the in-progress draft), Down walks forward and finally restores
/// the draft.
fn history_recall(state: &mut UiState, dir: Direction) {
    let next = match (dir, state.input_history_pos) {
        (Direction::Back, None) if !state.input_history.is_empty() => {
            state.input_draft = std::mem::take(&mut state.input);
            Some(state.input_history.len() - 1)
        }
        (Direction::Back, Some(pos)) => Some(pos.saturating_sub(1)),
        (Direction::Forward, Some(pos)) if pos + 1 < state.input_history.len() => Some(pos + 1),
        (Direction::Forward, Some(_)) => None,
        _ => return, // Up with empty history, or Down while not browsing
    };
    state.input_history_pos = next;
    state.input = match next {
        Some(pos) => state.input_history[pos].clone(),
        None => std::mem::take(&mut state.input_draft),
    };
    state.cursor = state.input.len();
}

/// Submit field content: it becomes one history entry, and every non-empty
/// line becomes its own message/command (multi-line input expands into
/// multiple messages; the daemon further splits over-long lines to the
/// server's maximum message length).
fn submit(state: &mut UiState, content: String) {
    state.input.clear();
    state.input_history_pos = None;
    state.input_draft.clear();
    if state.input_history.last() != Some(&content) {
        state.input_history.push(content.clone());
    }
    for line in content.lines() {
        if !line.trim().is_empty() {
            submit_line(state, line);
        }
    }
}

/// Dispatch one submitted line.
fn submit_line(state: &mut UiState, line: &str) {
    match parse(line) {
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
        InputAction::Help => {
            for line in crate::input::HELP {
                emit_local(
                    state,
                    HistoryKind::ClientStatus {
                        text: (*line).to_string(),
                    },
                );
            }
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

/// Render the full client: history, status bar, and the promptui text field
/// (which grows with its content, up to its max height).
pub fn render(state: &mut UiState, frame: &mut Frame<'_>) {
    let width = frame.area().width;
    let input_height = text_input::with_field(state, |field| field.required_height(width));
    let chunks = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(input_height),
    ])
    .split(frame.area());

    state.viewport_height = chunks[0].height;
    history_view::clamp_scroll(state, chunks[0].width, chunks[0].height);
    history_view::render(state, chunks[0], frame.buffer_mut());
    status_bar::render(state, chunks[1], frame.buffer_mut());

    text_input::with_field(state, |field| {
        (&*field).render(chunks[2], frame.buffer_mut());
        if let Some(pos) = field.cursor_position(chunks[2]) {
            frame.set_cursor_position(pos);
        }
    });
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
    fn multiline_input_expands_into_multiple_messages() {
        let mut state = state_with_channel();
        for c in "one".chars() {
            handle_event(&mut state, &key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        // Alt+Enter inserts a newline (promptui Input::Newline).
        handle_event(&mut state, &key(KeyCode::Enter, KeyModifiers::ALT));
        for c in "two".chars() {
            handle_event(&mut state, &key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        handle_event(&mut state, &key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            state.pending,
            vec![
                ClientToDaemon::Command(ClientCommand::Msg {
                    target: "#diku".into(),
                    text: "one".into(),
                }),
                ClientToDaemon::Command(ClientCommand::Msg {
                    target: "#diku".into(),
                    text: "two".into(),
                }),
            ]
        );
        assert!(state.input.is_empty());
        // The whole multi-line entry is one history item.
        assert_eq!(state.input_history, vec!["one\ntwo"]);
    }

    #[test]
    fn input_history_up_down_with_draft() {
        let mut state = state_with_channel();
        for line in ["first", "second"] {
            for c in line.chars() {
                handle_event(&mut state, &key(KeyCode::Char(c), KeyModifiers::NONE));
            }
            handle_event(&mut state, &key(KeyCode::Enter, KeyModifiers::NONE));
        }
        // Start a draft, then browse back.
        for c in "dra".chars() {
            handle_event(&mut state, &key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        handle_event(&mut state, &key(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(state.input, "second");
        handle_event(&mut state, &key(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(state.input, "first");
        handle_event(&mut state, &key(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(state.input, "first", "stops at the oldest entry");
        handle_event(&mut state, &key(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(state.input, "second");
        handle_event(&mut state, &key(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(state.input, "dra", "draft restored past the newest entry");
        assert_eq!(state.input_history_pos, None);
    }

    #[test]
    fn tab_completes_commands() {
        let mut state = state_with_channel();
        for c in "/con".chars() {
            handle_event(&mut state, &key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        handle_event(&mut state, &key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(state.input, "/connect ", "unique match gets a space");

        state.input = "/n".into();
        state.cursor = state.input.len();
        handle_event(&mut state, &key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            state.input, "/n",
            "ambiguous nick/notice/names: no progress"
        );

        state.input = "/no".into();
        state.cursor = state.input.len();
        handle_event(&mut state, &key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(state.input, "/notice ");

        state.input = "/msg someone hi".into();
        state.cursor = state.input.len();
        handle_event(&mut state, &key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            state.input, "/msg someone hi",
            "no completion past the command"
        );
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
