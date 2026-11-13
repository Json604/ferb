//! The exclusion rules (`ferb/sense/exclusions.py`) against `rust/fixtures/sense/exclusions.json`.

mod common;

use common::{fixture, input, tmp, Diffs};
use ferb_sense::exclusions::Exclusions;
use serde_json::{json, Value};

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| input(x.as_str().unwrap()))
        .collect()
}

fn opt(v: &Value) -> Option<String> {
    v.as_str().map(input)
}

/// The folders and links make.py made.
fn layout() {
    let base = tmp().join("excl");
    std::fs::create_dir_all(base.join("private/deep")).unwrap();
    std::fs::create_dir_all(base.join("open")).unwrap();
    let link = |to: &std::path::Path, at: &str| {
        let _ = std::os::unix::fs::symlink(to, base.join(at));
    };
    link(&base.join("private"), "link");
    link(&base.join("nowhere/x"), "dangling");
    link(&base.join("loop2"), "loop1");
    link(&base.join("loop1"), "loop2");
    link(std::path::Path::new("../excl/private"), "up");
}

// decision: SENSE-403
#[test]
fn exclusions_apply_as_python_applies_them() {
    layout();
    let f = fixture("exclusions.json");
    let mut d = Diffs::default();
    for case in f.as_array().unwrap() {
        let b = &case["build"];
        let e = Exclusions::build(&strs(&b["apps"]), &strs(&b["domains"]), &strs(&b["paths"]));
        if case.get("error").is_some() {
            // Fixed against Python (ENG-6): Python's Path.resolve raised RuntimeError on the link loop in
            // exclude_paths, so the whole exclusion list failed to build; here the looping entry is kept unresolved.
            let looped = input(&format!("{}/excl/loop1", "<TMP>"));
            d.eq(
                "a loop in exclude_paths is kept as given",
                true,
                e.paths.contains(&looped),
            );
            continue;
        }
        let what = format!("build {}", b);
        d.check(&format!("{what} apps"), &case["apps"], &json!(e.apps));
        d.check(
            &format!("{what} domains"),
            &case["domains"],
            &json!(e.domains),
        );
        d.check(&format!("{what} paths"), &case["paths"], &json!(e.paths));
        for p in case["app"].as_array().unwrap() {
            let got = e.app(opt(&p[0]).as_deref(), opt(&p[1]).as_deref());
            d.check(&format!("{what} app {p}"), &p[2], &json!(got));
        }
        for p in case["host"].as_array().unwrap() {
            d.check(
                &format!("{what} host {p}"),
                &p[1],
                &json!(e.host(opt(&p[0]).as_deref())),
            );
        }
        for p in case["url"].as_array().unwrap() {
            d.check(
                &format!("{what} url {p}"),
                &p[1],
                &json!(e.url(opt(&p[0]).as_deref())),
            );
        }
        for p in case["path"].as_array().unwrap() {
            let got = e.path(opt(&p[0]).as_deref());
            if p[1].get("error").is_some() {
                // Fixed against Python (ENG-6): a path under a link loop raised RuntimeError out of the trail;
                // here it is compared unresolved, which matches no excluded folder.
                d.eq(&format!("{what} looping path {}", p[0]), false, got);
            } else {
                d.check(&format!("{what} path {p}"), &p[1], &json!(got));
            }
        }
        for p in case["excluded"].as_array().unwrap() {
            let data = common::input_value(&p[0]).as_object().unwrap().clone();
            d.check(
                &format!("{what} excluded {p}"),
                &p[1],
                &json!(e.excluded("x", &data)),
            );
        }
    }
    d.finish("exclusions");
}
