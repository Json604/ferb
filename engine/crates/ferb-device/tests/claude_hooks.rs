//! What the archived sessions left, taken away (kb HAND-718): FERB's hook entries leave Claude Code's settings and
//! nothing else in the file changes; the sessions' folder and config table go; a second pass finds nothing.
//! `engine/fixtures/retired-sessions` holds invented settings in Claude Code's own layout, written by hand: FERB's
//! entries from two FERB.app installs beside the owner's own hooks, and the same file with only FERB's entries out.

use std::path::{Path, PathBuf};

use ferb_device::claude_hooks::{is_ferbs, remove, retire_sessions, strip_file};

fn fixture(name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/retired-sessions")
        .join(name);
    std::fs::read_to_string(p).unwrap()
}

fn owner_only(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

// decision: HAND-718
#[test]
fn only_ferbs_hooks_leave_claude_codes_settings_byte_for_byte() {
    let before = fixture("settings-before.json");
    let after = fixture("settings-after.json");
    assert_eq!(remove(&before).unwrap().as_deref(), Some(after.as_str()));
    // nothing of FERB's left: nothing to write
    assert_eq!(remove(&after).unwrap(), None);
    // a file without a final newline keeps none
    let bare = before.trim_end_matches('\n');
    assert_eq!(
        remove(bare).unwrap().as_deref(),
        Some(after.trim_end_matches('\n'))
    );
    // FERB's: any FERB.app's `ferb`, quoted or not, with `agent-hook claude` after it; nothing else
    assert!(is_ferbs(
        "/Applications/FERB.app/Contents/Resources/bin/ferb agent-hook claude Stop"
    ));
    assert!(is_ferbs(
        "'/Users/a b/FERB.app/Contents/Resources/bin/ferb' agent-hook claude Notification"
    ));
    for other in [
        "/usr/local/bin/ferbish agent-hook claude Stop",
        "/Applications/FERB.app/Contents/Resources/bin/ferb doctor",
        "ferb",
        "say done",
    ] {
        assert!(!is_ferbs(other), "{other}");
    }
}

// decision: HAND-718
#[test]
fn a_settings_file_that_isnt_an_object_is_left_alone() {
    for text in ["[1, 2]", "{ not json", "\"text\""] {
        assert!(remove(text).is_err(), "{text}");
    }
    assert_eq!(remove("").unwrap(), None);
    assert_eq!(remove("{}\n").unwrap(), None);
    // only FERB's hooks: the event and then `hooks` itself go
    let only = "{\n  \"hooks\": {\n    \"Stop\": [\n      {\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"/Applications/FERB.app/Contents/Resources/bin/ferb agent-hook claude Stop\"\n          }\n        ]\n      }\n    ]\n  },\n  \"model\": \"opus\"\n}\n";
    assert_eq!(
        remove(only).unwrap().as_deref(),
        Some("{\n  \"model\": \"opus\"\n}\n")
    );
}

// decision: HAND-718
#[test]
fn the_file_is_written_whole_where_it_is_and_a_link_stays_a_link() {
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    let real = dir.path().join("dotfiles-settings.json");
    std::fs::write(&real, fixture("settings-before.json")).unwrap();
    owner_only(&real, 0o640);
    let link = claude.join("settings.json");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(strip_file(&link).unwrap());
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        std::fs::read_to_string(&real).unwrap(),
        fixture("settings-after.json")
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&real).unwrap().permissions().mode() & 0o777,
        0o640
    );
    // the second time: nothing to do, nothing written
    assert!(!strip_file(&link).unwrap());
    // no file: nothing to do
    assert!(!strip_file(&dir.path().join("none.json")).unwrap());
    // no temporary file left beside it
    let left: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left.len(), 2, "{left:?}");
}

// decision: HAND-718
#[test]
fn the_start_takes_away_the_hooks_the_folder_and_the_config_table_once() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ferb-home");
    let user = dir.path().join("user");
    std::fs::create_dir_all(home.join("sessions")).unwrap();
    std::fs::write(home.join("sessions/chat-layouts.json"), "{}").unwrap();
    owner_only(&home, 0o700);
    std::fs::create_dir_all(user.join(".claude")).unwrap();
    let settings = user.join(".claude/settings.json");
    std::fs::write(&settings, fixture("settings-before.json")).unwrap();
    let config_path = home.join("config.toml");
    std::fs::write(
        &config_path,
        "[memory]\nlocal_import = true\n\n[sessions]\nenabled = true\nchat_apps = [\"com.example.chat\"]\n\n[insomnia]\ngrace_minutes = 10\n",
    )
    .unwrap();
    owner_only(&config_path, 0o600);
    let config = ferb_core::config::Config::new(&home);

    let done = retire_sessions(&home, &user, &config).expect("something was taken away");
    assert_eq!(
        done["hooks_removed"],
        serde_json::json!(settings.to_string_lossy())
    );
    assert!(done["folder_removed"].is_string());
    assert_eq!(done["config"].as_array().map(Vec::len), Some(2));
    assert!(done.get("errors").is_none(), "{done}");
    assert_eq!(
        std::fs::read_to_string(&settings).unwrap(),
        fixture("settings-after.json")
    );
    assert!(!home.join("sessions").exists());
    assert_eq!(
        std::fs::read_to_string(&config_path).unwrap(),
        "[memory]\nlocal_import = true\n\n[insomnia]\ngrace_minutes = 10\n"
    );
    // every later start: nothing left, nothing to journal
    assert_eq!(retire_sessions(&home, &user, &config), None);
    // a Mac that never had sessions: nothing at all
    let fresh = dir.path().join("fresh");
    std::fs::create_dir_all(&fresh).unwrap();
    owner_only(&fresh, 0o700);
    assert_eq!(
        retire_sessions(
            &fresh,
            &dir.path().join("nobody"),
            &ferb_core::config::Config::new(&fresh)
        ),
        None
    );
}
