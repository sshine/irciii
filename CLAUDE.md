# irciii

A minimal irssi-like terminal IRC client.

## Architecture

This is a Cargo workspace with crates under `crates/`:

- **irciii-proto**: Shared plain-data types: history items, the client↔daemon wire protocol
  (postcard over TCP, u32-LE length-prefixed), session discovery files, and `UiState` (the
  hot-reload-safe state passed into the UI worker)
- **irciii-daemon**: Owns IRC connections (the `irc` crate), the canonical chat history, and
  serves attached clients over TCP; survives `/detach`
- **irciii-ui**: The hot-reloadable UI worker (`crate-type = ["lib", "cdylib"]`): all
  rendering, command parsing, and the hook registry
- **irciii-client**: The host process: terminal, TCP connection to the daemon, and the
  (optionally hot-reloaded) UI worker; `hot-reload` feature gates libloading
- **irciii**: The CLI binary; spawns daemon + client, or `--attach <session-id>`
- **conventional-commit-check**: The commit-msg git hook binary

Constraints to preserve:

- `UiState` must stay plain data — no trait objects or fn pointers (dangling vtables after a
  dylib reload). Hooks live entirely inside `irciii-ui` and never cross the FFI boundary.
- The `hot-reload` feature is dev-only; release/nix builds link `irciii-ui` statically.
- History is structured (`HistoryKind`), never pre-rendered; rendering happens per visible
  line so presentation changes (e.g. timestamp format) re-render reactively.
- Hooks only fire while a client is attached (accepted tradeoff).

## Development

### Nix (recommended)

This project uses a Nix flake for reproducible development:

```sh
direnv allow    # auto-activates the dev shell
# or manually:
nix develop     # enter the dev shell
```

Git hooks are managed by hk-nix (pre-commit: treefmt; pre-push: deadnix, clippy,
readme-check, lock-check; commit-msg: conventional commits).

### Common commands

```sh
just            # list all available commands
just fmt        # format code
just lint       # run clippy
just test       # run all tests
just test-net   # network tests against irc.libera.chat (not part of ci)
just dev        # run the client with hot-reloading of irciii-ui
just ci         # run all CI checks locally
```

### Snapshot testing

Uses [insta](https://insta.rs/) for snapshot tests:

```sh
just snap       # run tests and review snapshot changes
```

## Conventions

- Use `thiserror` for error types; avoid `unwrap()` (denied by clippy lint)
- All public items should have doc comments
- Run `just ci` before pushing
- Use insta snapshots for rendered history lines and complex data structures
