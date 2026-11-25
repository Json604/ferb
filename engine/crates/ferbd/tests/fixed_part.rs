//! ORCH-721 (ORCH-719 step 1): each role's fixed part, what every one of its model calls carries before any work,
//! stays under a ceiling. The fixed part is FERB's own instructions for the role (its prompt files with the effect
//! help, no live facts, profile or catalogs: those are measured by `ferb stats`) plus the tool descriptions the role is
//! offered, as `json.dumps` writes the listing the real program gives the role's grant (the frozen catalog's grants,
//! engine/fixtures/catalog/tools.json).
//!
//! The ceilings are guards set at the sizes measured on 2026-11-25 plus about 5%, not the owner's numbers: growing a
//! role's fixed part past one is a deliberate change made in the same commit as the code that grows it (ORCH-719
//! step 5 brings the owner's tool counts: a worker at most 25 full descriptions, a checker 10).

mod support;

use ferb_context::prompts::PromptRoot;
use ferb_supervisor::prompts::{system, SystemInputs};
use serde_json::{json, Value};
use support::*;

/// (role, its prompt file, live facts in its instructions, the catalog grant it is listed with, ceiling in chars).
/// Measured 2026-11-25: planner 35,620 + 6 tools 10,096 = 45,716; worker 14,773 + 81 tools 115,979 = 130,752; checker
/// 17,436 + 39 tools 41,151 = 58,587.
const ROLES: [(&str, &str, bool, &str, usize); 3] = [
    ("planner", "supervisor.md", false, "planner_lookups", 48_000),
    (
        "worker",
        "worker.md",
        true,
        "worker_owner_all_scopes",
        137_500,
    ),
    (
        "checker",
        "verifier.md",
        true,
        "checker_owner_all_scopes",
        61_500,
    ),
];

// decision: ORCH-721
#[test]
fn each_roles_fixed_part_stays_under_its_ceiling() {
    let catalog = fixture("catalog/tools.json");
    let prompts = PromptRoot::new(repo().join("engine/runtime")).unwrap();
    let tmp = root();
    let run = Run::start(tmp.path(), "01", json!({}));
    run.control(json!({"do": "attach_identity"}));
    let mut over = Vec::new();
    let mut measured = Vec::new();
    for (role, file, live, grant, ceiling) in ROLES {
        let instructions =
            system(&prompts, Some(file), live, &[], &SystemInputs::default()).unwrap();
        let g = &catalog["listings"][grant]["grant"];
        let listing: Value = run.control(
            json!({"do": "listing", "identity": g["identity"], "scopes": g["scopes"],
            "observe_only": g["observe_only"], "tools": g["tools"], "max_calls": g["max_calls"]}),
        );
        let tools = listing.as_array().map_or(0, Vec::len);
        let schema_chars = ferb_py::json::dumps(&listing).chars().count();
        let total = instructions.chars().count() + schema_chars;
        measured.push(format!(
            "{role}: instructions {} + {tools} tools {schema_chars} = {total}",
            instructions.chars().count()
        ));
        if total > ceiling {
            over.push(format!("{role}: {total} chars, ceiling {ceiling}"));
        }
    }
    eprintln!("{}", measured.join("\n"));
    assert!(
        over.is_empty(),
        "a fixed part grew past its ceiling: {over:?}\n{}",
        measured.join("\n")
    );
}
