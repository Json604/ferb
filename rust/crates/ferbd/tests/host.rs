//! `ferbd` as FERB.app's agent (src/host.rs, what `ui/mac/FERBAgent/main.c` guaranteed): a copy of this build's
//! program at the nested agent app's place in an invented FERB.app runs only `daemon`, `mcp` and `grant-probe`, only
//! with the account's own FERB home or, in this ad-hoc-signed build, the development override; the same program
//! outside an `.app` is untouched. Nothing here starts a daemon or reads the owner's `~/.ferb`: each refusal happens
//! before any home is opened.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ferbd");

/// This build's ferbd copied to `<root>/FERB.app/Contents/Helpers/FERB.app/Contents/MacOS/FERB`.
fn agent(root: &Path) -> PathBuf {
    let exe = root.join("FERB.app/Contents/Helpers/FERB.app/Contents/MacOS/FERB");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::copy(BIN, &exe).unwrap();
    exe
}

fn run(exe: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    let out = Command::new(exe)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/nonexistent-home")
        .envs(env.iter().copied())
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

// decision: ENG-7
#[test]
fn the_agent_runs_only_ferbs_own_entry_points() {
    let tmp = tempfile::tempdir().unwrap();
    let exe = agent(tmp.path());
    let refused: [&[&str]; 10] = [
        &[],
        &["setup", "--service"],
        &["uninstall", "--purge", "--yes"],
        &["--version"],
        &["daemon", "--extra"],
        &["mcp", "x"],
        &["grant-probe", "x"],
        &["-m", "ferb.daemon"],
        &["-c", "print('ran')"],
        &["logs"],
    ];
    for argv in refused {
        let (code, out, err) = run(&exe, argv, &[]);
        assert_eq!(code, 64, "{argv:?}: {err}");
        assert_eq!(out, "", "{argv:?}");
        assert!(
            err.contains("runs only FERB's own entry points"),
            "{argv:?}: {err}"
        );
    }
}

// decision: ENG-7
#[test]
fn the_agent_runs_only_with_the_owners_home_or_the_development_override() {
    let tmp = tempfile::tempdir().unwrap();
    let exe = agent(tmp.path());
    let foreign = tmp.path().join("other-ferb");
    std::fs::create_dir_all(&foreign).unwrap();
    let foreign = foreign.to_str().unwrap();
    let (code, _, err) = run(&exe, &["daemon"], &[("FERB_HOME", foreign)]);
    assert_eq!(code, 64, "{err}");
    assert!(err.contains("FERB_HOME isn't your ~/.ferb"), "{err}");
    // the override: absolute only, and never beside a FERB_HOME naming another folder
    let (code, _, err) = run(&exe, &["daemon"], &[("FERB_DEV_HOME", "relative/home")]);
    assert_eq!(code, 64, "{err}");
    let (code, _, err) = run(
        &exe,
        &["daemon"],
        &[("FERB_DEV_HOME", foreign), ("FERB_HOME", "/elsewhere")],
    );
    assert_eq!(code, 64, "{err}");
    // this test build is signed ad hoc by the linker: the override is honored (the grant probe reads only this
    // process's own grants and prints one line)
    let (code, out, err) = run(&exe, &["grant-probe"], &[("FERB_DEV_HOME", foreign)]);
    assert_eq!(code, 0, "{err}");
    assert!(out.trim_start().starts_with('{'), "{out}");
}

#[test]
fn outside_an_app_the_program_keeps_every_mode() {
    // the checkout's and the tests' ferbd: the internal commands answer, FERB_HOME is honored
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("h");
    let (code, out, err) = run(
        Path::new(BIN),
        &["--version"],
        &[("FERB_HOME", home.to_str().unwrap())],
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("ferb "), "{out}");
}
