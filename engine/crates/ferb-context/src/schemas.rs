//! Every structured-answer schema FERB asks a model for, built as Python builds it (ferb/orchestrator/schemas.py and
//! the memory, intro and check modules), key for key in Python's order: `json.dumps` of these is what each provider
//! receives, so the order is a contract. Strict form throughout (every property required, no extra properties,
//! null where optional). The JSON-level readers of schemas.py (`ungrouped`, `grouped`, the `mode="before"`
//! validators) are here too; the pydantic models themselves are the orchestration port's.

use serde_json::{json, Map, Value};

/// ferb/clock/when.py:28 `DAYS`.
pub const DAYS: [&str; 7] = [
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];

/// ferb/clock/when.py:29 `DAY_WORDS`: today, tomorrow, day_after_tomorrow, end_of_month, the days, next_<day>.
pub fn day_words() -> Vec<String> {
    let mut out: Vec<String> = ["today", "tomorrow", "day_after_tomorrow", "end_of_month"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    out.extend(DAYS.iter().map(|d| d.to_string()));
    out.extend(DAYS.iter().map(|d| format!("next_{d}")));
    out
}

/// ferb/orchestrator/plans.py:32 `STEP_KINDS`.
pub const STEP_KINDS: [&str; 4] = ["do", "wait", "ask", "converse"];
/// ferb/memory/items.py:48 `KINDS`.
pub const MEMORY_KINDS: [&str; 5] = ["fact", "preference", "person", "project", "env"];
/// schemas.py:20 `ACTION_CHECKS` (ORCH-576).
pub const ACTION_CHECKS: [&str; 6] = [
    "sent",
    "paid",
    "submitted",
    "trashed",
    "deleted",
    "downloaded",
];
/// schemas.py:306 `NEXT`.
pub const NEXT: [&str; 5] = ["accept", "retry", "ask", "accept_unconfirmed", "fail"];
/// schemas.py:309 `FINDING_KINDS` (ORCH-545).
pub const FINDING_KINDS: [&str; 2] = ["contradicted", "unsupported"];

/// schemas.py:21 `CHECK_KINDS`.
pub fn check_kinds() -> Vec<&'static str> {
    let mut out = vec![
        "none",
        "file_exists",
        "file_contains",
        "tab_url",
        "account",
        "after_change",
        "after_value",
    ];
    out.extend(ACTION_CHECKS);
    out
}

fn string() -> Value {
    json!({"type": "string"})
}

fn s_enum<S: AsRef<str>>(values: &[S]) -> Value {
    json!({"type": "string", "enum": values.iter().map(|v| v.as_ref()).collect::<Vec<_>>()})
}

/// schemas.py:25 `_strings`.
pub fn strings(enumerated: Option<&[&str]>) -> Value {
    let mut item = Map::new();
    item.insert("type".into(), json!("string"));
    if let Some(values) = enumerated.filter(|v| !v.is_empty()) {
        item.insert("enum".into(), json!(values));
    }
    json!({"type": "array", "items": Value::Object(item)})
}

/// schemas.py:32 `_nullable`: `{"type": [t, "null"]}`.
pub fn nullable(t: &str) -> Value {
    json!({"type": [t, "null"]})
}

/// ferb/clock/when.py:44 `_nullable`: `{"anyOf": [{"type": t, **extra}, {"type": "null"}]}`.
fn any_nullable(t: &str, extra: Option<(&str, Value)>) -> Value {
    let mut branch = Map::new();
    branch.insert("type".into(), json!(t));
    if let Some((k, v)) = extra {
        branch.insert(k.into(), v);
    }
    json!({"anyOf": [Value::Object(branch), {"type": "null"}]})
}

fn or_null(schema: Value) -> Value {
    json!({"anyOf": [{"type": "null"}, schema]})
}

fn array_of(items: Value) -> Value {
    json!({"type": "array", "items": items})
}

/// schemas.py:36 `_obj`: every property required, none other allowed, in this order.
pub fn obj(props: Vec<(&str, Value)>) -> Value {
    let required: Vec<&str> = props.iter().map(|(k, _)| *k).collect();
    let mut properties = Map::new();
    for (k, v) in props {
        properties.insert(k.to_string(), v);
    }
    json!({"type": "object", "additionalProperties": false, "required": required,
           "properties": Value::Object(properties)})
}

/// The other modules' schemas are written out as dicts with their own key order (not `_obj`'s).
fn raw_obj(required: &[&str], props: Vec<(&str, Value)>) -> Value {
    let mut properties = Map::new();
    for (k, v) in props {
        properties.insert(k.to_string(), v);
    }
    json!({"type": "object", "additionalProperties": false, "required": required,
           "properties": Value::Object(properties)})
}

