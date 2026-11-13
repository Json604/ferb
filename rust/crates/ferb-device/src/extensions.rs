//! FERB's managed unpacked extension (ferb/device/extensions.py): published whole at a stable path, the manifest
//! last. The setup window's per-profile rows (`not_paired_step`, `browser_rows`) are built from
//! ferb/act/browser/autoinstall.py's profile states and go with that port.

use std::path::{Path, PathBuf};

use crate::install::InstallError;

/// The extension's folder under FERB's home.
pub fn extension_path(home: &Path) -> PathBuf {
    home.join("extensions").join("chromium")
}

/// extensions.py:553: copies `source` to `extension_path(home)` through a staged folder beside it, every file before
/// `manifest.json`. A bundle without a version or a background script is refused (ValueError).
pub fn install_extension(home: &Path, source: &Path) -> Result<PathBuf, InstallError> {
    let manifest_path = source.join("manifest.json");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| InstallError::Io(crate::py::os_error_text(&e, Some(&manifest_path))))?;
    let manifest: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| InstallError::Other {
            type_name: "JSONDecodeError".into(),
            message: e.to_string(),
        })?;
    let version_ok = manifest.get("version").is_some_and(ferb_py::value::truthy);
    if !version_ok || !source.join("background.js").is_file() {
        return Err(InstallError::Other {
            type_name: "ValueError".into(),
            message: "incomplete FERB extension bundle".into(),
        });
    }
    let io = |e: std::io::Error, p: &Path| InstallError::Io(crate::py::os_error_text(&e, Some(p)));
    let target = extension_path(home);
    std::fs::create_dir_all(&target).map_err(|e| io(e, &target))?;
    let parent = target.parent().expect("the extension folder has a parent");
    let temp = tempfile::Builder::new()
        .prefix(".extension-")
        .tempdir_in(parent)
        .map_err(|e| io(e, parent))?;
    let mut files = Vec::new();
    collect(source, source, &mut files).map_err(|e| io(e, source))?;
    files.sort();
    files.sort_by_key(|rel| rel.file_name().is_some_and(|n| n == "manifest.json"));
    for rel in &files {
        let copy = temp.path().join(rel);
        if let Some(p) = copy.parent() {
            std::fs::create_dir_all(p).map_err(|e| io(e, p))?;
        }
        let data = std::fs::read(source.join(rel)).map_err(|e| io(e, &source.join(rel)))?;
        std::fs::write(&copy, data).map_err(|e| io(e, &copy))?;
    }
    for rel in &files {
        let dest = target.join(rel);
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p).map_err(|e| io(e, p))?;
        }
        std::fs::rename(temp.path().join(rel), &dest).map_err(|e| io(e, &dest))?;
    }
    Ok(target)
}

/// Every regular file under `dir` (links left out), relative to `root`.
fn collect(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            collect(root, &path, out)?;
        } else if kind.is_file() {
            out.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
        }
    }
    Ok(())
}

/// autoinstall.py `valid_id`: 32 letters a to p.
pub fn valid_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

/// extensions.py:580: FERB's Chrome Web Store page, or "" when no valid store id is set.
pub fn store_page(store_id: &str) -> String {
    if valid_id(store_id) {
        format!("https://chromewebstore.google.com/detail/{store_id}")
    } else {
        String::new()
    }
}

/// extensions.py:586: a profile as the setup window shows it.
pub fn profile_state(
    extension_id: &str,
    removed: bool,
    enabled: bool,
    paired: bool,
) -> &'static str {
    if extension_id.is_empty() {
        return if removed { "removed" } else { "not_installed" };
    }
    if !enabled {
        return "installed";
    }
    if paired {
        "paired"
    } else {
        "on"
    }
}
