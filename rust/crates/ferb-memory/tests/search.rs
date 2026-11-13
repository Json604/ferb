//! The fused search, the background embedder and the vector store against what Python did on the same store
//! (`rust/fixtures/memory/make.py`: search.json; ENG-3). Each scenario replays the seed rows and calls on a store
//! the Rust store crate migrated, and must return the same hits (scores as float64 bits) and leave the same rows.
//! Also: the pure helpers case by case, and what this build's SQLite FTS5 tokenizes and ranks against Python's.

mod common;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use common::*;
use ferb_memory::embeddings::{Embedder, StaticEmbedder};
use ferb_memory::search::{self, ChatRow, Clock, MemorySearch, Ranking, StoreFacts};
use ferb_memory::terms;
use ferb_memory::times::{self, DateTime};
use ferb_memory::vectors;
use rusqlite::{params_from_iter, types::Value as Sql, Connection};
use serde_json::{json, Value};

struct FixedClock(Cell<DateTime>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime {
        self.0.get()
    }
}

fn dt(s: &str) -> DateTime {
    times::parse_iso(s).unwrap()
}

fn sql_value(v: &Value) -> Sql {
    match v {
        Value::Null => Sql::Null,
        Value::Bool(b) => Sql::Integer(i64::from(*b)),
        Value::Number(n) if n.is_f64() => Sql::Real(n.as_f64().unwrap()),
        Value::Number(n) => Sql::Integer(n.as_i64().unwrap()),
        Value::String(s) => Sql::Text(s.clone()),
        other => panic!("no SQL form for {other}"),
    }
}

fn run_sql(conn: &Connection, statements: &Value) {
    for st in statements.as_array().unwrap() {
        let params: Vec<Sql> = st[1].as_array().unwrap().iter().map(sql_value).collect();
        conn.execute(s(&st[0]), params_from_iter(params)).unwrap();
    }
}

fn dump(conn: &Connection, table: &str) -> Value {
    let order = if table == "recall_state" {
        "name"
    } else {
        "rowid"
    };
    let mut stmt = conn
        .prepare(&format!("SELECT * FROM {table} ORDER BY {order}"))
        .unwrap();
    let names: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    let rows = stmt
        .query_map([], |r| {
            let mut m = serde_json::Map::new();
            for (i, n) in names.iter().enumerate() {
                let v = match r.get::<_, Sql>(i)? {
                    Sql::Null => Value::Null,
                    Sql::Integer(x) => json!(x),
                    Sql::Real(x) => json!(x),
                    Sql::Text(t) => json!(t),
                    Sql::Blob(b) => json!(hex(&b)),
                };
                m.insert(n.clone(), v);
            }
            Ok(Value::Object(m))
        })
        .unwrap();
    Value::Array(rows.map(Result::unwrap).collect())
}

fn hit_json<T>(h: &search::Hit<T>, row: Option<&ChatRow>) -> Value {
    let mut v =
        json!({"kind": h.kind, "doc": h.doc, "score": bits(h.score), "at": h.at, "owner": h.owner});
    if let Some(r) = row {
        v["row"] = json!({"doc": r.doc, "kind": r.kind, "session": r.session, "at": r.at, "request": r.request,
                          "text": r.text});
    }
    v
}

/// The fixture's embedder: the tiny static model, and a forget run on the store while a marked text is encoded
/// (what another connection's forget does while a batch is being embedded).
struct Forgetful<'a> {
    inner: &'a StaticEmbedder,
    conn: &'a Connection,
    forget_on: RefCell<Vec<(String, Value)>>,
    fired: Cell<usize>,
}

impl Embedder for Forgetful<'_> {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }

    fn encode(&self, text: &str) -> Option<Vec<i8>> {
        let mut pending = self.forget_on.borrow_mut();
        let mut i = 0;
        while i < pending.len() {
            if text.contains(pending[i].0.as_str()) {
                run_sql(self.conn, &pending[i].1);
                pending.remove(i);
                self.fired.set(self.fired.get() + 1);
            } else {
                i += 1;
            }
        }
        self.inner.encode(text)
    }
}

