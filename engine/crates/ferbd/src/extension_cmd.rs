//! `ferb browsers extension [path|install|package] [--out DIR]` (ferb/cli/__init__.py:189): FERB's managed unpacked
//! extension's folder, the developer's own install of it (the files, the native host to each installed browser, then
//! each browser's setup step), and the Chrome Web Store zip (ferb-browser `autoinstall::package`).

use std::io::Write;
use std::path::{Path, PathBuf};

use ferb_device::cli::plain_error;

/// The extension's source: the one the installer copies from (FERB.app's Resources/extensions/chromium, or a
/// checkout's extension; setup_env.rs `installer_at`).
pub fn source(home: &Path) -> PathBuf {
    crate::setup_env::installer(home).sources.extension.clone()
}

/// `package`: the zip in `out_dir` (default: the folder holding FERB's managed extension, `~/.ferb/extensions`),
/// written to `out`; a failure is one line on `err` and status 1.
pub fn package(
    source: &Path,
    home: &Path,
    out_dir: Option<&str>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let dir = match out_dir {
        Some(d) => PathBuf::from(d),
        None => ferb_device::extensions::extension_path(home)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default(),
    };
    match ferb_browser::autoinstall::package(source, &dir) {
        Ok((target, notes)) => {
            let _ = writeln!(out, "Chrome Web Store package: {}", target.display());
            for note in notes {
                let _ = writeln!(out, "  Before uploading: {note}");
            }
            0
        }
        Err(e) => {
            let _ = writeln!(err, "ferb: cannot package the extension: {e}");
            1
        }
    }
}

/// `path`: the managed extension's folder.
pub fn path(home: &Path, out: &mut dyn Write) -> i32 {
    let _ = writeln!(
        out,
        "{}",
        ferb_device::extensions::extension_path(home).display()
    );
    0
}

/// `install`: the extension's files under FERB's home, FERB's native host to each installed browser (setup.py:85
/// `installer(..., browser_folders=True)`: even from FERB.app, as the developer asked for it), then the browsers step.
pub fn install(home: &Path, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let mut run = || -> Result<(), ferb_device::cli::CliError> {
        let src = source(home);
        let target = ferb_device::extensions::install_extension(home, &src)?;
        let mut env = crate::setup_env::env(home, Box::new(std::io::stdout()), false);
        let support = user_support();
        env.inst.native_hosts_dirs = crate::setup_env::native_hosts_dirs(
            &ferb_browser::macos::MacPlatform,
            &support,
            false,
            false,
        );
        env.inst.write_native_host()?;
        let _ = writeln!(
            out,
            "Extension files and native host prepared: {}",
            target.display()
        );
        let _ = writeln!(out, "Existing managed installations reload when idle.");
        let _ = out.flush();
        ferb_device::cli::setup::extension_setup(&mut env)
    };
    match run() {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(err, "{}", plain_error(&e));
            1
        }
    }
}

fn user_support() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(ferb_core::paths::account_home)
        .unwrap_or_default()
        .join("Library")
        .join("Application Support")
}
