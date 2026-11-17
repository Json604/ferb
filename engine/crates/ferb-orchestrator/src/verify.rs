//! ferb/orchestrator/verify.py: literal checks and code's facts about a step's result. File existence and content, tab
//! URLs and the latest acting call's read-back yield literal predicates. An action check (sent, paid, submitted,
//! uploaded, trashed, deleted, downloaded) yields only what code can see itself (ORCH-664): the Mac's state now (a
//! file is there or gone, its bytes, the Trash, a taken download), a receipt a tool wrote when it acted (which bytes,
//! to whom, where), and, in the supervisor, the owner's word and "FERB did nothing in this step". It never concludes
//! "not done" from a missing record of one route; whatever it can't see goes to the checker.
//!
//! decision: ORCH-664, ORCH-666, ORCH-667, ORCH-668, ORCH-613, ORCH-309

use std::io::Read;

use serde_json::{json, Map, Value};

use crate::cards;
use crate::models::Condition;
use crate::ports::Policy;
use crate::sql::Result;

pub const UNCHECKED: &str = "unchecked";
pub const DECLINED: &str = "owner";
pub const DECLINED_SAID: &str = "You said no to it on its card, so FERB didn't do it";
/// The action check kinds (ORCH-576's six and ORCH-667's `uploaded`): what each names tells code which facts to
/// gather; none is a veto.
pub const ACTION_CHECKS: &[&str] = &[
    "sent",
    "paid",
    "submitted",
    "trashed",
    "deleted",
    "downloaded",
    "uploaded",
];

/// ORCH-665: a condition's verdict.
pub const MET: &str = "met";
pub const NOT_MET: &str = "not_met";
/// FERB acted, but on something other than what the condition names (ORCH-668).
pub const DIFFERENT: &str = "different";
pub const CANT_TELL: &str = "cant_tell";

/// ORCH-665: who gave a verdict. The Mac's own state read now.
pub const BY_MAC: &str = "mac";
/// The owner's Yes or No on a card.
pub const BY_OWNER: &str = "owner";
/// What a tool wrote about its own action when it acted.
pub const BY_RECEIPT: &str = "receipt";
/// FERB's own records of what it did: here, that it did nothing that could have done it.
pub const BY_RECORD: &str = "record";
/// The checker, on its look.
pub const BY_CHECKER: &str = "checker";

/// What a verdict's met is: true for met, false for not met and different, null for can't tell.
pub fn met_of(verdict: &str) -> Option<bool> {
    match verdict {
        MET => Some(true),
        NOT_MET | DIFFERENT => Some(false),
        _ => None,
    }
}

/// Code's own fact kept beside a verdict someone else gave (ORCH-665): who, what it said, on what basis.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CodeFact {
    pub by: String,
    pub verdict: String,
    pub basis: String,
}

/// verify.py:31 `ConditionResult`, with ORCH-665's verdict, who gave it, its basis, and for `different` what was asked
/// and what was acted on. The added fields are stored only when set, so a plain result keeps its recorded bytes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConditionResult {
    pub index: i64,
    pub text: String,
    pub met: Option<bool>,
    pub path: String,
    pub evidence: String,
    pub how: String,
    /// "" (a result from before ORCH-665: read it from `met` and `path`), or one of MET, NOT_MET, DIFFERENT, CANT_TELL.
    pub verdict: String,
    /// "" or one of BY_MAC, BY_OWNER, BY_RECEIPT, BY_RECORD, BY_CHECKER.
    pub by: String,
    pub basis: String,
    pub asked: String,
    pub acted_on: String,
    /// Code's fact when the verdict is someone else's (a disagreement, or "nothing done" beside an outcome that holds).
    pub code: Option<CodeFact>,
}

impl ConditionResult {
    pub fn new(
        index: i64,
        text: &str,
        met: Option<bool>,
        path: &str,
        evidence: &str,
    ) -> ConditionResult {
        ConditionResult {
            index,
            text: text.into(),
            met,
            path: path.into(),
            evidence: evidence.into(),
            ..ConditionResult::default()
        }
    }

    /// ORCH-665: a fact of code's (path "code"): its verdict, who, the basis (also its evidence).
    pub fn fact(index: i64, text: &str, verdict: &str, by: &str, basis: &str) -> ConditionResult {
        ConditionResult {
            verdict: verdict.into(),
            by: by.into(),
            basis: basis.into(),
            ..ConditionResult::new(index, text, met_of(verdict), "code", basis)
        }
    }

    /// The verdict: the stored one, else read from `met`.
    pub fn verdict_of(&self) -> &str {
        if !self.verdict.is_empty() {
            return &self.verdict;
        }
        match self.met {
            Some(true) => MET,
            Some(false) => NOT_MET,
            None => CANT_TELL,
        }
    }

    /// ORCH-664: whether code's own fact proves the condition false (the Mac's state, or FERB did nothing); the
    /// owner's No is apart (path "owner"), and a receipt never proves "not done".
    pub fn proved_false(&self) -> bool {
        self.path == "code"
            && self.verdict == NOT_MET
            && (self.by == BY_MAC || self.by == BY_RECORD)
    }