/// ferb/clock/when.py:48 `WHEN_SCHEMA`.
pub fn when_schema() -> Value {
    raw_obj(
        &["in_minutes", "in_days", "day", "date", "time"],
        vec![
            ("in_minutes", any_nullable("number", None)),
            ("in_days", any_nullable("integer", None)),
            (
                "day",
                any_nullable("string", Some(("enum", json!(day_words())))),
            ),
            ("date", any_nullable("string", None)),
            ("time", any_nullable("string", None)),
        ],
    )
}

/// ferb/orchestrator/probes.py:12 `PROBE_SCHEMA`.
pub fn probe_schema() -> Value {
    raw_obj(
        &["tool", "args_json", "field", "op", "value_json"],
        vec![
            ("tool", string()),
            ("args_json", string()),
            ("field", string()),
            ("op", s_enum(&["equals", "not_equals", "changed"])),
            ("value_json", string()),
        ],
    )
}

/// schemas.py:43 `NAME_SCHEMA` (T-1055).
pub fn name_schema() -> Value {
    string()
}

/// schemas.py:44 `TARGETS_SCHEMA`.
pub fn targets_schema() -> Value {
    obj(vec![
        ("apps", strings(None)),
        ("sites", strings(None)),
        ("paths", strings(None)),
        ("recipients", strings(None)),
    ])
}

/// schemas.py:45 `CONDITION_SCHEMA`.
pub fn condition_schema() -> Value {
    let check = obj(vec![
        ("kind", s_enum(&check_kinds())),
        ("path", nullable("string")),
        ("text", nullable("string")),
        ("url_contains", nullable("string")),
        ("site", nullable("string")),
        ("expected", nullable("string")),
        ("tool", nullable("string")),
    ]);
    obj(vec![
        ("text", string()),
        ("owner_text", string()),
        (
            "layer",
            s_enum(&["outcome", "intake", "artifact", "external"]),
        ),
        ("check", or_null(check)),
    ])
}

/// schemas.py:57 `CONVERSATION_SCHEMA`.
pub fn conversation_schema() -> Value {
    obj(vec![
        ("identity", s_enum(&["owner", "ferb"])),
        ("account", string()),
        ("site", string()),
        ("app", string()),
        ("thread", string()),
        ("recipients", strings(None)),
        ("topic", string()),
        ("handover_when", string()),
        ("window_id", nullable("string")),
    ])
}

/// schemas.py:63 `EVENT_MATCH_SCHEMA`.
pub fn event_match_schema() -> Value {
    array_of(obj(vec![("field", string()), ("value", string())]))
}

/// schemas.py:65 `STEP_SCHEMA`.
pub fn step_schema() -> Value {
    obj(vec![
        ("id", string()),
        ("kind", s_enum(&STEP_KINDS)),
        ("label", string()),
        ("goal", string()),
        ("depends_on", strings(None)),
        ("inputs", strings(None)),
        ("worker_type", string()),
        ("conditions", strings(None)),
        ("until", nullable("string")),
        ("event", nullable("string")),
        ("event_match", event_match_schema()),
        ("question", string()),
        ("options", strings(None)),
        ("conversation", or_null(conversation_schema())),
        (
            "repeat",
            or_null(obj(vec![
                ("end_when", string()),
                ("event", nullable("string")),
                ("event_match", event_match_schema()),
            ])),
        ),
        ("earlier_results", strings(None)),
    ])
}

/// schemas.py:79 `SEND_SCOPE_SCHEMA`, with its description (step 10b) added last.
pub fn send_scope_schema() -> Value {
    let mut schema = obj(vec![
        ("capability", string()),
        ("identity", s_enum(&["owner", "ferb"])),
        ("account", nullable("string")),
        ("site", nullable("string")),
        ("recipients", strings(None)),
        ("content", nullable("string")),
    ]);
    schema.as_object_mut().expect("an object").insert(
        "description".into(),
        json!(
            "One exact send scope. Posting from the owner's browser (a campaign): capability browser.act, site the \
             page's host (x.com), recipients [] for public posts, content null unless the owner gave the exact text, \
             account the account the posts go out from."
        ),
    );
    schema
}

/// schemas.py:88 `PLAN_SCHEMA`.
pub fn plan_schema() -> Value {
    obj(vec![
        ("steps", array_of(step_schema())),
        ("input_paths", strings(None)),
        ("big_job", json!({"type": "boolean"})),
        ("time_estimate", string()),
        ("cost_estimate", string()),
        ("standing_sends", array_of(send_scope_schema())),
    ])
}

/// schemas.py:93 `PART_SCHEMA` (ORCH-319).
pub fn part_schema() -> Value {
    obj(vec![
        ("id", string()),
        ("quote", string()),
        ("label", string()),
        ("direct", nullable("string")),
        ("answer_step", nullable("string")),
        ("conditions", array_of(json!({"type": "integer"}))),
    ])
}

