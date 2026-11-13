//! policy/standing.py, the parts that need no store (D30): a scope's concrete bounds (`SendScope`, from
//! orchestrator/plans.py), whether the owner named every destination of one, and whether a call is a send a scope
//! may cover. The scopes themselves are stored by `stores::StandingPermissions` (Mac-side).

use ferb_core::types::{ActionSpec, Target};
use serde::{Deserialize, Serialize};

use crate::confirm::SEND_CLASS;
use crate::provenance;
use crate::rules::{declared_kinds, GENERIC_INPUT};

/// A schedule's runs hold its recorded scopes under `schedule:<schedule id>`.
pub const SCHEDULE_GRANT: &str = "schedule:";

/// orchestrator/plans.py:96 `SendScope`: concrete bounds of a standing send permission (D30).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendScope {
    pub capability: String,
    pub identity: String,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub site: Option<String>,
    #[serde(default)]
    pub recipients: Vec<String>,
    #[serde(default = "empty_content")]
    pub content: Option<String>,
}

fn empty_content() -> Option<String> {
    Some(String::new())
}

impl SendScope {
    /// `model_dump_json()`.
    pub fn dump_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// `standing.owner_named`: every destination of a scope appears in the owner's own words (S3).
pub fn owner_named(scope: &SendScope, owner_text: &str) -> bool {
    let target = Target {
        account: scope.account.clone(),
        site: scope.site.clone(),
        recipients: scope.recipients.clone(),
        ..Target::default()
    };
    let values = provenance::arg_sources(&target, owner_text, &[], "", "");
    !values.is_empty()
        && values
            .iter()
            .all(|(_, found)| found.iter().any(|f| f == "owner_words"))
}

/// `StandingPermissions.sends`: whether a call is a send a scope may cover.
pub fn sends(spec: &ActionSpec) -> bool {
    let declared = declared_kinds(spec);
    if spec.target.amount.is_some()
        || spec.target.contact.is_some()
        || declared.iter().any(|k| !SEND_CLASS.contains(&k.as_str()))
    {
        return false;
    }
    SEND_CLASS.contains(&spec.kind.as_str())
        || (GENERIC_INPUT.contains(&spec.kind.as_str())
            && declared.iter().any(|k| SEND_CLASS.contains(&k.as_str())))
}
