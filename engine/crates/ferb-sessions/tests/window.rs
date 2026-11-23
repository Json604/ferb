//! PHN-797: Watch shows the window the session is in, found as the send path finds it (HAND-682, HAND-683). The
//! windows are what the window server listed for VS Code on the owner's Mac (2026-11-23, read only): the project's
//! window titled "mac-review-chat-card.png — FERB", a transparent 1470×32 bar, a 64×64 one and four untitled 1470×33
//! strips off screen, all at layer 0. The old finder wanted a window titled exactly as the session or its folder, else
//! the app's only window: with seven, it found none and said "Its window isn't open on your Mac."

use ferb_sessions::host::{resolve, Proc, Table};
use ferb_sessions::proof::Proof;
use ferb_sessions::window::*;

fn win(id: i64, title: &str, size: (f64, f64), on_screen: bool) -> Win {
    Win {
        id,
        title: title.into(),
        size,
        on_screen,
    }
}

/// VS Code's windows on his Mac, front to back (the transparent bar is left out by alpha before this list).
fn vscode() -> Vec<Win> {
    vec![
        win(266533, "", (64.0, 64.0), false),
        win(
            266532,
            "mac-review-chat-card.png — FERB",
            (1470.0, 923.0),
            true,
        ),
        win(266531, "", (1470.0, 33.0), false),
        win(266530, "", (1470.0, 33.0), false),
        win(266529, "", (1470.0, 33.0), false),
        win(266528, "", (1470.0, 33.0), false),
    ]
}

fn session() -> Proof {
    Proof {
        title: "Fix the phone's Watch page".into(),
        program: "2.1.330".into(),
        folder: "FERB".into(),
    }
}

fn proc(pid: i32, ppid: i32, exe: &str) -> Proc {
    Proc {
        pid,
        ppid,
        pgid: pid,
        tpgid: 0,
        tty: String::new(),
        exe: exe.into(),
    }
}

// decision: HAND-713
// decision: PHN-797
// decision: HAND-682
// decision: HAND-654
#[test]
fn a_session_in_an_editor_s_terminal_is_watched_in_the_window_that_names_its_folder() {
    // his process tree: claude under zsh under VS Code's Code Helper, under Code itself (pids as read)
    let code = "/Applications/Visual Studio Code.app/Contents/MacOS/Code";
    let helper = "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper.app/Contents/MacOS/Code Helper";
    let t: Table = [
        proc(92863, 1, code),
        proc(92887, 92863, helper),
        proc(92896, 92887, "/bin/zsh"),
        proc(
            93334,
            92896,
            "/Users/o/.local/share/claude/versions/2.1.330",
        ),
    ]
    .into_iter()
    .map(|p| (p.pid, p))
    .collect();
    let host = resolve(&t, 93334, &|_| vec![]).expect("a host");
    // the helper is the agent's nearest app process; the host is the app itself, which owns the windows
    assert_eq!(host.pid, 92863);
    let found = session_window(&vscode(), &session(), "", 0).expect("its window");
    assert_eq!(found.id, 266532);
    assert!(found.on_screen);
}

// decision: HAND-683
// decision: HAND-713
// decision: PHN-797
#[test]
fn the_terminal_the_owner_chose_is_the_window_watched() {
    let mut wins = vscode();
    wins.insert(0, win(7, "notes.md — other-project", (1200.0, 800.0), true));
    // he chose the second window's first terminal: that window, even though another one names the folder
    let found = session_window(&wins, &session(), "w7:0:Terminal 1, 2.1.330", 0).expect("chosen");
    assert_eq!(found.id, 7);
    assert_eq!(chosen_window("w7:0:Terminal 1, 2.1.330"), Some(7));
    assert_eq!(chosen_window("front"), None);
    // a choice whose window is gone: back to the folder's window
    let found = session_window(&wins, &session(), "w99:0:x", 0).expect("by folder");
    assert_eq!(found.id, 266532);
    // an app chat's own window comes first
    assert_eq!(
        session_window(&wins, &session(), "", 7).map(|f| f.id),
        Some(7)
    );
}

// decision: HAND-713
// decision: PHN-797
#[test]
fn the_session_s_title_then_its_folder_then_the_front_window() {
    let wins = vec![
        win(1, "README.md — site", (900.0, 700.0), true),
        win(
            2,
            "Fix the phone's Watch page — claude",
            (900.0, 700.0),
            false,
        ),
        win(3, "main.rs — FERB", (900.0, 700.0), true),
    ];
    assert_eq!(
        session_window(&wins, &session(), "", 0).map(|f| f.id),
        Some(2)
    );
    let untitled = Proof {
        title: String::new(),
        ..session()
    };
    assert_eq!(
        session_window(&wins, &untitled, "", 0).map(|f| f.id),
        Some(3)
    );
    // a folder is a whole word: "FERBsite" isn't "FERB"
    let other = vec![
        win(1, "a.md — FERBsite", (900.0, 700.0), true),
        win(2, "b.md — notes", (900.0, 700.0), true),
    ];
    assert_eq!(naming_folder(&other, "FERB").len(), 0);
    // nothing names it: the app's front window, as before
    assert_eq!(
        session_window(&other, &untitled, "", 0).map(|f| f.id),
        Some(1)
    );
    // two windows name the folder: the one on this Space
    let two = vec![
        win(1, "a.rs — FERB", (900.0, 700.0), false),
        win(2, "b.rs — FERB", (900.0, 700.0), true),
    ];
    assert_eq!(
        session_window(&two, &untitled, "", 0).map(|f| f.id),
        Some(2)
    );
    // no titles (the Mac gave none): only the windows big enough to show a terminal count
    let bare = vec![
        win(1, "", (1470.0, 33.0), true),
        win(2, "", (1200.0, 800.0), true),
    ];
    assert_eq!(
        session_window(&bare, &untitled, "", 0).map(|f| f.id),
        Some(2)
    );
    assert_eq!(session_window(&[], &untitled, "", 0), None);
}

// decision: HAND-713
// decision: PHN-797
#[test]
fn a_window_minimized_or_on_another_space_that_ferb_couldn_t_bring_back_is_said_as_such() {
    // the folder's window is off this Space: found, not on screen; FERB brings it onto the screen itself, and says so
    // only when it couldn't
    let wins = vec![win(5, "main.rs — FERB", (900.0, 700.0), false)];
    let found = session_window(&wins, &session(), "", 0).expect("found");
    assert!(!found.on_screen);
    assert_eq!(
        Unseen::Minimized.words("Visual Studio Code", &found.title),
        "Its window in Visual Studio Code (“main.rs — FERB”) is minimized in the Dock, and FERB couldn't bring it \
         back onto your screen."
    );
    assert_eq!(
        Unseen::OtherSpace.words("Visual Studio Code", &found.title),
        "Its window in Visual Studio Code (“main.rs — FERB”) is on another desktop (Space), and FERB couldn't switch \
         to it."
    );
    assert_eq!(
        Unseen::NoWindow.words("Terminal", ""),
        "Terminal has no window open for this session."
    );
}
