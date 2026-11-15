//! Keys and the input events FERB posts (ferb/act/mac/input.py): combos as the model writes them, the shortcut guard
//! (HAND-200, HAND-219), the secure-field rule (HAND-218), and each gesture as a list of steps of plain events the
//! native layer posts (`native::events`). Pure: nothing here posts anything.

use std::collections::BTreeSet;

/// input.py:262.
pub const SCREEN_WIDE: &str = "this key combination acts on the owner's whole Mac (switching apps or desktops, \
Spotlight, Mission Control, Force Quit, locking or logging out), not on one app, so FERB doesn't press it: work in the \
app itself (HAND-200)";

/// Virtual key codes (Carbon Events.h, US layout), input.py:205.
pub fn keycode(key: &str) -> Option<u16> {
    let letters = "asdfhgzxcvbqweryt";
    let letter_codes = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12, 13, 14, 15, 16, 17];
    let row = "123465=97-80]ou[ip";
    let row_codes = [
        18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35,
    ];
    let rest = "lj'k;\\,/nm.`";
    let rest_codes = [37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 50];
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        for (set, codes) in [
            (letters, &letter_codes[..]),
            (row, &row_codes[..]),
            (rest, &rest_codes[..]),
        ] {
            if let Some(i) = set.chars().position(|x| x == c) {
                return Some(codes[i]);
            }
        }
        return None;
    }
    Some(match key {
        "return" => 36,
        "tab" => 48,
        "space" => 49,
        "delete" => 51,
        "escape" => 53,
        "forwarddelete" => 117,
        "home" => 115,
        "end" => 119,
        "pageup" => 116,
        "pagedown" => 121,
        "help" => 114,
        "left" => 123,
        "right" => 124,
        "down" => 125,
        "up" => 126,
        "f1" => 122,
        "f2" => 120,
        "f3" => 99,
        "f4" => 118,
        "f5" => 96,
        "f6" => 97,
        "f7" => 98,
        "f8" => 100,
        "f9" => 101,
        "f10" => 109,
        "f11" => 103,
        "f12" => 111,
        "f13" => 105,
        "f14" => 107,
        "f15" => 113,
        "f16" => 106,
        "f17" => 64,
        "f18" => 79,
        "f19" => 80,
        "capslock" => 57,
        "fn" => 63,
        "volumeup" => 72,
        "volumedown" => 73,
        "mute" => 74,
        "keypad0" => 82,
        "keypad1" => 83,
        "keypad2" => 84,
        "keypad3" => 85,
        "keypad4" => 86,
        "keypad5" => 87,
        "keypad6" => 88,
        "keypad7" => 89,
        "keypad8" => 91,
        "keypad9" => 92,
        "keypadenter" => 76,
        "keypadplus" => 69,
        "keypadminus" => 78,
        "keypadmultiply" => 67,
        "keypaddivide" => 75,
        "keypaddecimal" => 65,
        "keypadequals" => 81,
        "keypadclear" => 71,
        _ => return None,
    })
}

fn key_alias(k: &str) -> &str {
    match k {
        "enter" => "return",
        "esc" => "escape",
        "backspace" => "delete",
        "del" | "fwddelete" => "forwarddelete",
        "pgup" => "pageup",
        "pgdn" | "pgdown" => "pagedown",
        "spacebar" => "space",
        "comma" => ",",
        "period" => ".",
        "slash" => "/",
        "semicolon" => ";",
        "quote" => "'",
        "minus" => "-",
        "equals" | "equal" | "plus" => "=",
        "backslash" => "\\",
        "grave" => "`",
        "leftbracket" => "[",
        "rightbracket" => "]",
        "arrowleft" => "left",
        "arrowright" => "right",
        "arrowup" => "up",
        "arrowdown" => "down",
        k => k,
    }
}

