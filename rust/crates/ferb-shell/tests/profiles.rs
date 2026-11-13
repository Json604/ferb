//! The sandbox profile TEXT against Python's (rust/fixtures/shell/profiles.json): many invented protection lists,
//! folder layouts (links, missing folders, names with quotes, brackets, carets, backslashes, spaces and non-ASCII),
//! lifts, folders, listening ports, pinned ports, report tags, Apple Event targets and preference domains.
// decision: HAND-203, HAND-204, HAND-205, HAND-214, HAND-215, HAND-217, HAND-243, HAND-244, HAND-429, GATE-1, GATE-505

mod common;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::{fixture, strs, Root, Tally};
use ferb_shell::protection::{Entry, LocalPort, Protection};
use ferb_shell::sandbox::{app_program_rx, q, rx, Options, Sandbox};
use serde_json::Value;

/// The protection lists as Python's Protection answered them for one layout.
struct Recorded {
    home: PathBuf,
    ferb_home: PathBuf,
    workspace: PathBuf,
    uploads: PathBuf,
    reads: Vec<Entry>,
    writes: Vec<Entry>,
    app_entries: Vec<PathBuf>,
    service_files: Vec<PathBuf>,
    guarded: Vec<PathBuf>,
    forms_now: HashMap<PathBuf, Vec<PathBuf>>,
    app_data: HashMap<PathBuf, bool>,
    local_ports: Vec<LocalPort>,
    rules: Vec<Value>,
}

impl Protection for Recorded {
    fn home(&self) -> &Path {
        &self.home
    }
    fn ferb_home(&self) -> &Path {
        &self.ferb_home
    }
    fn workspace(&self) -> &Path {
        &self.workspace
    }
    fn uploads(&self) -> &Path {
        &self.uploads
    }
    fn reads(&self) -> &[Entry] {
        &self.reads
    }
    fn writes(&self) -> &[Entry] {
        &self.writes
    }
    fn app_entries(&self) -> Vec<PathBuf> {
        self.app_entries.clone()
    }
    fn service_files(&self) -> Vec<PathBuf> {
        self.service_files.clone()
    }
    fn guarded_parents(&self) -> Vec<PathBuf> {
        self.guarded.clone()
    }
    fn forms_now(&self, path: &Path) -> Vec<PathBuf> {
        self.forms_now
            .get(path)
            .cloned()
            .unwrap_or_else(|| panic!("Python never asked forms_now({})", path.display()))
    }
    fn app_data(&self, path: &Path) -> bool {
        *self
            .app_data
            .get(path)
            .unwrap_or_else(|| panic!("Python never asked app_data({})", path.display()))
    }
    fn local_ports(&self) -> &[LocalPort] {
        &self.local_ports
    }
    fn rules(&self) -> Vec<Value> {
        self.rules.clone()
    }
}

fn recorded(root: &Root, layout: &Value) -> Recorded {
    let p = &layout["protection"];
    let path = |v: &Value| root.path(v.as_str().unwrap());
    let entries = |v: &Value| -> Vec<Entry> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|e| Entry {
                id: e["id"].as_str().unwrap().to_string(),
                path: path(&e["path"]),
                typed: path(&e["typed"]),
            })
            .collect()
    };
    let paths = |v: &Value| -> Vec<PathBuf> { strs(v).iter().map(|s| root.path(s)).collect() };
    Recorded {
        home: path(&p["home"]),
        ferb_home: path(&p["ferb_home"]),
        workspace: path(&p["workspace"]),
        uploads: path(&p["uploads"]),
        reads: entries(&p["reads"]),
        writes: entries(&p["writes"]),
        app_entries: paths(&p["app_entries"]),
        service_files: paths(&p["service_files"]),
        guarded: paths(&p["guarded_parents"]),
        forms_now: layout["forms_now"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (root.path(k), paths(v)))
            .collect(),
        app_data: layout["app_data"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (root.path(k), v.as_bool().unwrap()))
            .collect(),
        local_ports: p["local_ports"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| LocalPort {
                port: l["port"].as_u64().unwrap() as u16,
                executable: path(&l["executable"]),
            })
            .collect(),
        rules: p["rules"].as_array().unwrap().clone(),
    }
}

fn ports(v: &Value) -> Option<BTreeSet<u32>> {
    v.as_array()
        .map(|a| a.iter().map(|x| x.as_u64().unwrap() as u32).collect())
}

