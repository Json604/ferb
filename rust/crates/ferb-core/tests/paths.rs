//! FERB's locations against rust/fixtures/core/paths.json, in temporary folders only.

#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};

use common::{fixture, strs};
use ferb_core::paths;

#[test]
fn the_home_and_socket_follow_ferb_home_as_python_reads_it() {
    let fx = fixture("paths.json");
    for case in fx["homes"].as_array().unwrap() {
        let given = case["given"].as_str().unwrap();
        let home = paths::ferb_home_from(Some(given), Path::new("/nowhere"));
        assert_eq!(
            home.to_str().unwrap(),
            case["home"].as_str().unwrap(),
            "FERB_HOME={given:?}"
        );
        assert_eq!(
            paths::socket_path(&home).to_str().unwrap(),
            case["socket"].as_str().unwrap(),
            "{given:?}"
        );
    }
    let explicit = &fx["explicit"];
    let socket = paths::socket_path(Path::new(explicit["home"].as_str().unwrap()));
    assert_eq!(
        socket.to_str().unwrap(),
        explicit["socket"].as_str().unwrap()
    );
    assert_eq!(
        paths::ferb_home_from(None, Path::new("/Users/invented")),
        Path::new("/Users/invented/.ferb")
    );
    assert_eq!(paths::APP_ID, fx["app_id"].as_str().unwrap());
    assert!(paths::account_home().is_some_and(|h| h.is_absolute()));
}

/// ui/mac/FERBAgent/main.c:50-55 (APP-592): the shipping host takes FERB_HOME only unset, empty or naming the
/// account's own ~/.ferb.
#[test]
fn the_shipping_host_refuses_a_home_that_is_not_the_accounts_own() {
    let tmp = tempfile::tempdir().unwrap();
    let account = tmp.path().join("account");
    let own = account.join(".ferb");
    let other = tmp.path().join("other");
    std::fs::create_dir_all(&own).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&own, &link).unwrap();
    let ok = |given: Option<&str>| paths::shipping_home(given, Some(&account));
    assert_eq!(ok(None).unwrap(), own);
    assert_eq!(ok(Some("")).unwrap(), own);
    assert_eq!(ok(Some(own.to_str().unwrap())).unwrap(), own);
    assert_eq!(ok(Some(link.to_str().unwrap())).unwrap(), own); // the same folder by another name
    assert_eq!(ok(Some(other.to_str().unwrap())), Err(paths::ForeignHome));
    assert_eq!(
        ok(Some(tmp.path().join("missing").to_str().unwrap())),
        Err(paths::ForeignHome)
    );
    assert_eq!(paths::shipping_home(None, None), Err(paths::ForeignHome));
    // the account's ~/.ferb doesn't exist yet: only an unset FERB_HOME is taken
    let fresh = tmp.path().join("fresh");
    assert_eq!(
        paths::shipping_home(None, Some(&fresh)).unwrap(),
        fresh.join(".ferb")
    );
    assert!(paths::shipping_home(Some(other.to_str().unwrap()), Some(&fresh)).is_err());
}

#[test]
fn the_app_bundle_is_found_only_from_its_own_runtime_folder() {
    let fx = fixture("paths.json");
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    for case in fx["bundles"].as_array().unwrap() {
        let prefix = root.join(case["prefix"].as_str().unwrap());
        std::fs::create_dir_all(&prefix).unwrap();
        let found =
            paths::app_bundle(&prefix).map(|b| b.strip_prefix(&root).unwrap().to_path_buf());
        assert_eq!(
            found,
            case["bundle"].as_str().map(PathBuf::from),
            "{}",
            case["prefix"]
        );
    }
}

#[test]
fn the_installed_app_is_the_first_candidate_with_ferbs_bundle_id() {
    let fx = fixture("paths.json");
    let tmp = tempfile::tempdir().unwrap();
    for (name, info) in fx["plists"].as_object().unwrap() {
        let contents = tmp.path().join(name).join("FERB.app").join("Contents");
        std::fs::create_dir_all(&contents).unwrap();
        let file = contents.join("Info.plist");
        match info.as_object() {
            None => std::fs::write(&file, info.as_str().unwrap()).unwrap(),
            Some(entries) => {
                let dict: plist::Dictionary = entries
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            plist::Value::String(v.as_str().unwrap().to_string()),
                        )
                    })
                    .collect();
                let value = plist::Value::Dictionary(dict);
                if name == "good" {
                    value.to_file_binary(&file).unwrap();
                } else {
                    value.to_file_xml(&file).unwrap();
                }
            }
        }
    }
    for case in fx["installed"].as_array().unwrap() {
        let candidates: Vec<PathBuf> = strs(&case["candidates"])
            .iter()
            .map(|n| tmp.path().join(n).join("FERB.app"))
            .collect();
        let found = paths::installed_app_in(&candidates);
        let name = found.as_ref().map(|p| {
            p.parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string()
        });
        assert_eq!(
            name.as_deref(),
            case["found"].as_str(),
            "{}",
            case["candidates"]
        );
    }
}
