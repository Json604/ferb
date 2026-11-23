//! App chats read from stand-in Accessibility trees shaped like the desktop chat apps' (bubbles on the right for the
//! owner, a full-width column for the app, an agent pane with both on the left, an app that labels its owner's
//! messages): who said what from the layout first, FERB's own text only confirming; replying versus finished by code.

use ferb_sessions::appchat::*;

fn text(x: f64, y: f64, w: f64, words: &str) -> Node {
    Node {
        role: "AXStaticText".into(),
        value: words.into(),
        x,
        y,
        w,
        h: 18.0,
        enabled: true,
        ..Node::default()
    }
}

fn input(y: f64, focused: bool) -> Node {
    Node {
        role: "AXTextArea".into(),
        label: "Prompt".into(),
        x: 40.0,
        y,
        w: 600.0,
        h: 40.0,
        enabled: true,
        focused,
        editable: true,
        ..Node::default()
    }
}

fn button(label: &str) -> Node {
    Node {
        role: "AXButton".into(),
        label: label.into(),
        x: 650.0,
        y: 900.0,
        w: 30.0,
        h: 30.0,
        enabled: true,
        ..Node::default()
    }
}

fn claude() -> AppFacts {
    facts_for("com.anthropic.claudefordesktop", "Claude", &[])
}

/// A chat with the owner's bubbles on the right, typed by him in the app (FERB sent nothing).
fn bubbles() -> Vec<Node> {
    vec![
        Node {
            role: "AXButton".into(),
            label: "New chat".into(),
            x: 10.0,
            y: 10.0,
            w: 80.0,
            h: 20.0,
            ..Node::default()
        },
        text(400.0, 100.0, 300.0, "Can you fix the login test?"),
        text(
            40.0,
            150.0,
            660.0,
            "Sure. The test waits on a timer that never fires.",
        ),
        text(
            40.0,
            170.0,
            640.0,
            "I changed it to wait for the request instead.",
        ),
        text(450.0, 230.0, 250.0, "Thanks, run it again"),
        text(40.0, 280.0, 660.0, "All 12 tests pass."),
        Node {
            composer: true,
            ..input(880.0, false)
        },
        input(300.0, false), // a search box higher up: not the composer
        list(),
    ]
}

/// The message list's container above the composer.
fn list() -> Node {
    Node {
        role: "AXScrollArea".into(),
        x: 40.0,
        y: 90.0,
        w: 660.0,
        h: 300.0,
        enabled: true,
        ..Node::default()
    }
}

// decision: HAND-670
#[test]
fn the_owner_s_own_messages_are_his_by_the_app_s_layout_with_nothing_sent_by_ferb() {
    let look = read(&bubbles(), &claude(), &[]);
    let said: Vec<(Speaker, &str)> = look
        .blocks
        .iter()
        .map(|b| (b.speaker, b.text.as_str()))
        .collect();
    assert_eq!(
        said,
        [
            (Speaker::Owner, "Can you fix the login test?"),
            (Speaker::App, "Sure. The test waits on a timer that never fires.\nI changed it to wait for the request instead."),
            (Speaker::Owner, "Thanks, run it again"),
            (Speaker::App, "All 12 tests pass."),
        ]
    );
    // the composer is the one the proof marked, never the search box
    assert_eq!(look.composer, Some(6));
    assert!(!look.stop_control);
}

// decision: HAND-670
#[test]
fn an_app_with_no_side_of_its_own_is_read_by_alignment_and_labels_it_shows() {
    // an added app the table doesn't know: alignment within the column decides
    let generic = facts_for("com.example.chat", "Some Chat", &[]);
    let look = read(&bubbles(), &generic, &[]);
    assert_eq!(look.blocks[0].speaker, Speaker::Owner);
    assert_eq!(look.blocks[1].speaker, Speaker::App);
    // an agent pane with every message on the left: the app's own label marks the owner's (data)
    let pane = AppFacts {
        owner_labels: vec!["You".into()],
        owner_side: Side::Layout,
        ..generic.clone()
    };
    let nodes = vec![
        text(20.0, 100.0, 40.0, "You"),
        text(20.0, 120.0, 300.0, "rename the module"),
        text(20.0, 160.0, 320.0, "Renamed it and updated 4 imports."),
        Node {
            composer: true,
            x: 20.0,
            w: 340.0,
            ..input(600.0, true)
        },
        list(),
    ];
    let look = read(&nodes, &pane, &[]);
    let said: Vec<Speaker> = look.blocks.iter().map(|b| b.speaker).collect();
    assert_eq!(said, [Speaker::Owner, Speaker::App]);
    // without the label, FERB's own sent text confirms the owner's message
    let plain = AppFacts {
        owner_labels: vec![],
        ..pane
    };
    let look = read(&nodes[1..], &plain, &["rename the module".into()]);
    assert_eq!(look.blocks[0].speaker, Speaker::Owner);
}