// decision: MEM-6, MEM-27, MEM-34
#[test]
fn search_scenarios_match_python() {
    let fx = fixture("search.json");
    let dir = tempfile::tempdir().unwrap();
    let model_root = write_model(&dir.path().join("model"), &fx["model"]["files"]);
    let embedder = StaticEmbedder::open(&model_root, &fx["model"]["manifest"]).unwrap();
    assert_eq!(embedder.name(), s(&fx["embedder_name"]));
    let node_texts: HashMap<String, String> = fx["node_texts"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), s(v).to_owned()))
        .collect();
    let mut compared = 0;
    // "main" is what Python itself does; "main_fixed" is the same run with the fix this port makes (a vector of
    // what was deleted while its batch was encoded is not stored). Rust replays the fixed one exactly.
    // decision: MEM-2 (deliberate difference from Python, journal "Fixed against Python")
    let scenarios = fx["scenarios"].as_array().unwrap();
    let docs = |name: &str| -> Vec<Vec<String>> {
        let sc = scenarios.iter().find(|x| x["name"] == name).unwrap();
        sc["steps"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|st| st["op"] == "dump" && st["out"].get("recall_vectors").is_some())
            .map(|st| {
                st["out"]["recall_vectors"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| s(&r["doc"]).to_owned())
                    .collect()
            })
            .collect()
    };
    let orphans = ["memory:m-012", "n-sum", "journal:j-05"];
    for (python, fixed) in docs("main").iter().zip(docs("main_fixed")) {
        let left: Vec<&String> = python.iter().filter(|d| !fixed.contains(d)).collect();
        assert_eq!(
            left,
            orphans.iter().collect::<Vec<_>>(),
            "Python keeps the deleted docs' vectors, even after drop_stale"
        );
    }
    for scenario in scenarios.iter().filter(|x| x["name"] != "main") {
        let db =
            ferb_store::Db::open(dir.path().join(s(&scenario["name"])).join("ferb.db")).unwrap();
        db.migrate().unwrap();
        let conn = db.conn();
        let clock = FixedClock(Cell::new(dt("2026-01-10T09:00:00+00:00")));
        // learning.search_text, as Python computed it for each seeded node (kind, key) -> text
        let mut by_node: HashMap<(String, String), String> = HashMap::new();
        let forgetful = Forgetful {
            inner: &embedder,
            conn,
            forget_on: RefCell::new(Vec::new()),
            fired: Cell::new(0),
        };
        for step in scenario["steps"].as_array().unwrap() {
            let (op, args, want) = (s(&step["op"]), &step["args"], &step["out"]);
            let vector = |args: &Value| -> Option<Vec<i8>> {
                if let Some(t) = args.get("vector_of") {
                    return embedder.encode(s(t));
                }
                args.get("vector").filter(|v| !v.is_null()).map(i8s)
            };
            let model = args.get("model").and_then(Value::as_str).map(str::to_owned);
            let got: Value = match op {
                "sql" => {
                    for st in args["statements"].as_array().unwrap() {
                        if s(&st[0]).starts_with("INSERT INTO learning_nodes") {
                            let p = &st[1];
                            let text = node_texts[s(&p[0])].clone();
                            by_node.insert((s(&p[1]).to_owned(), s(&p[2]).to_owned()), text);
                        }
                    }
                    run_sql(conn, &args["statements"]);
                    Value::Null
                }
                "clock" => {
                    clock.0.set(dt(s(&args["now"])));
                    Value::Null
                }
                "facts" => {
                    let search = MemorySearch::new(conn, &clock, model);
                    let v = vector(args);
                    let leave: Vec<String> = args.get("leave_out").map(strings).unwrap_or_default();
                    let hits = search
                        .facts(
                            &StoreFacts(conn),
                            s(&args["query"]),
                            v.as_deref(),
                            args.get("focus").and_then(Value::as_str).unwrap_or(""),
                            args.get("limit")
                                .and_then(Value::as_u64)
                                .map(|x| x as usize),
                            &leave,
                        )
                        .unwrap();
                    Value::Array(hits.iter().map(|h| hit_json(h, None)).collect())
                }
                "chat" => {
                    let search = MemorySearch::new(conn, &clock, model);
                    let v = vector(args);
                    let kinds: Vec<String> = args.get("kinds").map(strings).unwrap_or_else(|| {
                        search::CHAT_KINDS.iter().map(|k| (*k).to_owned()).collect()
                    });
                    let kinds: Vec<&str> = kinds.iter().map(String::as_str).collect();
                    let session = args
                        .get("accept_session")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let accept = |row: &ChatRow| session.as_ref().is_none_or(|x| &row.session == x);
                    let exclude: Vec<String> = args.get("exclude").map(strings).unwrap_or_default();
                    let hits = search
                        .chat(
                            s(&args["query"]),
                            v.as_deref(),
                            args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize,
                            &kinds,
                            &accept,
                            &exclude,
                            args.get("since").and_then(Value::as_str),
                            args.get("until").and_then(Value::as_str),
                        )
                        .unwrap();
                    Value::Array(hits.iter().map(|h| hit_json(h, Some(&h.data))).collect())
                }
                "index_chat" => json!(MemorySearch::new(conn, &clock, None)
                    .index_chat(&|| false)
                    .unwrap()),
                "forget_while_embedding" => {
                    forgetful
                        .forget_on
                        .borrow_mut()
                        .push((s(&args["marker"]).to_owned(), args["statements"].clone()));
                    Value::Null
                }
                "embed_pending" => {
                    let lookup = |kind: &str, key: &str, _b: &Value| -> String {
                        by_node
                            .get(&(kind.to_owned(), key.to_owned()))
                            .cloned()
                            .unwrap_or_default()
                    };
                    let mut search = MemorySearch::new(conn, &clock, None);
                    let got = search
                        .embed_pending(&forgetful, &lookup, &|| false, &mut || {})
                        .unwrap();
                    json!({"embedded": got.embedded, "batches": got.batches, "removed": got.removed})
                }
                "drop_stale" => json!(MemorySearch::new(conn, &clock, None)
                    .drop_stale(s(&args["model"]))
                    .unwrap()),
                "pending" => {
                    let lookup = |kind: &str, key: &str, _b: &Value| -> String {
                        by_node
                            .get(&(kind.to_owned(), key.to_owned()))
                            .cloned()
                            .unwrap_or_default()
                    };
                    json!(MemorySearch::new(conn, &clock, None)
                        .pending(s(&args["model"]), &lookup)
                        .unwrap())
                }
                "store" => {
                    for r in args["rows"].as_array().unwrap() {
                        vectors::store(
                            conn,
                            s(&r[0]),
                            s(&r[1]),
                            s(&r[2]),
                            s(&r[3]),
                            s(&r[4]),
                            &i8s(&r[5]),
                        )
                        .unwrap();
                    }
                    Value::Null
                }
                "nearest" => {
                    let kinds = strings(&args["kinds"]);
                    let kinds: Vec<&str> = kinds.iter().map(String::as_str).collect();
                    let near = vectors::nearest(
                        conn,
                        s(&args["model"]),
                        &vector(args).unwrap(),
                        &kinds,
                        args.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize,
                    )
                    .unwrap();
                    Value::Array(near.iter().map(|(d, x)| json!([d, bits(*x)])).collect())
                }
                "imported_from" => {
                    let got = search::imported_from(conn, &strings(&args["ids"])).unwrap();
                    Value::Object(got.into_iter().map(|(k, v)| (k, json!(v))).collect())
                }
                "dump" => {
                    let got: serde_json::Map<String, Value> = args["tables"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|t| (s(t).to_owned(), dump(conn, s(t))))
                        .collect();
                    assert_eq!(&Value::Object(got), want, "dump at step {step}");
                    compared += 1;
                    continue;
                }
                other => panic!("unknown op {other}"),
            };
            if &got != want {
                if let (Some(g), Some(w)) = (got.as_array(), want.as_array()) {
                    let at = g.iter().zip(w).position(|(a, b)| a != b);
                    panic!(
                        "{op} {args}: lengths {} {}; first difference at {at:?}: {:?} vs {:?}",
                        g.len(),
                        w.len(),
                        at.map(|i| &g[i]),
                        at.map(|i| &w[i])
                    );
                }
                panic!("{op} {args}: {got} vs {want}");
            }
            compared += 1;
        }
        drop(db);
    }
    assert!(compared > 90, "{compared}");
}

