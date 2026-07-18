//! Hot reloading of the `irciii-ui` dylib (dev only, `hot-reload` feature).
//!
//! Watches the built `libirciii_ui` artifact's mtime; on change the old
//! library is dropped and the new one loaded, while the caller keeps the same
//! plain-data `UiState`. The artifact is copied to a scratch path with a
//! counter suffix before loading so the dynamic loader never caches a stale
//! image for the same path.

use std::path::PathBuf;
use std::time::SystemTime;

use crossterm::event::Event;
use irciii_proto::ui_state::UiState;
use irciii_proto::wire::DaemonToClient;
use libloading::{Library, Symbol};
use ratatui::Frame;

type HandleEventFn = extern "C" fn(&mut UiState, &Event);
type ApplyMsgFn = extern "C" fn(&mut UiState, &DaemonToClient);
type RenderFn = for<'a, 'b> extern "C" fn(&'a mut UiState, &'a mut Frame<'b>);

/// Where the freshly built UI dylib lives (see `just dev`).
fn lib_path() -> PathBuf {
    match std::env::var_os("IRCIII_UI_LIB") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/debug/libirciii_ui.so"
        )),
    }
}

/// Loads and reloads the UI dylib.
pub struct Reloader {
    lib: Option<Library>,
    loaded_mtime: Option<SystemTime>,
    counter: u64,
}

impl Reloader {
    /// Load the dylib for the first time (panics if it is missing: run
    /// `cargo build -p irciii-ui` or use `just dev`).
    pub fn new() -> Self {
        let mut this = Self {
            lib: None,
            loaded_mtime: None,
            counter: 0,
        };
        assert!(
            this.maybe_reload(),
            "hot-reload: cannot load {} (build it with: cargo build -p irciii-ui)",
            lib_path().display()
        );
        this
    }

    fn mtime() -> Option<SystemTime> {
        std::fs::metadata(lib_path())
            .and_then(|m| m.modified())
            .ok()
    }

    /// Reload if the artifact changed; returns true when a (re)load happened.
    pub fn maybe_reload(&mut self) -> bool {
        let Some(mtime) = Self::mtime() else {
            return false;
        };
        if self.loaded_mtime == Some(mtime) {
            return false;
        }
        // Copy to a fresh path so dlopen doesn't hand back the cached image.
        self.counter += 1;
        let scratch = std::env::temp_dir().join(format!(
            "irciii-ui-hot-{}-{}.so",
            std::process::id(),
            self.counter
        ));
        if std::fs::copy(lib_path(), &scratch).is_err() {
            return false;
        }
        // Drop the old library before loading the new one.
        self.lib = None;
        // SAFETY: the dylib is our own workspace crate built by the same
        // compiler; see the module docs of `irciii_ui::api`.
        match unsafe { Library::new(&scratch) } {
            Ok(lib) => {
                self.lib = Some(lib);
                self.loaded_mtime = Some(mtime);
                true
            }
            Err(_) => false,
        }
    }

    fn lib(&self) -> &Library {
        self.lib
            .as_ref()
            .expect("Reloader::new ensures a library is loaded")
    }

    /// Handle one terminal event.
    pub fn handle_event(&mut self, state: &mut UiState, event: &Event) {
        // SAFETY: symbol signature matches irciii_ui::api::ui_handle_event.
        let f: Symbol<'_, HandleEventFn> = unsafe {
            self.lib()
                .get(b"ui_handle_event")
                .expect("symbol ui_handle_event")
        };
        f(state, event);
    }

    /// Apply one daemon message.
    pub fn apply_msg(&mut self, state: &mut UiState, msg: &DaemonToClient) {
        // SAFETY: symbol signature matches irciii_ui::api::ui_apply_msg.
        let f: Symbol<'_, ApplyMsgFn> = unsafe {
            self.lib()
                .get(b"ui_apply_msg")
                .expect("symbol ui_apply_msg")
        };
        f(state, msg);
    }

    /// Render one frame.
    pub fn render(&mut self, state: &mut UiState, frame: &mut Frame<'_>) {
        // SAFETY: symbol signature matches irciii_ui::api::ui_render.
        let f: Symbol<'_, RenderFn> =
            unsafe { self.lib().get(b"ui_render").expect("symbol ui_render") };
        f(state, frame);
    }
}

impl Default for Reloader {
    fn default() -> Self {
        Self::new()
    }
}
