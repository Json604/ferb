//! The policy against what the Python code did (decision: ENG-3). `rust/fixtures/policy/make.py` ran ferb/policy on
//! invented data and on the inputs its guarding tests use, and wrote every result; each test here replays one
//! section and must get the same, text byte for byte. Seams answer from Python's call logs.

#![recursion_limit = "512"]

mod common;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use common::*;
use ferb_policy::confirm::{self, Facts, Values};
use ferb_policy::provenance::{self, ArgSources};
use ferb_policy::{
    carried, destinations, idna, norm, parity, payments, py, pyurl, rules, shlex, small, terminal,
    tls, urls,
};
use serde_json::{json, Value};

fn char_spans(text: &str, spans: &[(usize, usize)]) -> Value {
    let idx = |b: usize| text[..b].chars().count();
    Value::Array(
        spans
            .iter()
            .map(|&(a, b)| json!([idx(a), idx(b)]))
            .collect(),
    )
}

fn url_json(u: Option<urls::Url>) -> Value {
    match u {
        None => Value::Null,
        Some(u) => {
            json!({"scheme": u.scheme, "username": u.username, "password": u.password, "host": u.host,
            "port": u.port, "path": u.path, "query": u.query, "fragment": u.fragment})
        }
    }
}

fn res<T>(r: Result<T, impl std::fmt::Debug>, f: impl FnOnce(T) -> Value) -> Value {
    match r {
        Ok(v) => json!({"ok": f(v)}),
        Err(_) => json!({"raises": "Error"}),
    }
}

/// Python raised (any class) where Rust returned Err: the same; otherwise the values must be equal.
fn outcome_eq(want: &Value, got: &Value) -> Value {
    if raised(want) && got.get("raises").is_some() {
        return want.clone();
    }
    got.clone()
}

// decision: ENG-3
#[test]
fn python_text_handling() {
    let fx = fixture("text.json");
    let mut d = Diffs::new();
    for c in fx["unicode"].as_array().unwrap() {
        let t = s(&c["text"]);
        let got = json!({"text": t, "lower": py::lower(&t), "upper": py::upper(&t), "casefold": py::casefold(&t),
            "capitalize": py::capitalize(&t), "nfc": norm::nfc(&t), "nfd": norm::nfd(&t), "nfkc": norm::nfkc(&t),
            "split": py::split_ws(&t), "strip": py::strip(&t), "isdigit": py::all_digits(&t),
            "isalnum": py::all_alnum(&t), "isprintable": py::all_printable(&t)});
        d.check("unicode", &c["text"], c, &got);
    }
    for c in fx["idna"].as_array().unwrap() {
        if let Some(input) = c.get("input") {
            let got = res(idna::uts46_remap(&s(input)), |v| json!(v));
            d.check(
                "uts46_remap",
                input,
                &c["remap"],
                &outcome_eq(&c["remap"], &got),
            );
        } else if let Some(label) = c.get("label") {
            let a = res(idna::alabel(&s(label)), |v| json!(v));
            let u = res(idna::ulabel(&s(label)), |v| json!(v));
            d.check("alabel", label, &c["alabel"], &outcome_eq(&c["alabel"], &a));
            d.check("ulabel", label, &c["ulabel"], &outcome_eq(&c["ulabel"], &u));
        } else {
            let w: Vec<u32> = s(&c["punycode"]).chars().map(|c| c as u32).collect();
            d.check(
                "punycode",
                &c["punycode"],
                &c["encoded"],
                &json!(idna::punycode_encode(&w)),
            );
        }
    }
    for c in fx["urls"].as_array().unwrap() {
        let u = s(&c["url"]);
        if let Some(want) = c.get("parse") {
            d.check(
                "urls.parse",
                &c["url"],
                want,
                &json!({"ok": url_json(urls::parse(&u))}),
            );
        }
        if let Some(want) = c.get("urlsplit") {
            let got = res(pyurl::urlsplit(&u), |p| {
                let port = res(p.port(), |x| json!(x));
                let mut unsplit = p.clone();
                unsplit.fragment.clear();
                json!({"parts": [p.scheme, p.netloc, p.path, p.query, p.fragment], "username": p.username(),
                    "password": p.password(), "hostname": p.hostname(),
                    "port": if port.get("raises").is_some() { json!({"raises": "ValueError"}) } else { port },
                    "unsplit": pyurl::urlunsplit(&unsplit)})
            });
            d.check("urlsplit", &c["url"], want, &outcome_eq(want, &got));
        }
        if c.get("quote").is_some() {
            let got = json!({"quote": pyurl::quote(&u, ""), "quote_path": pyurl::quote(&u, ":/"),
                "quote_plus": pyurl::quote_plus(&u), "page_scope": confirm::page_scope(&u),
                "site_key": confirm::site_key(Some(&u)), "registrable": {"ok": confirm::registrable(&u)}});
            let want = json!({"quote": c["quote"], "quote_path": c["quote_path"], "quote_plus": c["quote_plus"],
                "page_scope": c["page_scope"], "site_key": c["site_key"], "registrable": c["registrable"]});
            d.check("quote/scope", &c["url"], &want, &got);
        }
        if c.get("unquote").is_some() {
            d.check(
                "unquote",
                &c["url"],
                &json!([c["unquote"], c["unquote_plus"]]),
                &json!([pyurl::unquote(&u), pyurl::unquote_plus(&u)]),
            );
        }
    }
    for c in fx["ips"].as_array().unwrap() {
        let t = s(&c["text"]);
        let got = match pyurl::ip_address(&t) {
            Some(pyurl::Ip::V4(n)) => json!({"ok": pyurl::ipv4_text(n)}),
            Some(pyurl::Ip::V6(n, scope)) => json!({"ok": pyurl::ipv6_text(n, scope.as_deref())}),
            None => json!({"raises": "ValueError"}),
        };
        d.check("ip_address", &c["text"], &c["ip"], &got);
    }
    for c in fx["shlex"].as_array().unwrap() {
        let got = match shlex::split(&s(&c["text"])) {
            Ok(t) => json!({"ok": t}),
            Err(shlex::Unclosed) => json!({"raises": "ValueError"}),
        };
        d.check("shlex", &c["text"], &c["tokens"], &got);
    }
    for c in fx["regex"].as_array().unwrap() {
        let t = s(&c["text"]);
        if c.get("run").is_some() {
            d.check(
                "_RUN",
                &c["text"],
                &c["run"],
                &char_spans(&t, &parity::run_spans(&t)),
            );
            let gd: Vec<(usize, usize)> = parity::grouped_digits(&t)
                .into_iter()
                .map(|m| {
                    let a = m.as_ptr() as usize - t.as_ptr() as usize;
                    (a, a + m.len())
                })
                .collect();
            d.check(
                "_GROUPED_DIGITS",
                &c["text"],
                &c["grouped_digits"],
                &char_spans(&t, &gd),
            );
            d.check(
                "_JOINED",
                &c["text"],
                &c["joined"],
                &json!(parity::joined(&t)),
            );
            d.check(
                "_FINE",
                &c["text"],
                &c["fine"],
                &json!(confirm::fine_split(&t)),
            );
        } else {
            d.check(
                "_URL_IN_TEXT",
                &c["text"],
                &c["url_in_text"],
                &char_spans(&t, &parity::url_spans(&t)),
            );
            d.check(
                "_HOST_PATH",
                &c["text"],
                &c["host_path"],
                &char_spans(&t, &parity::host_path_spans(&t)),
            );
        }
    }
    for c in fx["decodings"].as_array().unwrap() {
        let got: BTreeSet<String> = parity::decodings(&s(&c["piece"])).into_iter().collect();
        d.check("_decodings", &c["piece"], &c["decodings"], &json!(got));
    }
    for c in fx["amount_keys"].as_array().unwrap() {
        let got = payments::amount_key(c["shown"].as_str()).map(|(u, n)| json!([u, n]));
        d.check("amount_key", &c["shown"], &c["key"], &json!({"ok": got}));
    }
    d.finish("python text handling");
}