/// schemas.py:96 `TASK_SCHEMA`.
pub fn task_schema() -> Value {
    let mut conditions = array_of(condition_schema());
    conditions
        .as_object_mut()
        .expect("an object")
        .insert("minItems".into(), json!(1));
    obj(vec![
        ("goal", string()),
        ("name", name_schema()),
        ("identity", s_enum(&["owner", "ferb"])),
        ("worker_type", string()),
        ("targets", targets_schema()),
        ("success_conditions", conditions),
        ("plan", or_null(plan_schema())),
        ("folders", strings(None)),
        ("keep_tabs_open", json!({"type": "boolean"})),
        ("parts", array_of(part_schema())),
        ("earlier_results", strings(None)),
    ])
}

/// schemas.py:117 `UNTIL_SCHEMA` (L36 §6).
pub fn until_schema() -> Value {
    let mut day_enum = Map::new();
    day_enum.insert("type".into(), json!("string"));
    day_enum.insert("enum".into(), json!(day_words()));
    obj(vec![
        ("quote", string()),
        (
            "day",
            json!({"anyOf": [Value::Object(day_enum), {"type": "null"}]}),
        ),
        ("date", nullable("string")),
        ("in_days", nullable("integer")),
    ])
}

/// schemas.py:120 `REMEMBER_SCHEMA`.
pub fn remember_schema() -> Value {
    obj(vec![
        ("kind", s_enum(&MEMORY_KINDS)),
        ("subject", string()),
        ("key", string()),
        ("content", string()),
        ("quote", string()),
        ("login_origin", nullable("string")),
        ("until", or_null(until_schema())),
        ("replaces", strings(None)),
        ("review_in_days", json!({"type": "integer"})),
    ])
}

/// schemas.py:136 `SCHEDULE_UNTIL_SCHEMA` (ORCH-320).
pub fn schedule_until_schema() -> Value {
    let mut days = vec![String::new()];
    days.extend(day_words());
    obj(vec![
        ("quote", string()),
        ("day", s_enum(&days)),
        ("date", string()),
        ("in_days", json!({"type": "integer"})),
        ("time", string()),
        ("in_minutes", json!({"type": "number"})),
    ])
}

/// schemas.py:141 `WHEN_ACTIVE_SCHEMA` (ORCH-317).
pub fn when_active_schema() -> Value {
    json!({"type": "boolean", "description":
        "true when the owner's words want it when they're at their Mac (\"whenever I'm on my Mac\", \"when I'm back \
         at my Mac\"): it comes due as `when` and `repeat` say and FERB holds it until they're active at the Mac. \
         `when` may then leave the time out (all null: from today; a day alone: from that day). false otherwise."})
}

/// schemas.py:145 `TIMER_SCHEMA`.
pub fn timer_schema() -> Value {
    obj(vec![
        ("when", when_schema()),
        ("message", string()),
        ("quote", string()),
        ("when_active", when_active_schema()),
        ("name", name_schema()),
        ("kind", s_enum(&["reminder", "timer"])),
    ])
}

/// schemas.py:150 `RANDOM_TIMES_SCHEMA` (T-1066).
pub fn random_times_schema() -> Value {
    array_of(obj(vec![
        ("count", json!({"type": "integer"})),
        ("start", string()),
        ("end", string()),
    ]))
}

/// schemas.py:152 `SCHEDULE_SCHEMA`.
pub fn schedule_schema() -> Value {
    obj(vec![
        ("request", string()),
        ("name", name_schema()),
        ("when", when_schema()),
        (
            "repeat",
            s_enum(&["none", "daily", "weekdays", "weekly", "every"]),
        ),
        ("weekday", nullable("string")),
        ("every_minutes", nullable("integer")),
        ("times", strings(None)),
        ("random", random_times_schema()),
        ("quote", string()),
        ("targets", targets_schema()),
        ("standing_sends", array_of(send_scope_schema())),
        ("watch", s_enum(&["none", "until_met", "standing"])),
        ("until", or_null(schedule_until_schema())),
        ("when_active", when_active_schema()),
        ("folders", strings(None)),
        ("identity", s_enum(&["owner", "ferb"])),
        ("worker_type", string()),
        ("profile", string()),
        ("account", string()),
    ])
}

/// schemas.py:173 `SCHEDULE_TIMING_SCHEMA` (T-1066).
pub fn schedule_timing_schema() -> Value {
    obj(vec![
        (
            "repeat",
            s_enum(&["", "none", "daily", "weekdays", "weekly", "every"]),
        ),
        ("times", strings(None)),
        ("every_minutes", json!({"type": "integer"})),
        ("weekday", string()),
        ("random", random_times_schema()),
    ])
}

/// schemas.py:177 `SCHEDULE_CHANGE_SCHEMA`.
pub fn schedule_change_schema() -> Value {
    obj(vec![
        ("id", string()),
        (
            "action",
            s_enum(&["cancel", "pause", "resume", "set_end", "edit"]),
        ),
        ("quote", string()),
        ("request", string()),
        ("timing", schedule_timing_schema()),
        ("until", or_null(schedule_until_schema())),
        ("name", name_schema()),
    ])
}

