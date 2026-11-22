//! HAND-682: the terminal FERB types into must be the session's, by structure. The words are what VS Code's integrated
//! terminal really showed on the owner's Mac (2026-11-22, read only): its input "Terminal 1, 2.1.330 …" (Claude Code's
//! own installer names its program by its version) in a window titled "FERB-site-signin-setup.md — FERB", and none of
//! the session's title anywhere, which is why the title-only proof refused every send.

use ferb_sessions::proof::*;

fn vscode(own: &str) -> Focus {
    Focus {
        own: own.into(),
        around: format!("{own}\nFERB-site-signin-setup.md — FERB"),
        terminal: true,
        terminal_app: false,
    }
}

fn session() -> Proof {
    Proof {
        title: "FERB Rust migration research review".into(),
        program: program_name("/Users/o/.local/share/claude/versions/2.1.330"),
        folder: "FERB".into(),
    }
}

// decision: HAND-682
#[test]
fn an_editor_s_terminal_is_the_session_s_by_its_program_and_folder() {
    let term = "Terminal 1, 2.1.330 Run the command: Toggle Screen Reader Accessibility Mode";
    assert_eq!(session().program, "2.1.330");
    // the title isn't shown: the old proof refused; the program in the terminal and the folder in the window prove it
    assert_eq!(proven(&session(), &vscode(term)), Ok(()));
    // the same title-only proof as before: refused
    let title_only = Proof {
        program: String::new(),
        folder: String::new(),
        ..session()
    };
    assert_eq!(
        proven(&title_only, &vscode(term)),
        Err(Unproven::NotTheSession)
    );
    // a shell in another terminal of that window: its own words don't name the program
    assert_eq!(
        proven(&session(), &vscode("Terminal 2, zsh")),
        Err(Unproven::NotTheSession)
    );
    // the program only in the window's title (a file named like it) isn't the terminal's own
    let f = Focus {
        own: "Terminal 2, zsh".into(),
        around: "Terminal 2, zsh\n2.1.330 notes.md — FERB".into(),
        ..vscode("")
    };
    assert_eq!(proven(&session(), &f), Err(Unproven::NotTheSession));
    // a newer version is another program; a longer number isn't this one
    assert_eq!(
        proven(&session(), &vscode("Terminal 1, 2.1.331")),
        Err(Unproven::NotTheSession)
    );
    assert_eq!(
        proven(&session(), &vscode("Terminal 1, 12.1.330")),
        Err(Unproven::NotTheSession)
    );
    // another folder's window
    let other = Focus {
        around: format!("{term}\nmain.rs — website"),
        ..vscode(term)
    };
    assert_eq!(proven(&session(), &other), Err(Unproven::NotTheSession));
    // the editor's text, not a terminal: never, whatever words are around
    let editor = Focus {
        terminal: false,
        ..vscode("Terminal 1, 2.1.330 FERB Rust migration research review")
    };
    assert_eq!(proven(&session(), &editor), Err(Unproven::NotATerminal));
}

// decision: HAND-660
#[test]
fn a_terminal_app_shows_the_session_s_title_and_its_window_names_the_program() {
    let terminal = Focus {
        own: String::new(),
        around: "FERB — ✳ FERB Rust migration research review — 2.1.330 — 80×24".into(),
        terminal: true,
        terminal_app: true,
    };
    assert_eq!(proven(&session(), &terminal), Ok(()));
    let untitled = Proof {
        title: String::new(),
        ..session()
    };
    assert_eq!(proven(&untitled, &terminal), Ok(()));
}

// decision: HAND-682
#[test]
fn the_cursor_goes_only_to_the_one_input_that_names_the_program() {
    let inputs = [
        "Search",
        "Terminal 1, 2.1.330 Run the command",
        "Terminal 2, zsh",
    ];
    assert_eq!(terminal_input(&inputs, "2.1.330"), Some(1));
    // two terminals running it: FERB can't tell which
    let two = ["Terminal 1, 2.1.330", "Terminal 2, 2.1.330"];
    assert_eq!(terminal_input(&two, "2.1.330"), None);
    assert_eq!(terminal_input(&inputs, "codex"), None);
    assert!(has_token("a — FERB", "ferb") && !has_token("FERBx", "ferb") && !has_token("", "x"));
}