fn prep_json(t: &provenance::Text) -> Value {
    let sorted = |h: &HashSet<String>| {
        let mut v: Vec<&String> = h.iter().collect();
        v.sort();
        json!(v)
    };
    json!({"folded": t.folded, "tokens": sorted(&t.tokens), "paths": sorted(&t.paths), "hosts": sorted(&t.hosts),
        "amounts": t.amounts, "phones": sorted(&t.phones),
        "words": t.words.iter().map(|w| json!([w.folded, w.capital, w.sentence_start, w.tight])).collect::<Vec<_>>()})
}

fn pairs(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .map(|a| a.iter().map(|x| (s(&x[0]), s(&x[1]))).collect())
        .unwrap_or_default()
}

fn arg_json(a: &ArgSources) -> Value {
    Value::Object(a.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
}

/// The fixture's temporary root, as a fixed fake folder (nothing is read there).
fn untmp(text: &str) -> String {
    text.replace("<TMP>", "/tmp/policy-fixture")
}

// decision: GATE-508, GATE-509, GATE-555
#[test]
fn provenance_matches_python() {
    let fx = fixture("provenance.json");
    let mut d = Diffs::new();
    for c in fx["prepare"].as_array().unwrap() {
        let t = s(&c["text"]);
        d.check(
            "prepare",
            &c["text"],
            &c["out"],
            &prep_json(&provenance::prepare(&t)),
        );
        d.check(
            "embedded",
            &c["text"],
            &c["embedded"],
            &json!(provenance::embedded(&t)),
        );
        d.check(
            "phone_runs",
            &c["text"],
            &c["phone_runs"],
            &json!(provenance::phone_runs(&t)),
        );
        d.check(
            "raw_tokens",
            &c["text"],
            &c["raw_tokens"],
            &json!(provenance::raw_tokens(&t)),
        );
    }
    for c in fx["from_words"].as_array().unwrap() {
        let got: Vec<Value> = destinations::from_words(&s(&c["text"]))
            .into_iter()
            .map(|(k, v)| json!([k, v]))
            .collect();
        d.check("from_words", &c["text"], &c["out"], &json!(got));
    }
    for c in fx["shapes"].as_array().unwrap() {
        let v = s(&c["value"]);
        let (dk, dv) = destinations::destination_of(&v);
        let got = json!({"value": v, "shape": provenance::destination_shape(&v), "typed": provenance::typed_destination(&v),
            "host_of": provenance::host_of(&v), "destination_of": [dk, dv],
            "normalize": {"email": destinations::normalize("email", &v), "phone": destinations::normalize("phone", &v),
                "host": destinations::normalize("host", &v), "name": destinations::normalize("name", &v)}});
        d.check("shapes", &c["value"], c, &got);
    }
    for c in fx["sources"].as_array().unwrap() {
        let got = provenance::sources(
            &s(&c["value"]),
            &s(&c["owner"]),
            &pairs(&c["external"]),
            &strs(&c["aliases"]),
            &s(&c["kind"]),
            &s(&c["answer"]),
        );
        d.check("sources", c, &c["out"], &json!(got));
    }
    for c in fx["arg_sources"].as_array().unwrap() {
        let mut t = target(&c["target"]).unwrap();
        t.path = t.path.map(|p| untmp(&p));
        let got = provenance::arg_sources(
            &t,
            &s(&c["owner"]),
            &pairs(&c["external"]),
            &s(&c["answer"]),
            &untmp(&s(&c["home"])),
        );
        let want: Value = serde_json::from_str(&untmp(&c["out"].to_string())).unwrap();
        d.check("arg_sources", c, &want, &arg_json(&got));
    }
    d.finish("provenance");
}

// decision: GATE-207, GATE-516, GATE-517, GATE-518, GATE-527, GATE-528, GATE-535, GATE-538, GATE-581
#[test]
fn confirm_shapes_and_words_match_python() {
    let fx = fixture("confirm.json");
    let mut d = Diffs::new();
    for c in fx["labels"].as_array().unwrap() {
        let l = s(&c["label"]);
        d.check(
            "shown_origin",
            &c["label"],
            &json!([c["origin"], c["place"]]),
            &json!([confirm::shown_origin(&l), confirm::shown_place(&l)]),
        );
    }
    for c in fx["label_sets"].as_array().unwrap() {
        let ls = strs(&c["labels"]);
        let it = || ls.iter().map(String::as_str);
        d.check(
            "shown_origins",
            &c["labels"],
            &json!([c["origins"], c["places"]]),
            &json!([confirm::shown_origins(it()), confirm::shown_places(it())]),
        );
    }
    for c in fx["ferb_words"].as_array().unwrap() {
        let w = s(&c["text"]);
        let got = json!({"text": w, "reads": confirm::reads_as_ferbs(&w), "check": confirm::reads_as_ferbs_check(&w),
            "from_task": confirm::from_task(&w), "task_question": confirm::task_question(&w),
            "from_task_lines": confirm::from_task_lines(&w), "reply_lines": confirm::reply_lines(&w),
            "reason_line": confirm::reason_line(&w)});
        d.check("ferb words", &c["text"], c, &got);
    }
    for c in fx["values"].as_array().unwrap() {
        let v = s(&c["value"]);
        let sorted = |h: HashSet<String>| {
            let b: BTreeSet<String> = h.into_iter().collect();
            json!(b)
        };
        let contacts: BTreeSet<String> = confirm::contacts_in(&v).into_iter().collect();
        let got = json!({"value": v, "kind": confirm::value_kind(&v), "year": confirm::is_year(&v),
            "date": confirm::is_date(&v), "is_value": confirm::is_value(&v), "spellings": sorted(confirm::spellings(&v)),
            "spellings_plain": sorted(parity::spellings_of(&v, false)), "whole_values": confirm::whole_values(&v),
            "account_key": confirm::account_key(Some(&v)), "site_key": confirm::site_key(Some(&v)),
            "browser_page": confirm::browser_page(Some(&v)), "shown_quoted": confirm::shown_quoted(&v),
            "contacts_in": contacts, "value_forms": sorted(parity::value_forms(&py::lower(&v), None))});
        d.check("values", &c["value"], c, &got);
    }
    for c in fx["whole_values"].as_array().unwrap() {
        d.check(
            "whole_values",
            &c["text"],
            &c["out"],
            &json!(confirm::whole_values(&s(&c["text"]))),
        );
    }
    for c in fx["registrable"].as_array().unwrap() {
        d.check(
            "registrable",
            &c["host"],
            &c["out"],
            &json!({"ok": confirm::registrable(&s(&c["host"]))}),
        );
    }
    for c in fx["page_scope"].as_array().unwrap() {
        d.check(
            "page_scope",
            &c["url"],
            &c["out"],
            &json!(confirm::page_scope(&s(&c["url"]))),
        );
    }
    for c in fx["own_contact"].as_array().unwrap() {
        d.check(
            "own_contact",
            c,
            &c["out"],
            &json!(confirm::own_contact(&s(&c["value"]), &strs(&c["own"]))),
        );
    }
    for c in fx["amounts"].as_array().unwrap() {
        d.check(
            "shown_amount",
            c,
            &c["out"],
            &json!(confirm::shown_amount(
                c["amount"].as_f64(),
                c["currency"].as_str()
            )),
        );
    }
    for c in fx["samples"].as_array().unwrap() {
        d.check(
            "sample",
            &c["items"],
            &c["out"],
            &json!(confirm::sample(strs(&c["items"]))),
        );
    }
    for c in fx["keys"].as_array().unwrap() {
        let parts = c["parts"].as_array().unwrap().clone();
        d.check("key", &c["parts"], &c["out"], &json!(confirm::key(&parts)));
    }
    for c in fx["routine"].as_array().unwrap() {
        let n = confirm::Need::new(&s(&c["rule"]), "k".into(), values(&c["values"]));
        d.check("routine", c, &c["out"], &json!(confirm::routine(&n)));
    }
    for c in fx["pages"].as_array().unwrap() {
        let a = c["address"].as_str();
        d.check(
            "browser_page",
            &c["address"],
            &json!([c["out"], c["need"]]),
            &json!([
                confirm::browser_page(a),
                need_json(&confirm::browser_page_need(
                    "Chrome",
                    a.unwrap_or(""),
                    "Type"
                ))
            ]),
        );
    }
    for c in fx["pairs"].as_array().unwrap() {
        let a = strs(&c["args"]);
        d.check(
            "pair_need",
            &c["args"],
            &c["need"],
            &need_json(&confirm::pair_need(&a[0], &a[1], &a[2], &a[3])),
        );
    }
    for c in fx["verbs"].as_array().unwrap() {
        let args = c["args"].as_object().unwrap().clone();
        d.check(
            "input_verb",
            c,
            &c["out"],
            &json!(confirm::input_verb(&s(&c["tool"]), &args)),
        );
    }
    d.check(
        "read_lift_key",
        &Value::Null,
        &fx["read_lift_key"],
        &json!(confirm::read_lift_key("read:~/.ssh")),
    );
    d.check(
        "destination_key",
        &Value::Null,
        &fx["destination_key"],
        &json!(confirm::destination_key("x.example")),
    );
    for c in fx["sent"].as_array().unwrap() {
        d.check(
            "shown_sent",
            c,
            &c["out"],
            &json!(confirm::shown_sent(
                &pairs(&c["sent"]),
                c["total"].as_u64().unwrap() as usize
            )),
        );
    }
    d.finish("confirm shapes and words");
}

fn arg_sources(v: &Value) -> ArgSources {
    v.as_object()
        .map(|m| m.iter().map(|(k, x)| (k.clone(), strs(x))).collect())
        .unwrap_or_default()
}

// decision: GATE-202, GATE-205, GATE-214, GATE-215, GATE-218, GATE-221, GATE-508, GATE-538, GATE-556
#[test]
fn decide_matches_python() {
    let fx = fixture("decide.json");
    let mut d = Diffs::new();
    for c in fx.as_array().unwrap() {
        let case = &c["case"];
        let log = &c["log"];
        let missed = std::cell::RefCell::new(Vec::<String>::new());
        let lookup = |table: &str, kind: &str, value: &str| -> bool {
            match log[table]
                .as_array()
                .and_then(|a| a.iter().find(|e| e[0] == kind && e[1] == value))
            {
                Some(e) => e[2].as_bool().unwrap_or(false),
                None => {
                    missed
                        .borrow_mut()
                        .push(format!("{table}({kind:?}, {value:?})"));
                    false
                }
            }
        };
        let known = |k: &str, v: &str| lookup("known", k, v);
        let approved_fn = |k: &str, v: &str| lookup("approved", k, v);
        let own_called = std::cell::Cell::new(0);
        let own = || {
            own_called.set(own_called.get() + 1);
            strs(&case["own"])
        };
        let outside: Vec<String> = strs(&case["outside_read"]);
        let declared: Vec<String> = strs(&case["declared"]);
        let declared: Vec<&str> = declared.iter().map(String::as_str).collect();
        let place = case["place"].as_str();
        let content = case["content"].as_str();
        let facts = Facts {
            read_private: case["read_private"].as_bool().unwrap(),
            read_external: case["read_external"].as_bool().unwrap(),
            known: &known,
            approved: if case.get("approved").is_some() {
                Some(&approved_fn)
            } else {
                None
            },
            outside_read: &outside,
            place,
            content,
            own: &own,
        };
        let t = target(&case["target"]).unwrap();
        let needs = confirm::decide(
            &s(&case["kind"]),
            &declared,
            &t,
            &arg_sources(&case["arg_sources"]),
            &facts,
        );
        let got = json!({"ok": needs.iter().map(need_json).collect::<Vec<_>>()});
        let mut got_all = json!({"out": got, "missed": missed.borrow().clone(),
            "own_calls": own_called.get()});
        let want_all = json!({"out": c["out"], "missed": [], "own_calls": log["own"].as_array().map_or(0, Vec::len)});
        if !same(&want_all, &got_all) {
            got_all["missed"] = json!(missed.borrow().clone());
        }
        d.check("decide", case, &want_all, &got_all);
    }
    d.finish("decide");
}

fn records(v: &Value) -> Vec<(i64, provenance::Text)> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|x| (x[0].as_i64().unwrap(), provenance::prepare(&s(&x[1]))))
                .collect()
        })
        .unwrap_or_default()
}