#[test]
fn every_profile_text_is_pythons() {
    let fx = fixture("profiles.json");
    let root = Root::new();
    let mut tally = Tally::default();
    for layout in fx["layouts"].as_array().unwrap() {
        root.build(layout["ops"].as_array().unwrap());
        let prot: Arc<dyn Protection> = Arc::new(recorded(&root, layout));
        let lines = strs(&layout["lines"]);
        for (i, case) in layout["cases"].as_array().unwrap().iter().enumerate() {
            let scan = case["scan"].clone();
            let proxy = case["proxy"].as_u64().map(|p| p as u16);
            let socket = case["socket"].as_str().map(|s| root.path(s));
            let mut sb = Sandbox::new(
                prot.clone(),
                root.path("@ROOT@/profiles"),
                Arc::new(move || proxy),
                socket,
            )
            .with_scan(Arc::new(move || {
                if scan.is_null() {
                    return None;
                }
                let outside = ports(&scan["outside"])?;
                Some((outside, ports(&scan["inside"]).unwrap_or_default()))
            }));
            if let Some(domains) = case["app_domains"].as_array() {
                sb = sb.with_app_domains(
                    domains
                        .iter()
                        .map(|d| d.as_str().unwrap().to_string())
                        .collect(),
                );
            }
            let direct: Vec<LocalPort> = case["direct"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| LocalPort {
                    port: d["port"].as_u64().unwrap() as u16,
                    executable: root.path(d["executable"].as_str().unwrap()),
                })
                .collect();
            let folders: Vec<String> = strs(&case["folders"]).iter().map(|f| root.sub(f)).collect();
            let report = case["report"].as_str().map(|r| root.sub(r));
            let opts = Options {
                lift: case["lift"].as_str(),
                apps: case["apps"].as_bool().unwrap(),
                direct: &direct,
                report: report.as_deref(),
                events_to: case["events_to"].as_str(),
                folders: &folders,
            };
            let what = format!(
                "{} case {i} ({:?}, apps {})",
                layout["name"], case["lift"], case["apps"]
            );
            match (sb.build(&opts), case.get("error")) {
                (Ok((text, allowed)), None) => {
                    let want: String = case["lines"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|n| format!("{}\n", root.sub(&lines[n.as_u64().unwrap() as usize])))
                        .collect();
                    if text != want {
                        let got: Vec<&str> = text.lines().collect();
                        let exp: Vec<&str> = want.lines().collect();
                        let first = got
                            .iter()
                            .zip(&exp)
                            .position(|(a, b)| a != b)
                            .unwrap_or(got.len().min(exp.len()));
                        tally.check(
                            &format!("{what} line {first}"),
                            got.get(first).copied(),
                            exp.get(first).copied(),
                        );
                    } else {
                        tally.check(&what, true, true);
                    }
                    let allowed: Vec<(u64, String)> = allowed
                        .iter()
                        .map(|p| (p.port as u64, p.executable.to_string_lossy().into_owned()))
                        .collect();
                    let want_allowed: Vec<(u64, String)> = case["allowed"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|a| (a[0].as_u64().unwrap(), root.sub(a[1].as_str().unwrap())))
                        .collect();
                    tally.check(&format!("{what} allowed"), allowed, want_allowed);
                }
                (Err(e), Some(err)) => {
                    tally.check(&what, e.to_string(), root.sub(err.as_str().unwrap()))
                }
                (got, want) => tally.check(
                    &what,
                    format!("{:?}", got.map(|_| "a profile")),
                    format!("{want:?}"),
                ),
            }
        }
    }
    tally.done("profile");
}

#[test]
fn rx_q_and_app_rules_are_pythons() {
    let fx = fixture("profiles.json");
    let root = Root::new();
    let mut tally = Tally::default();
    for pair in fx["rx"].as_array().unwrap() {
        tally.check(
            "rx",
            rx(pair[0].as_str().unwrap()),
            pair[1].as_str().unwrap().to_string(),
        );
    }
    for pair in fx["q"].as_array().unwrap() {
        tally.check(
            "q",
            q(pair[0].as_str().unwrap()),
            pair[1].as_str().unwrap().to_string(),
        );
    }
    for pair in fx["app_program_rx"].as_array().unwrap() {
        tally.check(
            "app rx",
            app_program_rx(&root.sub(pair[0].as_str().unwrap())),
            root.sub(pair[1].as_str().unwrap()),
        );
    }
    tally.done("rx/q");
}
