//! The session model's pure parts on invented records shaped as the agent-remote lab observed them (Claude Code
//! 2.1.328's transcripts and hooks, codex-cli 0.170's app-server messages): no real session, no file of the owner's.

use std::collections::HashMap;

use ferb_sessions::claude::{self, HookEvent, Line};
use ferb_sessions::host::{self, Proc};
use ferb_sessions::model::*;
use ferb_sessions::{codex, hooks, paths, ws};
use serde_json::{json, Value};

fn items(line: &Value) -> Vec<claude::Draft> {
    match claude::parse_line(&line.to_string()) {
        Line::Items(d) => d,
        other => panic!("{other:?}"),
    }
}

// decision: HAND-661
#[test]
fn the_transcript_is_read_by_an_allowlist_and_requests_never() {
    let user = json!({"type": "user", "timestamp": "2026-11-19T05:53:54.297Z",
        "message": {"role": "user", "content": "Reply with the word OK"}});
    let d = items(&user);
    assert_eq!(d.len(), 1);
    assert_eq!(
        (d[0].kind, d[0].text.as_str()),
        (ItemKind::User, "Reply with the word OK")
    );
    let assistant = json!({"type": "assistant", "timestamp": "t", "message": {"content": [
        {"type": "thinking", "thinking": "secret plan"},
        {"type": "text", "text": "Running the tests."},
        {"type": "tool_use", "name": "Bash", "input": {"command": "npm test\n  --watch=false", "description": "run"}},
        {"type": "tool_use", "name": "Edit", "input": {"file_path": "/p/src/x.rs", "old_string": "a"}},
        {"type": "tool_use", "name": "TodoWrite", "input": {"todos": []}}]}});
    let d = items(&assistant);
    assert_eq!(d.len(), 4, "thinking is never shown");
    assert_eq!(
        (d[1].tool.as_str(), d[1].text.as_str()),
        ("Bash", "npm test --watch=false")
    );
    assert_eq!(
        (d[2].tool.as_str(), d[2].text.as_str()),
        ("Edit", "/p/src/x.rs")
    );
    assert_eq!((d[3].tool.as_str(), d[3].text.as_str()), ("TodoWrite", ""));
    // the whole model request (with the owner's account in it) is never read into anything
    for kind in ["api-request", "api-request-blob", "api-request-shape"] {
        let l = json!({"type": kind, "text": "you are talking to owner@example.com", "message": {"content": "x"}});
        assert_eq!(claude::parse_line(&l.to_string()), Line::Skip, "{kind}");
    }
    // unknown kinds, a subagent's own turns, Claude Code's notes and broken lines are skipped without error
    for l in [
        json!({"type": "queue-operation", "x": 1}).to_string(),
        json!({"type": "user", "isSidechain": true, "message": {"content": "sub"}}).to_string(),
        json!({"type": "user", "isMeta": true, "message": {"content": "meta"}}).to_string(),
        json!({"type": "user", "message": {"content": "<local-command-stdout>x</local-command-stdout>"}}).to_string(),
        json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": "ok"}]}}).to_string(),
        "{not json".to_string(),
        String::new(),
    ] {
        assert_eq!(claude::parse_line(&l), Line::Skip, "{l}");
    }
}

#[test]
fn interrupts_commands_errors_and_titles() {
    let d = items(
        &json!({"type": "user", "message": {"content": [{"type": "text", "text": "[Request interrupted by user]"}]}}),
    );
    assert_eq!(d[0].kind, ItemKind::Interrupted);
    let d = items(
        &json!({"type": "user", "message": {"content": "<command-name>/model</command-name>\n<command-args></command-args>"}}),
    );
    assert_eq!((d[0].kind, d[0].text.as_str()), (ItemKind::User, "/model"));
    let d = items(
        &json!({"type": "assistant", "isApiErrorMessage": true, "message": {"content": [{"type": "text", "text": "API Error: 529"}]}}),
    );
    assert_eq!(d[0].kind, ItemKind::Error);
    assert_eq!(
        claude::parse_line(&json!({"type": "ai-title", "aiTitle": "Fix  the\nlogin"}).to_string()),
        Line::Title {
            text: "Fix the login".into(),
            owners: false
        }
    );
    assert_eq!(
        claude::parse_line(&json!({"type": "custom-title", "customTitle": "Mine"}).to_string()),
        Line::Title {
            text: "Mine".into(),
            owners: true
        }
    );
}

fn hook(v: Value) -> claude::Hook {
    claude::parse_hook(&v).expect("a session")
}