    /// verify.py:39 `as_dict`, with ORCH-665's fields when set.
    pub fn as_dict(&self) -> Value {
        let mut d = Map::new();
        d.insert("index".into(), json!(self.index));
        d.insert("condition".into(), json!(self.text));
        d.insert(
            "met".into(),
            self.met.map(Value::Bool).unwrap_or(Value::Null),
        );
        d.insert("path".into(), json!(self.path));
        d.insert("evidence".into(), json!(self.evidence));
        if !self.how.is_empty() {
            d.insert("how".into(), json!(self.how));
        }
        if !self.verdict.is_empty() {
            d.insert("verdict".into(), json!(self.verdict));
            d.insert("by".into(), json!(self.by));
            d.insert("basis".into(), json!(self.basis));
        }
        if !self.asked.is_empty() {
            d.insert("asked".into(), json!(self.asked));
        }
        if !self.acted_on.is_empty() {
            d.insert("acted_on".into(), json!(self.acted_on));
        }
        if let Some(c) = &self.code {
            d.insert(
                "code".into(),
                json!({"by": c.by, "verdict": c.verdict, "basis": c.basis}),
            );
        }
        Value::Object(d)
    }

    /// The fields `as_dict` added (ORCH-665), read back from a stored result.
    pub fn with_stored(mut self, r: &Value) -> ConditionResult {
        let text = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        self.verdict = text("verdict");
        self.by = text("by");
        self.basis = text("basis");
        self.asked = text("asked");
        self.acted_on = text("acted_on");
        self.code = r.get("code").filter(|c| c.is_object()).map(|c| {
            let t = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            CodeFact {
                by: t("by"),
                verdict: t("verdict"),
                basis: t("basis"),
            }
        });
        self
    }
}

/// `Path(p).name`.
fn name_of(p: &str) -> String {
    ferb_fs::pypath::name(p)
}

/// A file's SHA-256 now, as lowercase hex (None: it can't be read as a file).
pub fn sha256_of(path: &str) -> Option<String> {
    use sha2::Digest;
    if !ferb_fs::pypath::is_file(path) {
        return None;
    }
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// aftercheck.py `compare`: an expectation against the acting call's after block.
pub fn compare(expect: &Map<String, Value>, after: &Map<String, Value>) -> (Option<bool>, String) {
    if let Some(want) = expect.get("value_contains") {
        let Some(value) = after.get("value").filter(|v| !v.is_null()) else {
            return (None, "no value was read back after the action".into());
        };
        let ok = crate::py::casefold(&crate::py::str_of(value))
            .contains(&crate::py::casefold(&crate::py::str_of(want)));
        return (
            Some(ok),
            format!(
                "after value {} the expected text",
                if ok { "contains" } else { "does not contain" }
            ),
        );
    }
    if let Some(want) = expect.get("changed") {
        let Some(changed) = after.get("changed").filter(|v| !v.is_null()) else {
            return (None, "nothing was read back after the action".into());
        };
        let did = ferb_py::value::truthy(changed);
        let ok = did == ferb_py::value::truthy(want);
        return (
            Some(ok),
            format!(
                "the screen {} as {}",
                if did { "changed" } else { "did not change" },
                if ok { "expected" } else { "not expected" }
            ),
        );
    }
    (None, "no expectation to compare".into())
}

/// Why resolving a condition's path failed (ferb/act/fs/files.py `resolve`).
#[derive(Debug, Clone, PartialEq)]
pub enum PathError {
    /// `PathRefused`: its message.
    Refused(String),
    /// Any other `OSError` / `UnicodeError`: (type name, `str(e)`).
    Os(String, String),
}

/// What the checks read from the rest of FERB.
pub trait CheckHost {
    /// `resolve(path, ctx, must_exist)`.
    fn resolve(&self, path: &str, must_exist: bool) -> std::result::Result<String, PathError>;
    /// Every current URL of the task's browser tabs.
    fn urls_of(&self, task_id: &str) -> Vec<String>;
    /// `SpanStore.latest_acting_call(task_id, tool)`.
    fn latest_acting_call(&self, task_id: &str, tool: Option<&str>) -> Result<Option<Value>>;
    fn policy(&self) -> &dyn Policy;
    /// The store (None: Checks built without spans).
    fn db(&self) -> Option<&ferb_store::Db>;
    /// ORCH-666: the owner's Trash folder, where a trashed item is looked for by name (None: only FERB's own trash
    /// receipts say where an item went).
    fn trash_dir(&self) -> Option<String> {
        None
    }
}

/// `str(e)` of an OSError for a path, as Python writes it.
pub fn os_error(e: &std::io::Error, path: &str) -> (String, String) {
    let kind = match e.kind() {
        std::io::ErrorKind::NotFound => "FileNotFoundError",
        std::io::ErrorKind::PermissionDenied => "PermissionError",
        std::io::ErrorKind::IsADirectory => "IsADirectoryError",
        _ => "OSError",
    };
    match e.raw_os_error() {
        Some(n) => {
            // SAFETY: strerror returns a pointer to a static, NUL-terminated message for any errno.
            let text = unsafe { std::ffi::CStr::from_ptr(libc::strerror(n)) }
                .to_string_lossy()
                .into_owned();
            (
                kind.into(),
                format!("[Errno {n}] {text}: {}", crate::py::repr_str(path)),
            )
        }
        None => (kind.into(), e.to_string()),
    }
}

/// verify.py:60 `Checks`.
pub struct Checks<'a> {
    pub host: &'a dyn CheckHost,
}