/// schemas.py:186 `REMINDER_CHANGE_SCHEMA`.
pub fn reminder_change_schema() -> Value {
    obj(vec![("id", string()), ("quote", string())])
}

/// schemas.py:190 `SIDE_ENTRIES_SCHEMA` (ORCH-630).
pub fn side_entries_schema() -> Value {
    obj(vec![
        (
            "answer_question",
            or_null(obj(vec![("approval_id", string()), ("answer", string())])),
        ),
        ("remember", array_of(remember_schema())),
        (
            "forget",
            array_of(obj(vec![("item_id", string()), ("quote", string())])),
        ),
        ("timers", array_of(timer_schema())),
        ("schedules", array_of(schedule_schema())),
        ("schedule_changes", array_of(schedule_change_schema())),
        ("show_schedules", json!({"type": "boolean"})),
        ("reminder_cancels", array_of(reminder_change_schema())),
        (
            "conversation_changes",
            array_of(obj(vec![
                ("id", string()),
                ("action", s_enum(&["pause", "resume", "takeover", "close"])),
                ("quote", string()),
            ])),
        ),
    ])
}

/// schemas.py:204 `SIDE_ENTRIES`: the side entries' names, in order.
pub const SIDE_ENTRIES: [&str; 9] = [
    "answer_question",
    "remember",
    "forget",
    "timers",
    "schedules",
    "schedule_changes",
    "show_schedules",
    "reminder_cancels",
    "conversation_changes",
];

/// schemas.py:205 `SUPERVISOR_SCHEMA`: the planner's answer.
pub fn supervisor_schema() -> Value {
    let control = obj(vec![
        ("task_id", string()),
        ("action", s_enum(&["stop", "retry", "continue", "pause"])),
        ("new_goal", json!({"type": "boolean"})),
        (
            "standing_sends",
            json!({"anyOf": [{"type": "string", "enum": ["unchanged"]},
                             {"type": "array", "items": send_scope_schema()}]}),
        ),
        ("supersedes", strings(None)),
    ]);
    obj(vec![
        ("kind", s_enum(&["reply", "clarify", "task"])),
        ("answers_question", json!({"type": "boolean"})),
        ("task", or_null(task_schema())),
        ("control", or_null(control)),
        ("side_entries", or_null(side_entries_schema())),
        ("text", string()),
    ])
}

/// The flags of schemas.py:281 `worker_schema`, each adding its role's fields in this order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkerFlags {
    pub inputs: bool,
    pub proposals: bool,
    pub conversation: bool,
    pub watch: bool,
    pub loop_: bool,
    pub parts: bool,
}

impl WorkerFlags {
    /// The flag names, in `worker_schema`'s order.
    pub const NAMES: [&'static str; 6] = [
        "inputs",
        "proposals",
        "conversation",
        "watch",
        "loop",
        "parts",
    ];

    /// Bit `i` of `bits` is flag `NAMES[i]`.
    pub fn from_bits(bits: u8) -> WorkerFlags {
        WorkerFlags {
            inputs: bits & 1 != 0,
            proposals: bits & 2 != 0,
            conversation: bits & 4 != 0,
            watch: bits & 8 != 0,
            loop_: bits & 16 != 0,
            parts: bits & 32 != 0,
        }
    }

    fn on(&self) -> [bool; 6] {
        [
            self.inputs,
            self.proposals,
            self.conversation,
            self.watch,
            self.loop_,
            self.parts,
        ]
    }

    /// The name the fixture gives this set: `worker_schema(inputs,loop)`.
    pub fn label(&self) -> String {
        let on: Vec<&str> = Self::NAMES
            .iter()
            .zip(self.on())
            .filter(|(_, on)| *on)
            .map(|(n, _)| *n)
            .collect();
        format!("worker_schema({})", on.join(","))
    }
}

/// schemas.py:281 `worker_schema`: the shared report (`_REPORT`), plus each wanted role's fields, in flag order.
pub fn worker_schema(flags: WorkerFlags) -> Value {
    let mut props: Vec<(&str, Value)> = vec![
        ("status", s_enum(&["done", "not_done"])),
        ("reason", string()),
        ("question", nullable("string")),
        ("answer", string()),
        ("artifacts", strings(None)),
        ("how", string()),
    ];
    let [inputs, proposals, conversation, watch, loop_, parts] = flags.on();
    if inputs {
        props.push((
            "input_notes",
            array_of(obj(vec![
                ("input", string()),
                ("note", string()),
                ("evidence", strings(None)),
            ])),
        ));
    }
    if proposals {
        props.push(("conversation_proposals", array_of(conversation_schema())));
    }
    if conversation {
        props.push((
            "conversation_action",
            s_enum(&["wait", "handover", "finished", "blocked"]),
        ));
    }
    if watch {
        props.push(("watch_met", json!({"type": ["boolean", "null"]})));
    }
    if loop_ {
        props.push(("loop_action", s_enum(&["continue", "finished", "blocked"])));
        props.push(("probe", or_null(probe_schema())));
    }
    if parts {
        props.push((
            "part_updates",
            array_of(obj(vec![
                ("part", string()),
                ("text", string()),
                ("why", string()),
            ])),
        ));
    }
    obj(props)
}

