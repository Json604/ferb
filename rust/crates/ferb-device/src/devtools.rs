//! Whether this Mac has Apple's developer tools (ferb/device/devtools.py, DMN-511, DMN-512). Never installs anything
//! and never starts a tool that may be Apple's install stub; `xcode-select -p` opens no dialog.

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::install::Output;

pub const STUB_TOOLS: [&str; 6] = ["python3", "pip3", "git", "make", "cc", "clang"];
pub const STUB_FOLDER: &str = "/usr/bin";
pub const INSTALL_FOLDERS: [&str; 2] = [
    "/Library/Developer/CommandLineTools/usr/bin",
    "/Applications/Xcode.app/Contents/Developer",
];

/// devtools.py:694: the developer folder `xcode-select -p` names when it exists. `run` answers None when the
/// program couldn't run.
pub fn developer_folder(run: &mut dyn FnMut(&[String]) -> Option<Output>) -> Option<PathBuf> {
    let done = run(&["/usr/bin/xcode-select".to_string(), "-p".to_string()])?;
    let folder = PathBuf::from(crate::py::strip(&done.stdout));
    (done.code == 0 && folder.is_absolute() && folder.is_dir()).then_some(folder)
}

fn executable(p: &Path) -> bool {
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c` is a valid NUL-terminated path; access only reads it.
    unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
}

/// devtools.py:704: the stub tools a command with this PATH would reach only as Apple's stub (or not at all).
pub fn missing_tools(
    path: &str,
    stub_folder: &Path,
    run: &mut dyn FnMut(&[String]) -> Option<Output>,
) -> Vec<String> {
    if developer_folder(run).is_some() {
        return Vec::new();
    }
    let mut missing = Vec::new();
    for name in STUB_TOOLS {
        let found = path
            .split(':')
            .filter(|d| !d.is_empty())
            .map(|d| Path::new(d).join(name))
            .find(|p| executable(p) && p.is_file());
        if found
            .as_ref()
            .is_none_or(|f| f.parent() == Some(stub_folder))
        {
            missing.push(name.to_string());
        }
    }
    missing
}

/// devtools.py:719.
pub fn installed_now(folders: &[PathBuf]) -> bool {
    folders.iter().any(|f| f.is_dir())
}

/// devtools.py:725: the worker's fact (empty when nothing is missing).
pub fn missing_fact(missing: &[String]) -> String {
    if missing.is_empty() {
        return String::new();
    }
    format!(
        "- This Mac has no developer tools (Apple's Command Line Tools aren't installed): in a terminal command {} are only Apple's stubs; running one fails and opens macOS's \"install developer tools\" dialog in front of the owner, so never run them. When a task really needs one, ask the owner once (owner_ask) to install the developer tools; after their Yes, `xcode-select --install` opens Apple's installer for them. FERB doesn't install them itself.",
        missing.join(", ")
    )
}
