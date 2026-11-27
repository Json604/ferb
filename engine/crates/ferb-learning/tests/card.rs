//! The card (front door step 8; ORCH-719 item 7, MEM-480, MEM-489): the always-sent profile holds only lines the owner
//! stated or approved and observed facts (MEM-480), never an imported line; code renders each with one short label and a short handle in place of its
//! 32-hex id, within 1,500 tokens (at 4 characters a token, as the profile counts). The handle names the item for a
//! forget or a lookup. Through the real memory items and core profile; temporary folders, injected time, invented
//! data.

mod support;

use ferb_learning::profile::{from_handle, handle, CARD_HEADER, CARD_TOKENS, CHARS_PER_TOKEN};
use ferb_learning::sql;
use support::*;

fn save(env: &Env, kind: &str, key: &str, content: &str, origin: &str) -> String {
    env.memory
        .save(
            kind,
            "owner",
            key,
            content,
            origin,
            &[("journal".into(), format!("j-{key}"))],
            0.9,
            &[],
            "",
            Some(origin == "owner_stated"),
        )
        .unwrap()
        .item
        .id
}

fn row(env: &Env, id: &str, by: &str, position: i64) {
    env.store
        .exec(
            "INSERT OR REPLACE INTO memory_profile (item_id, section, position, added_by, added_at) VALUES (?, ?, ?, \
             ?, '2026-10-20T09:00:00+00:00')",
            sql![id, "About you", position, by],
        )
        .unwrap();
}

// decision: ORCH-719
// decision: MEM-480, MEM-490
// decision: MEM-489
#[test]
fn the_card_holds_what_he_said_approved_or_was_observed_with_a_handle_for_each() {
    let env = Env::new("card", None);
    env.profile.card.set(true);
    let said = save(
        &env,
        "fact",
        "legal_name",
        "INVENTED-SAID my name is Sam",
        "owner_stated",
    );
    let approved = save(
        &env,
        "preference",
        "tea",
        "INVENTED-APPROVED prefers tea",
        "inferred",
    );
    // an observed fact (the Mac scan's), written as the scan writes its row: in the profile without his Yes
    let observed = "0123456789abcdef0123456789obsrvd".to_owned();
    env.store
        .exec(
            "INSERT INTO memory_items (id, kind, subject, key, content, origin, status, confidence, observed_at, \
             valid_from, created_at) VALUES (?, 'env', 'owner', 'browser', 'INVENTED-OBSERVED uses an invented \
             browser', 'observed', 'active', 1.0, '2026-10-20T09:00:00+00:00', '2026-10-20T09:00:00+00:00', \
             '2026-10-20T09:00:00+00:00')",
            sql![observed],
        )
        .unwrap();
    row(&env, &said, "code", 0);
    row(&env, &approved, "owner", 1);
    row(&env, &observed, "code", 2);
    let text = env.profile.prompt_text().unwrap();
    assert!(text.starts_with(CARD_HEADER), "{text}");
    assert!(
        text.contains(&format!(
            "- INVENTED-SAID my name is Sam (his words) [{}]",
            handle(&said)
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "- INVENTED-APPROVED prefers tea (approved) [{}]",
            handle(&approved)
        )),
        "{text}"
    );
    // MEM-480 (owner): an observed fact stays always on
    assert!(
        text.contains(&format!(
            "- INVENTED-OBSERVED uses an invented browser (observed) [{}]",
            handle(&observed)
        )),
        "{text}"
    );
    // FERB's own understanding without his Yes is not on the card
    let unasked = save(
        &env,
        "preference",
        "coffee",
        "INVENTED-UNASKED prefers coffee",
        "inferred",
    );
    row(&env, &unasked, "code", 3);
    env.profile.joins_by_itself.set(true);
    let joined = env.profile.prompt_text().unwrap();
    env.profile.joins_by_itself.set(false);
    assert!(!joined.contains("INVENTED-UNASKED"), "{joined}");
    for id in [&said, &approved, &observed] {
        assert!(!text.contains(id.as_str()), "no 32-hex id: {text}");
    }
    // the handle names the item for a forget or a lookup; anything else names nothing
    assert_eq!(
        from_handle(&env.store, &handle(&said)).unwrap(),
        Some(said.clone())
    );
    assert_eq!(
        from_handle(&env.store, &format!("[{}]", handle(&approved))).unwrap(),
        Some(approved.clone())
    );
    assert_eq!(from_handle(&env.store, &said).unwrap(), None);
    assert_eq!(from_handle(&env.store, "m:nothere1").unwrap(), None);
    // a Python fixture's replay keeps the profile's own lines
    env.profile.card.set(false);
    let before = env.profile.prompt_text().unwrap();
    assert!(
        before.contains(&said) && before.contains("INVENTED-OBSERVED"),
        "{before}"
    );
}

// decision: ORCH-719
#[test]
fn the_card_stays_within_its_1500_tokens() {
    let env = Env::new("card-limit", None);
    env.profile.card.set(true);
    for i in 0..200 {
        let id = save(
            &env,
            "fact",
            &format!("invented_fact_{i}"),
            &format!("INVENTED-FACT-{i:03} the owner said something long enough to count for the card's limit"),
            "owner_stated",
        );
        row(&env, &id, "code", i);
    }
    let text = env.profile.prompt_text().unwrap();
    let chars = text.chars().count() as i64;
    eprintln!("the card with 200 lines offered: {chars} characters");
    assert!(
        chars <= CARD_TOKENS * CHARS_PER_TOKEN + 200,
        "{chars} characters"
    );
    assert!(text.contains("INVENTED-FACT-000"));
    assert!(!text.contains("INVENTED-FACT-199"), "the rest are left out");
    assert!(
        text.contains("left out to stay within its budget; memory.read finds them"),
        "{text}"
    );
}