/// schemas.py:310 `FINDINGS_SCHEMA` (ORCH-545).
pub fn findings_schema() -> Value {
    array_of(obj(vec![
        ("quote", string()),
        ("kind", s_enum(&FINDING_KINDS)),
        ("evidence", string()),
    ]))
}

/// schemas.py:313 `CHECKER_SCHEMA` (ORCH-302, ORCH-333).
pub fn checker_schema() -> Value {
    obj(vec![
        (
            "results",
            array_of(obj(vec![
                ("index", json!({"type": "integer"})),
                ("met", json!({"type": ["boolean", "null"]})),
                ("applies", json!({"type": "boolean"})),
                ("evidence", string()),
                ("how", json!({"type": ["string", "null"]})),
            ])),
        ),
        ("next", s_enum(&NEXT)),
        ("findings", findings_schema()),
        ("missing", string()),
        ("instructions", string()),
        ("worker_type", nullable("string")),
        ("question", string()),
        ("reason", string()),
    ])
}

/// schemas.py:329 `FIX_SCHEMA`.
pub fn fix_schema() -> Value {
    obj(vec![("answer", string())])
}

/// schemas.py:330 `RECHECK_SCHEMA`.
pub fn recheck_schema() -> Value {
    obj(vec![("findings", findings_schema())])
}

/// schemas.py:333 `SUMMARY_SCHEMA` (L28).
pub fn summary_schema() -> Value {
    obj(vec![("summary", string())])
}

/// schemas.py:340 `QUESTION_SCHEMA`.
pub fn question_schema() -> Value {
    obj(vec![
        ("ask_owner", json!({"type": "boolean"})),
        ("answer", string()),
        ("question", string()),
    ])
}

/// The attention call's schema, built inside `Supervisor.attention` (supervisor.py:5506-5509).
pub fn attention_schema() -> Value {
    let quote = obj(vec![("text", string()), ("origin", string())]);
    obj(vec![
        ("tell", json!({"type": "boolean"})),
        ("text", string()),
        ("source_refs", strings(None)),
        ("quotes", array_of(quote)),
    ])
}

/// ferb/memory/profile.py:63 `PROFILE_SCHEMA`.
pub fn profile_schema() -> Value {
    raw_obj(
        &["sections", "ended"],
        vec![
            (
                "sections",
                array_of(raw_obj(
                    &["title", "items"],
                    vec![("title", string()), ("items", strings(None))],
                )),
            ),
            (
                "ended",
                array_of(raw_obj(
                    &["id", "why"],
                    vec![("id", string()), ("why", string())],
                )),
            ),
        ],
    )
}

/// ferb/memory/distill.py:42 `QUOTE_SCHEMA` (L36 §2).
pub fn distill_quote_schema() -> Value {
    or_null(raw_obj(
        &["handle", "text"],
        vec![("handle", string()), ("text", string())],
    ))
}

/// ferb/memory/distill.py:62 `DISTILL_SCHEMA`.
pub fn distill_schema() -> Value {
    let item = raw_obj(
        &[
            "kind",
            "subject",
            "key",
            "content",
            "origin",
            "confidence",
            "handles",
            "quote",
            "retire",
        ],
        vec![
            ("kind", s_enum(&MEMORY_KINDS)),
            ("subject", string()),
            ("key", string()),
            ("content", string()),
            ("origin", s_enum(&["observed", "inferred", "third_party"])),
            ("confidence", json!({"type": "number"})),
            ("handles", strings(None)),
            ("quote", distill_quote_schema()),
            ("retire", json!({"type": "boolean"})),
        ],
    );
    raw_obj(
        &["narrative", "items"],
        vec![("narrative", string()), ("items", array_of(item))],
    )
}

/// ferb/memory/import_extract.py:48 `RECALL_SCHEMA`.
pub fn recall_schema() -> Value {
    let item = raw_obj(
        &["kind", "subject", "key", "content", "message", "quote"],
        vec![
            ("kind", s_enum(&MEMORY_KINDS)),
            ("subject", string()),
            ("key", string()),
            ("content", string()),
            ("message", string()),
            ("quote", string()),
        ],
    );
    raw_obj(&["candidates"], vec![("candidates", array_of(item))])
}