/// The modifiers in their fixed order (dict order of input.py:225) and their CGEventFlags bits.
pub const MODIFIERS: [(&str, u64); 5] = [
    ("cmd", 1 << 20),
    ("shift", 1 << 17),
    ("alt", 1 << 19),
    ("ctrl", 1 << 18),
    ("fn", 1 << 23),
];

fn modifier_alias(m: &str) -> &str {
    match m {
        "command" | "meta" | "super" => "cmd",
        "option" | "opt" => "alt",
        "control" | "ctl" => "ctrl",
        "function" => "fn",
        m => m,
    }
}

fn modifier_index(m: &str) -> Option<usize> {
    MODIFIERS.iter().position(|(n, _)| *n == m)
}

/// Python's `str.lower()` for the ASCII-and-beyond text a combo holds.
fn lower(s: &str) -> String {
    ferb_py::text::lower(s)
}

fn strip(s: &str) -> &str {
    s.trim_matches(|c: char| ferb_py::text::is_space(c))
}

/// input.py:230: "Cmd+Shift+S" -> ["cmd", "shift", "s"]; aliases resolved, modifiers in a fixed order, the key last.
pub fn normalize(combo: &str) -> Vec<String> {
    let squeezed = combo.replace(' ', "");
    let mut parts: Vec<String> = squeezed
        .split('+')
        .filter(|p| !strip(p).is_empty())
        .map(|p| lower(strip(p)))
        .collect();
    if strip(combo) == "+" {
        parts = vec!["=".to_string()];
    }
    let Some(last) = parts.pop() else {
        return vec![];
    };
    let all_mods = parts
        .iter()
        .all(|p| modifier_index(modifier_alias(p)).is_some());
    let mut mods: Vec<String> = if all_mods {
        let set: BTreeSet<usize> = parts
            .iter()
            .filter_map(|p| modifier_index(modifier_alias(p)))
            .collect();
        set.into_iter()
            .map(|i| MODIFIERS[i].0.to_string())
            .collect()
    } else {
        parts
            .iter()
            .map(|p| modifier_alias(p).to_string())
            .collect()
    };
    mods.push(key_alias(&last).to_string());
    mods
}

/// input.py:320.
pub fn valid_combo(combo: &str) -> bool {
    let parts = normalize(combo);
    match parts.split_last() {
        Some((key, mods)) => {
            keycode(key).is_some() && mods.iter().all(|m| modifier_index(m).is_some())
        }
        None => false,
    }
}

fn split(combo: &str) -> Option<(String, BTreeSet<String>)> {
    let mut parts = normalize(combo);
    let key = parts.pop()?;
    Some((key, parts.into_iter().collect()))
}

fn has(mods: &BTreeSet<String>, m: &str) -> bool {
    mods.contains(m)
}

/// input.py:268: a key combination that acts on the owner's whole Mac (HAND-200, HAND-219).
pub fn screen_wide(combo: &str) -> bool {
    let Some((key, mods)) = split(combo) else {
        return false;
    };
    let arrows = ["left", "right", "up", "down"];
    (key == "tab" && has(&mods, "cmd"))
        || (key == "space" && (has(&mods, "cmd") || has(&mods, "ctrl")))
        || (arrows.contains(&key.as_str()) && has(&mods, "ctrl"))
        || (key == "escape" && has(&mods, "cmd") && has(&mods, "alt"))
        || ((key == "f3" || key == "f4") && mods.is_empty())
        || (key == "q" && has(&mods, "cmd") && has(&mods, "ctrl"))
        || (key == "q" && has(&mods, "cmd") && has(&mods, "shift"))
}

/// input.py:283: "quit" for ⌘Q, "hide" for ⌘H, "hide_others" for ⌥⌘H.
pub fn quit_or_hide(combo: &str) -> Option<&'static str> {
    let (key, mods) = split(combo)?;
    let only =
        |names: &[&str]| mods.len() == names.len() && names.iter().all(|n| mods.contains(*n));
    if key == "q" && only(&["cmd"]) {
        return Some("quit");
    }
    if key == "h" && only(&["cmd"]) {
        return Some("hide");
    }
    if key == "h" && only(&["cmd", "alt"]) {
        return Some("hide_others");
    }
    None
}

