//! The hourly profile check (MEM-484): a profile over its size is due again only when what the pass reads changed
//! since the last pass. Live (2026-11-17..25) the pass resent one byte-identical 20.7k-character input 38 times,
//! because `due` held whenever anything was left out of the rendered profile. Through the real memory items and core
//! profile with a stand-in model; temporary folders only; time is injected.

mod support;

use serde_json::{json, Value};

use ferb_learning::profile::{INPUT_KEY, PASS_KIND};
use ferb_learning::{py, sql};
use support::*;

const HOUR: i64 = 3_600_000_000;

fn later(env: &Env, hours: i64) {
    let now = *env.clock.0.borrow();
    *env.clock.0.borrow_mut() = py::shift(&now, hours * HOUR);
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn note(env: &Env, n: usize) -> String {
    env.memory
        .save(
            "fact",
            "owner",
            &format!("note {n}"),
            &format!("owner note number {n} {}", "word ".repeat(200)),
            "owner_stated",
            &[("journal".into(), "j-1".into())],
            0.7,
            &[],
            "",
            None,
        )
        .unwrap()
        .item
        .id
}

fn model_calls(env: &Env) -> usize {
    env.events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e[0] == "model.start")
        .count()
}

/// One pass whose stand-in answer places every eligible fact in one section (more than fits), as a model would.
fn pass(env: &Env, runtime: &tokio::runtime::Runtime) {
    let ids: Vec<String> = env
        .profile
        .eligible()
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    env.answers.borrow_mut().push_back(
        json!({"answer": {"sections": [{"title": "About you", "items": ids}], "ended": []}}),
    );
    runtime
        .block_on(op(env, "profile.run_pass", &json!({})))
        .unwrap();
}

// decision: MEM-484
#[test]
fn an_unchanged_input_never_reruns_the_pass_and_a_change_does() {
    let env = Env::new("profile_due", None);
    *env.clock.0.borrow_mut() = py::parse("2026-11-20T09:00:00+00:00").unwrap();
    let runtime = rt();
    for n in 0..14 {
        note(&env, n);
    }
    assert!(env.profile.due().unwrap(), "never passed: due");
    pass(&env, &runtime);
    assert!(
        env.profile.render().unwrap().1 > 0,
        "the profile is over its size, as the owner's was"
    );
    assert_eq!(model_calls(&env), 1);

    // every hourly check for two days: what the pass reads is the same, so no call (Python's rule made one an hour)
    for _ in 0..47 {
        later(&env, 1);
        assert!(
            !env.profile.due().unwrap(),
            "an unchanged input is never due"
        );
    }
    assert_eq!(model_calls(&env), 1);

    // a new fact changes what the pass reads: due at the next hourly check
    note(&env, 14);
    later(&env, 1);
    assert!(env.profile.due().unwrap(), "a changed input is due");
    pass(&env, &runtime);
    later(&env, 1);
    assert!(!env.profile.due().unwrap(), "and settles again after it");
    assert_eq!(model_calls(&env), 2);

    // the weekly pass still runs whatever the input
    later(&env, 7 * 24);
    assert!(env.profile.due().unwrap(), "the weekly pass");
}

// decision: MEM-484
#[test]
fn a_pass_row_from_an_older_build_is_due_once_then_settles() {
    let env = Env::new("profile_due_old_row", None);
    *env.clock.0.borrow_mut() = py::parse("2026-11-20T09:00:00+00:00").unwrap();
    let runtime = rt();
    for n in 0..14 {
        note(&env, n);
    }
    pass(&env, &runtime);
    let last = |env: &Env| -> Value {
        let row = env
            .store
            .one(
                "SELECT data_json FROM journal WHERE kind = ? ORDER BY seq DESC LIMIT 1",
                sql![PASS_KIND],
            )
            .unwrap()
            .unwrap();
        serde_json::from_str(&row.s("data_json")).unwrap()
    };
    let mut data = last(&env);
    assert!(data.get(INPUT_KEY).is_some_and(Value::is_string));
    // a later pass row as a build before MEM-484 wrote it: no input key
    data.as_object_mut().unwrap().remove(INPUT_KEY);
    runtime
        .block_on(op(
            &env,
            "journal.append",
            &json!({"kind": PASS_KIND, "data": data, "origin": "ferb"}),
        ))
        .unwrap();
    assert!(last(&env).get(INPUT_KEY).is_none());
    later(&env, 2);
    assert!(env.profile.due().unwrap(), "no recorded input: due once");
    pass(&env, &runtime);
    later(&env, 2);
    assert!(!env.profile.due().unwrap());
}