fn hits_json(h: &[confirm::PrivateValue]) -> Value {
    Value::Array(
        h.iter()
            .map(|v| json!([v.piece, v.matched, v.first_read]))
            .collect(),
    )
}

// decision: GATE-506, GATE-507, GATE-516, GATE-517, GATE-518, GATE-524, GATE-526, GATE-527, GATE-528, GATE-532,
// GATE-533, GATE-535, GATE-536, GATE-541, GATE-570, GATE-571
#[test]
fn the_sink_rule_matches_python() {
    let fx = fixture("sink.json");
    let mut d = Diffs::new();
    for c in fx["urls"].as_array().unwrap() {
        let private = records(&c["private"]);
        let outside = records(&c["outside"]);
        let owner = provenance::prepare(&s(&c["owner"]));
        let origins: Option<confirm::Origins> = c["origins"].as_object().map(|m| {
            m.iter()
                .map(|(k, v)| (k.parse().unwrap(), strs(v).into_iter().collect()))
                .collect()
        });
        let sites: Option<confirm::Sites> = c["sites"].as_object().map(|m| {
            m.iter()
                .map(|(k, v)| (k.parse().unwrap(), strs(v).into_iter().collect()))
                .collect()
        });
        let pages: Option<confirm::Pages> = c["pages"]
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.parse().unwrap(), s(v))).collect());
        let hits = confirm::url_private_values(
            &s(&c["url"]),
            &private,
            &outside,
            &owner,
            origins.as_ref(),
            sites.as_ref(),
            pages.as_ref(),
        );
        d.check("url sink", c, &c["out"], &json!({"ok": hits_json(&hits)}));
    }
    for c in fx["texts"].as_array().unwrap() {
        let hits = confirm::text_private_values(
            &s(&c["text"]),
            &records(&c["private"]),
            &records(&c["outside"]),
            &provenance::prepare(&s(&c["owner"])),
        );
        d.check("form sink", c, &c["out"], &json!({"ok": hits_json(&hits)}));
    }
    d.finish("the sink rule");
}

