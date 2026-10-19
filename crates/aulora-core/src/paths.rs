use std::path::PathBuf;

/// Base directory for aulora state. `$AULORA_HOME` overrides `~/.aulora`.
pub fn home() -> PathBuf {
    if let Some(h) = std::env::var_os("AULORA_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join(".aulora")
}

/// Unix socket paths are limited to ~104 bytes on macOS (108 on Linux).
/// When the home directory is too deep, fall back to a short path in /tmp
/// that is still unique per home directory.
pub fn socket() -> PathBuf {
    let p = home().join("aulora.sock");
    if p.as_os_str().len() <= 100 {
        return p;
    }
    let mut h: u32 = 0x811c9dc5;
    for b in home().as_os_str().as_encoded_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    PathBuf::from(format!("/tmp/aulora-{h:08x}.sock"))
}

pub fn db() -> PathBuf {
    home().join("state.db")
}

/// Hook events written here while the daemon is down.
pub fn spool() -> PathBuf {
    home().join("spool.jsonl")
}

/// Written when the daemon is stopped on purpose, so hooks firing as its
/// panes shut down don't start it again.
pub fn stopped_marker() -> PathBuf {
    home().join("stopped")
}

pub fn lock() -> PathBuf {
    home().join("daemon.lock")
}

pub fn log() -> PathBuf {
    home().join("daemon.log")
}

pub fn ensure_home() -> std::io::Result<PathBuf> {
    let h = home();
    std::fs::create_dir_all(&h)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&h, std::fs::Permissions::from_mode(0o700));
    }
    Ok(h)
}

/// Replace the home directory prefix with `~`.
pub fn tildify(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let h = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(h.as_ref())
            && (rest.is_empty() || rest.starts_with('/'))
        {
            return format!("~{rest}");
        }
    }
    path.to_string()
}