/// ferb/memory/import_extract.py:56 `VERIFY_SCHEMA`.
pub fn verify_schema() -> Value {
    let item = raw_obj(
        &[
            "id",
            "supported",
            "kind",
            "subject",
            "key",
            "content",
            "message",
            "quote",
            "same_as",
            "ended",
            "until",
            "review_in_days",
        ],
        vec![
            ("id", string()),
            ("supported", json!({"type": "boolean"})),
            ("kind", s_enum(&MEMORY_KINDS)),
            ("subject", string()),
            ("key", string()),
            ("content", string()),
            ("message", string()),
            ("quote", string()),
            ("same_as", string()),
            ("ended", json!({"type": "boolean"})),
            ("until", string()),
            ("review_in_days", json!({"type": "integer"})),
        ],
    );
    raw_obj(&["verdicts"], vec![("verdicts", array_of(item))])
}

/// ferb/memory/mac_profile.py:21 `MAC_SCHEMA`.
pub fn mac_schema() -> Value {
    let fact = raw_obj(
        &["kind", "key", "content"],
        vec![
            ("kind", s_enum(&MEMORY_KINDS)),
            ("key", string()),
            ("content", string()),
        ],
    );
    raw_obj(
        &["summary", "facts"],
        vec![("summary", string()), ("facts", array_of(fact))],
    )
}

/// ferb/memory/playbooks.py:32 `SCHEMA`.
pub fn playbook_schema() -> Value {
    raw_obj(
        &["same_kind", "members", "title", "text"],
        vec![
            ("same_kind", json!({"type": "boolean"})),
            ("members", strings(None)),
            ("title", string()),
            ("text", string()),
        ],
    )
}

/// ferb/daemon/areas/intro.py:34 `SUGGEST_SCHEMA` (DMN-500).
pub fn suggest_schema() -> Value {
    let item = raw_obj(
        &["request", "app"],
        vec![("request", string()), ("app", string())],
    );
    raw_obj(&["requests"], vec![("requests", array_of(item))])
}

/// ferb/runtimes/check.py:30 `IMAGE_SCHEMA`: its own key order (properties first).
pub fn image_schema() -> Value {
    json!({"type": "object", "properties": {"colour": {"type": "string"}}, "required": ["colour"],
           "additionalProperties": false})
}

/// Every named answer schema, by the name the fixture (answer_schemas.json) gives it.
pub fn named() -> Vec<(&'static str, Value)> {
    vec![
        ("SUPERVISOR_SCHEMA", supervisor_schema()),
        ("WORKER_SCHEMA", worker_schema(WorkerFlags::default())),
        (
            "WORKER_STEP_SCHEMA",
            worker_schema(WorkerFlags {
                inputs: true,
                proposals: true,
                parts: true,
                ..WorkerFlags::default()
            }),
        ),
        (
            "WORKER_CONVERSATION_SCHEMA",
            worker_schema(WorkerFlags {
                conversation: true,
                ..WorkerFlags::default()
            }),
        ),
        (
            "WORKER_WATCH_SCHEMA",
            worker_schema(WorkerFlags {
                watch: true,
                ..WorkerFlags::default()
            }),
        ),
        (
            "WORKER_LOOP_SCHEMA",
            worker_schema(WorkerFlags {
                loop_: true,
                ..WorkerFlags::default()
            }),
        ),
        (
            "WORKER_ALL_SCHEMA",
            worker_schema(WorkerFlags::from_bits(63)),
        ),
        ("CHECKER_SCHEMA", checker_schema()),
        ("FIX_SCHEMA", fix_schema()),
        ("RECHECK_SCHEMA", recheck_schema()),
        ("SUMMARY_SCHEMA", summary_schema()),
        ("QUESTION_SCHEMA", question_schema()),
        ("ATTENTION_SCHEMA", attention_schema()),
        ("PROBE_SCHEMA", probe_schema()),
        ("PROFILE_SCHEMA", profile_schema()),
        ("DISTILL_SCHEMA", distill_schema()),
        ("RECALL_SCHEMA", recall_schema()),
        ("VERIFY_SCHEMA", verify_schema()),
        ("MAC_SCHEMA", mac_schema()),
        ("PLAYBOOK_SCHEMA", playbook_schema()),
        ("SUGGEST_SCHEMA", suggest_schema()),
        ("IMAGE_SCHEMA", image_schema()),
    ]
}

