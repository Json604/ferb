//! config.toml against what the Python Config did with the same files (rust/fixtures/core/config.json), in
//! temporary folders only.

#![cfg(unix)]

mod common;

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use common::{fixture, strs};
use ferb_core::config::{
    write_private, Config, ConfigError, Document, PrivacyDefaults, DEFAULT_EXCLUDE_APPS,
    LIMITS_DECIDED, UPGRADES,
};
use serde_json::{json, Value};

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().mode() & 0o7777
}

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// A home as the Python fixture made it: 0700, with `text` as its 0600 config.toml (None: no file).
fn home_with(text: Option<&str>) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir(&home).unwrap();
    chmod(&home, 0o700);
    if let Some(text) = text {
        std::fs::write(home.join("config.toml"), text).unwrap();
        chmod(&home.join("config.toml"), 0o600);
    }
    (tmp, home)
}

fn privacy(fx: &Value) -> PrivacyDefaults {
    let list = |name: &str| {
        strs(&fx["privacy_defaults"][name])
            .into_iter()
            .map(String::from)
            .collect()
    };
    PrivacyDefaults {
        never_read: list("never_read"),
        never_write: list("never_write"),
        terminal_apps: list("terminal_apps"),
        exclude_paths: list("exclude_paths"),
    }
}

fn run(cfg: &Config, op: &Value, lists: &PrivacyDefaults) -> Result<Value, ConfigError> {
    let text = |i: usize| op[i].as_str().unwrap();
    Ok(match text(0) {
        "get" => cfg.get(text(1), text(2))?.unwrap_or_else(|| op[3].clone()),
        "section" => Value::Object(cfg.section(text(1))?),
        "set" => cfg.set(text(1), text(2), &op[3]).map(|()| Value::Null)?,
        "remove" => cfg.remove(text(1), &strs(&op[2])).map(|()| Value::Null)?,
        "add_defaults" => json!(cfg.add_defaults(text(1), text(2), &strs(&op[3]))?),
        "upgrade" => Value::Array(cfg.upgrade(text(1))?),
        "exclude_apps" => json!(cfg.exclude_apps()?),
        "ensure_privacy_defaults" => cfg.ensure_privacy_defaults(lists).map(|()| Value::Null)?,
        other => panic!("{other}"),
    })
}

fn file_text(cfg: &Config) -> Option<String> {
    cfg.path
        .exists()
        .then(|| std::fs::read_to_string(&cfg.path).unwrap())
}

