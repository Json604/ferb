//! Every terminal as a session (kb HAND-692, HAND-693): found from an invented process table, whatever runs in it, and
//! a terminal's text turned into what is new each read.

use ferb_sessions::host::{Proc, Table};
use ferb_sessions::terminals::{self, Screen, Terminal};

fn proc(pid: i32, ppid: i32, pgid: i32, tpgid: i32, tty: &str, exe: &str) -> (i32, Proc) {
    (
        pid,
        Proc {
            pid,
            ppid,
            pgid,
            tpgid,
            tty: tty.into(),
            exe: exe.into(),
        },
    )
}

const TERMINAL: &str = "/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal";
const CODE_HELPER: &str =
    "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper.app/Contents/MacOS/Code Helper";

// decision: HAND-692
#[test]
fn every_terminal_is_found_whatever_runs_in_it_and_never_ferb_s_own() {
    let t: Table = [
        proc(100, 1, 100, 0, "", TERMINAL),
        // Terminal's tab: login, the login shell, then a program nobody listed, in front
        proc(110, 100, 110, 130, "/dev/ttys000", "/usr/bin/login"),
        proc(120, 110, 120, 130, "/dev/ttys000", "/bin/zsh"),
        proc(130, 120, 130, 130, "/dev/ttys000", "/u/.local/bin/agy"),
        proc(
            131,
            130,
            130,
            130,
            "/dev/ttys000",
            "/u/.local/bin/agy-helper",
        ),
        // an editor's terminal: its shell in front
        proc(200, 1, 200, 0, "", CODE_HELPER),
        proc(210, 200, 210, 210, "/dev/ttys002", "/bin/zsh"),
        // a terminal FERB itself runs
        proc(
            7,
            1,
            7,
            0,
            "",
            "/Applications/FERB.app/Contents/MacOS/ferbd",
        ),
        proc(300, 7, 300, 300, "/dev/ttys009", "/bin/sh"),
        // a tmux client in Terminal's other tab: its panes are the terminals
        proc(400, 100, 400, 410, "/dev/ttys003", "/bin/zsh"),
        proc(410, 400, 410, 410, "/dev/ttys003", "/opt/homebrew/bin/tmux"),
        proc(500, 1, 500, 0, "", "/opt/homebrew/bin/tmux"),
        proc(510, 500, 510, 510, "/dev/ttys004", "/bin/zsh"),
        // no terminal at all
        proc(600, 1, 600, 0, "", "/usr/bin/caffeinate"),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        terminals::find(&t, 7),
        [
            Terminal {
                tty: "/dev/ttys000".into(),
                leader: 110,
                front: 130
            },
            Terminal {
                tty: "/dev/ttys002".into(),
                leader: 210,
                front: 210
            },
            Terminal {
                tty: "/dev/ttys004".into(),
                leader: 510,
                front: 510
            },
        ]
    );
    assert_eq!(terminals::program_name("-zsh", "/bin/zsh"), "zsh");
    assert_eq!(terminals::program_name("agy", "/u/.local/bin/agy"), "agy");
    assert_eq!(
        terminals::program_name("", "/u/.local/share/claude/versions/2.1.330"),
        "2.1.330"
    );
}

// decision: HAND-692
#[test]
fn a_terminal_s_window_is_the_one_its_app_names_it_by() {
    let w = vec![
        (1, "Desktop — agy — 120×30".to_string()),
        (2, "FERB — -zsh — 80×24".to_string()),
        (3, "FERB — vim — 80×24".to_string()),
    ];
    assert_eq!(
        terminals::window_of(&w, "agy", "Desktop").map(|x| x.0),
        Some(1)
    );
    assert_eq!(
        terminals::window_of(&w, "vim", "FERB").map(|x| x.0),
        Some(3)
    );
    // two windows name the folder and neither the program: FERB can't tell
    assert_eq!(terminals::window_of(&w, "python", "FERB"), None);
    assert_eq!(
        terminals::window_of(&w[..1], "python", "elsewhere").map(|x| x.0),
        Some(1)
    );
}

// decision: HAND-693
#[test]
fn a_terminal_s_text_gives_only_what_is_new() {
    let mut s = Screen::default();
    // the first read shows the screen as it is
    assert_eq!(
        s.read("Last login: Mon\nowner@mac ~ % \n\n\n").as_deref(),
        Some("Last login: Mon\nowner@mac ~ %")
    );
    // typing at the prompt: its line is open, nothing settled yet
    assert_eq!(s.read("Last login: Mon\nowner@mac ~ % ls"), None);
    // the command ran: the settled lines after the last read's
    assert_eq!(
        s.read("Last login: Mon\nowner@mac ~ % ls\na.txt  b.txt\nowner@mac ~ % ")
            .as_deref(),
        Some("owner@mac ~ % ls\na.txt  b.txt")
    );
    // the screen scrolled: the lines that went off the top are known, only the new ones show
    assert_eq!(
        s.read("owner@mac ~ % ls\na.txt  b.txt\nowner@mac ~ % echo hi\nhi\nowner@mac ~ % ")
            .as_deref(),
        Some("owner@mac ~ % echo hi\nhi")
    );
    // a full-screen program redraws: nothing while it changes, the screen once it holds still
    assert_eq!(s.read("Antigravity CLI\n⠋ thinking\n> "), None);
    assert_eq!(s.read("Antigravity CLI\n⠙ thinking\n> "), None);
    assert_eq!(
        s.read("Antigravity CLI\nDone: 3 files\n> ").as_deref(),
        None,
        "changed since the last read"
    );
    assert_eq!(
        s.read("Antigravity CLI\nDone: 3 files\n> ").as_deref(),
        Some("Antigravity CLI\nDone: 3 files\n>")
    );
    assert_eq!(
        s.read("Antigravity CLI\nDone: 3 files\n> "),
        None,
        "shown once"
    );
    assert!(terminals::shows("x\n% run the  tests\n", "run the tests"));
    assert!(!terminals::shows("x\n% run\n", "run the tests"));
}