/// schemas.py's other module-level `*_SCHEMA` constants (the parts the answers are built from), by name.
pub fn parts() -> Vec<(&'static str, Value)> {
    vec![
        ("NAME_SCHEMA", name_schema()),
        ("TARGETS_SCHEMA", targets_schema()),
        ("CONDITION_SCHEMA", condition_schema()),
        ("CONVERSATION_SCHEMA", conversation_schema()),
        ("EVENT_MATCH_SCHEMA", event_match_schema()),
        ("STEP_SCHEMA", step_schema()),
        ("SEND_SCOPE_SCHEMA", send_scope_schema()),
        ("PLAN_SCHEMA", plan_schema()),
        ("PART_SCHEMA", part_schema()),
        ("TASK_SCHEMA", task_schema()),
        ("UNTIL_SCHEMA", until_schema()),
        ("REMEMBER_SCHEMA", remember_schema()),
        ("SCHEDULE_UNTIL_SCHEMA", schedule_until_schema()),
        ("WHEN_ACTIVE_SCHEMA", when_active_schema()),
        ("TIMER_SCHEMA", timer_schema()),
        ("RANDOM_TIMES_SCHEMA", random_times_schema()),
        ("SCHEDULE_SCHEMA", schedule_schema()),
        ("SCHEDULE_TIMING_SCHEMA", schedule_timing_schema()),
        ("SCHEDULE_CHANGE_SCHEMA", schedule_change_schema()),
        ("REMINDER_CHANGE_SCHEMA", reminder_change_schema()),
        ("SIDE_ENTRIES_SCHEMA", side_entries_schema()),
        ("FINDINGS_SCHEMA", findings_schema()),
        ("WHEN_SCHEMA", when_schema()),
    ]
}

// ---------------------------------------------------------------------------------------------------------------
// reading answers (the JSON-level parts of schemas.py's readers)
// ---------------------------------------------------------------------------------------------------------------

/// schemas.py:377 `NO_CHECK`.
pub fn no_check() -> Value {
    json!({"kind": "none", "path": null, "text": null, "url_contains": null, "site": null,
           "expected": null, "tool": null})
}