// decision: HAND-662
#[test]
fn hooks_are_read_and_a_permission_or_question_is_a_card_with_its_answers() {
    assert!(claude::parse_hook(&json!({"hook_event_name": "Stop"})).is_none());
    let h = hook(
        json!({"hook_event_name": "PermissionRequest", "session_id": "s1", "cwd": "/p", "transcript_path": "/t.jsonl",
        "tool_name": "Bash", "tool_input": {"command": "touch spike-perm.txt", "description": "Create"},
        "permission_suggestions": [{"type": "setMode", "mode": "acceptEdits", "destination": "session"}], "prompt_id": "p"}),
    );
    let HookEvent::Permission {
        tool,
        input,
        suggestions,
    } = &h.event
    else {
        panic!()
    };
    let c = claude::card("c1", "claude:s1", tool, input, suggestions, "t");
    assert_eq!(
        (c.kind, c.detail.as_str()),
        (CardKind::Permission, "touch spike-perm.txt")
    );
    let ids: Vec<&str> = c.choices.iter().map(|x| x.id.as_str()).collect();
    assert_eq!(ids, ["allow_once", "allow_for_session", "deny"]);
    let r: Value =
        serde_json::from_str(&claude::reply("allow_once", &c, input, suggestions)).unwrap();
    assert_eq!(
        r,
        json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": "allow"}}})
    );
    // "for this session" is the agent's own rule from its own suggestions, never FERB's judgment
    let r: Value =
        serde_json::from_str(&claude::reply("allow_for_session", &c, input, suggestions)).unwrap();
    assert_eq!(
        r["hookSpecificOutput"]["decision"]["updatedPermissions"],
        *suggestions
    );
    let r: Value = serde_json::from_str(&claude::reply("deny", &c, input, suggestions)).unwrap();
    assert_eq!(r["hookSpecificOutput"]["decision"]["behavior"], "deny");
    assert_eq!(claude::reply("something else", &c, input, suggestions), "");
    // no suggestions: no "for this session"
    let c2 = claude::card("c2", "s", "Bash", input, &Value::Null, "t");
    assert_eq!(c2.choices.len(), 2);
    // a question arrives as a permission request too (HAND-662); its answer carries the asked input and the answer
    let q = json!({"questions": [{"question": "Pick a number", "header": "N", "multiSelect": false,
        "options": [{"label": "One", "description": "1"}, {"label": "Two", "description": "2"}]}]});
    let c = claude::card("c3", "s", claude::QUESTION_TOOL, &q, &Value::Null, "t");
    assert_eq!(
        (c.kind, c.question.as_str()),
        (CardKind::Question, "Pick a number")
    );
    assert_eq!(
        c.choices.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(),
        ["One", "Two"]
    );
    let r: Value = serde_json::from_str(&claude::reply("Two", &c, &q, &Value::Null)).unwrap();
    assert_eq!(
        r["hookSpecificOutput"]["decision"]["updatedInput"]["answers"],
        json!({"Pick a number": "Two"})
    );
    assert_eq!(
        r["hookSpecificOutput"]["decision"]["updatedInput"]["questions"],
        q["questions"]
    );
    // two questions, or several answers at once: answered on the Mac (no choices)
    let multi = json!({"questions": [{"question": "a", "options": [{"label": "x"}]}, {"question": "b", "options": [{"label": "y"}]}]});
    assert!(
        claude::card("c4", "s", claude::QUESTION_TOOL, &multi, &Value::Null, "t")
            .choices
            .is_empty()
    );
    // the Mac answered first: the finished tool is the card's (no tool_use_id: by tool and input, HAND-662)
    let answered = json!({"questions": q["questions"], "answers": {"Pick a number": "One"}});
    assert!(claude::same_call(
        claude::QUESTION_TOOL,
        &q,
        claude::QUESTION_TOOL,
        &answered
    ));
    assert!(claude::same_call("Bash", input, "Bash", input));
    assert!(!claude::same_call(
        "Bash",
        input,
        "Bash",
        &json!({"command": "rm -rf /"})
    ));
}

#[test]
fn claude_agents_list_is_read_leniently() {
    let a = claude::parse_agents(
        r#"[{"pid": 41, "cwd": "/p", "sessionId": "s1", "name": "fix", "kind": "interactive", "status": "busy"},
        {"pid": 42, "cwd": "/q", "sessionId": "s2", "status": "idle"}, {"nothing": 1}]"#,
    );
    assert_eq!(a.len(), 2);
    assert!(a[0].busy && !a[1].busy);
    assert_eq!(
        claude::parse_agents(r#"{"agents": [{"sessionId": "s3"}]}"#)[0].session,
        "s3"
    );
    assert!(claude::parse_agents("not json").is_empty());
}

