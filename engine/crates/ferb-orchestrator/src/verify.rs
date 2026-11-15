//! ferb/orchestrator/verify.py: literal checks and observed result facts. File existence and content, tab URLs, the
//! latest acting call's read-back and the action checks of ORCH-576 yield literal predicates, never final outcome
//! judgments; anything else goes to the checker.
//!
//! decision: ORCH-576, ORCH-609, ORCH-613, ORCH-624, ORCH-309

use std::io::Read;

use serde_json::{json, Map, Value};

use crate::cards;
use crate::models::Condition;
use crate::ports::Policy;
use crate::sql::Result;

pub const UNCHECKED: &str = "unchecked";
pub const DECLINED: &str = "owner";
pub const DECLINED_SAID: &str = "You said no to it on its card, so FERB didn't do it";
/// schemas.py:20 `ACTION_CHECKS`.
pub const ACTION_CHECKS: &[&str] = &[
    "sent",
    "paid",
    "submitted",
    "trashed",
    "deleted",
    "downloaded",
];

/// verify.py:31 `ConditionResult`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConditionResult {
    pub index: i64,
    pub text: String,
    pub met: Option<bool>,
    pub path: String,
    pub evidence: String,
    pub how: String,
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
            how: String::new(),
        }
    }

    /// verify.py:39 `as_dict`.
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
        Value::Object(d)
    }
}

/// `Path(p).name`.
fn name_of(p: &str) -> String {
    ferb_fs::pypath::name(p)
}

/// verify.py:45 `not_done_said` (ORCH-609).
pub fn not_done_said(c: &Condition) -> String {
    let k = &c.check;
    let on = match k.site.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => format!(" on {s}"),
        None => String::new(),
    };
    let name = match k.path.as_deref().filter(|p| !p.is_empty()) {
        Some(p) => name_of(p),
        None => "the file".into(),
    };
    match k.kind.as_str() {
        "sent" => match k.expected.as_deref().filter(|e| !e.is_empty()) {
            Some(e) => format!("Nothing was sent to {e}{on}."),
            None => format!("Nothing was sent{on}."),
        },
        "paid" => format!("No payment went through{on}."),
        "submitted" => format!("No form{on} was submitted."),
        "trashed" => format!("{name} wasn't moved to the Trash."),
        "deleted" => format!("{name} wasn't deleted."),
        "downloaded" => format!("{name} wasn't downloaded."),
        _ => String::new(),
    }
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

    fn action(&self, i: i64, c: &Condition, task_id: &str) -> Result<ConditionResult> {
        let db = self.host.db().expect("action checks need the store");
        let policy = self.host.policy();
        let k = &c.check;
        let result = |met: Option<bool>, evidence: String| {
            ConditionResult::new(i, &c.text, met, "code", &evidence)
        };
        let on = match k.site.as_deref().filter(|s| !s.is_empty()) {
            Some(s) => format!(" on {s}"),
            None => String::new(),
        };
        let not_met = |evidence: String| -> Result<ConditionResult> {
            Ok(self
                .declined(i, c, task_id)?
                .unwrap_or_else(|| result(Some(false), evidence)))
        };
        let expected = k.expected.clone().unwrap_or_else(|| "None".into());
        match k.kind.as_str() {
            "sent" => {
                let (kinds, aims) = self.aimed(c).expect("a send aims");
                if cards::ran(db, task_id, &kinds)?.iter().any(&aims) {
                    return Ok(result(Some(true), format!("FERB sent it to {expected}")));
                }
                return not_met(format!(
                    "FERB didn't send it; no send to {expected}{on} went through in this task"
                ));
            }
            "paid" => {
                let (_, aims) = self.aimed(c).expect("a payment aims");
                if cards::paid(db, policy, task_id)?.iter().any(&aims) {
                    return Ok(result(Some(true), format!("FERB paid{on} after your Yes")));
                }
                return not_met(format!(
                    "FERB didn't pay; no payment you said Yes to went through{on} in this task"
                ));
            }
            "submitted" => {
                let (kinds, aims) = self.aimed(c).expect("a submit aims");
                for call in cards::ran(db, task_id, &kinds)? {
                    if aims(&call) && self.submits(&call)? {
                        return Ok(result(
                            Some(true),
                            format!(
                                "FERB submitted the form on {}",
                                k.site.as_deref().unwrap_or("None")
                            ),
                        ));
                    }
                }
                return not_met(format!(
                    "FERB didn't submit it; no form on {} went through in this task",
                    k.site.as_deref().unwrap_or("None")
                ));
            }
            _ => {}
        }
        let want = match self.host.resolve(k.path.as_deref().unwrap_or(""), false) {
            Ok(p) => p,
            Err(PathError::Refused(e)) => {
                return Ok(result(
                    Some(false),
                    format!("{}: {e}", k.path.as_deref().unwrap_or("None")),
                ))
            }
            Err(PathError::Os(kind, e)) => {
                return Err(crate::sql::Error::Value(format!("{kind}: {e}")))
            }
        };
        if k.kind == "downloaded" {
            if cards::downloaded(db, task_id)?
                .iter()
                .any(|p| same(p, &want))
            {
                return Ok(result(Some(true), format!("FERB downloaded {want}")));
            }
            if ferb_fs::pypath::exists(&want) {
                return Ok(result(
                    None,
                    format!(
                        "{want} is there, but FERB has no record of downloading it in this task"
                    ),
                ));
            }
            return Ok(result(
                Some(false),
                format!("FERB didn't download it; {want} isn't there and no download of it went through in this task"),
            ));
        }
        let (kinds, aims) = self.aimed(c).expect("trash and delete aim");
        let done = cards::ran(db, task_id, &kinds)?.iter().any(&aims);
        let did = if k.kind == "trashed" {
            format!("moved {want} to the Trash")
        } else {
            format!("deleted {want} permanently")
        };
        if done {
            return Ok(result(Some(true), format!("FERB {did}")));
        }
        not_met(format!("FERB didn't do it; nothing in this task {did}"))
    }

    /// verify.py:185 `_submits` (ORCH-624).
    fn submits(&self, call: &Map<String, Value>) -> Result<bool> {
        Ok(cards::structural_kinds(call).contains("submit")
            || call.get("observed_change") == Some(&Value::Bool(true))
            || cards::approved(self.host.db().expect("store"), call)?)
    }

    /// verify.py:195 `_aimed`: an action check's call kinds and whether a call has its target.
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
            return Ok(Some(ConditionResult::new(
                i,
                &c.text,
                Some(false),
                DECLINED,
                DECLINED_SAID,
            )));
        }
        Ok(None)
    }
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