fn card_json(c: &confirm::Card) -> Value {
    json!({"question": c.question, "detail": c.detail, "exact": c.exact, "yes": c.yes, "no": c.no,
        "aside": c.aside, "clause": c.clause, "tail": c.tail})
}

fn words_res(r: Result<String, confirm::Missing>) -> Value {
    match r {
        Ok(t) => json!({"ok": t}),
        Err(_) => json!({"raises": "KeyError"}),
    }
}

// decision: GATE-225, GATE-513, GATE-514, GATE-557, GATE-559, GATE-566, GATE-567, GATE-578
#[test]
fn the_questions_words_match_python() {
    let fx = fixture("wording.json");
    let mut d = Diffs::new();
    for c in fx.as_array().unwrap() {
        let needs: Vec<confirm::Need> = c["needs"].as_array().unwrap().iter().map(need).collect();
        let typed = s(&c["typed"]);
        let apps = LogApps::new(&c["apps"]);
        let card = match confirm::card(&needs, &typed, &apps) {
            Ok(card) => json!({"ok": card_json(&card)}),
            Err(_) => json!({"raises": "KeyError"}),
        };
        let got = json!({"summary": words_res(confirm::summary(&needs, &typed)),
            "line": words_res(confirm::line(&needs, &s(&c["call"]), &s(&c["why"]), &typed)), "card": card,
            "missed": apps.missed.borrow().clone()});
        let want =
            json!({"summary": c["summary"], "line": c["line"], "card": c["card"], "missed": []});
        d.check("wording", &c["needs"], &want, &got);
    }
    d.finish("wording");
}

