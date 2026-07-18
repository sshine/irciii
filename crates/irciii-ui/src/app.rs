//! The UI worker's update and render logic, operating on [`UiState`].

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use irciii_proto::history::{HistoryItem, HistoryKind};
use irciii_proto::ui_state::{QuitKind, UiState};
use irciii_proto::wire::{ClientCommand, ClientToDaemon, DaemonToClient, TargetKind};
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

    // Any key skips the startup splash.
    if chrono::Utc::now().timestamp_millis() < state.splash_until_ms {
        state.splash_until_ms = 0;
        return;
    }

    // ^X cycles the active target; promptui's Input does not map Ctrl+X, so
    // match the raw event before converting.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('x') {
        state.cycle_target();
        return;
    }

    // ^Y yanks back what ^U killed (promptui doesn't map Ctrl+Y either).
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('y') {
        if !state.kill_ring.is_empty() {
            let restored = format!("{}{}", state.input, state.kill_ring);
            state.input = restored;
            // with_field rebuilds from the mismatch, cursor at the end.
        }
        return;
    }

    // Editing a recalled history item must not modify the original: the
    // moment its content changes it detaches into the current draft (and a
    // copy lands on top of the stack when submitted).
    let browse_pos = state.input_history_pos;
    let before = browse_pos.is_some().then(|| state.input.clone());

    match Input::from(*key) {
        Input::PageUp => state.scroll_offset += page(state),
        Input::PageDown => {
            state.scroll_offset = state.scroll_offset.saturating_sub(page(state));
        }
        // Up/Down recall input history while the field is a single visual
        // line; once the content has a line break or wraps, they navigate
        // between the field's lines instead (promptui behavior).
        Input::Up if !field_is_multiline(state) => history_recall(state, Direction::Back),
        Input::Down if !field_is_multiline(state) => history_recall(state, Direction::Forward),
        Input::Tab => {
            // /command completion first; otherwise complete the last word
            // against the active channel's members.
            let completed = crate::input::complete_command(&state.input).or_else(|| {
                let members = state
                    .active_target
                    .checked_sub(1)
                    .and_then(|i| state.snapshot.targets.get(i))
                    .map(|t| t.members.clone())
                    .unwrap_or_default();
                crate::input::complete_nick(&state.input, &members)
            });
            if let Some(completed) = completed {
                state.input = completed;
                // with_field sees the mismatch and rebuilds, cursor at end.
            }
        }
        // ^U kills the buffer into the kill ring (restored by ^Y).
        Input::ClearAll => {
            if !state.input.is_empty() {
                state.kill_ring = state.input.clone();
            }
            text_input::with_field(state, |field| field.input(Input::ClearAll));
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

    // Content changed while browsing (and not by the recall itself, which
    // moves the position): detach from the history item into a fresh draft.
    if let (Some(pos), Some(before)) = (browse_pos, before)
        && state.input_history_pos == Some(pos)
        && state.input != before
    {
        state.input_history_pos = None;
    }
}

/// Whether the field content occupies more than one visual line: it contains
/// a line break, or it is long enough to wrap at the current width.
fn field_is_multiline(state: &UiState) -> bool {
    if state.input.contains('\n') {
        return true;
    }
    // Approximate the field's wrap width: viewport width minus border,
    // padding, scrollbar, and cursor reserve.
    let width = if state.heights_width == 0 {
        80
    } else {
        state.heights_width
    };
    let wrap = width.saturating_sub(8).max(8) as usize;
    unicode_width::UnicodeWidthStr::width(state.input.as_str()) > wrap
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
        // Bare /who implies the active channel; on (status) or a query it
        // stays bare (the server decides what a plain WHO means).
        InputAction::Command(ClientCommand::Who { mask: None }) => {
            let mask = state
                .active_target
                .checked_sub(1)
                .and_then(|i| state.snapshot.targets.get(i))
                .filter(|t| t.kind == TargetKind::Channel)
                .map(|t| t.name.clone());
            state
                .pending
                .push(ClientToDaemon::Command(ClientCommand::Who { mask }));
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
        InputAction::Help => emit_local(state, HistoryKind::Help),
        InputAction::Clear => {
            // Remove clearable blocks (whois, help) from the log entirely.
            state.history.retain(|item| !item.kind.is_card());
            state.heights.clear();
            state.heights.resize(state.history.len(), None);
            state.anchored_len = state.history.len();
            state.scroll_offset = 0;
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
            // (status) is not part of the cycle while targets exist: follow
            // the first target whenever we're sitting on (status).
            if state.active_target == 0 && !state.snapshot.targets.is_empty() {
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
    if chrono::Utc::now().timestamp_millis() < state.splash_until_ms {
        crate::splash::render(frame);
        return;
    }
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

    render_command_overlay(state, frame, chunks[0]);
}

/// While typing a `/command`, overlay the matching commands (with their
/// one-line summaries) just above the input area; the list narrows as more
/// characters are typed.
fn render_command_overlay(
    state: &UiState,
    frame: &mut Frame<'_>,
    history_area: ratatui::layout::Rect,
) {
    let Some(matches) = crate::input::command_matches(&state.input) else {
        return;
    };
    if matches.is_empty() {
        return;
    }
    let theme = promptui_core::theme::Theme::default();
    let rows = (matches.len() as u16).min(history_area.height);
    let area = ratatui::layout::Rect::new(
        history_area.x,
        history_area.bottom().saturating_sub(rows),
        history_area.width,
        rows,
    );
    let bg = ratatui::style::Style::default().bg(theme.element_bg);
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if let Some(cell) = frame.buffer_mut().cell_mut((x, y)) {
                cell.set_char(' ');
                cell.set_style(bg);
            }
        }
    }
    for (row, (name, summary)) in matches.iter().take(rows as usize).enumerate() {
        let line = ratatui::text::Line::from(vec![
            ratatui::text::Span::styled(
                format!(" /{name:<12}"),
                // Commands render in the shared accent cyan everywhere.
                ratatui::style::Style::default()
                    .fg(crate::styles::ACCENT)
                    .bg(theme.element_bg)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ),
            ratatui::text::Span::styled(
                (*summary).to_string(),
                ratatui::style::Style::default()
                    .fg(theme.text_muted)
                    .bg(theme.element_bg),
            ),
        ]);
        line.render(
            ratatui::layout::Rect::new(area.x, area.y + row as u16, area.width, 1),
            frame.buffer_mut(),
        );
    }
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
        state.splash_until_ms = 0; // tests don't want keys eaten by the splash
        state.snapshot = SessionSnapshot {
            connected_to: Some("irc.libera.chat".into()),
            nick: "sshine".into(),
            user_modes: "+Ziw".into(),
            targets: vec![TargetInfo {
                name: "#diku".into(),
                kind: TargetKind::Channel,
                modes: "+n".into(),
                topic: None,
                members: Vec::new(),
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
        // (status) is skipped while at least one target exists.
        handle_event(&mut state, &key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert_eq!(state.active_target_name(), "#diku");

        state.snapshot.targets.push(TargetInfo {
            name: "n_ko".into(),
            kind: TargetKind::Query,
            modes: String::new(),
            topic: None,
            members: Vec::new(),
        });
        handle_event(&mut state, &key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert_eq!(state.active_target_name(), "n_ko");
        handle_event(&mut state, &key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert_eq!(state.active_target_name(), "#diku");
    }

    #[test]
    fn tab_completes_nick_in_channel() {
        let mut state = state_with_channel();
        state.snapshot.targets[0].members = vec![
            "sshine".into(),
            "zfnmxt".into(),
            "n_ko".into(),
            "concieggs".into(),
        ];
        // At line start: unique match gets ", ".
        for c in "zfn".chars() {
            handle_event(&mut state, &key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        handle_event(&mut state, &key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(state.input, "zfnmxt, ");
        // Mid-line: unique match gets a space.
        for c in "hi n_".chars() {
            handle_event(&mut state, &key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        handle_event(&mut state, &key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(state.input, "zfnmxt, hi n_ko ");
    }

    #[test]
    fn bare_who_implies_active_channel_but_not_query() {
        // Active channel: /who fills in the channel name.
        let mut state = state_with_channel();
        for c in "/who".chars() {
            handle_event(&mut state, &key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        handle_event(&mut state, &key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            state.pending,
            vec![ClientToDaemon::Command(ClientCommand::Who {
                mask: Some("#diku".into()),
            })]
        );

        // Active query: /who stays bare.
        let mut state = state_with_channel();
        state.snapshot.targets.push(TargetInfo {
            name: "n_ko".into(),
            kind: TargetKind::Query,
            modes: String::new(),
            topic: None,
            members: Vec::new(),
        });
        state.active_target = 2;
        for c in "/who".chars() {
            handle_event(&mut state, &key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        handle_event(&mut state, &key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            state.pending,
            vec![ClientToDaemon::Command(ClientCommand::Who { mask: None })]
        );
    }

    #[test]
    fn clear_removes_clearable_blocks() {
        let mut state = state_with_channel();
        emit_local(&mut state, HistoryKind::Help);
        emit_local(
            &mut state,
            HistoryKind::ClientStatus {
                text: "keep".into(),
            },
        );
        assert_eq!(state.history.len(), 2);
        for c in "/clear".chars() {
            handle_event(&mut state, &key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        handle_event(&mut state, &key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(state.history.len(), 1, "help block removed, status kept");
        assert!(matches!(
            state.history[0].kind,
            HistoryKind::ClientStatus { .. }
        ));
        assert_eq!(state.heights.len(), 1);
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
    fn up_down_navigate_wrapped_or_multiline_content() {
        let mut state = state_with_channel();
        state.heights_width = 40;
        // A line longer than the wrap width: Up must NOT recall history.
        state.input_history.push("older entry".into());
        state.input = "a".repeat(60);
        handle_event(&mut state, &key(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(
            state.input,
            "a".repeat(60),
            "wrapped line: Up navigates, not recalls"
        );
        assert_eq!(state.input_history_pos, None);

        // A short line: Up recalls.
        state.input = "hi".into();
        handle_event(&mut state, &key(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(state.input, "older entry");
        assert_eq!(state.input_history_pos, Some(0));
    }

    #[test]
    fn editing_recalled_item_detaches_into_draft() {
        let mut state = state_with_channel();
        for c in "original".chars() {
            handle_event(&mut state, &key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        handle_event(&mut state, &key(KeyCode::Enter, KeyModifiers::NONE));
        handle_event(&mut state, &key(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(state.input, "original");
        assert_eq!(state.input_history_pos, Some(0));

        // Editing detaches: the position resets, the original stays intact.
        handle_event(&mut state, &key(KeyCode::Char('!'), KeyModifiers::NONE));
        assert_eq!(state.input, "original!");
        assert_eq!(state.input_history_pos, None);
        assert_eq!(state.input_history, vec!["original"]);

        // Submitting the edit puts the copy on top of the stack.
        handle_event(&mut state, &key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(state.input_history, vec!["original", "original!"]);
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
