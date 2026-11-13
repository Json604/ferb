//! `_learning_candidates` (memory.py:323, MEM-27) against `rust/fixtures/daemon-a/candidates.json` (made by
//! `make_candidates.py` beside it from the real Memory, Learning and CoreProfile): the fixture's rows go into a group
//! A store, and each query's candidates for the attention check and a worker must be Python's, byte for byte
//! (`json.dumps`): their order, texts, references, origins and the closing count line.

mod a_support;

use a_support::{fixture, Daemon, Seq};
use ferb_clock::clock::FakeClock;
use serde_json::Value;

// decision: MEM-27
#[tokio::test(flavor = "multi_thread")]
async fn the_prompts_memory_is_pythons() {
    let f = fixture("daemon-a/candidates.json");
    let tmp = tempfile::tempdir().unwrap();
    let now = ferb_clock::py::parse(f["now"].as_str().unwrap());
    let d = Daemon::start(tmp.path(), "01", "id", FakeClock::new(now), Seq::new("id")).await;
    let tables = f["tables"].as_object().unwrap();
    let mut statements: Vec<(String, Vec<Value>)> = Vec::new();
    for name in tables.keys().rev() {
        statements.push((format!("DELETE FROM {name}"), vec![]));
    }
    for (name, t) in tables {
        let cols: Vec<String> = t["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| match c.as_str().unwrap() {
                "_rowid" => "rowid".to_string(),
                other => other.to_string(),
            })
            .collect();
        let marks = vec!["?"; cols.len()].join(", ");
        for row in t["rows"].as_array().unwrap() {
            statements.push((
                format!("INSERT INTO {name} ({}) VALUES ({marks})", cols.join(", ")),
                row.as_array().unwrap().clone(),
            ));
        }
    }
    d.sql(statements).await;
    let cases = f["cases"].as_array().unwrap().clone();
    let mut diffs = Vec::new();
    for case in &cases {
        let (words, focus) = (
            case["words"].as_str().unwrap().to_string(),
            case["focus"].as_str().unwrap().to_string(),
        );
        let limits = if case["limits"] == "attention" {
            ferb_memory::search::ATTENTION
        } else {
            ferb_memory::search::WORKER
        };
        let got = d
            .on(move |st| {
                Box::pin(async move {
                    ferb_daemon::areas_a::candidates::learning_candidates(
                        &st, &words, limits, &focus,
                    )
                })
            })
            .await;
        let got = ferb_py::json::dumps(&Value::Array(got));
        let want = ferb_py::json::dumps(&case["out"]);
        if got != want {
            diffs.push(format!(
                "{:?} ({}):\n  python: {want}\n  rust:   {got}",
                case["words"], case["limits"]
            ));
        }
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
    println!("{} queries equal", cases.len());
}

/// An invented model: a text about beans (or "legumes") points one way, anything else another.
struct Beans;

impl ferb_memory::embeddings::Embedder for Beans {
    fn name(&self) -> &str {
        "beans-test@000000000000:8"
    }
    fn dimensions(&self) -> usize {
        8
    }
    fn encode(&self, text: &str) -> Option<Vec<i8>> {
        let t = text.to_lowercase();
        Some(if t.contains("beans") || t.contains("legumes") {
            vec![127, 0, 0, 0, 0, 0, 0, 0]
        } else {
            vec![0, 127, 0, 0, 0, 0, 0, 0]
        })
    }
}

/// Once the local model is set on the learning store (`learning.embedder = ...`, after it was built), the prompt's
/// memory and memory.read rank by meaning too: a query sharing no word with a fact finds it by its vector
/// (`_query_vector` reaching `MemorySearch.facts`), and finds nothing without the model.
#[tokio::test(flavor = "multi_thread")]
async fn a_model_set_later_ranks_the_prompts_memory_by_meaning() {
    let f = fixture("daemon-a/candidates.json");
    let tmp = tempfile::tempdir().unwrap();
    let now = ferb_clock::py::parse(f["now"].as_str().unwrap());
    let d = Daemon::start(tmp.path(), "01", "id", FakeClock::new(now), Seq::new("id")).await;
    let t = &f["tables"]["memory_items"];
    let cols: Vec<String> = t["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().replace("_rowid", "rowid"))
        .collect();
    let marks = vec!["?"; cols.len()].join(", ");
    let rows: Vec<(String, Vec<Value>)> = t["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                format!(
                    "INSERT INTO memory_items ({}) VALUES ({marks})",
                    cols.join(", ")
                ),
                r.as_array().unwrap().clone(),
            )
        })
        .collect();
    d.sql(rows).await;
    let found = d
        .on(|st| {
            Box::pin(async move {
                let before =
                    ferb_daemon::areas_a::candidates::memory_item_search(&st, "legumes").unwrap();
                st.learning.set_embedder(std::rc::Rc::new(Beans));
                {
                    struct Now;
                    impl ferb_memory::search::Clock for Now {
                        fn now(&self) -> ferb_memory::times::DateTime {
                            ferb_py::time::DateTime::from_isoformat("2026-01-10T09:00:00+00:00")
                                .unwrap()
                        }
                    }
                    let mut s = ferb_memory::search::MemorySearch::new(st.db().conn(), &Now, None);
                    s.embed_pending(
                        &Beans,
                        &ferb_learning::notes::search_text,
                        &|| false,
                        &mut || {},
                    )
                    .unwrap();
                }
                let (vector, _) = ferb_daemon::areas_a::candidates::query_vector(&st, "legumes");
                let after =
                    ferb_daemon::areas_a::candidates::memory_item_search(&st, "legumes").unwrap();
                let pushed = ferb_daemon::areas_a::candidates::learning_candidates(
                    &st,
                    "legumes",
                    ferb_memory::search::WORKER,
                    "",
                );
                (before, vector, after, pushed.len())
            })
        })
        .await;
    let (before, vector, after, pushed) = found;
    assert!(before.is_empty(), "{before:?}");
    assert_eq!(vector, Some(vec![127, 0, 0, 0, 0, 0, 0, 0]));
    assert!(after.contains(&"id-000001".to_string()), "{after:?}");
    assert!(!after.contains(&"id-000002".to_string()), "{after:?}");
    assert!(pushed > 0);
}
