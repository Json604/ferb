//! supervisor.py's module-level functions against rust/fixtures/supervisor/units.json (the Python functions run on
//! invented inputs by make.py). Times are UTC there (TZ=UTC); the port takes the zone.

mod common;

use common::*;
use ferb_clock::zone::Zone;
use ferb_context::tokens::Tokenizer;
use ferb_orchestrator::Condition;
use ferb_supervisor::chat_line::{ChatLine, Role};
use ferb_supervisor::lines::*;
use ferb_supervisor::planner::{front_window_block, result_line, FrontWindow};
use ferb_supervisor::seams::JournalRow;
use serde_json::{json, Map, Value};

fn t(v: &Value) -> ferb_clock::py::Time {
    ferb_clock::py::parse(v.as_str().expect("a time")).expect("ISO time")
}

fn pairs(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| (s(&p[0]), s(&p[1])))
        .collect()
}

fn obj(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

fn block_json(b: &ferb_context::assemble::ContextBlock) -> Value {
    json!({"text": b.text, "reference": b.reference, "protected": b.protected, "cache": b.cache,
           "priority": b.priority, "origin": b.origin, "source_refs": b.source_refs, "payload": b.payload,
           "history": b.history, "fold": b.fold, "values": b.values})
}

/// What a unit case's Python call returned, as Rust computes it; None for a group the test doesn't know.
fn run(group: &str, a: &Value, zone: &Zone) -> Option<Value> {
    let a0 = &a[0];
    Some(match group {
        "short_report" => json!(short_report(&s(a0), a[1].as_bool().unwrap())),
        "heading_only" => json!(heading_only(&s(a0))),
        "first_content_line" => json!(first_content_line(&s(a0))),
        "said" => json!(said(&s(a0))),
        "sentence" => json!(sentence(&s(a0))),
        "clipped" => json!(clipped(&s(a0))),
        "bare" => json!(bare(&s(a0))),
        "since_line" => json!(since_line(&s(a0))),
        "without_history_labels" => json!(without_history_labels(&s(a0))),
        "without_whole" => json!(without_whole(&s(a0), &s(&a[1]))),
        "sleeping_line" => json!(sleeping_line(&obj(a0))),
        "quoted" => {
            let names = strs(a0);
            json!(quoted(
                &names.iter().map(String::as_str).collect::<Vec<_>>()
            ))
        }
        "not_set" => json!(not_set(&pairs(a0))),
        "not_saved" => json!(not_saved(&pairs(a0))),
        "with_reasons" => json!(with_reasons(&pairs(a0))),
        "sites_line" => json!(sites_line(&strs(a0))),
        "within" => {
            let (shown, more) = within(&strs(a0), a[1].as_u64().unwrap() as usize);
            json!([shown, more])
        }
        "stored_refs" => json!(stored_refs(&strs(a0))),
        "exposure_output" => Value::Object(exposure_output(&obj(a0))),
        "result_reference" => json!(result_reference(&s(a0), a[1].as_str())),
        "sent_from" => json!(sent_from(&strs(a0))),
        "work_in" => {
            let home = "/FX/user"; // make.py ran with HOME in its fixed folder, written /FX
            let folders: Vec<String> = strs(a0).iter().map(|f| f.replace("~HOME", home)).collect();
            json!(work_in(&folders, home))
        }
        "day" => json!(day(&t(a0), &t(&a[1]), zone)),
        "reminder_line" => {
            let timer = TimerFacts {
                fire_at: t(a0),
                kind: s(&a[1]),
                message: s(&a[2]),
                state: String::new(),
            };
            let now = a[4].as_str().map(|_| t(&a[4]));
            json!(reminder_line(
                &timer,
                a[3].as_bool().unwrap(),
                now.as_ref(),
                zone
            ))
        }
        "already_set" => {
            let timer = TimerFacts {
                fire_at: t(a0),
                kind: String::new(),
                message: s(&a[2]),
                state: s(&a[1]),
            };
            json!(already_set(&timer, zone))
        }
        "takes_words" => {
            let ap = QuestionFacts {
                capability: s(a0),
                options_only: a[1].as_bool().unwrap(),
                ..Default::default()
            };
            json!(takes_words(&ap, &FERB_CHECKS))
        }
        "plain_choice" => {
            let ap = QuestionFacts {
                summary: s(a0),
                reason: s(&a[1]),
                options: strs(&a[2]),
                labels: strs(&a[3]),
                ..Default::default()
            };
            json!(plain_choice(&ap))
        }
        "entry_payload" => json!(entry_payload(&s(a0), &obj(&a[1]))),
        "stored_values" => stored_values(&s(a0), &obj(&a[1])).unwrap_or(Value::Null),
        "result_line" => json!(result_line(&JournalRow::from_json(a0).unwrap(), zone)),
        "front_window_block" => {
            let front = (!a0["front"].is_null()).then(|| FrontWindow {
                facts: obj(&a0["front"]["facts"]),
                image_ref: s(&a0["front"]["image_ref"]),
                withheld: s(&a0["front"]["withheld"]),
                masking: s(&a0["front"]["masking"]),
                origin: s(&a0["front"]["origin"]),
            });
            let nth = a0["nth"].as_array().map(|n| {
                (
                    n[0].as_u64().unwrap() as usize,
                    n[1].as_u64().unwrap() as usize,
                )
            });
            let got = front_window_block(
                front.as_ref(),
                a0["image"].as_bool().unwrap(),
                &s(&a0["heading"]),
                a0["withheld"].as_str(),
                a0["missing"].as_str(),
                nth,
                &wrap,
            );
            json!({"block": block_json(&got.block), "origins": got.origins})
        }
        "summary_blocks" => {
            let prev = a0.as_object().map(|e| s(&e["data"]["text"]));
            let today = a[2].as_str().map(|_| t(&a[2]));
            let blocks = summary_blocks(
                prev.as_deref(),
                &strs(&a[1]),
                today.as_ref(),
                zone,
                Tokenizer::Utf8Bytes,
                &wrap,
            );
            Value::Array(blocks.iter().map(block_json).collect())
        }
        "within_tokens" => json!(within_tokens(
            &s(a0),
            a[1].as_u64().unwrap() as usize,
            Tokenizer::Utf8Bytes
        )),
        "chat_line" => chat_line(&strs(a0)),
        "since_block" => since(a0, zone),
        "questions_block" => questions(a0, zone),
        "latest_blocks" => latest(a0, zone),
        "previous_run_block" => {
            let entry = a0
                .as_object()
                .map(|p| JournalRow::from_json(&p["entry"]).unwrap());
            let ran = a0.as_object().map(|p| t(&p["ran_at"]));
            let status = s(&a0["status"]);
            let got = ferb_supervisor::planner::previous_run_block(
                entry
                    .as_ref()
                    .map(|e| (e, ran.as_ref().unwrap(), status.as_str())),
                zone,
            );
            json!({"block": block_json(&got.block), "origins": got.origins})
        }
        "watch_condition" => condition_json(&watch_condition(a0.as_bool())),
        "checked_conditions" => {
            let cs: Vec<Condition> = a0.as_array().unwrap().iter().map(condition).collect();
            Value::Array(
                checked_conditions(&cs, a[1].as_bool().unwrap(), a[2].as_bool())
                    .iter()
                    .map(condition_json)
                    .collect(),
            )
        }
        "judged_kind" => {
            let judged: Vec<(usize, Condition)> = a0
                .as_array()
                .unwrap()
                .iter()
                .map(|p| (p[0].as_u64().unwrap() as usize, condition(&p[1])))
                .collect();
            json!(judged_kind(&judged, a[1].as_u64().unwrap() as usize))
        }
        "stopped_plan_summary" => {
            let steps: Vec<ferb_orchestrator::Step> = a0
                .as_array()
                .unwrap()
                .iter()
                .map(|v| {
                    ferb_orchestrator::validate::step_validate(&ferb_context::Py::from_value(v))
                        .unwrap()
                })
                .collect();
            let states = obj(&a[1]);
            json!(stopped_plan_summary(&steps, &|id: &str| states
                .get(id)
                .map(|st| s(&st["status"]))))
        }
        _ => return None,
    })
}

use ferb_supervisor::words::FERB_CHECKS;

struct Since<'a>(&'a Value, Vec<JournalRow>);