impl<'a> Checks<'a> {
    /// verify.py:66 `code_checkable`.
    pub fn code_checkable(&self, c: &Condition, task_id: Option<&str>) -> bool {
        let k = &c.check;
        let spans = self.host.db().is_some();
        let some = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.is_empty());
        if k.kind == "after_change" || k.kind == "after_value" {
            return spans && task_id.is_some();
        }
        if ACTION_CHECKS.contains(&k.kind.as_str()) {
            return spans
                && task_id.is_some()
                && (k.kind == "paid"
                    || match k.kind.as_str() {
                        "sent" => some(&k.expected),
                        "submitted" => some(&k.site),
                        _ => some(&k.path),
                    });
        }
        (k.kind == "file_exists" && some(&k.path))
            || (k.kind == "file_contains" && some(&k.path) && some(&k.text))
            || (k.kind == "tab_url" && some(&k.url_contains))
    }

    /// verify.py:79 `run`.
    pub fn run(&self, i: i64, c: &Condition, task_id: &str) -> Result<ConditionResult> {
        let k = &c.check;
        let result = |met: Option<bool>, evidence: String| {
            ConditionResult::new(i, &c.text, met, "code", &evidence)
        };
        let path = k.path.clone().unwrap_or_default();
        if k.kind == "file_exists" || k.kind == "file_contains" {
            let p = match self.host.resolve(&path, true) {
                Ok(p) => p,
                Err(PathError::Refused(e)) => {
                    return Ok(result(Some(false), format!("{path}: {e}")))
                }
                Err(PathError::Os(kind, e)) => {
                    return Ok(result(
                        None,
                        format!(
                            "{path}: literal {} observation unavailable: {kind}: {e}",
                            k.kind
                        ),
                    ))
                }
            };
            if k.kind == "file_exists" {
                return Ok(result(Some(true), format!("{p} exists")));
            }
            let needle = k.text.clone().unwrap_or_default();
            let mut found = false;
            if ferb_fs::pypath::is_file(&p) {
                let mut bytes = Vec::new();
                match std::fs::File::open(&p).and_then(|mut f| f.read_to_end(&mut bytes)) {
                    Ok(_) => {}
                    Err(e) => {
                        let (kind, text_) = os_error(&e, &p);
                        return Ok(result(
                            None,
                            format!(
                                "{path}: literal {} observation unavailable: {kind}: {text_}",
                                k.kind
                            ),
                        ));
                    }
                }
                // Python reads the text 65,536 characters at a time and the read that meets a byte that isn't
                // UTF-8 raises: the needle counts only when found in the reads before it
                let (good, bad) = match std::str::from_utf8(&bytes) {
                    Ok(t) => (t, None),
                    Err(e) => (
                        std::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap_or(""),
                        Some(e),
                    ),
                };
                let searched = match bad {
                    None => good,
                    Some(_) => {
                        let whole_reads = good.chars().count() / 65536 * 65536;
                        crate::py::prefix(good, whole_reads)
                    }
                };
                found = searched.contains(needle.as_str());
                if !found {
                    if let Some(e) = bad {
                        let at = e.valid_up_to();
                        let why = if e.error_len().is_none() {
                            "unexpected end of data"
                        } else {
                            "invalid start byte"
                        };
                        return Ok(result(
                            None,
                            format!(
                                "{path}: literal {} observation unavailable: UnicodeDecodeError: 'utf-8' codec can't decode \
                                 byte 0x{:02x} in position {}: {why}",
                                k.kind,
                                bytes[at],
                                at % 8192
                            ),
                        ));
                    }
                }
            }
            return Ok(result(
                Some(found),
                format!(
                    "{p} {} the expected text",
                    if found {
                        "contains"
                    } else {
                        "does not contain"
                    }
                ),
            ));
        }
        if k.kind == "after_change" || k.kind == "after_value" {
            let call = if self.host.db().is_some() {
                self.host.latest_acting_call(task_id, k.tool.as_deref())?
            } else {
                None
            };
            let Some(call) = call else {
                return Ok(result(
                    None,
                    format!(
                        "no {} call recorded in this task",
                        k.tool
                            .as_deref()
                            .filter(|t| !t.is_empty())
                            .unwrap_or("acting")
                    ),
                ));
            };
            let output = call
                .get("output")
                .filter(|o| ferb_py::value::truthy(o))
                .cloned()
                .unwrap_or(json!({}));
            let tool = call
                .get("tool")
                .map(crate::py::str_of)
                .unwrap_or_else(|| "None".into());
            let error = call
                .get("error")
                .filter(|e| !e.is_null())
                .map(crate::py::str_of);
            let Some(Value::Object(after)) = output.get("after").cloned() else {
                let tail = match error.as_deref().filter(|e| !e.is_empty()) {
                    Some(e) => format!(" ({e})"),
                    None => String::new(),
                };
                return Ok(result(
                    None,
                    format!("the latest {tool} call recorded no after-read{tail}"),
                ));
            };
            let mut expect = Map::new();
            if k.kind == "after_value" {
                expect.insert(
                    "value_contains".into(),
                    json!(k.text.clone().unwrap_or_default()),
                );
            } else {
                let expected = match k.expected.as_deref() {
                    Some(e) if !e.is_empty() => e.to_string(),
                    _ => "true".into(),
                };
                expect.insert(
                    "changed".into(),
                    json!(crate::py::lower(crate::py::strip(&expected)) != "false"),
                );
            }
            let failed = error.as_deref().is_some_and(|e| !e.is_empty())
                || output.get("ok") == Some(&Value::Bool(false));
            if failed
                && expect.get("changed") == Some(&Value::Bool(true))
                && after.get("changed") == Some(&Value::Bool(true))
            {
                let said = match error.as_deref().filter(|e| !e.is_empty()) {
                    Some(e) => e.to_string(),
                    None => "ok: false".into(),
                };
                return Ok(result(
                    None,
                    format!("the latest {tool} call changed the screen but reported an error ({said}), so the change isn't confirmed"),
                ));
            }
            let (met, evidence) = compare(&expect, &after);
            return Ok(result(met, evidence));
        }
        if ACTION_CHECKS.contains(&k.kind.as_str()) {
            return self.action(i, c, task_id);
        }
        if k.kind == "tab_url" {
            let urls = self.host.urls_of(task_id);
            let want = k.url_contains.clone().unwrap_or_default();
            let matches: Vec<&String> = urls.iter().filter(|u| u.contains(want.as_str())).collect();
            let evidence = ferb_py::json::dumps(
                &json!({"url_contains": k.url_contains, "all_task_urls": urls,
                                                        "matching_urls": matches}),
            );
            return Ok(result(Some(!matches.is_empty()), evidence));
        }
        Ok(result(
            None,
            "Inspect the requested outcome with current source evidence".into(),
        ))
    }

    /// ORCH-666: an action check's facts. Each says only what code saw: the Mac's state now (`mac`), or a receipt a tool
    /// wrote when it acted (`receipt`), or that nothing that could carry a file from the Mac ran (`record`, uploads
    /// only). What none of them covers is CANT_TELL with the facts as its basis, for the checker's look.
    fn action(&self, i: i64, c: &Condition, task_id: &str) -> Result<ConditionResult> {
        let db = self.host.db().expect("action checks need the store");
        // the step's own calls, not its checks' observations
        let calls: Vec<cards::Recorded> = cards::recorded(db, task_id)?
            .into_iter()
            .filter(|r| !r.task_id.ends_with(".verify"))
            .collect();
        match c.check.kind.as_str() {
            "sent" => Ok(self.sent(i, c, &calls)),
            "paid" => self.paid(i, c, task_id),
            "submitted" => Ok(self.submitted(i, c, &calls)),
            "uploaded" => Ok(self.uploaded(i, c, &calls)),
            "downloaded" => self.downloaded(i, c, task_id),
            _ => Ok(self.removed(i, c, &calls)),
        }
    }

    fn on(c: &Condition) -> String {
        match c.check.site.as_deref().filter(|s| !s.is_empty()) {
            Some(s) => format!(" on {s}"),
            None => String::new(),
        }
    }

    /// The recipients a send-class call's tool resolved (`target.recipients`, `target.contact`).
    fn reached(r: &cards::Recorded) -> Vec<String> {
        let target = r.target();
        let mut out: Vec<String> = match target.get("recipients") {
            Some(Value::Array(a)) => a
                .iter()
                .filter(|x| ferb_py::value::truthy(x))
                .map(crate::py::str_of)
                .collect(),
            _ => Vec::new(),
        };
        if let Some(contact) = target.get("contact").filter(|v| ferb_py::value::truthy(v)) {
            out.push(crate::py::str_of(contact));
        }
        out
    }

    /// sent {expected, site?}: the recipients FERB's own send tools resolved (by the call's kind or structural facts,
    /// never the worker's declaration). One to the expected recipient is a receipt for the checker; sends only to
    /// other addresses of the same kind (both email addresses, both phone numbers, both handles) are `different`.
    fn sent(&self, i: i64, c: &Condition, calls: &[cards::Recorded]) -> ConditionResult {
        let policy = self.host.policy();
        let k = &c.check;
        let want = k.expected.clone().unwrap_or_default();
        let on = Self::on(c);
        let mut reached: Vec<String> = Vec::new();
        for r in calls {
            let site_ok = k.site.as_deref().is_none_or(str::is_empty)
                || policy.site_key(k.site.as_deref()) == policy.site_key(r.site().as_deref());
            if r.error.is_some()
                || !r.performed()
                || !site_ok
                || !r
                    .structural()
                    .iter()
                    .any(|x| cards::SEND_KINDS.contains(&x.as_str()))
            {
                continue;
            }
            for to in Self::reached(r) {
                if !reached.contains(&to) {
                    reached.push(to);
                }
            }
        }
        let want_key = policy.account_key(Some(&want));
        if reached
            .iter()
            .any(|r| policy.account_key(Some(r)) == want_key || same_address(r, &want))
        {
            return ConditionResult::fact(
                i,
                &c.text,
                CANT_TELL,
                BY_RECEIPT,
                &format!(
                    "FERB's own send to {want}{on} is recorded; whether it arrived is for the check's look"
                ),
            );
        }
        if let Some((kind, _)) = address_kind(&want) {
            let others: Vec<String> = reached
                .iter()
                .filter(|r| address_kind(r).is_some_and(|(k2, _)| k2 == kind))
                .cloned()
                .collect();
            if !others.is_empty() {
                let mut r = ConditionResult::fact(
                    i,
                    &c.text,
                    DIFFERENT,
                    BY_RECEIPT,
                    &format!(
                        "FERB's recorded send{on} went to {}, not to {want}",
                        others.join(", ")
                    ),
                );
                r.asked = want;
                r.acted_on = others.join(", ");
                return r;
            }
        }
        let basis = if reached.is_empty() {
            format!(
                "no send of FERB's own to {want}{on} is recorded; a send through a page's own button or an app's window \
                 may not be, so whether it went is for the check's look"
            )
        } else {
            format!(
                "FERB's recorded sends{on} went to {}; whether one of them is {want} is for the check's look",
                reached.join(", ")
            )
        };
        ConditionResult::fact(i, &c.text, CANT_TELL, BY_RECEIPT, &basis)
    }

    /// paid {site?}: a payment FERB pressed after the owner's Yes is a receipt for the checker; none recorded is no
    /// proof of anything (a page's own button, a redirect host). The amount was checked at the press (the gateway).
    fn paid(&self, i: i64, c: &Condition, task_id: &str) -> Result<ConditionResult> {
        let db = self.host.db().expect("store");
        let policy = self.host.policy();
        let k = &c.check;
        let on = Self::on(c);
        let pays: Vec<Map<String, Value>> = cards::paid(db, policy, task_id)?
            .into_iter()
            .filter(|call| {
                k.site.as_deref().is_none_or(str::is_empty)
                    || policy.site_key(k.site.as_deref())
                        == policy.site_key(
                            call.get("target")
                                .and_then(|t| t.get("site"))
                                .filter(|v| !v.is_null())
                                .map(crate::py::str_of)
                                .as_deref(),
                        )
            })
            .collect();
        let basis = match pays.last() {
            Some(call) => {
                let amount = call
                    .get("facts")
                    .and_then(|f| f.get("page_amount"))
                    .filter(|v| ferb_py::value::truthy(v))
                    .map(|v| format!(" ({})", crate::py::str_of(v)))
                    .unwrap_or_default();
                format!(
                    "FERB pressed pay{on}{amount} after your Yes; whether the payment went through is for the check's look"
                )
            }
            None => format!(
                "no payment of FERB's own after your Yes is recorded{on}; a page's own button or a bank's page may not \
                 be, so whether one went through is for the check's look"
            ),
        };
        Ok(ConditionResult::fact(
            i, &c.text, CANT_TELL, BY_RECEIPT, &basis,
        ))
    }

    /// submitted {site}: FERB's own actions on the site and whether a page reported a form submitted during one. A
    /// page's own button can submit without a form, so none of it is a verdict.
    fn submitted(&self, i: i64, c: &Condition, calls: &[cards::Recorded]) -> ConditionResult {
        let policy = self.host.policy();
        let site = c.check.site.clone().unwrap_or_default();
        let key = policy.site_key(Some(&site));
        let here: Vec<&cards::Recorded> = calls
            .iter()
            .filter(|r| r.acting() && policy.site_key(r.site().as_deref()) == key)
            .collect();
        let Some(last) = here.last() else {
            return ConditionResult::fact(
                i,
                &c.text,
                CANT_TELL,
                BY_RECEIPT,
                &format!(
                    "no action of FERB's own on {site} is recorded in this step; work on another host of the site or \
                     through an app's window may not be, so whether it went through is for the check's look"
                ),
            );
        };
        let submits = here.iter().filter(|r| r.submit_seen()).count();
        let seen = if submits > 0 {
            "; the page reported a form submitted during one of them"
        } else {
            ""
        };
        let note = last
            .output
            .get("after")
            .and_then(|a| a.get("note"))
            .filter(|n| ferb_py::value::truthy(n))
            .map(|n| format!(" (read back: {})", crate::py::str_of(n)))
            .unwrap_or_default();
        let kind = last
            .input
            .get("kind")
            .map(crate::py::str_of)
            .unwrap_or_default();
        ConditionResult::fact(
            i,
            &c.text,
            CANT_TELL,
            BY_RECEIPT,
            &format!(
                "FERB acted {} time(s) on {site} in this step{seen}; the last was a {kind}{note}. A page's own button can \
                 submit without a form, so whether it went through is for the check's look",
                here.len()
            ),
        )
    }

    /// uploaded {path} (ORCH-667): the receipts of FERB's upload tool (each file's path and SHA-256 as read, and
    /// whether the page's file field holds the same bytes) against the named file (by path, or by its bytes now).
    /// An upload of other files only is `different`; no call that can carry a file from the Mac at all is `record`.
    /// ORCH-670: the named path counts as the same file whatever its bytes are now.
    fn uploaded(&self, i: i64, c: &Condition, calls: &[cards::Recorded]) -> ConditionResult {
        let path = c.check.path.clone().unwrap_or_default();
        let want = match self.host.resolve(&path, false) {
            Ok(p) => p,
            Err(PathError::Refused(e)) | Err(PathError::Os(_, e)) => {
                return ConditionResult::fact(
                    i,
                    &c.text,
                    CANT_TELL,
                    BY_RECEIPT,
                    &format!("{path}: {e}"),
                )
            }
        };
        let name = name_of(&want);
        let now = sha256_of(&want);
        let uploads: Vec<(&cards::Recorded, Vec<(String, String)>)> = calls
            .iter()
            .filter_map(|r| r.uploaded_files().map(|f| (r, f)))
            .collect();
        let same_file =
            |(p, sha): &(String, String)| same(p, &want) || now.as_deref() == Some(sha.as_str());
        if let Some((r, _)) = uploads
            .iter()
            .rev()
            .find(|(_, files)| files.iter().any(same_file))
        {
            if r.output.get("bytes_match") == Some(&Value::Bool(false)) {
                let mut out = ConditionResult::fact(
                    i,
                    &c.text,
                    DIFFERENT,
                    BY_RECEIPT,
                    &format!("FERB gave the page {name}, but the page's file field holds different bytes"),
                );
                out.asked = want;
                out.acted_on = "different bytes in the page's file field".into();
                return out;
            }
            let checked = if r.output.get("bytes_match") == Some(&Value::Bool(true)) {
                " (the page's file field holds its bytes)"
            } else {
                ""
            };
            return ConditionResult::fact(
                i,
                &c.text,
                CANT_TELL,
                BY_RECEIPT,
                &format!(
                    "FERB uploaded {name} from your Mac{checked}; whether the site took it is for the check's look"
                ),
            );
        }
        if !uploads.is_empty() {
            let mut acted: Vec<String> = Vec::new();
            for (_, files) in &uploads {
                for (p, _) in files {
                    if !acted.contains(p) {
                        acted.push(p.clone());
                    }
                }
            }
            let mut out = ConditionResult::fact(
                i,
                &c.text,
                DIFFERENT,
                BY_RECEIPT,
                &format!(
                    "FERB uploaded {} from your Mac, not {want}",
                    acted.join(", ")
                ),
            );
            out.asked = want;
            out.acted_on = acted.join(", ");
            return out;
        }
        let routes: Vec<String> = calls
            .iter()
            .filter(|r| r.performed() && r.carries_bytes())
            .map(|r| r.tool())
            .fold(Vec::new(), |mut v, t| {
                if !v.contains(&t) {
                    v.push(t);
                }
                v
            });
        if !routes.is_empty() {
            return ConditionResult::fact(
                i,
                &c.text,
                CANT_TELL,
                BY_RECEIPT,
                &format!(
                    "FERB's own upload of {name} isn't recorded, but {} ran, which can carry a file from the Mac; \
                     whether {name} reached the site is for the check's look",
                    routes.join(", ")
                ),
            );
        }
        let mut out = ConditionResult::fact(
            i,
            &c.text,
            NOT_MET,
            BY_RECORD,
            &format!(
                "no file from your Mac was uploaded in this step: FERB's upload of {name} isn't recorded, and nothing \
                 else that can carry a file from the Mac (input through the Mac tools, a script, a shell command) ran"
            ),
        );
        out.asked = want;
        out
    }

    /// trashed / deleted {path}: the Mac's state now. Still there is not met (or `different` when FERB's receipts
    /// show the same action on another path); gone is met for a deletion FERB has a receipt for, and for a trash when
    /// the item is in the Trash; the other of the two actions on it is `different`.
    fn removed(&self, i: i64, c: &Condition, calls: &[cards::Recorded]) -> ConditionResult {
        let trashed = c.check.kind == "trashed";
        let path = c.check.path.clone().unwrap_or_default();
        let want = match self.host.resolve(&path, false) {
            Ok(p) => p,
            Err(PathError::Refused(e)) | Err(PathError::Os(_, e)) => {
                return ConditionResult::fact(
                    i,
                    &c.text,
                    CANT_TELL,
                    BY_RECEIPT,
                    &format!("{path}: {e}"),
                )
            }
        };
        let name = name_of(&want);
        let done = |kind: &str| -> Vec<&cards::Recorded> {
            calls
                .iter()
                .filter(|r| r.error.is_none() && r.performed() && r.structural().contains(kind))
                .collect()
        };
        let (trash, delete) = (done("trash"), done("delete_permanent"));
        let on_want = |r: &&cards::Recorded| {
            let t = cards::target_path(&r.input);
            !t.is_empty() && (same(&t, &want) || holds(&t, &want))
        };
        let asked_kind = if trashed { &trash } else { &delete };
        let other_kind = if trashed { &delete } else { &trash };
        let verb = if trashed {
            "moved to the Trash"
        } else {
            "deleted"
        };
        if ferb_fs::pypath::exists(&want) {
            let elsewhere: Vec<String> = asked_kind
                .iter()
                .filter(|r| !on_want(r))
                .map(|r| cards::target_path(&r.input))
                .filter(|t| !t.is_empty())
                .collect();
            if !elsewhere.is_empty() {
                let mut out = ConditionResult::fact(
                    i,
                    &c.text,
                    DIFFERENT,
                    BY_RECEIPT,
                    &format!(
                        "{want} is still there, and FERB {verb} {} instead",
                        elsewhere.join(", ")
                    ),
                );
                out.asked = want;
                out.acted_on = elsewhere.join(", ");
                return out;
            }
            return ConditionResult::fact(
                i,
                &c.text,
                NOT_MET,
                BY_MAC,
                &format!("{want} is still there"),
            );
        }
        // where FERB's own trash receipts put it (a folder holding it: its place inside), if still there
        let in_trash = || -> Option<String> {
            for r in trash.iter().filter(|r| on_want(r)) {
                let target = cards::target_path(&r.input);
                let Some(to) = r
                    .output
                    .get("trashed_to")
                    .filter(|v| ferb_py::value::truthy(v))
                    .map(crate::py::str_of)
                else {
                    continue;
                };
                let at = if same(&target, &want) {
                    to
                } else {
                    let home = std::env::var("HOME").unwrap_or_default();
                    let real =
                        |p: &str| ferb_fs::pypath::realpath(&ferb_fs::pypath::expanduser(p, &home));
                    let (o, inner) = (real(&target), real(&want));
                    format!("{to}{}", &inner[o.len().min(inner.len())..])
                };
                if ferb_fs::pypath::exists(&at) {
                    return Some(at);
                }
            }
            let dir = self.host.trash_dir()?;
            let at = format!("{}/{name}", dir.trim_end_matches('/'));
            ferb_fs::pypath::exists(&at).then_some(at)
        };
        if trashed {
            if delete.iter().any(on_want) && !trash.iter().any(on_want) {
                let mut out = ConditionResult::fact(
                    i,
                    &c.text,
                    DIFFERENT,
                    BY_RECEIPT,
                    &format!("{want} was deleted for good, not moved to the Trash"),
                );
                out.asked = "moved to the Trash".into();
                out.acted_on = "deleted for good".into();
                return out;
            }
            return match in_trash() {
                Some(at) => ConditionResult::fact(
                    i,
                    &c.text,
                    MET,
                    BY_MAC,
                    &format!("{want} is gone from its folder and is in the Trash ({at})"),
                ),
                None => ConditionResult::fact(
                    i,
                    &c.text,
                    CANT_TELL,
                    BY_MAC,
                    &format!(
                        "{want} is gone from its folder, but FERB didn't find it in the Trash"
                    ),
                ),
            };
        }
        if trash.iter().any(on_want) && !delete.iter().any(on_want) {
            if let Some(at) = in_trash() {
                let mut out = ConditionResult::fact(
                    i,
                    &c.text,
                    DIFFERENT,
                    BY_RECEIPT,
                    &format!("{want} was moved to the Trash ({at}), not deleted for good"),
                );
                out.asked = "deleted for good".into();
                out.acted_on = "moved to the Trash".into();
                return out;
            }
        }
        if asked_kind.iter().any(on_want) || other_kind.iter().any(on_want) || in_trash().is_some()
        {
            return ConditionResult::fact(i, &c.text, MET, BY_MAC, &format!("{want} is gone"));
        }
        // gone, with nothing showing it was ever there: a wrong path costs a fact, never a false "done"
        ConditionResult::fact(
            i,
            &c.text,
            CANT_TELL,
            BY_MAC,
            &format!("{want} isn't there, and FERB's records don't show it there before"),
        )
    }

    /// downloaded {path}: the named file on the Mac with the bytes of one of this step's taken downloads is met; the
    /// file absent with nothing downloaded and no other route that could fetch one is not met; a download under another
    /// name or a file FERB didn't download is a fact for the checker (a guessed name is never a failure).
    fn downloaded(&self, i: i64, c: &Condition, task_id: &str) -> Result<ConditionResult> {
        let db = self.host.db().expect("store");
        let path = c.check.path.clone().unwrap_or_default();
        let want = match self.host.resolve(&path, false) {
            Ok(p) => p,
            Err(PathError::Refused(e)) | Err(PathError::Os(_, e)) => {
                return Ok(ConditionResult::fact(
                    i,
                    &c.text,
                    CANT_TELL,
                    BY_RECEIPT,
                    &format!("{path}: {e}"),
                ))
            }
        };
        let taken = cards::taken_downloads(db, task_id)?;
        if ferb_fs::pypath::exists(&want) {
            let sha = sha256_of(&want);
            if taken
                .iter()
                .any(|(p, s)| same(p, &want) || (sha.is_some() && s.as_deref() == sha.as_deref()))
            {
                return Ok(ConditionResult::fact(
                    i,
                    &c.text,
                    MET,
                    BY_MAC,
                    &format!("{want} is on your Mac with the bytes FERB downloaded in this task"),
                ));
            }
            return Ok(ConditionResult::fact(
                i,
                &c.text,
                CANT_TELL,
                BY_MAC,
                &format!(
                    "{want} is there, but it isn't one of the files FERB downloaded in this task (it may have been there already)"
                ),
            ));
        }
        let landed: Vec<String> = taken
            .iter()
            .map(|(p, _)| p.clone())
            .filter(|p| ferb_fs::pypath::exists(p))
            .collect();
        // the browser records every download it takes; with no other route that can fetch a file (Mac input, a
        // script, a shell command) in the step, nothing downloaded and the file absent is a fact both ways
        if taken.is_empty() {
            let routes = cards::recorded(db, task_id)?
                .iter()
                .filter(|r| !r.task_id.ends_with(".verify"))
                .any(|r| r.performed() && r.carries_bytes());
            if !routes {
                return Ok(ConditionResult::fact(
                    i,
                    &c.text,
                    NOT_MET,
                    BY_MAC,
                    &format!(
                        "{want} isn't on your Mac, and nothing was downloaded in this task (no browser download, and no \
                         Mac input, script or shell command that could fetch one)"
                    ),
                ));
            }
        }
        let basis = if !landed.is_empty() {
            format!(
                "{want} isn't there; FERB's download in this task landed at {}",
                landed.join(", ")
            )
        } else if taken.is_empty() {
            format!(
                "{want} isn't there, and FERB's browser recorded no download in this task (a download through an app's \
                 window or a command wouldn't be)"
            )
        } else {
            format!("{want} isn't there; FERB's downloads in this task were moved or removed since")
        };
        Ok(ConditionResult::fact(i, &c.text, CANT_TELL, BY_MAC, &basis))
    }

    /// verify.py:195 `_aimed`: an action check's call kinds and whether a call has its target (ORCH-613's declined
    /// test reads the calls this way).
    #[allow(clippy::type_complexity)]
    fn aimed(
        &self,
        c: &Condition,
    ) -> Option<(
        Vec<&'static str>,
        Box<dyn Fn(&Map<String, Value>) -> bool + '_>,
    )> {
        let k = c.check.clone();
        let policy = self.host.policy();
        let target_site = |call: &Map<String, Value>| {
            call.get("target")
                .and_then(|t| t.get("site"))
                .filter(|v| !v.is_null())
                .map(crate::py::str_of)
        };
        match k.kind.as_str() {
            "sent" => {
                let want = policy.account_key(k.expected.as_deref());
                let site = k.site.clone();
                Some((
                    cards::SEND_KINDS.to_vec(),
                    Box::new(move |call: &Map<String, Value>| {
                        let target = call
                            .get("target")
                            .and_then(|t| t.as_object())
                            .cloned()
                            .unwrap_or_default();
                        let mut reached: Vec<String> = match target.get("recipients") {
                            Some(Value::Array(a)) => a
                                .iter()
                                .filter(|r| ferb_py::value::truthy(r))
                                .map(crate::py::str_of)
                                .collect(),
                            _ => Vec::new(),
                        };
                        if let Some(contact) =
                            target.get("contact").filter(|v| ferb_py::value::truthy(v))
                        {
                            reached.push(crate::py::str_of(contact));
                        }
                        let on_site = site.as_deref().is_none_or(str::is_empty)
                            || policy.site_key(site.as_deref())
                                == policy.site_key(target_site(call).as_deref());
                        reached.iter().any(|r| policy.account_key(Some(r)) == want) && on_site
                    }),
                ))
            }
            "paid" => {
                let site = k.site.clone();
                Some((
                    vec!["pay"],
                    Box::new(move |call: &Map<String, Value>| {
                        site.as_deref().is_none_or(str::is_empty)
                            || policy.site_key(site.as_deref())
                                == policy.site_key(target_site(call).as_deref())
                    }),
                ))
            }
            "submitted" => {
                let site = k.site.clone();
                let mut kinds = vec!["submit"];
                kinds.extend(cards::SEND_KINDS);
                Some((
                    kinds,
                    Box::new(move |call: &Map<String, Value>| {
                        policy.site_key(target_site(call).as_deref())
                            == policy.site_key(site.as_deref())
                    }),
                ))
            }
            "trashed" | "deleted" => {
                let want = self
                    .host
                    .resolve(k.path.as_deref().unwrap_or(""), false)
                    .ok()?;
                let kind = if k.kind == "trashed" {
                    "trash"
                } else {
                    "delete_permanent"
                };
                Some((
                    vec![kind],
                    Box::new(move |call: &Map<String, Value>| {
                        same(&cards::target_path(call), &want)
                    }),
                ))
            }
            _ => None,
        }
    }

    /// verify.py:224 `declined` (ORCH-613).
    pub fn declined(
        &self,
        i: i64,
        c: &Condition,
        task_id: &str,
    ) -> Result<Option<ConditionResult>> {
        let Some(db) = self.host.db() else {
            return Ok(None);
        };
        if !ACTION_CHECKS.contains(&c.check.kind.as_str()) {
            return Ok(None);
        }
        let Some((kinds, aims)) = self.aimed(c) else {
            return Ok(None);
        };
        let tried: Vec<Map<String, Value>> = cards::tried(db, task_id, &kinds)?
            .into_iter()
            .filter(|call| aims(call))
            .collect();
        if tried
            .last()
            .is_some_and(|t| t.get("declined").is_some_and(ferb_py::value::truthy))
        {
            let mut r = ConditionResult::new(i, &c.text, Some(false), DECLINED, DECLINED_SAID);
            r.verdict = NOT_MET.into();
            r.by = BY_OWNER.into();
            r.basis = DECLINED_SAID.into();
            return Ok(Some(r));
        }
        Ok(None)
    }
}

