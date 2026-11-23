//! HAND-711: the agent and chat apps' windows are listed by structure alone.

use ferb_sessions::apps::{self, Listed};

fn listed(pid: i32, id: i64, title: &str, size: (f64, f64)) -> Listed {
    Listed {
        pid,
        id,
        title: title.into(),
        normal: true,
        alpha: 1.0,
        size,
        on_screen: id % 2 == 0,
    }
}

// decision: HAND-711 — VS Code's own windows as this Mac's window server listed them on 2026-11-24 (one titled window,
// a transparent bar, a 64×64 icon and five 1470×33 strips): only the titled, drawn, big enough one is a session
#[test]
fn only_an_app_s_real_windows_are_listed_by_their_app_and_title() {
    let code = 92863;
    let mut wins = vec![
        Listed {
            alpha: 0.0,
            ..listed(code, 266536, "", (1470.0, 32.0))
        },
        listed(code, 266533, "", (64.0, 64.0)),
        listed(
            code,
            266532,
            "mac-review-chat-card.png — FERB",
            (1470.0, 923.0),
        ),
        listed(code, 266531, "", (1470.0, 33.0)),
        listed(code, 266530, "Tab strip", (1470.0, 33.0)),
        Listed {
            normal: false,
            ..listed(code, 266529, "A panel", (800.0, 600.0))
        },
        // another app's window is never its
        listed(93433, 266565, "YouTube", (1470.0, 842.0)),
    ];
    let apps = vec![("com.microsoft.VSCode".to_string(), "Code".to_string(), code)];
    let got = apps::pick(&apps, &wins);
    assert_eq!(got.len(), 1, "{got:?}");
    let w = &got[0];
    assert_eq!(
        (
            w.bundle_id.as_str(),
            w.app.as_str(),
            w.pid,
            w.window,
            w.title.as_str(),
            w.on_screen
        ),
        (
            "com.microsoft.VSCode",
            "Code",
            code,
            266532,
            "mac-review-chat-card.png — FERB",
            true
        )
    );
    assert_eq!(apps::session_id(w), "app:com.microsoft.VSCode:266532");
    wins.clear();
    assert!(apps::pick(&apps, &wins).is_empty());
}

// decision: HAND-711, HAND-712
#[test]
fn the_known_apps_and_the_owner_s_own_are_listed_and_an_entrypoint_names_its_app() {
    let b = apps::bundles(&[
        " com.example.chat ".into(),
        "com.openai.chat".into(),
        "".into(),
    ]);
    assert!(b.contains(&"com.example.chat".to_string()));
    assert!(b.contains(&"com.todesktop.230313mzl4w4u92".to_string()));
    assert_eq!(b.iter().filter(|x| *x == "com.openai.chat").count(), 1);
    assert_eq!(
        apps::of_entrypoint("claude-desktop"),
        Some("com.anthropic.claudefordesktop")
    );
    assert_eq!(apps::of_entrypoint("cli"), None);
    assert_eq!(
        ferb_sessions::claude::entrypoint(
            r#"{"type":"user","entrypoint":"claude-desktop","cwd":"/p"}"#
        )
        .as_deref(),
        Some("claude-desktop")
    );
    assert_eq!(
        ferb_sessions::claude::entrypoint(r#"{"type":"user"}"#),
        None
    );
}