fn ranking(v: &Value) -> Ranking {
    let mut r = Ranking::default();
    for (k, n) in v.as_object().unwrap() {
        r.set(k, n.as_u64().unwrap() as usize);
    }
    r
}

// decision: MEM-6, MEM-27
#[test]
fn the_pure_helpers_match_python() {
    let fx = fixture("search.json");
    let mut n = 0;
    for case in fx["pure"].as_array().unwrap() {
        let a = &case["args"];
        let str_list = |v: &Value| json!(v);
        let got: Value = match s(&case["fn"]) {
            "query_terms" => str_list(&json!(terms::query_terms(s(&a[0])))),
            "slug" => json!(terms::slug(s(&a[0]), None)),
            "slug_n" => json!(terms::slug(s(&a[0]), Some(a[1].as_u64().unwrap() as usize))),
            "subject_key" => json!(terms::subject_key(s(&a[0]))),
            "tokens" => json!(search::tokens(s(&a[0]))),
            "own_words" => json!(search::own_words(s(&a[0]))),
            "short" => json!(search::short(s(&a[0]), 300)),
            "short_n" => json!(search::short(s(&a[0]), a[1].as_u64().unwrap() as usize)),
            "chat_text" => json!(search::chat_text(s(&a[0]), &a[1]).unwrap()),
            "fact_text" => json!(search::fact_text(s(&a[0]), s(&a[1]), s(&a[2]), s(&a[3]))),
            "match" => json!(search::match_expr(&strings(&a[0]))),
            "tied" => {
                let rows: Vec<(String, f64)> = a[0]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| (s(&r[0]).to_owned(), unbits(s(&r[1]))))
                    .collect();
                let t = search::tied(
                    rows.iter().map(|(k, v)| (k.as_str(), *v)),
                    a[1].as_bool().unwrap(),
                );
                Value::Object(t.iter().map(|(k, v)| (k.to_owned(), json!(v))).collect())
            }
            "fused" => {
                let rankings: Vec<Ranking> = a[0].as_array().unwrap().iter().map(ranking).collect();
                Value::Object(
                    search::fused(&rankings)
                        .into_iter()
                        .map(|(k, v)| (k, json!(bits(v))))
                        .collect(),
                )
            }
            "age_days" => json!(bits(search::age_days(s(&a[0]), &dt(s(&a[1]))))),
            "recency" => json!(bits(search::recency(
                s(&a[0]),
                &dt(s(&a[1])),
                a[2].as_bool().unwrap()
            ))),
            "within" => {
                let (shown, left) = search::within(
                    &strings(&a[0]),
                    a[1].as_u64().unwrap() as usize,
                    a[2].as_u64().unwrap() as usize,
                );
                json!([shown, left])
            }
            "weighted" => {
                let counts = a[2].as_object().unwrap();
                let got = terms::weighted(&strings(&a[0]), a[1].as_i64().unwrap(), |t| {
                    Ok::<i64, ()>(counts.get(t).and_then(Value::as_i64).unwrap_or(0))
                })
                .unwrap();
                json!(got)
            }
            "contains_words" => json!(terms::contains_words(s(&a[0]), s(&a[1]))),
            "quoted" => json!(terms::quoted(s(&a[0]), s(&a[1]))),
            other => panic!("{other}"),
        };
        assert_eq!(got, case["out"], "{}({})", s(&case["fn"]), a);
        n += 1;
    }
    assert!(n > 2000);
    assert_eq!(search::WORKER.memory_tokens, 2000);
    assert_eq!(search::ATTENTION.memory_items, 20);
}