fn facts(v: &Value) -> terminal::Facts {
    v.as_object().cloned().unwrap_or_default()
}

fn chain(v: &Value) -> Vec<terminal::Facts> {
    v.as_array()
        .map(|a| a.iter().map(facts).collect())
        .unwrap_or_default()
}

fn terminal_input(v: &Value) -> ferb_core::types::TerminalInput {
    let mut fixed = v.clone();
    if let Some(m) = fixed.as_object_mut() {
        m.remove("out");
        m.remove("wide");
    }
    serde_json::from_value(fixed).expect("a TerminalInput")
}

// decision: GATE-585, GATE-586, GATE-566
#[test]
fn terminal_input_matches_python() {
    let fx = fixture("terminal.json");
    let mut d = Diffs::new();
    for c in fx["surface"].as_array().unwrap() {
        let ch = chain(&c["chain"]);
        let owner = strs(&c["owner_apps"]);
        let b = s(&c["bundle_id"]);
        let got = json!({"out": terminal::surface(&b, &ch, &owner, c["shell_app"].as_bool().unwrap(),
                c["unread"].as_bool().unwrap()),
            "can_host": terminal::can_host(&b, &owner, c["shell_app"].as_bool().unwrap()),
            "structural": ch.iter().map(terminal::structural).collect::<Vec<_>>()});
        d.check(
            "surface",
            c,
            &json!({"out": c["out"], "can_host": c["can_host"], "structural": c["structural"]}),
            &got,
        );
    }
    for c in fx["run_control"].as_array().unwrap() {
        d.check(
            "run_control",
            c,
            &c["out"],
            &json!(terminal::run_control(
                &chain(&c["chain"]),
                c["editor"].as_bool().unwrap()
            )),
        );
    }
    for c in fx["need"].as_array().unwrap() {
        let inp = terminal_input(c);
        d.check(
            "terminal need",
            c,
            &json!([c["out"], c["wide"]]),
            &json!([need_json(&terminal::need(&inp)), terminal::app_wide(&inp)]),
        );
    }
    for c in fx["misc"].as_array().unwrap() {
        let t = s(&c["text"]);
        let menu = if t.is_empty() {
            terminal::run_menu(&[])
        } else {
            terminal::run_menu(std::slice::from_ref(&t))
        };
        let got = json!({"text": t, "visible": terminal::visible(&t, true), "visible_raw": terminal::visible(&t, false),
            "newline": terminal::newline(&t), "run_menu": menu, "terminal_menu": terminal::terminal_menu(&t),
            "app_key": terminal::app_key(&t)});
        d.check("terminal misc", &c["text"], c, &got);
    }
    d.finish("terminal input");
}

fn tls_json(off: Option<tls::TlsOff>) -> Value {
    off.map_or(Value::Null, |o| json!({"how": o.how, "hosts": o.hosts}))
}

// decision: L39 (certificate checks turned off ask), GATE-207 (never a routine question)
#[test]
fn certificate_checks_match_python() {
    let fx = fixture("tls.json");
    let mut d = Diffs::new();
    for c in fx["commands"].as_array().unwrap() {
        let cmd = s(&c["command"]);
        let got = json!({"ok": tls_json(tls::certificate_checks_off(&cmd))});
        // Fixed against Python: `yarn config set enableStrictSsl` with no value after it raises IndexError in
        // tls._anywhere (tls.py:344-345: the flag test passes, then args[k + 1] is read past the end). The port
        // reads no value there, which turns nothing off: no question.
        if c["out"].get("raises") == Some(&json!("IndexError")) {
            assert_eq!(cmd, "yarn config set enableStrictSsl");
            d.check("tls (fixed)", &c["command"], &json!({"ok": null}), &got);
            continue;
        }
        d.check("tls", &c["command"], &c["out"], &got);
    }
    for c in fx["scripts"].as_array().unwrap() {
        d.check(
            "applescript tls",
            &c["script"],
            &c["out"],
            &json!({"ok": tls_json(tls::applescript_checks_off(&s(&c["script"])))}),
        );
    }
    d.finish("certificate checks");
}

fn pay_facts(v: &Value) -> payments::PayFacts {
    payments::PayFacts {
        chain: s(&v["chain"]),
        host: s(&v["host"]),
        frame_host: opt_s(&v["frame_host"]),
        amount: opt_s(&v["amount"]),
        merchant: opt_s(&v["merchant"]),
        submits_card: v["submits_card"].as_bool().unwrap_or(false),
        submits: v["submits"].as_bool().unwrap_or(false),
        shown: strs(&v["shown"]),
        declared: opt_s(&v["declared"]),
        page: opt_s(&v["page"]),
        control: opt_s(&v["control"]),
    }
}

