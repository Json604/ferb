//! ORCH-719 item 6 (the owner's number) and ORCH-759: each list of open items the planner and the front door read from
//! live records, after the cached part (running tasks, tasks waiting on the owner, open questions and cards, delegated
//! conversations), shows its newest 15 and counts the rest, so a heavy user's list never grows the turn. Invented data.

use std::collections::BTreeSet;

use ferb_context::tokens::Tokenizer;
use ferb_supervisor::planner::{questions_block, task_state, OpenQuestion, TreeView, WaitingTask};
use ferb_supervisor::words::{FERB_CHECKS, OPEN_ITEMS};
use serde_json::{json, Map, Value};

fn tree(n: usize) -> TreeView {
    TreeView {
        task_id: format!("task-{n:02}"),
        mine: true,
        goal: format!("INVENTED-RUNNING-{n:02}"),
        runtime: "default".into(),
        origins: BTreeSet::from(["inferred".to_string()]),
        ..TreeView::default()
    }
}

fn waiting(n: usize) -> WaitingTask {
    WaitingTask {
        task_id: format!("wait-{n:02}"),
        goal: format!("INVENTED-WAITING-{n:02}"),
        status: "needs_user".into(),
        prior_task: None,
        origins: BTreeSet::from(["inferred".to_string()]),
    }
}

fn question(n: usize) -> OpenQuestion {
    OpenQuestion {
        id: format!("ap-{n:02}"),
        kind: "owner_question".into(),
        capability: "owner.ask".into(),
        reason: format!("INVENTED-QUESTION-{n:02}"),
        options: Vec::new(),
        task_label: String::new(),
        card_row: Map::new(),
    }
}

fn facts(text: &str) -> Value {
    let open = text.find('{').unwrap();
    let close = text.rfind('}').unwrap();
    serde_json::from_str(&text[open..=close]).unwrap()
}

// decision: ORCH-719, ORCH-759
#[test]
fn each_list_shows_its_newest_fifteen_and_counts_the_rest() {
    // running trees oldest first; waiting tasks newest first (as their reads give them)
    let trees: Vec<TreeView> = (0..20).map(tree).collect();
    let waits: Vec<WaitingTask> = (0..20).rev().map(waiting).collect();
    let block = task_state(&trees, &waits, Tokenizer::Utf8Bytes, &|t: &str| {
        t.to_string()
    })
    .expect("a tasks block");
    let f = facts(&block.block.text);
    let running = f["running"].as_array().unwrap();
    assert_eq!(running.len(), OPEN_ITEMS);
    assert_eq!(running[0]["goal"], "INVENTED-RUNNING-05");
    assert_eq!(running[OPEN_ITEMS - 1]["goal"], "INVENTED-RUNNING-19");
    assert_eq!(
        f["older_running_not_shown"],
        "5 (still running; chat.history include_tasks=true lists this chat's)"
    );
    let shown = f["waiting_on_the_owner_in_this_chat"].as_array().unwrap();
    assert_eq!(shown.len(), OPEN_ITEMS);
    assert_eq!(shown[0]["goal"], "INVENTED-WAITING-19");
    assert_eq!(
        f["older_waiting_not_shown"],
        "5 (chat.history include_tasks=true)"
    );
    // what is left out names no task
    assert!(!block.block.text.contains("INVENTED-RUNNING-04"));
    assert!(!block.block.text.contains("INVENTED-WAITING-04"));
    assert_eq!(block.block.source_refs.len(), 2 * OPEN_ITEMS);

    let pending: Vec<OpenQuestion> = (0..18).map(question).collect();
    let conversations: Vec<Value> = (0..17)
        .map(|n| json!({"id": format!("conv-{n:02}")}))
        .collect();
    let q = questions_block(&pending, &conversations, &FERB_CHECKS, &|t: &str| {
        t.to_string()
    });
    let f = facts(&q.block.text);
    let asked = f["questions"].as_array().unwrap();
    assert_eq!(asked.len(), OPEN_ITEMS);
    assert_eq!(asked[0]["approval_id"], "ap-03");
    assert_eq!(
        f["older_questions_not_shown"],
        "3 (each still waits on its card)"
    );
    let convs = f["delegated_conversations"].as_array().unwrap();
    assert_eq!(convs.len(), OPEN_ITEMS);
    assert_eq!(convs[0]["id"], "conv-02");
    assert_eq!(f["older_conversations_not_shown"], 2);
}

// decision: ORCH-759
#[test]
fn a_short_list_is_as_before() {
    let trees: Vec<TreeView> = (0..3).map(tree).collect();
    let block = task_state(&trees, &[waiting(1)], Tokenizer::Utf8Bytes, &|t: &str| {
        t.to_string()
    })
    .unwrap();
    let f = facts(&block.block.text);
    assert_eq!(f["running"].as_array().unwrap().len(), 3);
    assert!(f.get("older_running_not_shown").is_none());
    assert!(f.get("older_waiting_not_shown").is_none());
    let q = questions_block(&[question(1)], &[], &FERB_CHECKS, &|t: &str| t.to_string());
    let f = facts(&q.block.text);
    assert!(f.get("older_questions_not_shown").is_none());
    assert!(f.get("older_conversations_not_shown").is_none());
}