/// What this build's bundled SQLite does with FTS5 against Python's SQLite (search.json "sqlite", 3.53.4): the
/// unicode61 tokenizer with FERB's categories (memory_fts, chat_fts) and its other options tokenize and rank (bm25
/// bits, order) identically. The trigram tokenizer (learning_titles, migration 0030) differs on text holding a NUL:
/// SQLite 3.46 stops reading the text at the NUL, Python's 3.53.4 reads past it; ranks over such a corpus differ.
#[test]
fn fts5_tokenizes_and_ranks_as_pythons_sqlite() {
    let fx = fixture("search.json");
    let sq = &fx["sqlite"];
    let conn = Connection::open_in_memory().unwrap();
    let version: String = conn
        .query_row("SELECT sqlite_version()", [], |r| r.get(0))
        .unwrap();
    let strings_ = strings(&sq["strings"]);
    let tables = [
        ("cats", "unicode61 categories 'L* N* Co M*'"),
        ("plain", "unicode61"),
        ("nodiac", "unicode61 remove_diacritics 0"),
        ("diac2", "unicode61 remove_diacritics 2"),
        ("trigram", "trigram"),
        ("trigram_ci", "trigram case_sensitive 0"),
    ];
    let mut token_diffs: Vec<(String, String)> = Vec::new();
    let mut rank_diffs: Vec<String> = Vec::new();
    let mut compared = 0;
    for (name, tok) in tables {
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE {name} USING fts5(x, tokenize=\"{tok}\");
             CREATE VIRTUAL TABLE {name}_v USING fts5vocab({name}, 'instance');"
        ))
        .unwrap();
        for t in &strings_ {
            conn.execute(&format!("INSERT INTO {name} (x) VALUES (?)"), [t])
                .unwrap();
        }
        let mut per: Vec<Vec<Value>> = vec![Vec::new(); strings_.len()];
        let mut stmt = conn
            .prepare(&format!(
                "SELECT term, doc FROM {name}_v ORDER BY doc, offset"
            ))
            .unwrap();
        for row in stmt
            .query_map([], |r| {
                Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?))
            })
            .unwrap()
        {
            let (term, doc) = row.unwrap();
            per[doc as usize - 1].push(term.map_or(Value::Null, Value::String));
        }
        for (i, terms) in per.iter().enumerate() {
            compared += 1;
            if json!(terms) != sq["tokens"][name][i] {
                token_diffs.push((name.to_owned(), strings_[i].clone()));
            }
        }
    }
    for case in sq["ranks"].as_array().unwrap() {
        let name = s(&case["table"]);
        let got = conn
            .prepare(&format!(
                "SELECT rowid, bm25({name}) FROM {name} WHERE {name} MATCH ? ORDER BY 2, rowid LIMIT 60"
            ))
            .and_then(|mut st| {
                st.query_map([s(&case["query"])], |r| {
                    Ok(json!([r.get::<_, i64>(0)?, bits(r.get::<_, f64>(1)?)]))
                })?
                .collect::<Result<Vec<_>, _>>()
            });
        compared += 1;
        match (got, case.get("rows")) {
            (Ok(rows), Some(want)) if json!(rows) == *want => {}
            (Err(_), None) => {}
            _ => rank_diffs.push(name.to_owned()),
        }
    }
    eprintln!(
        "SQLite {version} vs Python's {}: {compared} comparisons, {} token lists and {} rankings differ",
        sq["version"],
        token_diffs.len(),
        rank_diffs.len()
    );
    assert!(compared > 1600);
    for (table, text) in &token_diffs {
        assert!(
            table.starts_with("trigram") && text.contains('\0'),
            "{table}: {text:?}"
        );
    }
    assert!(
        rank_diffs.iter().all(|t| t.starts_with("trigram")),
        "{rank_diffs:?}"
    );
}