fn approval_json(a: Option<&payments::Approval>) -> Value {
    match a {
        None => Value::Null,
        Some(a) => {
            json!({"cid": a.cid, "chain": a.chain, "host": a.host, "amount": a.amount, "merchant": a.merchant,
            "at": ferb_core::when::isoformat(&a.at), "bank_host": a.bank_host, "bank_amount": a.bank_amount,
            "bank_merchant": a.bank_merchant, "bank_done": a.bank_done, "cards": a.cards,
            "paid_one_click": a.paid_one_click, "done": a.done, "declared": a.declared, "seen": a.seen, "page": a.page,
            "control": a.control})
        }
    }
}

// decision: GATE-204, GATE-216, GATE-217, GATE-578, GATE-580
#[test]
fn payments_match_python() {
    let fx = fixture("payments.json");
    let mut d = Diffs::new();
    for c in fx["same"].as_array().unwrap() {
        let (a, b) = (c["a"].as_str(), c["b"].as_str());
        d.check(
            "same",
            c,
            &json!([c["amount"], c["merchant"]]),
            &json!([payments::same_amount(a, b), payments::same_merchant(a, b)]),
        );
    }
    for c in fx["digits"].as_array().unwrap() {
        d.check(
            "digits_of",
            c,
            &c["out"],
            &json!(payments::digits_of(c["amount"].as_str())),
        );
    }
    for c in fx["page_key"].as_array().unwrap() {
        let got = res(payments::page_key(c["url"].as_str()), |v| json!(v));
        d.check("page_key", c, &c["out"], &outcome_eq(&c["out"], &got));
    }
    for c in fx["stated"].as_array().unwrap() {
        let got = payments::stated(
            c["read"].as_str(),
            c["declared"].as_str(),
            &strs(&c["shown"]),
        );
        d.check("stated", c, &c["out"], &got.to_json());
    }
    let base = ferb_core::when::parse("2026-01-02T03:04:05+00:00").unwrap();
    for steps in fx["sessions"].as_array().unwrap() {
        let clock = std::sync::Arc::new(std::sync::Mutex::new(base));
        let c2 = clock.clone();
        let mut pay = payments::Payments::new(Box::new(move || *c2.lock().unwrap()));
        for step in steps.as_array().unwrap() {
            let tree = s(&step["tree"]);
            let f = pay_facts(&step["facts"]);
            let got: Value = match step["op"].as_str().unwrap() {
                "check" => {
                    let c = pay.check(&tree, &f, &values(&step["values"]));
                    json!({"approval": approval_json(c.approval.as_ref()), "need": c.need.as_ref().map(need_json)})
                }
                "approve" => approval_json(Some(&pay.approve(&tree, &f, &s(&step["cid"])))),
                "ran" => {
                    pay.ran(&tree, &f, step["ok"].as_bool().unwrap());
                    Value::Null
                }
                "observe" => {
                    pay.observe(
                        &tree,
                        Some(&f.chain),
                        Some(&f.host),
                        f.amount.as_deref(),
                        f.merchant.as_deref(),
                    );
                    Value::Null
                }
                "for_otp" => {
                    let a = pay.for_otp(&tree, &f.host);
                    let card = a.as_ref().map(|a| {
                        let (q, w, e) = payments::otp_card(&f.host, a);
                        json!([q, w, e])
                    });
                    d.check(
                        "otp",
                        step,
                        &json!([step["otp_line"], step["otp_card"]]),
                        &json!([payments::otp_line(a.as_ref()), card]),
                    );
                    approval_json(a.as_ref())
                }
                "set_aside" => {
                    pay.set_aside(&tree, &f.chain);
                    Value::Null
                }
                "first_change" => {
                    json!(pay.first_change(&tree, &f.chain, step["state"].as_array().unwrap()))
                }
                "tick" => {
                    let secs = step["seconds"].as_i64().unwrap();
                    let mut t = clock.lock().unwrap();
                    *t += chrono::TimeDelta::seconds(secs);
                    Value::Null
                }
                "active" => approval_json(pay.active(&tree, &f.chain).as_ref()),
                "pressed" => match pay.active(&tree, &f.chain) {
                    None => Value::Null,
                    Some(a) => {
                        let now = step["now"].as_object().map(|n| payments::Pressed {
                            url: n["url"].as_str().map(str::to_string),
                            amounts: n["amounts"].as_array().map(|x| x.iter().map(s).collect()),
                        });
                        let got = res(
                            payments::pressed_change(
                                &a,
                                &f,
                                now.as_ref(),
                                step["asked_here"].as_bool().unwrap(),
                            ),
                            |v| json!(v),
                        );
                        outcome_eq(&step["out"], &got)
                    }
                },
                "forget" => {
                    pay.forget(std::slice::from_ref(&tree));
                    Value::Null
                }
                other => panic!("unknown op {other}"),
            };
            let want = step.get("out").cloned().unwrap_or(Value::Null);
            d.check(&format!("payments {}", step["op"]), step, &want, &got);
        }
    }
    d.finish("payments");
}

fn shown_items(v: &Value) -> Vec<carried::Shown> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| carried::Shown {
            labels: strs(&x[0]).into_iter().collect(),
            text: s(&x[1]),
            stored: x[2].as_object().map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), strs(v)))
                    .collect::<BTreeMap<_, _>>()
            }),
            page: s(&x[3]),
        })
        .collect()
}

