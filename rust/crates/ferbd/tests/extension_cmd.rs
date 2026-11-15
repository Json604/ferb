//! `ferb browsers extension path|package` (ferb/cli/__init__.py:189): the lines and statuses cli/__init__.py wrote,
//! on temporary folders only. `install` writes the native host to the browsers' folders and is not run here.

use std::path::{Path, PathBuf};

use ferbd::extension_cmd;

fn repo_extension() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../extensions/chromium")
}

fn text(b: &[u8]) -> String {
    String::from_utf8(b.to_vec()).unwrap()
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

#[test]
fn path_names_the_managed_extension_folder() {
    let home = tempfile::tempdir().unwrap();
    let mut out = Vec::new();
    assert_eq!(extension_cmd::path(home.path(), &mut out), 0);
    assert_eq!(
        text(&out),
        format!("{}\n", home.path().join("extensions/chromium").display())
    );
}

#[test]
fn package_writes_the_store_zip_where_asked_or_beside_the_managed_extension() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("source");
    copy_tree(&repo_extension(), &source);
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(source.join("manifest.json")).unwrap())
            .unwrap();
    let version = manifest["version"].as_str().unwrap();

    // --out: that folder (made when missing)
    let out_dir = home.path().join("build/store");
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let status = extension_cmd::package(
        &source,
        home.path(),
        Some(out_dir.to_str().unwrap()),
        &mut out,
        &mut err,
    );
    assert_eq!(status, 0, "{}", text(&err));
    let zip = out_dir.join(format!("ferb-extension-{version}.zip"));
    assert!(zip.is_file());
    assert_eq!(
        text(&out),
        format!("Chrome Web Store package: {}\n", zip.display())
    );
    assert!(err.is_empty());

    // no --out: the folder holding FERB's managed extension (~/.ferb/extensions)
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(
        extension_cmd::package(&source, home.path(), None, &mut out, &mut err),
        0
    );
    let zip = home
        .path()
        .join("extensions")
        .join(format!("ferb-extension-{version}.zip"));
    assert!(zip.is_file());
    assert_eq!(
        text(&out),
        format!("Chrome Web Store package: {}\n", zip.display())
    );
}

#[test]
fn package_says_what_the_listing_still_needs_and_refuses_an_incomplete_manifest() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("manifest.json"),
        r#"{"name": "FERB", "version": "0.1.0", "description": "x", "key": "k"}"#,
    )
    .unwrap();
    std::fs::write(source.join("background.js"), "// x\n").unwrap();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let dir = home.path().join("o");
    assert_eq!(
        extension_cmd::package(&source, home.path(), dir.to_str(), &mut out, &mut err),
        0
    );
    assert_eq!(
        text(&out),
        format!(
            "Chrome Web Store package: {}\n  Before uploading: manifest.json has no icons; the Chrome Web Store asks \
             for them (at least a 128x128 PNG)\n",
            dir.join("ferb-extension-0.1.0.zip").display()
        )
    );

    std::fs::write(source.join("manifest.json"), r#"{"name": "FERB"}"#).unwrap();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(
        extension_cmd::package(&source, home.path(), dir.to_str(), &mut out, &mut err),
        1
    );
    assert!(out.is_empty());
    assert_eq!(
        text(&err),
        "ferb: cannot package the extension: manifest.json lacks version, description\n"
    );
}
