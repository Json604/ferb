//! The literal checks of a task's success conditions (verify.py `Checks(self._urls_of, self.spans)`, compose.py:134):
//! a file exists or holds a text, a tab's URL, the task's last acting call read back, an action its own records show.
//! Code judges them; any other condition goes to the checker (V6). ferb-orchestrator's `Checks` over this daemon's
//! parts: paths resolved as the file tools resolve them (FERB's workspace, the owner's home), the browser drivers'
//! URLs of the task's tabs, the spans on a connection of its own.

use ferb_orchestrator::models::Condition;
use ferb_orchestrator::verify::{CheckHost, Checks, ConditionResult, PathError};
use serde_json::Value;

use super::tools::Parts;

struct Host {
    parts: Parts,
    db: ferb_store::Db,
    policy: super::gateway::PolicyWords,
}

/// Python's OSError subclass by errno.
fn os_class(errno: i32) -> &'static str {
    match errno {
        libc::ENOENT => "FileNotFoundError",
        libc::EACCES | libc::EPERM => "PermissionError",
        libc::EISDIR => "IsADirectoryError",
        libc::ENOTDIR => "NotADirectoryError",
        libc::EEXIST => "FileExistsError",
        _ => "OSError",
    }
}

impl CheckHost for Host {
    fn resolve(&self, path: &str, must_exist: bool) -> Result<String, PathError> {
        let workspace = self.parts.home().join("workspace");
        ferb_fs::files::resolve(
            path,
            &workspace.to_string_lossy(),
            &self.parts.user_home().to_string_lossy(),
            must_exist,
        )
        .map_err(|f| match f {
            ferb_fs::files::Fail::Refused(m) => PathError::Refused(m),
            ferb_fs::files::Fail::Os(e) => PathError::Os(os_class(e.errno).into(), e.to_string()),
            ferb_fs::files::Fail::Decode(at) => PathError::Os(
                "UnicodeDecodeError".into(),
                format!("'utf-8' codec can't decode byte at position {at}"),
            ),
            ferb_fs::files::Fail::Raised(m) => PathError::Os("RuntimeError".into(), m),
        })
    }
    fn urls_of(&self, task_id: &str) -> Vec<String> {
        [self.parts.owner_driver(), self.parts.ferb_driver()]
            .into_iter()
            .flatten()
            .flat_map(|d| d.urls_of(task_id))
            .collect()
    }
    fn latest_acting_call(
        &self,
        task_id: &str,
        tool: Option<&str>,
    ) -> ferb_orchestrator::sql::Result<Option<Value>> {
        let r = self.parts.redactor();
        let spans = ferb_store::spans::SpanStore::new(
            &self.db,
            Box::new(ferb_gateway::gateway::StoreTime(self.parts.clock())),
            Box::new(ferb_gateway::gateway::StoreIds(self.parts.ids())),
            Box::new(move |v: &Value| r.value(v)),
            Box::new(|kind: &str| ferb_policy::rules::REVERSIBLE.contains(&kind)),
        );
        spans
            .latest_acting_call(task_id, tool)
            .map_err(|e| ferb_orchestrator::sql::Error::Value(e.to_string()))
    }
    fn policy(&self) -> &dyn ferb_orchestrator::ports::Policy {
        &self.policy
    }
    fn db(&self) -> Option<&ferb_store::Db> {
        Some(&self.db)
    }
}

fn host(parts: &Parts) -> Option<Host> {
    let db = ferb_store::Db::open(parts.home().join("ferb.db")).ok()?;
    Some(Host {
        parts: parts.clone(),
        db,
        policy: super::gateway::PolicyWords,
    })
}

/// `checks.code_checkable(condition, task_id)`.
pub fn checkable(parts: &Parts, condition: &Condition, task_id: &str) -> bool {
    match host(parts) {
        Some(h) => Checks { host: &h }.code_checkable(condition, Some(task_id)),
        None => false,
    }
}

/// `checks.run(i, condition, task_id)`: code's verdict, with its evidence. A store error is an unknown verdict that
/// says so.
pub fn run(parts: &Parts, index: i64, condition: &Condition, task_id: &str) -> ConditionResult {
    let Some(h) = host(parts) else {
        return ConditionResult::new(
            index,
            &condition.text,
            None,
            "code",
            "the literal check couldn't read FERB's store",
        );
    };
    match (Checks { host: &h }).run(index, condition, task_id) {
        Ok(r) => r,
        Err(e) => ConditionResult::new(
            index,
            &condition.text,
            None,
            "code",
            &format!("the literal check couldn't run: {e}"),
        ),
    }
}

/// verify.py:224 `Checks.declined`: a condition the owner declined (its question answered No) is unmet by code, with
/// that No as its evidence; None when nothing was declined or the store can't be read.
pub fn declined(
    parts: &Parts,
    index: i64,
    condition: &Condition,
    task_id: &str,
) -> Option<ConditionResult> {
    let h = host(parts)?;
    Checks { host: &h }
        .declined(index, condition, task_id)
        .ok()
        .flatten()
}
