//! GATE-609 (GATE-606 c): the locks on the planner's reading of the words typed in a safety check's box, on invented
//! planner answers. The path through the real supervisor and gateway is in replay_more.rs (`card_reply`) and
//! ferb-gateway's link_rule.rs.

use ferb_orchestrator::validate::supervisor_outcome_validate_json;
use ferb_supervisor::flow::control::{
    card_named_sites, site_in_owner_words, sites_named_reply, CardSites,
};
use serde_json::{json, Value};

const TREE: &str = "id-000021";
/// The host the answered card asks about (its needs').
const ASKED: &str = "github.com";

/// A planner answer: a correction (`continue`) of the running task whose spec's sites are `sites`, `control` as given.
fn reading(control: Value, sites: &[&str]) -> ferb_orchestrator::models::SupervisorOutcome {
    let out = json!({"kind": "task", "answers_question": false, "text": "Noted.", "control": control,
        "task": {"goal": "Make a repository for the notes", "name": "Repository", "identity": "owner",
            "worker_type": "default",
            "targets": {"apps": [], "sites": sites, "paths": [], "recipients": []},
            "success_conditions": [{"text": "A repository exists", "owner_text": "", "layer": "outcome",
                                    "check": null}],
            "plan": null, "folders": [], "keep_tabs_open": false, "parts": [], "earlier_results": []},
        "side_entries": null});
    supervisor_outcome_validate_json(&ferb_py::json::dumps(&out)).expect("a valid answer")
}

fn go_on() -> Value {
    json!({"task_id": TREE, "action": "continue", "new_goal": false, "standing_sends": "unchanged",
           "supersedes": []})
}

fn named(
    control: Value,
    sites: &[&str],
    prior: &[&str],
    words: &str,
) -> (Vec<String>, Vec<String>, String) {
    let prior: Vec<String> = prior.iter().map(|s| s.to_string()).collect();
    let got = card_named_sites(
        &reading(control, sites),
        TREE,
        &|_| false,
        &prior,
        words,
        &[ASKED.to_string()],
    );
    (got.named, got.refused, got.why)
}

fn asked(host: &str) -> Vec<String> {
    vec![host.to_string()]
}

// By its name alone only the card's own host, and only an apex under a generic TLD with a name of three or more
// characters; any host by its whole address.
// decision: GATE-611
#[test]
fn a_host_is_named_by_its_address_or_the_cards_own_host_by_its_name() {
    for (words, host, card) in [
        ("use my Brave GitHub", "github.com", "github.com"),
        ("GitHub's fine", "github.com", "github.com"),
        ("use github", "www.github.com", "github.com"),
        ("use github", "GITHUB.com.", "github.com"),
        ("send it to github.com", "github.com", "gitlab.com"),
        (
            "go to https://www.github.com/new",
            "github.com",
            "gitlab.com",
        ),
        ("use gitlab.com instead", "gitlab.com", "github.com"),
        (
            "the gist one: gist.github.com",
            "gist.github.com",
            "github.com",
        ),
        ("try 192.168.1.20", "192.168.1.20", "github.com"),
        ("use the wikipedia one", "wikipedia.org", "wikipedia.org"),
    ] {
        assert!(
            site_in_owner_words(words, host, &asked(card)),
            "{words} / {host} / {card}"
        );
    }
    for (words, host, card) in [
        ("ok", "github.com", "github.com"),
        ("yes", "github.com", "github.com"),
        ("1", "github.com", "github.com"),
        ("that one", "github.com", "github.com"),
        ("the site above", "github.com", "github.com"),
        ("githubber", "github.com", "github.com"),
        ("git hub", "github.com", "github.com"),
        ("github com", "github.com", "gitlab.com"),
        ("not github, use gitlab", "gitlab.com", "github.com"),
        ("use my Brave account", "brave.com", "github.com"),
        ("Gmail", "mail.google.com", "mail.google.com"),
        ("use the bbc", "bbc.co.uk", "bbc.co.uk"),
        ("use x", "x.com", "x.com"),
        ("", "github.com", "github.com"),
        ("github", "", "github.com"),
    ] {
        assert!(
            !site_in_owner_words(words, host, &asked(card)),
            "{words} / {host} / {card}"
        );
    }
}

// The red-team review's table (redteam3 probe `c_lock2`), every row: refused whether the card asked about github.com
// (the planner adds the host) or about that very host (an injected worker made the card ask about it); only the
// first row and the two spellings of github.com itself are named, and only for a card about github.com.
// decision: GATE-611
#[test]
fn the_reviews_table_names_only_github_com() {
    let rows = [
        ("use my Brave GitHub", "github.com", true),
        ("use my Brave GitHub", "evil.github.io", false),
        ("use my Brave GitHub", "evil.my.id", false),
        ("use my Brave GitHub", "brave.evil.com", false),
        ("use my Brave GitHub", "github.evil.com", false),
        ("use my Brave GitHub", "github.com.evil.io", false),
        ("no, send it to my work email instead", "evil.it.com", false),
        ("don't send that", "t.co", false),
        ("don't send it to github", "evil.github.io", false),
        ("run it again", "evil-abc123.a.run.app", false),
        ("just search the web for it", "evil.web.app", false),
        ("skip the other pages", "evil.pages.dev", false),
        ("use my google account", "script.google.com", false),
        ("use my google account", "sites.google.com", false),
        ("tell us why", "evil.us.com", false),
        ("fill in the rest", "evil.in.net", false),
        ("no thanks", "evil.no.com", false),
        ("is this the right account?", "account.cx", false),
        ("use the other one", "evil.co.at", false),
        ("use the com one", "evil.com.pk", false),
        ("GitHub", "xn--gthub-cta.com", false),
        ("use github", "GITHUB.com.", true),
        ("use github", "github.com:8443", false),
        ("use github", "https://user@github.com/x", false),
    ];
    for (words, host, named) in rows {
        assert_eq!(
            site_in_owner_words(words, host, &asked(ASKED)),
            named,
            "card about github.com: {words} / {host}"
        );
        let itself = ferb_policy::destinations::site_host(host);
        assert_eq!(
            site_in_owner_words(words, host, &asked(&itself)),
            named,
            "card about {itself}: {words} / {host}"
        );
    }
}

