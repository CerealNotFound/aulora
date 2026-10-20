//! `aulora vscode`: package the editor extension (editors/vscode) as a
//! .vsix and install it with the editor's own CLI. The extension's files
//! are built into this binary, so this works from `cargo install` too.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

const PACKAGE_JSON: &str = include_str!("../../../editors/vscode/package.json");
const EXTENSION_JS: &str = include_str!("../../../editors/vscode/extension.js");
const README: &str = include_str!("../../../editors/vscode/README.md");

pub const ID: &str = "aulora.aulora";

fn version() -> String {
    serde_json::from_str::<serde_json::Value>(PACKAGE_JSON)
        .ok()
        .and_then(|v| v["version"].as_str().map(str::to_string))
        .unwrap_or_else(|| "0.0.0".into())
}

fn manifest(version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011" xmlns:d="http://schemas.microsoft.com/developer/vsx-schema-design/2011">
  <Metadata>
    <Identity Language="en-US" Id="aulora" Version="{version}" Publisher="aulora" />
    <DisplayName>aulora</DisplayName>
    <Description xml:space="preserve">Lets the aulora dashboard type prompts into the Claude Code sessions running in your terminals.</Description>
    <Categories>Other</Categories>
    <GalleryFlags>Public</GalleryFlags>
    <Properties>
      <Property Id="Microsoft.VisualStudio.Code.Engine" Value="^1.75.0" />
      <Property Id="Microsoft.VisualStudio.Code.ExtensionKind" Value="workspace" />
    </Properties>
  </Metadata>
  <Installation>
    <InstallationTarget Id="Microsoft.VisualStudio.Code" />
  </Installation>
  <Dependencies />
  <Assets>
    <Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true" />
    <Asset Type="Microsoft.VisualStudio.Services.Content.Details" Path="extension/README.md" Addressable="true" />
  </Assets>
</PackageManifest>
"#
    )
}

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension=".json" ContentType="application/json" /><Default Extension=".js" ContentType="application/javascript" /><Default Extension=".md" ContentType="text/markdown" /><Default Extension=".vsixmanifest" ContentType="text/xml" /></Types>
"#;

/// The extension as a .vsix: a zip in the layout `vsce` produces.
pub fn vsix() -> Vec<u8> {
    let v = version();
    zip(&[
        ("extension.vsixmanifest", manifest(&v).as_bytes()),
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        ("extension/package.json", PACKAGE_JSON.as_bytes()),
        ("extension/extension.js", EXTENSION_JS.as_bytes()),
        ("extension/README.md", README.as_bytes()),
    ])
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// A zip archive with every file stored uncompressed.
fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    const DOS_DATE: u16 = 0x21; // 1980-01-01
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let (n, len) = (name.len() as u16, data.len() as u32);
        let mut local = Vec::new();
        local.extend(0x0403_4b50u32.to_le_bytes());
        local.extend(20u16.to_le_bytes()); // version needed
        local.extend(0u16.to_le_bytes()); // flags
        local.extend(0u16.to_le_bytes()); // stored
        local.extend(0u16.to_le_bytes()); // time
        local.extend(DOS_DATE.to_le_bytes());
        local.extend(crc.to_le_bytes());
        local.extend(len.to_le_bytes());
        local.extend(len.to_le_bytes());
        local.extend(n.to_le_bytes());
        local.extend(0u16.to_le_bytes()); // extra
        out.extend(&local);
        out.extend(name.as_bytes());
        out.extend(*data);

        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend(20u16.to_le_bytes()); // version made by
        central.extend(&local[4..30]);
        central.extend(0u16.to_le_bytes()); // comment
        central.extend(0u16.to_le_bytes()); // disk
        central.extend(0u16.to_le_bytes()); // internal attributes
        central.extend(0u32.to_le_bytes()); // external attributes
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let cd_offset = out.len() as u32;
    out.extend(&central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(cd_offset.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

/// The editor's command-line tool: on PATH, or inside the macOS app.
pub fn editor_cli(editor: &str) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join(editor);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let app = match editor {
        "code" => "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",
        "code-insiders" => "/Applications/Visual Studio Code - Insiders.app/Contents/Resources/app/bin/code",
        "cursor" => "/Applications/Cursor.app/Contents/Resources/app/bin/cursor",
        "windsurf" => "/Applications/Windsurf.app/Contents/Resources/app/bin/windsurf",
        _ => return None,
    };
    Path::new(app).is_file().then(|| PathBuf::from(app))
}

fn cli(editor: &str) -> Result<PathBuf> {
    editor_cli(editor).with_context(|| format!("couldn't find `{editor}`; install its shell command, or pass --editor"))
}

pub fn install(editor: &str) -> Result<()> {
    let cli = cli(editor)?;
    let dir = std::env::temp_dir().join(format!("aulora-vsix-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!("aulora-{}.vsix", version()));
    std::fs::write(&file, vsix())?;
    let out = Command::new(&cli).arg("--install-extension").arg(&file).arg("--force").output();
    let _ = std::fs::remove_dir_all(&dir);
    let out = out.with_context(|| format!("running {}", cli.display()))?;
    if !out.status.success() {
        bail!("{} failed: {}", editor, String::from_utf8_lossy(&out.stderr).trim());
    }
    println!("installed the aulora extension {} in {editor}", version());
    println!("reload open windows to start it (Command Palette → Developer: Reload Window)");
    Ok(())
}

pub fn uninstall(editor: &str) -> Result<()> {
    let cli = cli(editor)?;
    let out = Command::new(&cli).args(["--uninstall-extension", ID]).output()?;
    if !out.status.success() {
        bail!("{} failed: {}", editor, String::from_utf8_lossy(&out.stderr).trim());
    }
    println!("removed the aulora extension from {editor}");
    Ok(())
}

/// The installed version, if any.
pub fn installed(editor: &str) -> Option<String> {
    let out = Command::new(editor_cli(editor)?).args(["--list-extensions", "--show-versions"]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| l.strip_prefix(&format!("{ID}@")).map(str::to_string))
}

pub fn status(editor: &str) -> Result<()> {
    match installed(editor) {
        Some(v) if v == version() => println!("installed in {editor} ({v})"),
        Some(v) => println!("installed in {editor} ({v}); this aulora has {}: run `aulora vscode install`", version()),
        None => println!("not installed in {editor}: run `aulora vscode install`"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_standard() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn vsix_is_a_zip_with_the_extension() {
        let z = vsix();
        assert_eq!(&z[..4], b"PK\x03\x04");
        let eocd = z.len() - 22;
        assert_eq!(&z[eocd..eocd + 4], b"PK\x05\x06");
        assert_eq!(u16::from_le_bytes([z[eocd + 10], z[eocd + 11]]), 5);
        let text = String::from_utf8_lossy(&z);
        assert!(text.contains("extension/extension.js"));
        assert!(text.contains(r#"Version="0.2.2""#));
    }
}