// decision: GATE-519, GATE-522, GATE-524, GATE-529, GATE-534
#[test]
fn stored_provenance_matches_python() {
    let fx = fixture("carried.json");
    let mut d = Diffs::new();
    for c in fx["derive"].as_array().unwrap() {
        let got = carried::derive(&s(&c["text"]), &shown_items(&c["shown"]), &strs(&c["made"]));
        d.check("derive", c, &c["out"], &json!({"ok": got}));
    }
    for c in fx["parts"].as_array().unwrap() {
        let payload = s(&c["payload"]);
        let segs: Vec<carried::Segment> = c["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(carried::Segment::from_json)
            .collect();
        let (main, by_site) = carried::sink_parts(&payload, &segs);
        let by: serde_json::Map<String, Value> = by_site
            .iter()
            .map(|(k, v)| (format!("{k:?}"), json!(v)))
            .collect();
        let want_by = &c["sink_parts"][1];
        // Python's dict keys are frozensets (not JSON); the fixture writes them through norm's str(): compare values
        let want_vals: Vec<Value> = want_by
            .as_object()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default();
        let got_vals: Vec<Value> = by.values().cloned().collect();
        d.check(
            "sink_parts",
            &c["payload"],
            &json!([c["sink_parts"][0], want_vals]),
            &json!([main, got_vals]),
        );
        d.check(
            "owner_said",
            &c["payload"],
            &c["owner_said"],
            &json!(carried::owner_said(&payload, &segs)),
        );
        d.check(
            "sink_text",
            &c["payload"],
            &c["sink_text"],
            &json!(carried::sink_text(&payload, &segs)),
        );
        let outputs = vec![
            carried::Output {
                origins: ["private".to_string()].into(),
                text: payload.clone(),
                segments: segs.clone(),
                page: String::new(),
            },
            carried::Output {
                origins: ["owner".to_string()].into(),
                text: payload.clone(),
                segments: Vec::new(),
                page: "https://x.com/p".into(),
            },
        ];
        let recs: Vec<Value> = carried::shown_records(&outputs)
            .iter()
            .map(|r| json!([r.labels, r.text, r.stored, r.page]))
            .collect();
        // a segment's record has no page (None in Python, "" here: derive reads both as none)
        let want: Vec<Value> = c["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                json!([
                    r[0],
                    r[1],
                    r[2],
                    if r[3].is_null() {
                        json!("")
                    } else {
                        r[3].clone()
                    }
                ])
            })
            .collect();
        d.check("shown_records", &c["payload"], &json!(want), &json!(recs));
    }
    d.finish("stored provenance");
}

// decision: P19 (the ledger's payload hash), W2 (a declaration only adds)
#[test]
fn rules_match_python() {
    let fx = fixture("rules.json");
    let mut d = Diffs::new();
    for c in fx["declare"].as_array().unwrap() {
        let spec = ferb_core::types::ActionSpec {
            target: target(&c["target"]).unwrap(),
            ..ferb_core::types::ActionSpec::new(&s(&c["kind"]))
        };
        let out = rules::declare(
            &spec,
            &s(&c["effect"]),
            &[" b@x.com ".into(), "a@x.com".into(), "".into()],
            Some(" acct "),
            Some(12.5),
            Some(" the item "),
            Some("  INRUPEES LONG "),
        );
        let got = json!({"out_kind": out.kind, "out_effect": out.effect_kind, "out_target": target_json(Some(&out.target)),
            "declared_kinds": rules::declared_kinds(&out)});
        d.check(
            "declare",
            c,
            &json!({"out_kind": c["out_kind"], "out_effect": c["out_effect"],
            "out_target": c["out_target"], "declared_kinds": c["declared_kinds"]}),
            &got,
        );
    }
    for c in fx["declared_of"].as_array().unwrap() {
        d.check(
            "declared_of",
            c,
            &c["out"],
            &json!(rules::declared_of(&c["value"])),
        );
    }
    for c in fx["hash"].as_array().unwrap() {
        let identity = if c["identity"] == "owner" {
            ferb_core::types::Identity::Owner
        } else {
            ferb_core::types::Identity::Ferb
        };
        let a = rules::ActionRequest {
            capability: s(&c["capability"]),
            kind: String::new(),
            identity,
            scopes: BTreeSet::new(),
            task_id: "t".into(),
            worker_id: "w".into(),
            request_id: "r".into(),
            task_scopes: BTreeSet::new(),
            target: target(&c["target"]).unwrap(),
            content: opt_s(&c["content"]),
            effect: String::new(),
            why: String::new(),
        };
        d.check(
            "payload_hash",
            c,
            &c["out"],
            &json!(rules::payload_hash(&a, c["args"].as_object())),
        );
    }
    d.finish("rules");
}

// decision: GATE-201, GATE-222, GATE-560
#[test]
fn foreground_and_app_keys_match_python() {
    let fx = fixture("foreground.json");
    let mut d = Diffs::new();
    for c in fx.as_array().unwrap() {
        let apps = LogApps::new(&c["apps"]);
        let got = if let Some(tool) = c.get("tool") {
            let args = c["args"].as_object().cloned().unwrap_or_default();
            json!({"ok": confirm::foreground_need(&s(tool), &args, &apps).as_ref().map(need_json)})
        } else {
            let app = s(&c["app"]);
            json!({"ok": [confirm::app_key(&app, &apps), confirm::app_label(&app, &apps)]})
        };
        let got = json!({"out": got, "missed": apps.missed.borrow().clone()});
        d.check(
            "foreground",
            c,
            &json!({"out": c["out"], "missed": []}),
            &got,
        );
    }
    d.finish("foreground");
}