/// input.py:299: a menu item's key equivalent (AXMenuItemCmdChar, AXMenuItemCmdModifiers) as a combo.
pub fn shortcut_combo(ch: &str, modifiers: i64) -> String {
    let mut mods: Vec<&str> = if modifiers & 8 != 0 {
        vec![]
    } else {
        vec!["cmd"]
    };
    for (bit, name) in [(1, "shift"), (2, "alt"), (4, "ctrl")] {
        if modifiers & bit != 0 {
            mods.push(name);
        }
    }
    let low = lower(ch);
    mods.push(&low);
    mods.join("+")
}

/// input.py:307 (HAND-218): a key press that puts text into the focused field.
pub fn enters_text(combo: &str) -> bool {
    let Some((key, mods)) = split(combo) else {
        return false;
    };
    if key == "v" && has(&mods, "cmd") {
        return true;
    }
    (key.chars().count() == 1
        || key == "space"
        || key.starts_with("keypad") && key != "keypadenter")
        && !has(&mods, "cmd")
        && !has(&mods, "ctrl")
}

/// The CGEventFlags of a normalized combo's modifiers.
pub fn flags(parts: &[String]) -> u64 {
    parts[..parts.len().saturating_sub(1)]
        .iter()
        .filter_map(|m| modifier_index(m).map(|i| MODIFIERS[i].1))
        .fold(0, |a, b| a | b)
}

// ---------- gestures as plain events ----------

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

/// One event, as `native::events` creates and posts it (marked with FERB's mark).
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Moved {
        at: (f64, f64),
    },
    Down {
        button: Button,
        at: (f64, f64),
        clicks: i64,
    },
    Up {
        button: Button,
        at: (f64, f64),
        clicks: i64,
    },
    Dragged {
        at: (f64, f64),
    },
    /// Wheel units (pixels or lines), vertical and horizontal, at a point.
    Wheel {
        pixels: bool,
        vertical: i32,
        horizontal: i32,
        at: (f64, f64),
    },
    /// One character as a key down then up with its Unicode string (keycode 0).
    Char {
        ch: char,
        down: bool,
    },
    Key {
        code: u16,
        flags: u64,
        down: bool,
    },
}

/// Where an event goes: the HID tap (real input), one process (CGEventPostToPid), or one window of one process
/// through the optional SkyLight route, at a window-local point.
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    Hid,
    Process(i32),
    Window {
        pid: i32,
        window: u32,
        local: (f64, f64),
    },
}

/// One step of a gesture: a name and the events posted together (a click's down and up are one step, so a yield or
/// a failed check never leaves the button held down).
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    pub what: String,
    pub events: Vec<(Event, Route)>,
}

fn to(pid: Option<i32>) -> Route {
    pid.map(Route::Process).unwrap_or(Route::Hid)
}

/// input.py:154: a move then a click.
pub fn click_steps(x: f64, y: f64, pid: Option<i32>) -> Vec<Planned> {
    let at = (x, y);
    vec![
        Planned {
            what: "move".into(),
            events: vec![(Event::Moved { at }, to(pid))],
        },
        Planned {
            what: "click".into(),
            events: vec![
                (
                    Event::Down {
                        button: Button::Left,
                        at,
                        clicks: 1,
                    },
                    to(pid),
                ),
                (
                    Event::Up {
                        button: Button::Left,
                        at,
                        clicks: 1,
                    },
                    to(pid),
                ),
            ],
        },
    ]
}