/// ORCH-668: an address's kind and its comparable form: an email address, a phone number (its digits) or a handle;
/// None for anything else (a name, "my manager"), which code never compares.
pub fn address_kind(text: &str) -> Option<(&'static str, String)> {
    let t = crate::py::lower(crate::py::strip(text));
    let phone_like = t.chars().all(|c| "+0123456789 -().".contains(c));
    if t.is_empty() || (t.contains(char::is_whitespace) && !phone_like) {
        return None;
    }
    if let Some(rest) = t.strip_prefix('@') {
        return (!rest.is_empty()
            && rest
                .chars()
                .all(|c| c.is_alphanumeric() || "._-".contains(c)))
        .then(|| ("handle", t.clone()));
    }
    if let Some((user, domain)) = t.split_once('@') {
        return (!user.is_empty()
            && domain.contains('.')
            && !domain.starts_with('.')
            && !domain.ends_with('.')
            && !domain.contains('@'))
        .then(|| ("email", t.clone()));
    }
    if phone_like {
        let digits: String = t.chars().filter(char::is_ascii_digit).collect();
        if digits.len() >= 7 {
            return Some(("phone", digits));
        }
    }
    None
}

/// Two addresses of the same kind that name the same place: equal emails or handles; phone numbers whose digits are
/// equal or where one is the other with a country or trunk prefix (at least 7 digits shared at the end).
fn same_address(a: &str, b: &str) -> bool {
    match (address_kind(a), address_kind(b)) {
        (Some(("phone", x)), Some(("phone", y))) => x.ends_with(&y) || y.ends_with(&x),
        (Some((k1, x)), Some((k2, y))) => k1 == k2 && x == y,
        _ => false,
    }
}

