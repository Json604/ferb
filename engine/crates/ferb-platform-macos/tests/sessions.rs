//! The Mac's side of the coding sessions (HAND-654, HAND-656, HAND-659) on this test's own process only: its row in
//! the process table, its ancestors, and the scripts Terminal would get (built, never run).
#![cfg(target_os = "macos")]

use ferb_platform_macos::sessions::*;

// decision: HAND-654
#[test]
fn the_process_table_holds_this_process_with_its_parent_and_program() {
    let me = std::process::id() as i32;
    let t = processes();
    let p = t.get(&me).expect("this process is listed");
    assert_eq!(p.ppid, unsafe { libc::getppid() });
    assert_eq!(
        std::path::Path::new(&p.exe).canonicalize().ok(),
        std::env::current_exe().unwrap().canonicalize().ok()
    );
    assert_eq!(p.pgid, unsafe { libc::getpgrp() });
    assert_eq!(parent_of(me), p.ppid);
    let up = my_ancestors();
    assert_eq!(up.first().copied(), Some(p.ppid));
    assert!(!up.contains(&1));
    assert!(!cwd_of(me).is_empty());
}

// decision: HAND-656
#[test]
fn terminal_gets_the_exact_text_as_one_message() {
    assert_eq!(terminal_payload("one line"), "one line");
    assert_eq!(terminal_payload("a\nb"), "\u{1b}[200~a\nb\u{1b}[201~");
    assert_eq!(
        applescript_string("say \"hi\" \\ \u{1b}x\n"),
        "\"say \\\"hi\\\" \\\\ \" & (ASCII character 27) & \"x\" & linefeed & \"\""
    );
    let s = terminal_type_script("/dev/ttys004", "npm test");
    assert!(s.contains("if tty of t is \"/dev/ttys004\" then"), "{s}");
    assert!(s.contains("do script \"npm test\" in t"), "{s}");
    assert!(!s.contains("activate"), "never brought to the front");
}

// decision: HAND-659
#[test]
fn what_ferb_runs_for_sessions_starts_from_an_empty_environment() {
    std::env::set_var("CLAUDE_CODE_CHILD_SESSION", "1");
    let c = clean_command("/usr/bin/env", std::path::Path::new("/tmp"));
    let out = run(c, std::time::Duration::from_secs(5)).unwrap();
    assert!(!out.contains("CLAUDE_CODE_CHILD_SESSION"), "{out}");
    assert!(out.contains("HOME=/tmp"), "{out}");
}