/// schemas.py:227 `ungrouped`: a planner answer with its side entries as top-level fields (a top-level field the
/// answer already has wins); an answer without the group is returned as it is.
pub fn ungrouped(answer: &Value) -> Value {
    let Value::Object(map) = answer else {
        return answer.clone();
    };
    if !map.contains_key("side_entries") {
        return answer.clone();
    }
    let mut out: Map<String, Value> = map
        .iter()
        .filter(|(k, _)| *k != "side_entries")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if let Some(Value::Object(group)) = map.get("side_entries") {
        for (k, v) in group {
            if !out.contains_key(k) {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    Value::Object(out)
}

/// schemas.py:240 `grouped`: `ungrouped`'s inverse, the form SUPERVISOR_SCHEMA asks for (side entries in one
/// object, null when all are empty; a condition's check null when it is kind "none").
pub fn grouped(answer: &Map<String, Value>) -> Value {
    let schema = supervisor_schema();
    let props = schema["properties"].as_object().expect("an object");
    let mut out: Map<String, Value> = answer
        .iter()
        .filter(|(k, _)| !SIDE_ENTRIES.contains(&k.as_str()) && props.contains_key(*k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut entries = Map::new();
    for k in SIDE_ENTRIES {
        let fallback = match k {
            "answer_question" => Value::Null,
            "show_schedules" => Value::Bool(false),
            _ => json!([]),
        };
        entries.insert(k.to_string(), answer.get(k).cloned().unwrap_or(fallback));
    }
    if let Some(one) = answer.get("schedule").filter(|v| !v.is_null()) {
        let mut list = match &entries["schedules"] {
            Value::Array(items) => items.clone(),
            v if ferb_py::value::truthy(v) => {
                // `[*(entries["schedules"] or []), …]` over a non-list truthy value: Python iterates it
                iterate(v)
            }
            _ => Vec::new(),
        };
        list.push(one.clone());
        entries.insert("schedules".into(), Value::Array(list));
    }
    let any = entries.values().any(ferb_py::value::truthy);
    out.insert(
        "side_entries".into(),
        if any {
            Value::Object(entries)
        } else {
            Value::Null
        },
    );
    if let Some(Value::Object(task)) = out.get("task").cloned() {
        let conditions: Vec<Value> = match task.get("success_conditions") {
            Some(v) if ferb_py::value::truthy(v) => iterate(v),
            _ => Vec::new(),
        }
        .into_iter()
        .map(|c| {
            let mut c = c.as_object().cloned().unwrap_or_default();
            let check = c.get("check").cloned().unwrap_or(Value::Null);
            let kind = if ferb_py::value::truthy(&check) {
                check.get("kind").cloned().unwrap_or(json!("none"))
            } else {
                json!("none")
            };
            let new = if kind == json!("none") {
                Value::Null
            } else {
                let mut m = Map::new();
                for k in no_check().as_object().expect("an object").keys() {
                    m.insert(k.clone(), check.get(k).cloned().unwrap_or(Value::Null));
                }
                Value::Object(m)
            };
            c.insert("check".into(), new);
            Value::Object(c)
        })
        .collect();
        let mut task = task;
        task.insert("success_conditions".into(), Value::Array(conditions));
        out.insert("task".into(), Value::Object(task));
    }
    let mut ordered = Map::new();
    for k in props.keys() {
        if out.contains_key(k) || k == "side_entries" {
            ordered.insert(k.clone(), out.get(k).cloned().unwrap_or(Value::Null));
        }
    }
    Value::Object(ordered)
}

/// `list(v)` of a JSON value: a list's items, a dict's keys, a string's characters.
fn iterate(v: &Value) -> Vec<Value> {
    match v {
        Value::Array(items) => items.clone(),
        Value::Object(map) => map.keys().map(|k| json!(k)).collect(),
        Value::String(s) => s.chars().map(|c| json!(c.to_string())).collect(),
        _ => Vec::new(),
    }
}

/// The `mode="before"` readers of `SupervisorOutcome` (schemas.py:498 `_one_schedule`, :518 `_entries_only`), in
/// the order pydantic runs them (the later-defined first): the JSON the model validates.
pub fn supervisor_before(answer: &Value) -> Value {
    one_schedule(&entries_only(answer))
}

/// schemas.py:518 `_entries_only` (ORCH-582): kind "task" with no task and no control whose side entries hold
/// something is a reply. Which fields are filled, never what the words say.
pub fn entries_only(answer: &Value) -> Value {
    let data = ungrouped(answer);
    let Value::Object(map) = &data else {
        return data;
    };
    let filled = |k: &str| map.get(k).is_some_and(ferb_py::value::truthy);
    if map.get("kind") == Some(&json!("task"))
        && map.get("task").is_none_or(Value::is_null)
        && map.get("control").is_none_or(Value::is_null)
        && (SIDE_ENTRIES.iter().any(|k| filled(k)) || filled("schedule"))
    {
        let mut out = map.clone();
        out.insert("kind".into(), json!("reply"));
        return Value::Object(out);
    }
    data
}

/// schemas.py:498 `_one_schedule`: an answer's single `schedule` (before T-1066) read as `schedules`; an answer in
/// the 2026-11-09 form (`show`, `about_tasks`) read as before them (ORCH-577).
pub fn one_schedule(answer: &Value) -> Value {
    let mut data = ungrouped(answer);
    if let Value::Object(map) = &data {
        if map.contains_key("schedule") {
            let one = map["schedule"].clone();
            let mut out: Map<String, Value> = map
                .iter()
                .filter(|(k, _)| *k != "schedule")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if !one.is_null() {
                let mut list = match out.get("schedules") {
                    Some(v) if ferb_py::value::truthy(v) => iterate(v),
                    _ => Vec::new(),
                };
                list.push(one);
                out.insert("schedules".into(), Value::Array(list));
            }
            data = Value::Object(out);
        }
    }
    if let Value::Object(map) = &data {
        if map.contains_key("show") || map.contains_key("about_tasks") {
            let shown = map.get("show").cloned();
            let mut out: Map<String, Value> = map
                .iter()
                .filter(|(k, _)| *k != "show" && *k != "about_tasks")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if !out.contains_key("show_schedules") {
                let has = matches!(&shown, Some(Value::Array(items)) if items.contains(&json!("schedules")));
                out.insert("show_schedules".into(), json!(has));
            }
            data = Value::Object(out);
        }
    }
    data
}

/// schemas.py:367 `Check._legacy_ledger_done`: kind ledger_done (before V6) reads as "none".
pub fn check_before(data: &Value) -> Value {
    match data {
        Value::Object(map) if map.get("kind") == Some(&json!("ledger_done")) => {
            let mut out = map.clone();
            out.insert("kind".into(), json!("none"));
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// schemas.py:387 `Condition._no_code_check` (ORCH-630): a null (or missing) check is kind "none".
pub fn condition_before(data: &Value) -> Value {
    match data {
        Value::Object(map) if map.get("check").is_none_or(Value::is_null) => {
            let mut out = map.clone();
            out.insert("check".into(), no_check());
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// schemas.py:426 `ScheduleUntil._unused`: the schema's "" and 0 for an unused field (not `quote`) are None.
pub fn schedule_until_before(data: &Value) -> Value {
    match data {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let unused = k != "quote" && is_empty_or_zero(v);
                    (k.clone(), if unused { Value::Null } else { v.clone() })
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `v in ("", 0)`: `0 == False == 0.0` in Python, so false and 0.0 count too.
fn is_empty_or_zero(v: &Value) -> bool {
    match v {
        Value::String(s) => s.is_empty(),
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::Bool(b) => !b,
        _ => false,
    }
}

/// schemas.py:530 `SupervisorOutcome.new_task` on the JSON: the task this answer starts as new work (ORCH-638).
pub fn new_task(answer: &Value) -> Option<&Value> {
    if answer.get("kind") != Some(&json!("task")) {
        return None;
    }
    let task = answer.get("task").filter(|t| !t.is_null())?;
    match answer.get("control") {
        None | Some(Value::Null) => Some(task),
        Some(control) => match control.get("action").and_then(Value::as_str) {
            Some("stop" | "pause") => Some(task),
            _ => None,
        },
    }
}