/// input.py:173 (negative lines = down).
#[allow(clippy::too_many_arguments)]
pub fn wheel_steps(
    x: f64,
    y: f64,
    lines: i32,
    pid: Option<i32>,
    horizontal: bool,
    window: Option<u32>,
    route: bool,
    pixels: bool,
    window_origin: (f64, f64),
) -> Vec<Planned> {
    let ev = Event::Wheel {
        pixels,
        vertical: if horizontal { 0 } else { lines },
        horizontal: if horizontal { lines } else { 0 },
        at: (x, y),
    };
    let r = match (route, pid, window) {
        (true, Some(pid), Some(window)) => Route::Window {
            pid,
            window,
            local: (x - window_origin.0, y - window_origin.1),
        },
        _ => to(pid),
    };
    vec![Planned {
        what: format!("scroll {}", ferb_py_signed(lines)),
        events: vec![(ev, r)],
    }]
}

fn ferb_py_signed(n: i32) -> String {
    if n >= 0 {
        format!("+{n}")
    } else {
        n.to_string()
    }
}

/// input.py:190: one step per character.
pub fn type_steps(text: &str, pid: Option<i32>) -> Vec<Planned> {
    let n = text.chars().count();
    text.chars()
        .enumerate()
        .map(|(i, ch)| Planned {
            what: format!("type {}/{}", i + 1, n),
            events: vec![
                (Event::Char { ch, down: true }, to(pid)),
                (Event::Char { ch, down: false }, to(pid)),
            ],
        })
        .collect()
}

/// input.py:244: one key press; None for a key `valid_combo` refuses.
pub fn key_steps(combo: &str, pid: Option<i32>) -> Option<Vec<Planned>> {
    let parts = normalize(combo);
    let code = keycode(parts.last()?)?;
    let f = flags(&parts);
    Some(vec![Planned {
        what: combo.to_string(),
        events: vec![
            (
                Event::Key {
                    code,
                    flags: f,
                    down: true,
                },
                to(pid),
            ),
            (
                Event::Key {
                    code,
                    flags: f,
                    down: false,
                },
                to(pid),
            ),
        ],
    }])
}

/// input.py:103: a bounded process-directed gesture, and the release its cleanup posts when a button is held.
pub fn pointer_steps(
    start: (f64, f64),
    end: (f64, f64),
    kind: &str,
    pid: i32,
    window: Option<u32>,
    route: bool,
    window_origin: (f64, f64),
) -> Option<Vec<Planned>> {
    if !["click", "dblclick", "rightclick", "hover", "drag"].contains(&kind) {
        return None;
    }
    let button = if kind == "rightclick" {
        Button::Right
    } else {
        Button::Left
    };
    let via = |pt: (f64, f64)| match (route, window) {
        (true, Some(window)) => Route::Window {
            pid,
            window,
            local: (pt.0 - window_origin.0, pt.1 - window_origin.1),
        },
        _ => Route::Process(pid),
    };
    let mut steps = vec![Planned {
        what: "pointer move".into(),
        events: vec![(Event::Moved { at: start }, via(start))],
    }];
    if kind != "hover" {
        let counts = if kind == "dblclick" { 2 } else { 1 };
        for count in 1..=counts {
            steps.push(Planned {
                what: "pointer down".into(),
                events: vec![(
                    Event::Down {
                        button,
                        at: start,
                        clicks: count,
                    },
                    via(start),
                )],
            });
            if kind == "drag" {
                for i in 1..=20 {
                    let k = i as f64 / 20.0;
                    let pt = (
                        start.0 + (end.0 - start.0) * k,
                        start.1 + (end.1 - start.1) * k,
                    );
                    steps.push(Planned {
                        what: "pointer drag".into(),
                        events: vec![(Event::Dragged { at: pt }, via(pt))],
                    });
                }
            }
            let up_at = if kind == "drag" { end } else { start };
            steps.push(Planned {
                what: "pointer up".into(),
                events: vec![(
                    Event::Up {
                        button,
                        at: up_at,
                        clicks: count,
                    },
                    via(up_at),
                )],
            });
        }
    }
    Some(steps)
}