fn names(home: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(home)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// decision: HAND-210, HAND-236, VOICE-410, DMN-582
#[test]
fn every_case_reads_answers_and_writes_as_python_did() {
    let fx = fixture("config.json");
    let lists = privacy(&fx);
    let cases = fx["cases"].as_array().unwrap();
    assert!(cases.len() > 80);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let (_tmp, home) = home_with(case["input"].as_str());
        let cfg = Config::new(&home);
        for step in case["steps"].as_array().unwrap() {
            let op = &step["op"];
            let expected: Value = serde_json::from_str(step["result"].as_str().unwrap()).unwrap();
            match (run(&cfg, op, &lists), step["error"].as_str()) {
                (Ok(got), None) => assert_eq!(got, expected, "{name}: {op}"),
                (Err(e), Some(message)) => assert_eq!(e.to_string(), message, "{name}: {op}"),
                (got, error) => {
                    panic!("{name}: {op}: got {got:?}, Python gave {expected} / {error:?}")
                }
            }
            assert_eq!(
                file_text(&cfg).as_deref(),
                step["text"].as_str(),
                "{name}: the file after {op}"
            );
        }
        if let Some(expected) = case["mode"].as_str() {
            assert_eq!(format!("0o{:o}", mode(&cfg.path)), expected, "{name}");
        }
        assert_eq!(
            format!("0o{:o}", mode(&home)),
            case["home_mode"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(
            names(&home),
            strs(&case["left_behind"]),
            "{name}: no temp file is left behind"
        );
        // what a fresh parse of the final file gives, and what the cache holds after FERB's own writes
        let values: Value = serde_json::from_str(case["values"].as_str().unwrap()).unwrap();
        let fresh = Config::new(&home);
        for (section, table) in values.as_object().unwrap() {
            if table.is_object() {
                assert_eq!(
                    &Value::Object(fresh.section(section).unwrap()),
                    table,
                    "{name}: [{section}]"
                );
                assert_eq!(
                    &Value::Object(cfg.section(section).unwrap()),
                    table,
                    "{name}: cached [{section}]"
                );
            } else {
                assert!(
                    fresh.section(section).unwrap().is_empty(),
                    "{name}: {section} is not a table"
                );
            }
        }
    }
    assert_eq!(DEFAULT_EXCLUDE_APPS, strs(&fx["default_exclude_apps"]));
    assert_eq!(UPGRADES, strs(&fx["upgrades"]));
}

/// Edits whose tomlkit result this port doesn't reproduce are refused before anything is written (and so are the
/// two Python itself raises on).
#[test]
fn an_edit_that_is_not_ported_is_refused_and_writes_nothing() {
    let fx = fixture("config.json");
    let lists = privacy(&fx);
    let refused = fx["refused"].as_array().unwrap();
    assert_eq!(refused.len(), 10);
    for case in refused {
        let input = case["input"].as_str().unwrap();
        let (_tmp, home) = home_with(Some(input));
        let cfg = Config::new(&home);
        let op = &case["steps"][0]["op"];
        let got = run(&cfg, op, &lists);
        assert!(
            matches!(got, Err(ConfigError::Unsupported(_))),
            "{}: {got:?}",
            case["name"]
        );
        assert_eq!(file_text(&cfg).as_deref(), Some(input));
        assert_eq!(names(&home), ["config.toml"]);
    }
}

/// Three edits come out with other text than tomlkit's (two array layouts; a table between the parts of an array
/// of tables, which tomlkit moves): the file still holds the same values.
#[test]
fn an_edit_laid_out_differently_still_holds_the_same_values() {
    let fx = fixture("config.json");
    let lists = privacy(&fx);
    for case in fx["layout_differs"].as_array().unwrap() {
        let (_tmp, home) = home_with(case["input"].as_str());
        let cfg = Config::new(&home);
        let step = &case["steps"][0];
        run(&cfg, &step["op"], &lists).unwrap();
        assert_ne!(
            file_text(&cfg).as_deref(),
            step["text"].as_str(),
            "{}: now the same; move it to the cases",
            case["name"]
        );
        let ours = Document::parse(&file_text(&cfg).unwrap())
            .unwrap()
            .values()
            .unwrap();
        let pythons = Document::parse(step["text"].as_str().unwrap())
            .unwrap()
            .values()
            .unwrap();
        assert_eq!(ours, pythons, "{}", case["name"]);
    }
}

#[test]
fn a_config_or_home_others_can_reach_is_refused_in_pythons_words() {
    let fx = fixture("config.json");
    let text = "[policy]\nautonomy = \"careful\"\n";
    for case in fx["insecure"].as_array().unwrap() {
        let (_tmp, home) = home_with(Some(text));
        let cfg = Config::new(&home);
        let loose = u32::from_str_radix(case["mode"].as_str().unwrap().trim_start_matches("0o"), 8)
            .unwrap();
        chmod(
            if case["target"] == "file" {
                &cfg.path
            } else {
                &home
            },
            loose,
        );
        let expected = case["message"]
            .as_str()
            .unwrap()
            .replace("<config>", cfg.path.to_str().unwrap())
            .replace("<home>", home.to_str().unwrap());
        for got in [
            cfg.get("policy", "autonomy").map(|_| ()),
            cfg.load().map(|_| ()),
        ] {
            match got {
                Err(ConfigError::Insecure(message)) => assert_eq!(message, expected),
                other => panic!("{case}: {other:?}"),
            }
        }
        chmod(&home, 0o700);
    }
}

// decision: DMN-12
#[test]
fn get_parses_once_and_again_only_when_the_file_changes() {
    let existing = "[secrets.\"ferb/email/password\"]\naccount_hint = \"ferb@example.com\"\n";
    let (_tmp, home) = home_with(Some(existing));
    let cfg = Config::new(&home);
    assert_eq!(cfg.get("secrets", "nope").unwrap(), None);
    for _ in 0..5 {
        let secrets = cfg.section("secrets").unwrap();
        assert_eq!(
            secrets["ferb/email/password"]["account_hint"],
            "ferb@example.com"
        );
    }
    assert_eq!(cfg.parses(), 1);
    // the owner edits it
    std::fs::write(
        &cfg.path,
        format!("{existing}\n[policy]\nautonomy = \"careful\"\n"),
    )
    .unwrap();
    assert_eq!(
        cfg.get("policy", "autonomy").unwrap(),
        Some(json!("careful"))
    );
    assert_eq!(cfg.parses(), 2);
    // FERB's own write updates the cache
    cfg.set("policy", "autonomy", &json!("balanced")).unwrap();
    let after_write = cfg.parses();
    assert_eq!(
        cfg.get("policy", "autonomy").unwrap(),
        Some(json!("balanced"))
    );
    assert_eq!(cfg.parses(), after_write);
    // a value handed out is a copy: changing it never changes the cache
    let mut copy = cfg.section("policy").unwrap();
    copy.insert("autonomy".into(), json!("changed"));
    assert_eq!(
        cfg.get("policy", "autonomy").unwrap(),
        Some(json!("balanced"))
    );
}

// decision: DMN-12
#[test]
fn a_loosened_mode_is_refused_even_with_a_cached_parse() {
    let (_tmp, home) = home_with(Some("[policy]\nk = 1\n"));
    let cfg = Config::new(&home);
    assert_eq!(cfg.get("policy", "autonomy").unwrap(), None);
    chmod(&cfg.path, 0o644);
    assert!(matches!(
        cfg.get("policy", "autonomy"),
        Err(ConfigError::Insecure(_))
    ));
    chmod(&cfg.path, 0o600);
    chmod(&home, 0o750);
    assert!(matches!(
        cfg.get("policy", "autonomy"),
        Err(ConfigError::Insecure(_))
    ));
    chmod(&home, 0o700);
    assert_eq!(cfg.get("policy", "k").unwrap(), Some(json!(1)));
}

#[test]
fn a_write_replaces_the_file_by_rename_and_a_failed_one_leaves_it_intact() {
    let (_tmp, home) = home_with(Some("[policy]\nautonomy = \"careful\"\n"));
    let cfg = Config::new(&home);
    let before = std::fs::metadata(&cfg.path).unwrap().ino();
    cfg.set("policy", "autonomy", &json!("hands-off")).unwrap();
    assert_ne!(std::fs::metadata(&cfg.path).unwrap().ino(), before);
    assert_eq!(mode(&cfg.path), 0o600);

    // the rename fails (the target is a folder that isn't empty): nothing changes and no temp file stays
    let blocked = home.join("blocked.toml");
    std::fs::create_dir(&blocked).unwrap();
    std::fs::write(blocked.join("kept"), "x").unwrap();
    assert!(write_private(&home, &blocked, "k = 1\n").is_err());
    assert_eq!(names(&home), ["blocked.toml", "config.toml"]);
    assert!(blocked.join("kept").exists());

    // a file that isn't TOML is an error, never an empty config, and a write is refused too
    std::fs::write(&cfg.path, "not toml = = =\n").unwrap();
    assert!(matches!(
        cfg.get("policy", "autonomy"),
        Err(ConfigError::Parse(_))
    ));
    assert!(matches!(
        cfg.set("policy", "autonomy", &json!("x")),
        Err(ConfigError::Parse(_))
    ));
    assert_eq!(
        std::fs::read_to_string(&cfg.path).unwrap(),
        "not toml = = =\n"
    );
}

/// STORE-403's config part (tests/test_config.py:186): the upgrade is one write, so a change that can't be made
/// leaves the whole file as it was.
#[test]
fn an_upgrade_is_all_or_nothing() {
    // `download_max_mb` can be upgraded; `retention.download_days` sits under dotted keys this port refuses
    let text = "[browser]\ndownload_max_mb = 100\n\n[retention]\ndownload_days.old = 30\n";
    let (_tmp, home) = home_with(Some(text));
    let cfg = Config::new(&home);
    assert!(matches!(
        cfg.upgrade(LIMITS_DECIDED),
        Err(ConfigError::Unsupported(_))
    ));
    assert_eq!(std::fs::read_to_string(&cfg.path).unwrap(), text);
}
