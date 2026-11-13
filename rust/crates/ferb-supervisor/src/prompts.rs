//! A role's instructions (supervisor.py:1230-1258 `Supervisor._system`): the shared and role prompt files, the role
//! blocks a run needs, what is usable now, the facts about this FERB, the owner's Tier 1 profile and the skill and
//! ability catalogs. Byte for byte what the Python build assembles (rust/fixtures/catalog/prompt_static.json and every
//! system text of rust/fixtures/daemon/chat_scenarios.json).
//!
//! The prompt files are read as they are (ENG-3: runtime Markdown is executable input): `PromptRoot` points at the
//! package that holds `orchestrator/prompts`.

use ferb_context::prompts::{PromptError, PromptRoot};
use serde_json::Value;

/// supervisor.py:165 `TOOLLESS_ROLES`: roles that run without tools; their instructions name no catalog.
pub const TOOLLESS_ROLES: [&str; 2] = ["question.md", "attention.md"];
/// supervisor.py:166 `NO_CATALOG_ROLES`: those and the planner (ORCH-515: its lookups describe themselves).
pub const NO_CATALOG_ROLES: [&str; 3] = ["question.md", "attention.md", "supervisor.md"];

/// gateway.py:2846 `EFFECT_HELP`: the effect contract, once in the instructions (`{effects}`).
pub const EFFECT_HELP: &str = "The kind of change this action makes, declared truthfully. send: it delivers the \
owner's message, post or email to people or a public audience (a sign-in, search, filter or settings change is edit or \
navigate); pay: the final Pay or confirm control of a payment, or an action FERB says can pay, with the amount you \
expect to pay in destination; destroy: a permanent delete; account_security: changing a password, two-factor or \
recovery settings, or deleting the account; signing in with existing credentials is edit. FERB checks a declared \
send, payment, delete or account-security change like one and asks the owner when it must; a declaration never grants \
permission.";

/// gateway.py:2854 `DESTINATION_HELP`.
pub const DESTINATION_HELP: &str = "with effect send, pay, destroy or account_security, what the action reaches, as \
the page or app shows it (never only in why). recipients: the people a message or email goes to (none for a public \
post); account: the account it posts, sends or pays from, or whose security it changes; amount: what a payment pays, \
and currency: its currency as the page shows it (₹, $, INR); item: what a delete removes. FERB checks these against \
the owner's words and any standing permission; they never grant permission.";

/// supervisor.py:1226 `_approach`: the one folder the prompts are read from.
pub const APPROACH: &str = "orchestrator/prompts";

/// What `_system` reads from the rest of FERB at the moment it assembles (the six inputs prompt_static.json names).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemInputs {
    /// `self.facts()`: the facts that hold across turns.
    pub facts: String,
    /// `self.live_facts()`: what changes between turns (read only when `live`).
    pub live_facts: String,
    /// `self.availability().text`: what is usable right now (`{capabilities}`).
    pub capabilities: String,
    /// `self.identity_context()`: Tier 1, the owner's core profile.
    pub identity_context: String,
    /// `json.dumps(self.skills_catalog())`: see [`catalog_line`].
    pub skills_line: String,
    /// `json.dumps(self.abilities_catalog())`.
    pub abilities_line: String,
}

/// `json.dumps(catalog)` with Python's defaults (ASCII escapes, `", "` and `": "`). An integer beyond 64 bits can't
/// be held by `serde_json::Value`; no catalog FERB builds holds one (the ability catalog's fields are text, lists and
/// flags), so this is the only shape the port takes.
pub fn catalog_line(catalog: &[Value]) -> String {
    ferb_py::json::dumps(catalog)
}

/// supervisor.py:1230 `_system(name, live=..., roles=...)`. `live` false (the planner, ORCH-325) leaves the live facts
/// out of the cacheable instructions.
pub fn system(
    root: &PromptRoot,
    name: Option<&str>,
    live: bool,
    roles: &[&str],
    inputs: &SystemInputs,
) -> Result<String, PromptError> {
    let facts = [
        inputs.facts.as_str(),
        if live { inputs.live_facts.as_str() } else { "" },
    ]
    .iter()
    .filter(|part| !part.is_empty())
    .copied()
    .collect::<Vec<_>>()
    .join("\n");
    let mut text = root.load(&format!("{APPROACH}/interaction.md"), &[])?;
    if let Some(name) = name {
        text.push_str("\n\n");
        text.push_str(&root.load(&format!("{APPROACH}/{name}"), &[])?);
    }
    for role in roles {
        text.push_str("\n\n");
        text.push_str(&root.load(&format!("{APPROACH}/roles/{role}.md"), &[])?);
    }
    if text.contains("{capabilities}") {
        text = text.replace("{capabilities}", &inputs.capabilities);
    }
    if text.contains("{effects}") {
        text = text.replace(
            "{effects}",
            &format!("{EFFECT_HELP} Destination: {DESTINATION_HELP}"),
        );
    }
    if !facts.is_empty() {
        text.push_str(&format!("\n\nFacts about this FERB:\n{facts}\n"));
    }
    if !inputs.identity_context.is_empty() {
        text.push_str("\n\n");
        text.push_str(&inputs.identity_context);
    }
    if !name.is_some_and(|n| NO_CATALOG_ROLES.contains(&n)) {
        text.push_str(
            "\nOwner-written skills (read bodies with skills.read only when relevant):\n",
        );
        text.push_str(&inputs.skills_line);
        text.push_str(
            "\nBuilt-in FERB abilities (abilities.list reports availability; read a relevant procedure with \
             abilities.read; unavailable abilities cannot execute):\n",
        );
        text.push_str(&inputs.abilities_line);
    }
    Ok(text)
}

/// worker.py:409-424: the runtime-tools block every worker run ends with (`runtime_tools.md` with its tool names,
/// a blank line, then the time block). `tool_names` is the offered tools' names with `.` written `_`, or `none`.
pub fn runtime_tools_block(
    root: &PromptRoot,
    tool_names: &[String],
    time_block: &str,
) -> Result<String, PromptError> {
    let names = if tool_names.is_empty() {
        "none".to_string()
    } else {
        tool_names
            .iter()
            .map(|n| n.replace('.', "_"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    Ok(root.load(
        &format!("{APPROACH}/runtime_tools.md"),
        &[("tool_names", names.as_str())],
    )? + "\n\n"
        + time_block)
}
