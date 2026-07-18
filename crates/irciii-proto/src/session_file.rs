//! Session discovery files.
//!
//! The daemon binds `127.0.0.1:0` and writes a small JSON file keyed by its
//! session uuid; `irciii --attach <uuid>` reads the file to find the port.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Contents of a session discovery file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFile {
    /// Host the daemon listens on (default 127.0.0.1).
    pub host: String,
    /// TCP port the daemon listens on (default 6678).
    pub port: u16,
    /// SHA-256 fingerprint (lowercase hex) of the daemon's self-signed TLS
    /// certificate; clients pin this instead of PKI verification.
    pub fingerprint: String,
    /// Daemon process id.
    pub pid: u32,
    /// RFC 3339 timestamp of daemon start.
    pub started: String,
}

/// List all session ids that currently have a session file.
pub fn list() -> io::Result<Vec<(Uuid, SessionFile)>> {
    let mut sessions = Vec::new();
    let dir = match fs::read_dir(session_dir()) {
        Ok(dir) => dir,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(sessions),
        Err(err) => return Err(err),
    };
    for entry in dir {
        let entry = entry?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|n| n.strip_suffix(".json"))
            .and_then(|n| n.parse::<Uuid>().ok())
        else {
            continue;
        };
        if let Ok(contents) = read(id) {
            sessions.push((id, contents));
        }
    }
    sessions.sort_by(|a, b| a.1.started.cmp(&b.1.started));
    Ok(sessions)
}

/// Directory holding session files: `$XDG_RUNTIME_DIR/irciii`, falling back to
/// `/tmp/irciii-$UID`.
pub fn session_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("irciii"),
        _ => {
            // No portable uid without libc; the euid via rustix/libc is overkill
            // here. `/tmp/irciii-$USER` is unique enough for the fallback path.
            let user = std::env::var("USER").unwrap_or_else(|_| "unknown".into());
            PathBuf::from(format!("/tmp/irciii-{user}"))
        }
    }
}

/// Path of the session file for a session id.
pub fn session_path(session_id: Uuid) -> PathBuf {
    session_dir().join(format!("{session_id}.json"))
}

/// Write a session file (creating the directory, mode 0700/0600 on unix).
pub fn write(session_id: Uuid, contents: &SessionFile) -> io::Result<PathBuf> {
    let dir = session_dir();
    fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    }
    let path = session_path(session_id);
    let json = serde_json::to_string_pretty(contents).map_err(io::Error::other)?;
    fs::write(&path, json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(path)
}

/// Read a session file by session id.
pub fn read(session_id: Uuid) -> io::Result<SessionFile> {
    let json = fs::read_to_string(session_path(session_id))?;
    serde_json::from_str(&json).map_err(io::Error::other)
}

/// Remove a session file; missing files are not an error.
pub fn remove(session_id: Uuid) -> io::Result<()> {
    match fs::remove_file(session_path(session_id)) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_via_tempdir() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Redirect XDG_RUNTIME_DIR for this test.
        // SAFETY: tests in this module run single-threaded over env access.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", dir.path()) };
        let id = Uuid::new_v4();
        let contents = SessionFile {
            host: "127.0.0.1".into(),
            port: 45678,
            fingerprint: "ab".repeat(32),
            pid: 1234,
            started: "2026-07-18T12:00:00Z".into(),
        };
        let path = write(id, &contents).expect("write");
        assert!(path.starts_with(dir.path()));
        assert_eq!(read(id).expect("read"), contents);
        remove(id).expect("remove");
        assert!(read(id).is_err());
        remove(id).expect("remove is idempotent");
    }
}