impl ferb_supervisor::planner::SinceReads for Since<'_> {
    fn task_column(&self, task_id: &str, column: &str) -> Option<String> {
        self.0["columns"]
            .get(format!("{task_id}|{column}"))
            .and_then(Value::as_str)
            .map(str::to_string)
    }
    fn tree_of(&self, task_id: &str) -> Option<String> {
        self.0["trees"]
            .get(task_id)
            .and_then(Value::as_str)
            .map(str::to_string)
    }
    fn task_origins(&self, root: &str) -> std::collections::BTreeSet<String> {
        set(&self.0["origins"][root])
    }
    fn schedule_label(&self, schedule_id: &str) -> String {
        self.0["labels"]
            .get(schedule_id)
            .and_then(Value::as_str)
            .unwrap_or(schedule_id)
            .to_string()
    }
    fn question_asked(&self, approval_id: &str) -> Option<JournalRow> {
        self.1
            .iter()
            .find(|r| {
                r.kind == "question.asked"
                    && r.data.get("approval_id").map(py) == Some(approval_id.to_string())
            })
            .cloned()
    }
}

fn py(v: &Value) -> String {
    ferb_supervisor::lines::py_str(v)
}

/// supervisor.py:4342 `_since_block` on make.py's stand-in `self`: the owner's earlier messages outside this turn
/// (journal.of_kind("chat.message") filtered as the method does), then the SINCE_KINDS rows after the newest.
fn since(a: &Value, zone: &Zone) -> Value {
    let rows: Vec<JournalRow> = a["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| JournalRow::from_json(r).unwrap())
        .collect();
    let current = strs(&a["current"]);
    let outside = |r: &JournalRow, k: &str| {
        r.data
            .get(k)
            .and_then(Value::as_str)
            .is_none_or(|v| !current.iter().any(|c| c == v))
    };
    let previous: Vec<JournalRow> = rows
        .iter()
        .filter(|r| r.kind == "chat.message" && outside(r, "request_id") && outside(r, "read_with"))
        .cloned()
        .collect();
    let after: Vec<JournalRow> = match previous.last() {
        Some(last) => {
            let at = rows.iter().position(|r| r.id == last.id).unwrap();
            rows[at + 1..]
                .iter()
                .filter(|r| ferb_supervisor::words::SINCE_KINDS.contains(&r.kind.as_str()))
                .cloned()
                .collect()
        }
        None => Vec::new(),
    };
    let reads = Since(a, rows.clone());
    match ferb_supervisor::planner::since_block(&previous, &after, &reads, zone, &wrap) {
        None => Value::Null,
        Some(got) => json!({"block": block_json(&got.block), "origins": got.origins}),
    }
}

/// supervisor.py:3757 `_questions_block` with `_card_row` built here from the approval and its question.asked time.
fn questions(a: &Value, zone: &Zone) -> Value {
    use ferb_supervisor::planner::{card_row, questions_block, routable, OpenQuestion};
    let pending: Vec<OpenQuestion> = a["pending"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| {
            let (kind, capability) = (s(&q["kind"]), s(&q["capability"]));
            let asked_at = q["asked_at"].as_str().map(|_| t(&q["asked_at"]));
            OpenQuestion {
                id: s(&q["id"]),
                card_row: card_row(
                    &kind,
                    &capability,
                    &s(&q["summary"]),
                    &s(&q["reason"]),
                    asked_at.as_ref(),
                    routable(&kind, &capability, &FERB_CHECKS),
                    zone,
                ),
                kind,
                capability,
                reason: s(&q["reason"]),
                options: strs(&q["options"]),
                task_label: s(&q["task_label"]),
            }
        })
        .collect();
    let got = questions_block(
        &pending,
        a["conversations"].as_array().unwrap(),
        &FERB_CHECKS,
        &wrap,
    );
    json!({"block": block_json(&got.block), "origins": got.origins})
}

/// supervisor.py:3887 `_latest_blocks` over make.py's invented rows.
fn latest(a: &Value, zone: &Zone) -> Value {
    use ferb_supervisor::planner::{latest_blocks, LatestReads};
    let rows: Vec<JournalRow> = a["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| JournalRow::from_json(r).unwrap())
        .collect();
    let found = rows.iter().find(|r| r.id == s(&a["latest"]));
    let task = found
        .map(|f| {
            f.data
                .get("task_id")
                .map(py)
                .unwrap_or_else(|| "None".into())
        })
        .unwrap_or_default();
    let parts: Vec<JournalRow> = rows
        .iter()
        .filter(|r| {
            r.kind == "chat.part" && r.data.get("task_id").map(py).as_deref() == Some(task.as_str())
        })
        .cloned()
        .collect();
    let order = strs(&a["order"][task.as_str()]);
    let got = latest_blocks(
        &LatestReads {
            found,
            part_rows: &parts,
            part_order: &order,
        },
        Tokenizer::Utf8Bytes,
        zone,
    );
    Value::Array(
        got.iter()
            .map(|p| json!({"block": block_json(&p.block), "origins": p.origins}))
            .collect(),
    )
}

fn opt(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

fn condition(v: &Value) -> Condition {
    let c = &v["check"];
    Condition {
        text: s(&v["text"]),
        owner_text: s(&v["owner_text"]),
        check: ferb_orchestrator::Check {
            kind: s(&c["kind"]),
            path: opt(&c["path"]),
            text: opt(&c["text"]),
            url_contains: opt(&c["url_contains"]),
            site: opt(&c["site"]),
            expected: opt(&c["expected"]),
            tool: opt(&c["tool"]),
        },
        layer: s(&v["layer"]),
    }
}

/// `Condition.model_dump()`, in field order.
fn condition_json(c: &Condition) -> Value {
    json!({"text": c.text, "owner_text": c.owner_text, "check": {"kind": c.check.kind, "path": c.check.path,
           "text": c.check.text, "url_contains": c.check.url_contains, "site": c.check.site,
           "expected": c.check.expected, "tool": c.check.tool}, "layer": c.layer})
}

/// make.py `chat_line`: the line after each step, the roles as set so far.
fn chat_line(script: &[String]) -> Value {
    let mut line = ChatLine::new();
    let mut roles: Vec<(String, Value)> = Vec::new();
    let mut out = Vec::new();
    let set = |roles: &mut Vec<(String, Value)>,
               decisions: Vec<ferb_supervisor::chat_line::Decision>| {
        for d in decisions {
            let v = match d.role {
                Role::Lead => json!(["lead", d.request_id]),
                Role::Merged(lead) => json!(["merged", lead]),
            };
            if let Some(slot) = roles.iter_mut().find(|(k, _)| *k == d.request_id) {
                slot.1 = v;
            }
        }
    };
    for step in script {
        match step.as_str() {
            "reply" => {
                line.replied();
                out.push(json!(["reply"]));
            }
            "release" => {
                let lead = line.holder.as_ref().map(|h| h.lead.clone());
                if let Some(lead) = &lead {
                    let d = line.release(lead);
                    set(&mut roles, d);
                }
                out.push(json!(["release", lead]));
            }
            name => {
                roles.push((name.to_string(), Value::Null));
                let arrived = line.arrive(name);
                set(&mut roles, arrived.decisions);
                out.push(json!(["arrive", name]));
            }
        }
        let holder = line.holder.as_ref().map(|h| {
            json!({"lead": h.lead, "arrivals": h.arrivals, "restarts": h.restarts, "restart": h.restart,
                   "thinking": h.thinking})
        });
        let mut state = Map::new();
        for (k, v) in &roles {
            state.insert(k.clone(), v.clone());
        }
        out.push(json!({"roles": state, "holder": holder, "queued": line.queued}));
    }
    Value::Array(out)
}

#[test]
fn every_unit_case() {
    let fx = read("rust/fixtures/supervisor/units.json");
    assert_eq!(
        fx["token_method"],
        "utf8_byte_ceiling_tokenizer_unavailable"
    );
    let zone = Zone::utc();
    let mut checked = 0;
    let mut skipped = Vec::new();
    for (group, cases) in fx["units"].as_object().unwrap() {
        for case in cases.as_array().unwrap() {
            let args = &case["args"];
            let Some(got) = run(group, args, &zone) else {
                skipped.push(group.clone());
                break;
            };
            let want = case.get("ok").cloned().expect("no case raises");
            assert_eq!(got, want, "{group} {args}");
            checked += 1;
        }
    }
    assert!(skipped.is_empty(), "groups with no Rust case: {skipped:?}");
    assert_eq!(checked, 328);
}