// decision: ORCH-519, D31
#[test]
fn small_modules_match_python() {
    let fx = fixture("small.json");
    let mut d = Diffs::new();
    let blocked = [
        small::blocked::blocked(
            small::blocked::REMINDERS,
            small::blocked::NOT_IN_WORDS,
            "nothing was set",
            "write it yourself.",
        ),
        small::blocked::blocked("  a  b. ", "x.", "y..", ""),
    ];
    d.check("blocked", &Value::Null, &fx["blocked"], &json!(blocked));
    let esc = [
        small::escape_key("read:~/.ssh", "ls ~/.ssh", "/x"),
        small::escape_key("admin", "é \"q\"", ""),
    ];
    d.check("escape_key", &Value::Null, &fx["escape"], &json!(esc));
    let env = small::Environment {
        apps: vec![("a".into(), "b".into())],
        paired: vec![
            vec!["chrome".into(), "Default".into(), "Me".into()],
            vec![
                "brave".into(),
                "P 1".into(),
                "Work".into(),
                "me@x.com".into(),
            ],
            vec!["safari".into(), "x".into(), "Ünï".into(), "".into()],
        ],
    };
    let running: BTreeSet<String> = ["com.b", "com.a", "ü.x"]
        .iter()
        .map(|x| x.to_string())
        .collect();
    let (stable, live) = small::services_summary(&env, "27.1", &running);
    d.check(
        "services",
        &Value::Null,
        &fx["services"],
        &json!([stable, live]),
    );
    let tj = [
        json!({"apps": ["b", "a", 1], "sites": "x", "paths": null}),
        json!("{\"recipients\": [\"x\"]}"),
        Value::Null,
    ];
    let got: Vec<Value> = tj
        .iter()
        .map(|v| small::Targets::from_json(v).to_json())
        .collect();
    d.check("targets", &Value::Null, &fx["targets"], &json!(got));
    d.finish("small modules");
    let _ = (HashMap::<(), ()>::new(), Values::new());
}

// decision: GATE-207 (the window covers routine questions only), GATE-204 (one payment's window), S4 (c)
#[test]
fn constants_and_bulk_deletes_match_python() {
    let fx = fixture("confirm.json");
    let mut d = Diffs::new();
    for c in fx["bulk_delete"].as_array().unwrap() {
        let got = match confirm::bulk_delete(&s(&c["path"]), c["facts"].as_object().unwrap()) {
            Ok(n) => json!({"ok": need_json(&n)}),
            Err(_) => json!({"raises": "KeyError"}),
        };
        d.check(
            "bulk_delete",
            &c["facts"],
            &outcome_eq(&c["out"], &c["out"]),
            &outcome_eq(&c["out"], &got),
        );
    }
    let sorted = |v: &[&str]| {
        let mut x: Vec<&str> = v.to_vec();
        x.sort_unstable();
        json!(x)
    };
    let pairs = |v: &[(&str, &str)]| {
        Value::Object(v.iter().map(|(k, x)| (k.to_string(), json!(x))).collect())
    };
    use ferb_policy::{protection as p, rules::Kind, standing};
    let got = json!({
        "ALLOW_WINDOW_S": confirm::ALLOW_WINDOW_S, "ROUTINE": sorted(confirm::ROUTINE), "PAY_WINDOW_S": payments::PAY_WINDOW_S,
        "SHOWN": confirm::SHOWN, "QUOTED_CHARS": confirm::QUOTED_CHARS, "SEND_CLASS": sorted(confirm::SEND_CLASS),
        "ALWAYS": {"pay": confirm::always_rule(Kind::PAY), "account_security": confirm::always_rule(Kind::ACCOUNT_SECURITY)},
        "UNREAD_AMOUNT": confirm::UNREAD_AMOUNT, "UNREAD": payments::UNREAD, "ACCOUNT_SECURITY": confirm::ACCOUNT_SECURITY,
        "TLS_QUESTION": confirm::TLS_QUESTION, "BROWSER_SCHEMES": confirm::BROWSER_SCHEMES,
        "FOREGROUND_TOOLS": sorted(confirm::FOREGROUND_TOOLS), "TYPING_ACTIONS": sorted(confirm::TYPING_ACTIONS),
        "GENERIC_INPUT": sorted(rules::GENERIC_INPUT), "REVERSIBLE": sorted(rules::REVERSIBLE),
        "READING_KINDS": sorted(rules::READING_KINDS), "ALWAYS_DECLARED": sorted(rules::ALWAYS_DECLARED),
        "EFFECTS": rules::EFFECTS, "ANNOTATIONS": rules::ANNOTATIONS,
        "EFFECT_KINDS": {"send": rules::effect_kind("send"), "pay": rules::effect_kind("pay"),
            "destroy": rules::effect_kind("destroy"), "account_security": rules::effect_kind("account_security")},
        "TERMINAL_APPS": p::DEFAULT_TERMINAL_APPS, "NEVER_WRITE": p::DEFAULT_NEVER_WRITE,
        "EXTRA_NEVER_READ": p::EXTRA_NEVER_READ, "SERVICE_LABELS": p::SERVICE_LABELS, "BUNDLE_SERVICE": p::BUNDLE_SERVICE,
        "NEVER_LIFTED": sorted(p::NEVER_LIFTED), "CODE_RUNNERS": pairs(terminal::CODE_RUNNERS),
        "OPENS_FILES": sorted(terminal::OPENS_FILES), "NO_FACTS_HOSTS": pairs(terminal::NO_FACTS_HOSTS),
        "RUN_CLASSES": sorted(terminal::RUN_CLASSES), "EDITOR_APPS": sorted(terminal::EDITOR_APPS),
        "UNASKED": terminal::UNASKED, "KINDS": destinations::KINDS, "HOW": destinations::HOW,
        "APPROVED": destinations::APPROVED, "TERMINAL_APP": confirm::TERMINAL_APP,
        "SCHEDULE_GRANT": standing::SCHEDULE_GRANT, "NEW_DEFAULTS": p::NEW_DEFAULTS, "PROVENANCE": carried::PROVENANCE,
        "MIN_LEN": provenance::MIN_LEN, "MIN_PHONE_DIGITS": provenance::MIN_PHONE_DIGITS,
        "DESTINATION_PHONE_DIGITS": provenance::DESTINATION_PHONE_DIGITS});
    d.check("constants", &Value::Null, &fx["constants"], &got);
    d.finish("constants and bulk deletes");
}
