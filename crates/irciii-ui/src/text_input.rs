//! The promptui [`TextField`] input area, synced with plain [`UiState`] state.
//!
//! `UiState` must stay plain data (no widgets, no fn pointers) so it survives
//! dylib reloads. The `TextField` therefore lives in worker-local storage and
//! is kept in sync with `state.input`: whenever the two disagree (fresh
//! worker, dylib reload, or history recall writing into `state.input`), the
//! field is rebuilt by replaying the content. The cursor lands at the end in
//! that case, which is exactly what recall and reload want.

use std::cell::RefCell;

use irciii_proto::ui_state::UiState;
use promptui_core::input::Input;
use promptui_core::theme::Theme;
use promptui_core::widgets::text_field::TextField;

thread_local! {
    static FIELD: RefCell<Option<TextField>> = const { RefCell::new(None) };
}

fn build(content: &str) -> TextField {
    let mut field = TextField::new(Theme::default());
    field.set_max_height(8);
    for c in content.chars() {
        let _ = field.input(if c == '\n' {
            Input::Newline
        } else {
            Input::Char(c)
        });
    }
    field
}

/// Run `f` against the input field, keeping `state.input` as the mirror of
/// the field's content.
pub fn with_field<R>(state: &mut UiState, f: impl FnOnce(&mut TextField) -> R) -> R {
    FIELD.with(|cell| {
        let mut slot = cell.borrow_mut();
        let stale = match slot.as_ref() {
            Some(field) => field.content() != state.input,
            None => true,
        };
        if stale {
            *slot = Some(build(&state.input));
        }
        let field = slot.as_mut().expect("field built above");
        let result = f(field);
        state.input = field.content();
        result
    })
}