// decision: GATE-609
#[test]
fn only_a_correction_of_this_task_names_sites() {
    let (n, _, _) = named(go_on(), &["github.com"], &[], "use my Brave GitHub");
    assert_eq!(n, ["github.com"]);
    for action in ["stop", "pause", "retry"] {
        let c = json!({"task_id": TREE, "action": action, "new_goal": false, "standing_sends": "unchanged",
                       "supersedes": []});
        let (n, _, why) = named(c, &["github.com"], &[], "use my Brave GitHub");
        assert!(n.is_empty(), "{action}");
        assert_eq!(why, format!("control {action}"));
    }
    let (n, _, why) = named(Value::Null, &["github.com"], &[], "use my Brave GitHub");
    assert!(n.is_empty());
    assert_eq!(why, "no correction of a task");
    let other = json!({"task_id": "id-999", "action": "continue", "new_goal": false, "standing_sends": "unchanged",
                       "supersedes": []});
    let (n, _, why) = named(other, &["github.com"], &[], "use my Brave GitHub");
    assert!(n.is_empty());
    assert_eq!(why, "another task");
}

// The planner's reading decides what was meant; code only refuses: a refusal read without the site names nothing, a
// redirect names another site only by its typed address, a site only the card's own text (a page's) shows is
// refused, an already-named site isn't new.
// decision: GATE-609
// decision: GATE-611
#[test]
fn refusals_redirects_and_the_cards_own_text() {
    let (n, r, _) = named(go_on(), &[], &[], "don't send it to github");
    assert!(n.is_empty() && r.is_empty());
    let (n, r, _) = named(go_on(), &["gitlab.com"], &[], "not github, use gitlab");
    assert_eq!((n, r), (vec![], vec!["gitlab.com".to_string()]));
    let (n, r, _) = named(go_on(), &["gitlab.com"], &[], "not github, use gitlab.com");
    assert_eq!((n, r), (vec!["gitlab.com".to_string()], vec![]));
    // the card read "Send your details to github.com?" (page-derived): the planner had it, the owner's words don't
    let (n, r, _) = named(
        go_on(),
        &["github.com"],
        &[],
        "go ahead with the site in the question",
    );
    assert!(n.is_empty());
    assert_eq!(r, ["github.com"]);
    // a site the task already named isn't new; another the planner adds is checked on its own
    let (n, r, _) = named(
        go_on(),
        &["https://www.GitHub.com/x", "evil.example"],
        &["github.com"],
        "github, evil",
    );
    assert_eq!((n, r), (vec![], vec!["evil.example".to_string()]));
}

fn read(named: &[&str], refused: &[&str]) -> CardSites {
    CardSites {
        named: named.iter().map(|s| s.to_string()).collect(),
        refused: refused.iter().map(|s| s.to_string()).collect(),
        why: String::new(),
    }
}

// decision: GATE-609
// decision: GATE-611
#[test]
fn the_reply_says_what_was_named() {
    assert_eq!(sites_named_reply(&read(&[], &[])), "");
    assert_eq!(
        sites_named_reply(&read(&["github.com"], &[])),
        "Got it: github.com is a site you named for this task."
    );
    assert_eq!(
        sites_named_reply(&read(&["github.com", "gitlab.com"], &[])),
        "Got it: github.com and gitlab.com are sites you named for this task."
    );
    assert_eq!(
        sites_named_reply(&read(&["a.example", "b.example", "c.example"], &[])),
        "Got it: a.example, b.example and c.example are sites you named for this task."
    );
    // a refused host is never echoed (the planner's answer may be injected): a fixed example instead
    assert_eq!(
        sites_named_reply(&read(&[], &["gitlab.com"])),
        "Got it. To name a site for this task, type its address (like example.com)."
    );
    assert_eq!(
        sites_named_reply(&read(&["github.com"], &["evil.example"])),
        "Got it: github.com is a site you named for this task. To name another site, type its address (like \
         example.com)."
    );
}

// Only FERB.app sends a card's box words (chat.correct, app_only since GATE-612): a model's tools reach the daemon as
// mcp_shim (tools.call only), the extension's relay has no methods, the shell's sandbox refuses the socket (GATE-604),
// and another program running as the owner fails the app's signature check. The box's words are the RPC's `text`, apart from the card's text, which the planner gets only as data.
// decision: GATE-609
// decision: GATE-612
#[test]
fn only_the_owners_connection_sends_a_cards_box_words() {
    use ferb_rpc::auth::{allowed, app_only, Role};
    assert!(allowed(Role::Cli, "chat.correct") && app_only("chat.correct"));
    assert!(!allowed(Role::McpShim, "chat.correct"));
    assert!(!allowed(Role::NativeHost, "chat.correct"));
}