// decision: HAND-671, HAND-674
#[test]
fn replying_until_the_stop_control_has_been_gone_3_s_or_a_chat_without_one_holds_still_10_s() {
    assert_eq!((STOP_GONE_S, STILL_S), (3.0, 10.0));
    assert_eq!(
        (LOOK_REPLYING_S, LOOK_IDLE_S, FOREGROUND_LOOK_S),
        (2.0, 10.0, 30.0)
    );
    let facts = claude();
    let mut w = Watch::default();
    let mut nodes = bubbles();
    // what was there before FERB looked counts as settled
    assert_eq!(
        w.see(&read(&nodes, &facts, &[]), true, 0.0),
        State::Finished
    );
    // a new reply streams in with the stop control up
    nodes.push(text(40.0, 330.0, 300.0, "Running"));
    nodes.push(button("Stop response"));
    assert_eq!(
        w.see(&read(&nodes, &facts, &[]), true, 2.0),
        State::Replying
    );
    let last = nodes.len() - 2;
    nodes[last].value = "Running the suite again now".into();
    // the stop control stays up long after the text stopped changing: still replying
    assert_eq!(
        w.see(&read(&nodes, &facts, &[]), true, 20.0),
        State::Replying
    );
    // it goes: replying until it has been gone STOP_GONE_S, then finished
    nodes.pop();
    assert_eq!(
        w.see(&read(&nodes, &facts, &[]), true, 22.0),
        State::Replying
    );
    assert_eq!(
        w.see(&read(&nodes, &facts, &[]), true, 20.0 + STOP_GONE_S - 0.1),
        State::Replying
    );
    assert_eq!(
        w.see(&read(&nodes, &facts, &[]), true, 20.0 + STOP_GONE_S + 0.1),
        State::Finished
    );

    // an app with no stop control of its own (generic): finished once nothing changed for STILL_S
    let mut g = Watch::default();
    let generic = facts_for("com.example.chat", "Some Chat", &[]);
    nodes.push(button("Stop response"));
    assert_eq!(
        g.see(&read(&nodes, &generic, &[]), false, 0.0),
        State::Finished
    );
    nodes.push(text(40.0, 360.0, 300.0, "More"));
    assert_eq!(
        g.see(&read(&nodes, &generic, &[]), false, 10.0),
        State::Replying
    );
    assert_eq!(
        g.see(
            &read(&nodes, &generic, &[]),
            false,
            10.0 + STOP_GONE_S + 0.1
        ),
        State::Replying
    );
    assert_eq!(
        g.see(&read(&nodes, &generic, &[]), false, 10.0 + STILL_S - 0.1),
        State::Replying
    );
    assert_eq!(
        g.see(&read(&nodes, &generic, &[]), false, 10.0 + STILL_S + 0.1),
        State::Finished
    );
}

#[test]
fn a_window_with_no_field_to_type_into_has_no_composer() {
    let nodes = vec![text(40.0, 100.0, 300.0, "Read only")];
    assert_eq!(read(&nodes, &claude(), &[]).composer, None);
    assert!(!known().is_empty() && known().iter().all(|f| !f.bundle_id.is_empty()));
}

fn cand(index: usize, label: &str, chain: &[(&str, &[&str])]) -> Candidate {
    Candidate {
        index,
        label: label.into(),
        focused: false,
        bottom: 900.0,
        chain: chain
            .iter()
            .map(|(role, marks)| Link {
                role: role.to_string(),
                marks: marks.iter().map(|m| m.to_string()).collect(),
                frame: None,
            })
            .collect(),
    }
}

const CURSOR: &str = "com.todesktop.230313mzl4w4u92";

