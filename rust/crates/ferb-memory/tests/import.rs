//! The import readers and the extraction checks against what Python produced (`rust/fixtures/memory/make.py`:
//! import.json, extract.json; ENG-3). Each export is written into a temporary folder as Python wrote it (zips,
//! folders, links, modification times) and read with the fixture's fake redactor and a fixed local zone
//! (TZ="FX-05:30").

mod common;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use common::*;
use ferb_memory::import_extract::{self as extract, Candidate, OwnerIndex};
use ferb_memory::import_sources::{self as sources, Conversation, Message, ReadOptions, Unzstd};
use ferb_memory::terms::Fact;
use ferb_memory::times::{self, FixedZone};
use ferb_memory::{html, Error};
use serde_json::{json, Value};

const ZONE: FixedZone = FixedZone(19_800);

fn at_json(m: &Message) -> Value {
    m.at.as_ref().map_or(Value::Null, |a| json!(times::iso(a)))
}

fn conv_json(c: &Conversation) -> Value {
    json!({"source": c.source, "id": c.id, "title": c.title, "claims": c.claims,
           "messages": c.messages.iter().map(|m| json!({"role": m.role, "text": m.text, "at": at_json(m)}))
               .collect::<Vec<_>>()})
}

fn conv_from(v: &Value) -> Conversation {
    Conversation {
        source: s(&v["source"]).into(),
        id: s(&v["id"]).into(),
        title: s(&v["title"]).into(),
        claims: v["claims"].as_bool().unwrap(),
        messages: v["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                Message::new(
                    s(&m["role"]),
                    s(&m["text"]),
                    m["at"].as_str().map(|a| times::parse_iso(a).unwrap()),
                )
            })
            .collect(),
    }
}

/// The fixture's decoder: each compressed rollout Python decompressed, by its bytes; anything else is damaged.
struct Table(HashMap<Vec<u8>, Vec<u8>>);

impl Unzstd for Table {
    fn decompress(&self, data: &[u8]) -> (Vec<u8>, bool) {
        match self.0.get(data) {
            Some(out) => (out.clone(), false),
            None => (Vec::new(), true),
        }
    }
}

fn write_files(root: &Path, files: &Value) {
    fs::create_dir_all(root).unwrap();
    for f in files.as_array().unwrap() {
        let p = root.join(s(&f["path"]));
        if f.get("dir").is_some() {
            fs::create_dir_all(&p).unwrap();
            continue;
        }
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        if let Some(link) = f.get("link") {
            std::os::unix::fs::symlink(root.join(s(link)), &p).unwrap();
            continue;
        }
        fs::write(&p, b64(s(&f["b64"]))).unwrap();
        if let Some(ns) = f.get("mtime").and_then(Value::as_u64) {
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_nanos(ns);
            fs::File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(t)
                .unwrap();
        }
    }
}

