//! Images on the clipboard, for prompts written in aulora. Terminals only
//! paste text, so an image is saved to a file whose path goes in the prompt.

use std::path::PathBuf;
use std::process::Command;

/// Save the clipboard's image as a PNG under ~/.aulora/images. None when
/// the clipboard holds no image.
pub fn save_image() -> Result<Option<PathBuf>, String> {
    let dir = aulora_core::paths::home().join("images");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%3f");
    let path = dir.join(format!("paste-{stamp}.png"));
    if cfg!(target_os = "macos") {
        let info = Command::new("osascript").args(["-e", "clipboard info"]).output().map_err(|e| e.to_string())?;
        let info = String::from_utf8_lossy(&info.stdout);
        if !info.contains("PNGf") && !info.contains("TIFF") {
            return Ok(None);
        }
        let script = format!(
            "set f to open for access (POSIX file \"{}\") with write permission\n\
             try\n write (the clipboard as «class PNGf») to f\n end try\n close access f",
            path.display()
        );
        let out = Command::new("osascript").args(["-e", &script]).output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
    } else {
        // Wayland, then X11.
        let out = Command::new("wl-paste")
            .args(["--type", "image/png"])
            .output()
            .or_else(|_| Command::new("xclip").args(["-selection", "clipboard", "-t", "image/png", "-o"]).output())
            .map_err(|e| format!("no clipboard tool (wl-paste or xclip): {e}"))?;
        if !out.status.success() || out.stdout.is_empty() {
            return Ok(None);
        }
        std::fs::write(&path, &out.stdout).map_err(|e| e.to_string())?;
    }
    match std::fs::metadata(&path) {
        Ok(m) if m.len() > 0 => Ok(Some(path)),
        _ => {
            let _ = std::fs::remove_file(&path);
            Ok(None)
        }
    }
}

/// Put text on the clipboard. Locally through the system's clipboard tool
/// (Terminal.app ignores OSC 52); OSC 52 as well, which reaches the local
/// clipboard over SSH in terminals that support it.
pub fn copy(text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{}\x07", aulora_core::proto::encode(text.as_bytes()));
    let _ = out.flush();
    if std::env::var_os("SSH_CONNECTION").is_some() {
        return Ok(());
    }
    let tools: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else {
        &[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"]), ("xsel", &["--clipboard", "--input"])]
    };
    let mut last = String::from("no clipboard tool found");
    for (tool, args) in tools {
        let child = Command::new(tool)
            .args(*args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match child {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                return match child.wait() {
                    Ok(s) if s.success() => Ok(()),
                    Ok(s) => Err(format!("{tool} failed ({s})")),
                    Err(e) => Err(e.to_string()),
                };
            }
            Err(e) => last = format!("{tool}: {e}"),
        }
    }
    Err(last)
}