/// ORCH-666: the action ledger's rows of a step (and the tasks under it) whose outcome is unknown and that the owner
/// hasn't had tried again: each one's place (its target's site, else its capability).
pub fn unknown_outcomes(db: &ferb_store::Db, task_id: &str) -> Result<Vec<String>> {
    let (where_, args) = ferb_store::spans::tree_where(task_id);
    let args: Vec<rusqlite::types::Value> = args.iter().map(|a| crate::sql::text(a)).collect();
    let mut out = Vec::new();
    for row in crate::sql::rows(
        db.conn(),
        &format!(
            "SELECT capability, target_json FROM action_ledger WHERE state = 'unknown' AND retried_by IS NULL \
             AND {where_} ORDER BY created_at, rowid"
        ),
        &args,
    )? {
        let target = crate::sql::loads(&crate::sql::col(&row, "target_json")).unwrap_or(Value::Null);
        let place = target
            .get("site")
            .filter(|v| ferb_py::value::truthy(v))
            .map(crate::py::str_of)
            .unwrap_or_else(|| crate::sql::col(&row, "capability"));
        if !out.contains(&place) {
            out.push(place);
        }
    }
    Ok(out)
}

/// Whether the folder `outer` holds `inner` (resolved as `same` resolves).
fn holds(outer: &str, inner: &str) -> bool {
    let home = std::env::var("HOME").unwrap_or_default();
    let real = |p: &str| ferb_fs::pypath::realpath(&ferb_fs::pypath::expanduser(p, &home));
    let (o, i) = (real(outer), real(inner));
    i.len() > o.len() && i.starts_with(&o) && i[o.len()..].starts_with('/')
}

/// verify.py:241 `_same`: two paths naming the same place, as written or resolved.
pub fn same(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let real = |p: &str| ferb_fs::pypath::realpath(&ferb_fs::pypath::expanduser(p, &home));
    real(a) == real(b)
}