// decision: HAND-697, HAND-698
#[test]
fn a_chat_pane_is_proven_by_structure_and_an_editor_or_a_settings_page_never_is() {
    let cursor = facts_for(CURSOR, "Cursor", &[]);
    // the editor's input, whatever its label: never a composer
    let editor = cand(
        3,
        "scratch.txt",
        &[
            ("AXTextArea", &["inputarea"]),
            ("AXGroup", &["monaco-editor"]),
            ("AXWindow", &[]),
        ],
    );
    assert_eq!(prove(std::slice::from_ref(&editor), &cursor), None);
    // a settings page: a search field and a text field, neither in a chat pane
    let search = cand(
        1,
        "Search settings",
        &[("AXTextField", &["AXSearchField"]), ("AXWindow", &[])],
    );
    let field = cand(
        2,
        "Claude export",
        &[
            ("AXTextField", &["setting-input"]),
            ("AXGroup", &["settings-editor"]),
            ("AXWindow", &[]),
        ],
    );
    assert_eq!(prove(&[search.clone(), field.clone()], &cursor), None);
    // the agent pane: its composer's marks inside its pane's
    let composer = cand(
        7,
        "Plan, search, build anything",
        &[
            ("AXTextArea", &["aislash-editor-input"]),
            ("AXGroup", &["composer-input"]),
            ("AXGroup", &["auxiliarybar", "part"]),
            ("AXWindow", &[]),
        ],
    );
    assert_eq!(
        prove(&[editor, search, field, composer.clone()], &cursor),
        Some(Proven {
            composer: 7,
            pane: 2
        })
    );
    // its fingerprint finds it again, never the editor
    let fp = fingerprint(&composer, 2);
    assert_eq!(
        matches(&fp, &[composer]),
        Some(Proven {
            composer: 7,
            pane: 2
        })
    );
    // an app the owner added: the one editable element that is no editor's
    let added = facts_for("com.example.chat", "Some Chat", &[]);
    let one = cand(4, "Message", &[("AXTextArea", &[]), ("AXWindow", &[])]);
    assert_eq!(
        prove(std::slice::from_ref(&one), &added),
        Some(Proven {
            composer: 4,
            pane: 1
        })
    );
    assert_eq!(
        prove(
            &[
                one.clone(),
                cand(5, "Search", &[("AXTextField", &[]), ("AXWindow", &[])])
            ],
            &added
        ),
        None
    );
    // icon-font glyphs aren't words
    assert_eq!(plain("\u{eb60} scratch.txt \u{f101}"), "scratch.txt");
}

// decision: HAND-699
#[test]
fn a_model_s_pick_is_learned_only_when_code_s_check_holds() {
    let nodes = vec![
        list(),
        text(400.0, 100.0, 300.0, "hi"),
        text(40.0, 150.0, 600.0, "Hello"),
        Node {
            composer: false,
            ..input(880.0, false)
        },
        Node {
            role: "AXTextArea".into(),
            x: 700.0,
            y: 80.0,
            w: 400.0,
            h: 800.0,
            editable: true,
            enabled: true,
            ..Node::default()
        },
    ];
    let reply = Candidate {
        chain: vec![
            Link {
                role: "AXTextArea".into(),
                marks: vec![],
                frame: Some((40.0, 880.0, 600.0, 40.0)),
            },
            Link {
                role: "AXGroup".into(),
                marks: vec![],
                frame: Some((0.0, 0.0, 720.0, 1000.0)),
            },
        ],
        ..cand(3, "Prompt", &[])
    };
    let editor = Candidate {
        chain: vec![Link {
            role: "AXTextArea".into(),
            marks: vec!["inputarea".into()],
            frame: None,
        }],
        ..cand(4, "", &[])
    };
    let read = WindowRead {
        nodes,
        cands: vec![reply, editor],
    };
    let pick = |list: usize, reply: usize| Pick {
        chat: true,
        message_list: Some(list),
        reply_box: Some(reply),
    };
    assert_eq!(
        check_pick(&read, &pick(0, 3)),
        Some(Proven {
            composer: 3,
            pane: 1
        })
    );
    assert_eq!(check_pick(&read, &pick(0, 4)), None, "the editor");
    assert_eq!(check_pick(&read, &pick(1, 3)), None, "a text is no list");
    assert_eq!(check_pick(&read, &Pick::default()), None, "none");
    let outline = outline(&read);
    assert!(
        outline.contains("3 AXTextArea editable \"Prompt\" @40,880 600x40"),
        "{outline}"
    );
    assert!(!outline.contains("Hello"), "values never: {outline}");
}