// decision: MEM-400, MEM-401, MEM-403, MEM-404, MEM-405, MEM-407, MEM-413, MEM-414
#[test]
fn every_reader_gives_pythons_conversations() {
    let fx = fixture("import.json");
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_string_lossy().into_owned();
    let zstd = Table(
        fx["zstd"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(path, plain)| {
                let packed = fx["cases"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|c| c["files"].as_array().unwrap().iter())
                    .find(|f| f["path"] == json!(path))
                    .unwrap();
                (b64(s(&packed["b64"])), b64(s(plain)))
            })
            .collect(),
    );
    let mut n = 0;
    let mut fixed_differs = 0;
    for case in fx["cases"].as_array().unwrap() {
        let name = s(&case["name"]);
        let root = dir.path().join("imports").join(name);
        write_files(&root, &case["files"]);
        fs::create_dir_all(root.join("home")).unwrap();
        let home = root.join("home");
        let redact = |t: &str| fake_redact(t);
        let opts = ReadOptions {
            now: times::parse_iso(s(&case["now"])).unwrap(),
            redact: &redact,
            home: Some(&home),
            zone: &ZONE,
            unzstd: Some(&zstd),
        };
        let path = case["path"].as_str().map(|p| root.join(p));
        let got = sources::read(s(&case["source"]), path.as_deref(), &opts);
        // decision: MEM-414 (deliberate difference): the Gemini parsers are closed after feeding
        let want = case.get("fixed").unwrap_or(case);
        if case.get("fixed").is_some_and(|f| f["out"] != case["out"]) {
            fixed_differs += 1;
        }
        match (got, want.get("out")) {
            (Ok(convs), Some(out)) => {
                let got: Vec<Value> = convs.iter().map(conv_json).collect();
                assert_eq!(&json!(got), out, "{name}");
            }
            (Err(Error::Unreadable(m)), None) => {
                assert_eq!(s(&want["raises"]), "ImportUnreadable", "{name}");
                assert_eq!(m.replace(&base, "/FX"), s(&want["message"]), "{name}");
            }
            (Err(Error::Crash(kind)), None) => {
                assert_ne!(s(&want["raises"]), "ImportUnreadable", "{name}: {kind}");
            }
            (got, _) => panic!("{name}: {got:?} vs {want}"),
        }
        n += 1;
    }
    assert!(n >= 30, "{n}");
    assert!(fixed_differs >= 1, "the HTML fix must show in a case");
}

// decision: MEM-401, MEM-407, MEM-413
#[test]
fn the_reader_helpers_match_python() {
    let fx = fixture("import.json");
    let mut n = 0;
    for case in fx["helpers"].as_array().unwrap() {
        let a = &case["args"];
        let parts_json = |v: Vec<String>| json!(v);
        let wrapped = |x: &str| format!("<div class=\"outer-cell\">{x}</div>{x}");
        let got: Value = match s(&case["fn"]) {
            "cut" => json!(sources::cut(s(&a[0]), a[1].as_u64().unwrap() as usize)),
            "owner_words" => json!(sources::owner_words(s(&a[0]))),
            "window_start" => {
                json!(sources::window_start(&times::parse_iso(s(&a[0])).unwrap())
                    .map(|d| times::iso(&d)))
            }
            "prompt" => json!(sources::prompt(s(&a[0]))),
            "unescape" => json!(html::unescape(s(&a[0]))),
            "text_parts_feed" => parts_json(sources::text_parts(s(&a[0]), false)),
            "text_parts_closed" => parts_json(sources::text_parts(s(&a[0]), true)),
            "strip_html" => json!(sources::strip_html_with(s(&a[0]), false)),
            "strip_html_closed" => json!(sources::strip_html(s(&a[0]))),
            "activity_feed" => json!(sources::activity_cells(&wrapped(s(&a[0])), false)),
            "activity_closed" => json!(sources::activity_cells(&wrapped(s(&a[0])), true)),
            "unix" => json!(sources::unix(Some(&a[0])).map(|d| times::iso(&d))),
            "iso" => json!(sources::iso(Some(&a[0])).map(|d| times::iso(&d))),
            other => panic!("{other}"),
        };
        assert_eq!(got, case["out"], "{}({})", s(&case["fn"]), a);
        n += 1;
    }
    assert!(n > 300, "{n}");
}

fn candidate_json(c: &Candidate) -> Value {
    json!({"id": c.id, "kind": c.kind, "subject": c.subject, "key": c.key, "content": c.content,
           "message": c.message, "quote": c.quote, "claim": c.claim,
           "evidence": c.evidence.iter().map(|(r, _, who)| json!([r, who])).collect::<Vec<_>>()})
}

fn fact(h: &Value) -> Fact {
    Fact {
        id: s(&h["id"]).into(),
        kind: s(&h["kind"]).into(),
        subject: s(&h["subject"]).into(),
        key: s(&h["key"]).into(),
        content: s(&h["content"]).into(),
        origin: s(&h["origin"]).into(),
        status: "active".into(),
        valid_from: times::parse_iso(s(&h["valid_from"])).unwrap(),
        valid_to: None,
        set_aside: false,
    }
}