#[test]
fn codex_threads_items_and_events() {
    let t = codex::thread(&json!({"id": "T", "cwd": "/p", "status": {"type": "active", "activeFlags": []}, "name": null,
        "preview": "Reply with OK", "ephemeral": false, "turns": [{"items": [
            {"type": "userMessage", "content": [{"type": "text", "text": "Reply with OK"}], "completedAtMs": 1795068524782i64},
            {"type": "reasoning", "summary": []},
            {"type": "agentMessage", "text": "OK"},
            {"type": "commandExecution", "command": ["npm", "test"]},
            {"type": "fileChange", "changes": [{"path": "a.rs"}, {"path": "b.rs"}]}]}]})).unwrap();
    assert_eq!(
        (t.status, t.title.as_str()),
        (Status::Working, "Reply with OK")
    );
    let kinds: Vec<(ItemKind, &str)> = t.items.iter().map(|d| (d.kind, d.text.as_str())).collect();
    assert_eq!(
        kinds,
        [
            (ItemKind::User, "Reply with OK"),
            (ItemKind::Assistant, "OK"),
            (ItemKind::Tool, "npm test"),
            (ItemKind::Tool, "a.rs, b.rs")
        ]
    );
    assert_eq!(t.items[0].at, "2026-11-19T06:08:44.782Z");
    assert_eq!(
        codex::status_of(&json!({"type": "active", "activeFlags": ["waitingOnApproval"]})),
        (Status::WaitingForYou, Waiting::Permission)
    );
    assert_eq!(
        codex::status_of(&json!({"type": "notLoaded"})).0,
        Status::Ended
    );
    assert_eq!(
        codex::status_of(&json!({"type": "systemError"})).0,
        Status::Idle
    );
    match codex::event(r#"{"id": 7, "result": {"data": ["T"]}}"#) {
        codex::Event::Answer {
            id: 7,
            result: Ok(v),
        } => assert_eq!(v["data"][0], "T"),
        other => panic!("{other:?}"),
    }
    match codex::event(
        r#"{"id": 3, "method": "item/commandExecution/requestApproval", "params": {"threadId": "T", "command": "rm -rf build"}}"#,
    ) {
        codex::Event::Asks { id, thread, card } => {
            assert_eq!(
                (id, thread.as_str(), card.detail.as_str()),
                (json!(3), "T", "rm -rf build")
            );
            assert_eq!(card.kind, CardKind::Permission);
        }
        other => panic!("{other:?}"),
    }
    // a request FERB doesn't understand is left to the app that runs the thread
    assert_eq!(
        codex::event(r#"{"id": 4, "method": "item/tool/requestUserInput", "params": {}}"#),
        codex::Event::Other
    );
    assert_eq!(
        codex::approval_answer("allow_for_session"),
        Some(json!({"decision": "acceptForSession"}))
    );
    assert_eq!(codex::approval_answer("maybe"), None);
}

fn table(rows: &[(i32, i32, i32, i32, &str, &str)]) -> host::Table {
    rows.iter()
        .map(|(pid, ppid, pgid, tpgid, tty, exe)| {
            (
                *pid,
                Proc {
                    pid: *pid,
                    ppid: *ppid,
                    pgid: *pgid,
                    tpgid: *tpgid,
                    tty: tty.to_string(),
                    exe: exe.to_string(),
                },
            )
        })
        .collect::<HashMap<_, _>>()
}

// decision: HAND-654
#[test]
fn the_host_is_found_from_the_process_tree_with_no_list_of_hosts() {
    let t = table(&[
        (100, 1, 100, 0, "", "/Applications/Visual Studio Code.app/Contents/MacOS/Code"),
        (110, 100, 110, 0, "", "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper.app/Contents/MacOS/Code Helper"),
        (120, 110, 120, 130, "/dev/ttys004", "/bin/zsh"),
        (130, 120, 130, 130, "/dev/ttys004", "/Users/o/.local/share/claude/versions/2.1.328"),
        (200, 1, 200, 0, "", "/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal"),
        (210, 200, 210, 0, "/dev/ttys007", "/usr/bin/login"),
        (220, 210, 220, 230, "/dev/ttys007", "/bin/zsh"),
        (230, 220, 230, 230, "/dev/ttys007", "/opt/homebrew/bin/codex"),
        (300, 1, 300, 0, "", "/opt/homebrew/bin/tmux"),
        (310, 300, 310, 320, "/dev/ttys010", "/bin/zsh"),
        (320, 310, 320, 320, "/dev/ttys010", "/opt/homebrew/bin/claude"),
        (400, 220, 400, 230, "/dev/ttys007", "/opt/homebrew/bin/tmux"),
        (500, 1, 500, 510, "/dev/ttys020", "/bin/zsh"),
        (510, 500, 510, 510, "/dev/ttys020", "/opt/homebrew/bin/claude"),
    ]);
    let none = |_s: i32| Vec::new();
    let f = host::resolve(&t, 130, &none).unwrap();
    assert_eq!(
        (f.bundle.as_str(), f.pid, f.via, f.tty.as_str()),
        (
            "/Applications/Visual Studio Code.app",
            100,
            Via::Direct,
            "/dev/ttys004"
        )
    );
    let f = host::resolve(&t, 230, &none).unwrap();
    assert_eq!(
        (f.bundle.as_str(), f.pid),
        ("/System/Applications/Utilities/Terminal.app", 200)
    );
    // tmux: its client's app, through tmux's own client list; detached: no window, tmux alone
    let attached = |_s: i32| vec![(400, "/dev/ttys007".to_string())];
    let f = host::resolve(&t, 320, &attached).unwrap();
    assert_eq!(
        (f.pid, f.via, f.tty.as_str(), f.tmux_server),
        (200, Via::Tmux, "/dev/ttys007", 300)
    );
    let f = host::resolve(&t, 320, &none).unwrap();
    assert_eq!((f.pid, f.via, f.tmux_server), (0, Via::Tmux, 300));
    // no app above it at all (a launchd job, a terminal FERB can't see): no host
    assert!(host::resolve(&t, 510, &none).is_none());
    assert_eq!(
        host::bundle_of("/Applications/X.app"),
        Some("/Applications/X.app".into())
    );
    assert_eq!(host::bundle_of("/usr/bin/zsh"), None);
}

// decision: HAND-658
#[test]
fn typed_text_is_allowed_only_when_the_agent_leads_its_terminal() {
    let t = table(&[
        (1, 0, 1, 0, "", "/sbin/launchd"),
        (10, 1, 10, 20, "/dev/ttys1", "/bin/zsh"),
        (20, 10, 20, 20, "/dev/ttys1", "/x/claude"),
        (30, 10, 30, 31, "/dev/ttys1", "/x/claude"),
        (40, 1, 40, 0, "", "/x/claude"),
    ]);
    assert!(host::in_front(&t, 20));
    assert!(
        !host::in_front(&t, 30),
        "a shell or another program has the terminal"
    );
    assert!(!host::in_front(&t, 40), "no terminal");
    assert!(!host::in_front(&t, 99), "gone");
}

// decision: HAND-656
#[test]
fn send_paths_go_agent_first_then_an_adapter_then_the_foreground() {
    let h = |bundle: &str, via: Via, pid: i32| Host {
        bundle_id: bundle.into(),
        name: "X".into(),
        pid,
        via: Some(via),
        tty: "/dev/ttys1".into(),
    };
    assert_eq!(
        paths::send_paths(Agent::Codex, None, true).0,
        [SendPath::AgentApi]
    );
    assert_eq!(
        paths::send_paths(Agent::ClaudeCode, Some(&h("x", Via::Tmux, 0)), false).0,
        [SendPath::Tmux]
    );
    assert_eq!(
        paths::send_paths(
            Agent::ClaudeCode,
            Some(&h(paths::TERMINAL, Via::Direct, 5)),
            false
        )
        .0,
        [SendPath::TerminalScript, SendPath::Foreground]
    );
    // any other app is never "unsupported": it gets the foreground path (HAND-652)
    assert_eq!(
        paths::send_paths(
            Agent::ClaudeCode,
            Some(&h("dev.zed.Zed", Via::Direct, 5)),
            false
        )
        .0,
        [SendPath::Foreground]
    );
    // Codex without its daemon falls back to its window like any agent
    assert_eq!(
        paths::send_paths(
            Agent::Codex,
            Some(&h("com.microsoft.VSCode", Via::Direct, 5)),
            false
        )
        .0,
        [SendPath::Foreground]
    );
    let (none, why) = paths::send_paths(Agent::ClaudeCode, None, false);
    assert!(none.is_empty() && !why.is_empty());
    assert!(
        !SendPath::AgentApi.types() && SendPath::Tmux.types() && SendPath::Foreground.foreground()
    );
}

const PROGRAM: &str = "/Applications/FERB.app/Contents/Resources/bin/ferb";

// decision: HAND-666
#[test]
fn ferb_s_hooks_go_in_and_out_of_claude_code_s_settings_touching_nothing_else() {
    let mine = json!({"model": "opus", "permissions": {"allow": ["Bash(ls)"]},
        "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "say done"}]}],
                  "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "/my/guard"}]}]}});
    let text = serde_json::to_string_pretty(&mine).unwrap();
    let added = hooks::install(Some(&text), PROGRAM).unwrap().unwrap();
    assert!(hooks::installed(Some(&added), PROGRAM));
    let v: Value = serde_json::from_str(&added).unwrap();
    assert_eq!(v["model"], "opus");
    assert_eq!(v["permissions"], mine["permissions"]);
    assert_eq!(
        v["hooks"]["PreToolUse"], mine["hooks"]["PreToolUse"],
        "an event FERB doesn't use is untouched"
    );
    assert_eq!(
        v["hooks"]["Stop"][0], mine["hooks"]["Stop"][0],
        "the owner's own Stop hook stays first"
    );
    assert_eq!(
        v["hooks"]["Stop"][1]["hooks"][0]["command"],
        format!("{PROGRAM} agent-hook claude Stop")
    );
    for e in claude::HOOK_EVENTS {
        assert!(v["hooks"][e].is_array(), "{e}");
    }
    // removed: exactly what was there before
    let removed = hooks::remove(Some(&added)).unwrap().unwrap();
    assert_eq!(serde_json::from_str::<Value>(&removed).unwrap(), mine);
    assert_eq!(
        hooks::remove(Some(&removed)).unwrap(),
        None,
        "nothing of FERB's left"
    );
    // an older FERB.app somewhere else: its entries are found and replaced, never doubled
    let old = hooks::install(
        Some(&text),
        "/Users/o/Downloads/FERB.app/Contents/Resources/bin/ferb",
    )
    .unwrap()
    .unwrap();
    let new = hooks::install(Some(&old), PROGRAM).unwrap().unwrap();
    let v: Value = serde_json::from_str(&new).unwrap();
    assert_eq!(v["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
    assert!(hooks::installed(Some(&new), PROGRAM));
    // no file yet, an empty one: FERB's hooks alone; a file that isn't settings: left alone, said
    assert!(hooks::install(None, PROGRAM).unwrap().is_some());
    assert!(hooks::install(Some("  "), PROGRAM).unwrap().is_some());
    assert!(hooks::install(Some("[1, 2]"), PROGRAM).is_err());
    assert!(hooks::install(Some("{broken"), PROGRAM).is_err());
    assert!(hooks::remove(Some("{broken")).is_err());
    // a path with a space is quoted, and still FERB's
    let spaced = hooks::command(
        "/Applications/My Apps/FERB.app/Contents/Resources/bin/ferb",
        "Stop",
    );
    assert!(spaced.starts_with('\'') && hooks::is_ferbs(&spaced));
    assert!(!hooks::is_ferbs(
        "/usr/local/bin/notferb agent-hook claude Stop"
    ));
    assert!(!hooks::is_ferbs("say done"));
}

#[test]
fn websocket_frames_round_trip() {
    let f = ws::frame_with(ws::TEXT, b"hello", [1, 2, 3, 4]);
    assert_eq!(f[0], 0x81);
    assert_eq!(f[1] & 0x80, 0x80, "a client's frames are masked");
    let mut r = ws::Reader::new(1 << 20);
    r.push(&f[..3]);
    assert_eq!(r.next_message().unwrap(), None, "not complete yet");
    r.push(&f[3..]);
    assert_eq!(
        r.next_message().unwrap(),
        Some(ws::Message::Text("hello".into()))
    );
    // a server's unmasked frames, fragmented, a long one, a ping
    let mut r = ws::Reader::new(1 << 20);
    r.push(&[0x01, 3, b'a', b'b', b'c']);
    r.push(&[0x80, 2, b'd', b'e']);
    assert_eq!(
        r.next_message().unwrap(),
        Some(ws::Message::Text("abcde".into()))
    );
    let long = vec![b'x'; 70_000];
    let mut frame = vec![0x81, 127];
    frame.extend_from_slice(&(long.len() as u64).to_be_bytes());
    frame.extend_from_slice(&long);
    r.push(&frame);
    assert!(matches!(r.next_message().unwrap(), Some(ws::Message::Text(t)) if t.len() == 70_000));
    r.push(&[0x89, 1, 7]);
    assert_eq!(r.next_message().unwrap(), Some(ws::Message::Ping(vec![7])));
    let mut small = ws::Reader::new(10);
    small.push(&[0x81, 20]);
    assert!(small.next_message().is_err(), "too large");
    assert!(ws::accepted("HTTP/1.1 101 Switching Protocols\r\n"));
    assert!(!ws::accepted("HTTP/1.1 400 Bad Request\r\n"));
}