// decision: MEM-408, MEM-409, MEM-410, MEM-413
#[test]
fn extraction_matches_python() {
    let fx = fixture("extract.json");
    let convs: Vec<Conversation> = fx["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .map(conv_from)
        .collect();
    for case in fx["parts"].as_array().unwrap() {
        let limit = case["limit"].as_u64().unwrap() as usize;
        let ps = extract::parts(&convs, limit, &ZONE);
        let got: Vec<Value> = ps
            .iter()
            .map(|p| json!({"key": p.key, "conversation": p.conversation.id, "lines": p.lines,
                            "refs": p.refs.iter().map(|(r, _)| r).collect::<Vec<_>>(), "chars": p.chars()}))
            .collect();
        assert_eq!(json!(got), case["parts"], "parts at {limit}");
        let chunks = extract::chunks("chatgpt", &ps, limit);
        let got: Vec<Value> = chunks
            .iter()
            .map(|c| {
                json!({"parts": c.parts.iter().map(|p| ps.iter().position(|q| q.key == p.key && q.lines == p.lines).unwrap()).collect::<Vec<_>>(),
                       "text": c.text(), "refs": c.refs().iter().map(|(r, _, _)| r).collect::<Vec<_>>()})
            })
            .collect();
        assert_eq!(json!(got), case["chunks"], "chunks at {limit}");
    }
    let index = OwnerIndex::new(&convs);
    let mut msgs: Vec<Value> = Vec::new();
    for (r, _, terms) in &index.messages {
        let mut t = terms.clone();
        t.sort();
        msgs.push(json!([r, t]));
    }
    assert_eq!(json!(msgs), fx["index"]["messages"]);
    let mut weight: Vec<(&String, &f64)> = index.weight.iter().collect();
    weight.sort_by(|a, b| a.0.cmp(b.0));
    let weight: serde_json::Map<String, Value> = weight
        .into_iter()
        .map(|(k, v)| (k.clone(), json!(bits(*v))))
        .collect();
    assert_eq!(Value::Object(weight), fx["index"]["weight"]);
    for f in fx["index"]["find"].as_array().unwrap() {
        let got: Vec<String> = index
            .find(s(&f["text"]), f["most"].as_u64().unwrap() as usize)
            .into_iter()
            .map(|(r, _)| r)
            .collect();
        assert_eq!(json!(got), f["out"], "find {}", f["text"]);
    }
    let small = OwnerIndex::new(&convs[..1]);
    let got: Vec<String> = small
        .find("bees ravi", 3)
        .into_iter()
        .map(|(r, _)| r)
        .collect();
    assert_eq!(json!(got), fx["index_small"]["find"]);

    let ps = extract::parts(&convs, extract::CHUNK_CHARS, &ZONE);
    let chunk = &extract::chunks("chatgpt", &ps, extract::CHUNK_CHARS)[0];
    let redact = |t: &str| fake_redact(t);
    let (kept, dropped) =
        extract::check_recall(&fx["recall"]["answer"], chunk, &index, &redact).unwrap();
    assert_eq!(
        json!(kept.iter().map(candidate_json).collect::<Vec<_>>()),
        fx["recall"]["kept"]
    );
    assert_eq!(json!(dropped), fx["recall"]["dropped"]);
    let tag = &kept[0].message.rsplit_once('.').unwrap().0.to_owned();
    let crash = extract::check_recall(
        &json!({"candidates": [{"message": format!("{tag}.1"), "quote": "bees"}]}),
        chunk,
        &index,
        &|t: &str| t.to_owned(),
    );
    assert!(
        matches!(crash, Err(Error::Crash("KeyError")))
            && fx["recall_crash"]["raises"] == "KeyError"
    );

    let held_ids: HashSet<String> = ["m-held", "m-two"]
        .iter()
        .map(|x| (*x).to_owned())
        .collect();
    for case in fx["verify"].as_array().unwrap() {
        let got = extract::check_verify(&case["answer"], &kept, &held_ids, "chatgpt");
        match (got, case.get("kept")) {
            (Ok((v, why)), Some(want)) => {
                let got: Vec<Value> = v
                    .iter()
                    .map(|v| json!({"kind": v.kind, "subject": v.subject, "key": v.key, "content": v.content,
                                    "said_at": times::iso(&v.said_at), "source": v.source, "same_as": v.same_as,
                                    "ended": v.ended, "until": v.until.map(|d| d.isoformat()),
                                    "review_in_days": v.review_in_days, "id": v.id}))
                    .collect();
                assert_eq!(&json!(got), want, "{}", case["name"]);
                assert_eq!(json!(why), case["dropped"], "{}", case["name"]);
            }
            (Err(_), None) => {}
            (got, _) => panic!("{}: {got:?} vs {case}", case["name"]),
        }
    }
    let held: Vec<Fact> = fx["held"].as_array().unwrap().iter().map(fact).collect();
    assert_eq!(
        extract::verify_context("chatgpt", &kept, &held, &ZONE),
        s(&fx["verify_context"])
    );
    assert_eq!(
        extract::verify_context("claude", &[], &[], &ZONE),
        s(&fx["verify_context_empty"])
    );
    for d in fx["digest"].as_array().unwrap() {
        let parts = strings(&d[0]);
        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        assert_eq!(extract::digest(&parts), s(&d[1]));
    }
    for case in fx["as_json"].as_array().unwrap() {
        let got = extract::as_json(&case["in"]);
        match (got, case.get("out")) {
            (Ok(m), Some(want)) => assert_eq!(&Value::Object(m), want),
            (Err(_), None) => {}
            (got, _) => panic!("{case}: {got:?}"),
        }
    }
    assert_eq!(
        extract::recall_schema()["properties"]["candidates"]["items"]["properties"]["kind"]["enum"]
            [4],
        "env"
    );

    // similar_held, over a store the Rust store crate migrated
    let sh = &fx["similar_held"];
    let dir = tempfile::tempdir().unwrap();
    let db = ferb_store::Db::open(dir.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    for st in sh["seed"].as_array().unwrap() {
        let params: Vec<rusqlite::types::Value> = st[1]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| match v {
                Value::Null => rusqlite::types::Value::Null,
                Value::Number(n) if n.is_f64() => rusqlite::types::Value::Real(n.as_f64().unwrap()),
                Value::Number(n) => rusqlite::types::Value::Integer(n.as_i64().unwrap()),
                Value::String(t) => rusqlite::types::Value::Text(t.clone()),
                _ => unreachable!(),
            })
            .collect();
        db.conn()
            .execute(s(&st[0]), rusqlite::params_from_iter(params))
            .unwrap();
    }
    let cands: Vec<Candidate> = sh["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| Candidate {
            id: s(&c["id"]).into(),
            kind: s(&c["kind"]).into(),
            subject: s(&c["subject"]).into(),
            key: s(&c["key"]).into(),
            content: s(&c["content"]).into(),
            message: s(&c["message"]).into(),
            quote: s(&c["quote"]).into(),
            claim: false,
            evidence: Vec::new(),
        })
        .collect();
    let facts = ferb_memory::search::StoreFacts(db.conn());
    let lookup = |id: &str| ferb_memory::search::FactLookup::get(&facts, id);
    let mut asked = Vec::new();
    let answers = sh["answers"].as_object().unwrap();
    let mut similar = |text: &str| {
        asked.push(text.to_owned());
        answers.get(text).map(strings).unwrap_or_default()
    };
    let got = extract::similar_held(
        &lookup,
        &cands,
        &mut similar,
        &times::parse_iso(s(&sh["now"])).unwrap(),
    )
    .unwrap();
    assert_eq!(json!(asked), sh["asked"]);
    assert_eq!(
        json!(got.iter().map(|f| &f.id).collect::<Vec<_>>()),
        sh["out"]
    );
}
