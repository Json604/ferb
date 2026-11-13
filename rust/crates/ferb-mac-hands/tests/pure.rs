//! The pure parts against what the Python build produced (decision: ENG-3): rust/fixtures/mac-hands/make.py ran the
//! Python code on invented data and wrote pure.json. Every case must match; one deliberate difference is named where
//! it is compared (typed characters outside the Basic Multilingual Plane, ENG-6).

mod common;

use std::sync::Arc;

use common::{deep_spec, load, py_sha, FakeTree, Src};
use ferb_mac_hands::ax::{outermost, Ax, AxNode, AxTree, El, Rect};
use ferb_mac_hands::keys::{self, Button, Event, Planned, Route};
use ferb_mac_hands::mask::{self, PageItem, PageState, WebSecrets};
use ferb_mac_hands::{aftercheck, applescript, apps, locks, ocr, permissions, profile, wav};
use serde_json::{json, Map, Value};

fn f(v: &Value) -> f64 {
    v.as_f64().expect("number")
}
fn rect(v: &Value) -> Rect {
    (f(&v[0]), f(&v[1]), f(&v[2]), f(&v[3]))
}
fn rects(v: &Value) -> Vec<Rect> {
    v.as_array()
        .map(|a| a.iter().map(rect).collect())
        .unwrap_or_default()
}
fn rj(r: &Rect) -> Value {
    json!([r.0, r.1, r.2, r.3])
}
fn rsj(rs: &[Rect]) -> Value {
    Value::from(rs.iter().map(rj).collect::<Vec<_>>())
}
fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

// decision: HAND-200, HAND-218, HAND-219
#[test]
fn keys_and_gestures() {
    let fx = load("pure.json");
    let k = &fx["keys"];
    let mut n = 0;
    for c in k["combos"].as_array().unwrap() {
        let combo = s(&c["combo"]);
        assert_eq!(
            json!(keys::normalize(combo)),
            c["normalize"],
            "normalize {combo:?}"
        );
        assert_eq!(
            json!(keys::valid_combo(combo)),
            c["valid"],
            "valid {combo:?}"
        );
        assert_eq!(
            json!(keys::screen_wide(combo)),
            c["screen_wide"],
            "screen_wide {combo:?}"
        );
        assert_eq!(
            json!(keys::quit_or_hide(combo)),
            c["quit_or_hide"],
            "quit_or_hide {combo:?}"
        );
        assert_eq!(
            json!(keys::enters_text(combo)),
            c["enters_text"],
            "enters_text {combo:?}"
        );
        n += 1;
    }
    for c in k["shortcuts"].as_array().unwrap() {
        assert_eq!(
            keys::shortcut_combo(s(&c["char"]), c["mods"].as_i64().unwrap()),
            s(&c["combo"])
        );
    }
    let g = &k["gestures"];
    let check = |steps: Vec<Planned>, want: &Value, label: &str| {
        let got: Vec<Value> = steps.iter().map(planned_json).collect();
        assert_eq!(Value::from(got), *want, "{label}");
    };
    for c in g["click"].as_array().unwrap() {
        let a = &c["args"];
        check(
            keys::click_steps(f(&a[0]), f(&a[1]), a[2].as_i64().map(|p| p as i32)),
            &c["steps"],
            "click",
        );
    }
    for c in g["wheel"].as_array().unwrap() {
        let a = &c["args"];
        let steps = keys::wheel_steps(
            f(&a[0]),
            f(&a[1]),
            a[2].as_i64().unwrap() as i32,
            a[3].as_i64().map(|p| p as i32),
            a[4].as_bool().unwrap(),
            a[5].as_i64().map(|w| w as u32),
            a[6].as_bool().unwrap(),
            a[7].as_bool().unwrap(),
            (f(&a[8][0]), f(&a[8][1])),
        );
        check(steps, &c["steps"], "wheel");
    }
    for c in g["type"].as_array().unwrap() {
        let a = &c["args"];
        let text = s(&a[0]);
        let mut want = c["steps"].clone();
        // ENG-6: Python sets len(ch) (code points) as the UTF-16 length, so a character outside the BMP is sent as
        // its lone high surrogate (the fixture reads it back as unreadable, null); Rust sends both halves.
        for (i, ch) in text.chars().enumerate() {
            if ch.len_utf16() == 2 {
                for e in want[i]["events"].as_array_mut().unwrap() {
                    assert_eq!(e["chars"], Value::Null);
                    e["chars"] = json!(ch.to_string());
                }
            }
        }
        check(
            keys::type_steps(text, a[1].as_i64().map(|p| p as i32)),
            &want,
            "type",
        );
    }
    for c in g["key"].as_array().unwrap() {
        let a = &c["args"];
        // a key event's characters are the system's translation of its key code, not something FERB sets
        let mut want = c["steps"].clone();
        for st in want.as_array_mut().unwrap() {
            for e in st["events"].as_array_mut().unwrap() {
                e["chars"] = Value::Null;
            }
        }
        check(
            keys::key_steps(s(&a[0]), a[1].as_i64().map(|p| p as i32)).unwrap(),
            &want,
            "key",
        );
    }
    for c in g["pointer"].as_array().unwrap() {
        let a = &c["args"];
        let route = a[1].as_bool().unwrap();
        let steps = keys::pointer_steps(
            (10.0, 20.0),
            (110.0, 70.0),
            s(&a[0]),
            33,
            if route { Some(8) } else { None },
            route,
            (5.0, 5.0),
        )
        .unwrap();
        check(steps, &c["steps"], "pointer");
    }
    assert!(n > 50);
}

fn route_json(r: &Route) -> Value {
    match r {
        Route::Hid => json!({"pid": null}),
        Route::Process(p) => json!({"pid": p}),
        Route::Window { pid, window, local } => {
            json!({"pid": pid, "window": window, "local": [local.0, local.1]})
        }
    }
}

fn planned_json(p: &Planned) -> Value {
    let events: Vec<Value> = p
        .events
        .iter()
        .map(|(e, r)| {
            let route = route_json(r);
            match e {
                Event::Moved { at } => json!({"type": 5, "route": route, "at": [at.0, at.1], "clicks": 1}),
                Event::Dragged { at } => json!({"type": 6, "route": route, "at": [at.0, at.1], "clicks": 1}),
                Event::Down { button, at, clicks } => {
                    let t = if *button == Button::Right { 3 } else { 1 };
                    json!({"type": t, "route": route, "at": [at.0, at.1], "clicks": clicks})
                }
                Event::Up { button, at, clicks } => {
                    let t = if *button == Button::Right { 4 } else { 2 };
                    json!({"type": t, "route": route, "at": [at.0, at.1], "clicks": clicks})
                }
                Event::Wheel { pixels, vertical, horizontal, at } => {
                    json!({"type": 22, "route": route, "at": [at.0, at.1], "v": vertical, "h": horizontal, "pixels": pixels})
                }
                Event::Char { ch, down } => json!({"type": if *down { 10 } else { 11 }, "route": route, "keycode": 0,
                                                   "flags": 0, "chars": ch.to_string()}),
                Event::Key { code, flags, down } => json!({"type": if *down { 10 } else { 11 }, "route": route,
                                                           "keycode": code, "flags": flags, "chars": null}),
            }
        })
        .collect();
    json!({"what": p.what, "events": events})
}

fn state_of(v: &Value) -> Option<PageState> {
    if v.is_null() {
        return None;
    }
    Some(PageState {
        top: s(&v["top"]).into(),
        viewport: rect(&v["viewport"]),
        items: v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| PageItem {
                key: s(&i["key"]).into(),
                rect: rect(&i["rect"]),
                paint: i["paint"].as_bool().unwrap(),
                child: i["child"].as_str().map(String::from),
            })
            .collect(),
    })
}

fn web_of(v: &Value) -> Option<WebSecrets> {
    if v.is_null() {
        return None;
    }
    Some(WebSecrets {
        state: state_of(&v["state"]),
        why: s(&v["why"]).into(),
        late: v["late"].as_bool().unwrap(),
        tab: v["tab"]
            .as_array()
            .map(|t| (s(&t[0]).into(), s(&t[1]).into())),
    })
}

fn web_json(w: &WebSecrets) -> Value {
    json!({"state": w.state.as_ref().map(|st| json!({"top": st.top, "viewport": rj(&st.viewport),
        "items": st.items.iter().map(|i| json!({"key": i.key, "rect": rj(&i.rect), "paint": i.paint, "child": i.child})).collect::<Vec<_>>()})),
        "why": w.why, "late": w.late, "tab": w.tab.as_ref().map(|t| json!([t.0, t.1]))})
}

fn tree_of(v: &Value) -> Option<AxTree> {
    if v.is_null() {
        return None;
    }
    let mut t = AxTree::default();
    for n in v["nodes"].as_array().unwrap() {
        t.nodes.push(AxNode::new(
            El::new(()),
            "textbox",
            "pw",
            Some("filled"),
            rect(&n["rect"]),
            n["secret"].as_bool().unwrap(),
            true,
        ));
    }
    t.secure = v["secure"].as_bool();
    t.secret_rects = if v["secret_rects"].is_null() {
        None
    } else {
        Some(rects(&v["secret_rects"]))
    };
    t.web_areas = rects(&v["web_areas"]);
    t.empty_web_areas = rects(&v["empty_web_areas"]);
    t.has_web = v["has_web"].as_bool().unwrap();
    Some(t)
}

// decision: HAND-518
#[test]
fn masks_match_python() {
    let fx = load("pure.json");
    let m = &fx["masks"];
    for c in m["page_masks"].as_array().unwrap() {
        let (a, b) = (
            state_of(&c["before"]).unwrap(),
            state_of(&c["after"]).unwrap(),
        );
        let got = match mask::page_masks(&a, &b) {
            Ok((r, vp)) => json!([rsj(&r), rj(&vp)]),
            Err(e) => json!({"withheld": e.0}),
        };
        assert_eq!(got, c["out"]);
    }
    for c in m["to_image_boxes"].as_array().unwrap() {
        let size = (
            c["size"][0].as_i64().unwrap(),
            c["size"][1].as_i64().unwrap(),
        );
        let got: Vec<Value> = mask::to_image_boxes(&rects(&c["rects"]), rect(&c["bounds"]), size)
            .iter()
            .map(|b| json!([b.0, b.1, b.2, b.3]))
            .collect();
        assert_eq!(Value::from(got), c["out"]);
    }
    for c in m["page_on_screen"].as_array().unwrap() {
        let got = match mask::page_on_screen(
            &rects(&c["rects"]),
            rect(&c["viewport"]),
            &rects(&c["areas"]),
        ) {
            Ok(r) => rsj(&r),
            Err(e) => json!({"withheld": e.0}),
        };
        assert_eq!(got, c["out"]);
    }
    let mut n = 0;
    for c in m["window_masks"].as_array().unwrap() {
        let before = (tree_of(&c["before"][0]), web_of(&c["before"][1]));
        let after = (tree_of(&c["after"][0]), web_of(&c["after"][1]));
        let (masks, note) = mask::window_masks(&before, &after, c["browser"].as_bool().unwrap());
        assert_eq!(rsj(&masks), c["masks"], "case {n}");
        assert_eq!(note, s(&c["note"]), "case {n}");
        n += 1;
    }
    assert_eq!(n, 400);
    for c in m["page_tab"].as_array().unwrap() {
        let titles: Vec<String> = c["titles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| s(t).to_string())
            .collect();
        let looks: Vec<mask::Look> = c["looks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| (None, web_of(w)))
            .collect();
        let refs: Vec<&mask::Look> = looks.iter().collect();
        let got = mask::page_tab(&titles, &refs);
        assert_eq!(
            got.map(|t| json!([t.0, t.1])).unwrap_or(Value::Null),
            c["out"]
        );
    }
    for c in m["from_reply"].as_array().unwrap() {
        assert_eq!(web_json(&WebSecrets::from_reply(&c["reply"])), c["out"]);
    }
    for c in m["same_title"].as_array().unwrap() {
        assert_eq!(json!(mask::same_title(s(&c[0]), s(&c[1]))), c[2]);
    }
    for c in m["outermost"].as_array().unwrap() {
        assert_eq!(rsj(&outermost(&rects(&c["areas"]))), c["out"]);
    }
}

struct NoRecognize;
impl ocr::Recognize for NoRecognize {
    fn recognize(&self, _: &[u8]) -> Result<Vec<ocr::TextRegion>, String> {
        Ok(vec![])
    }
}

#[test]
fn ocr_regions_match_python() {
    let fx = load("pure.json");
    let redactor = ferb_sanitize::Redactor::new(&[], &[]);
    for c in fx["ocr"].as_array().unwrap() {
        let lines: Vec<String> = c["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| s(l).to_string())
            .collect();
        assert_eq!(
            json!(ocr::neighbour_spans(&lines)),
            c["neighbours"],
            "{lines:?}"
        );
        let regions: Vec<ocr::TextRegion> = lines
            .iter()
            .enumerate()
            .map(|(i, t)| ocr::TextRegion {
                text: t.clone(),
                bbox: (i as i64 * 10, i as i64 * 20, 30 + i as i64, 15),
            })
            .collect();
        let given: Vec<mask::ImgBox> = c["rects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| {
                (
                    b[0].as_i64().unwrap(),
                    b[1].as_i64().unwrap(),
                    b[2].as_i64().unwrap(),
                    b[3].as_i64().unwrap(),
                )
            })
            .collect();
        let boxes = ocr::boxes_to_paint(&redactor, &regions, &given).unwrap();
        let painted = if boxes.is_empty() {
            Value::Null
        } else {
            json!(boxes
                .iter()
                .map(|b| json!([b.0, b.1, b.2, b.3]))
                .collect::<Vec<_>>())
        };
        assert_eq!(painted, c["painted"], "{lines:?}");
    }
    let _ = NoRecognize;
}

fn node_json(n: &AxNode) -> Value {
    json!({"ref": FakeTree::id_of(&n.r#ref), "role": n.role, "label": n.label, "value": n.value, "rect": rj(&n.rect),
           "secret": n.secret, "enabled": n.enabled, "ax_role": n.ax_role, "actions": n.actions,
           "selected": n.selected, "expanded": n.expanded, "container": n.container, "subrole": n.subrole,
           "path": n.path, "index": n.index, "identity": Value::Object(n.identity())})
}

fn full_tree_json(t: &AxTree) -> Value {
    let sig = t.signature();
    json!({"nodes": t.nodes.iter().map(node_json).collect::<Vec<_>>(), "text": t.text, "title": t.title,
           "truncated": t.truncated, "has_web": t.has_web, "truncated_by": t.truncated_by, "secure": t.secure,
           "secret_rects": t.secret_rects.as_ref().map(|r| rsj(r)), "web_areas": rsj(&t.web_areas),
           "empty_web_areas": rsj(&t.empty_web_areas), "headings": t.headings, "windows": t.windows,
           "containers": t.containers, "web_empty": t.web_empty,
           "window": t.window.as_ref().map(FakeTree::id_of), "masks_only": t.masks_only, "offset": t.offset,
           "next_offset": t.next_offset, "repeated_nodes": t.repeated_nodes,
           "signature": [sig.0, sig.1, sig.2, sig.3.iter().map(|n| json!([n.0, n.1, n.2, n.3, n.4, n.5])).collect::<Vec<_>>()]})
}

// decision: HAND-221, HAND-518
#[test]
fn accessibility_walks_match_python() {
    let fx = load("pure.json");
    let w = &fx["walks"];
    let deep = deep_spec(
        w["deep"][0].as_u64().unwrap() as usize,
        w["deep"][1].as_u64().unwrap() as usize,
    );
    for c in w["cases"].as_array().unwrap() {
        let spec = if c["spec"] == "deep" {
            &deep
        } else {
            &w["specs"][s(&c["spec"])]
        };
        let tree = Arc::new(FakeTree::from_spec(spec, f(&c["step"])));
        let ax = Ax::new(Src(tree.clone()));
        let kw = &c["kw"];
        let window = kw.get("window").map(|id| tree.el(s(id)));
        let got = ax.read_tree(
            c["pid"].as_i64().unwrap() as i32,
            kw.get("browser").and_then(Value::as_bool).unwrap_or(false),
            window.as_ref(),
            kw.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
            kw.get("budget_s").and_then(Value::as_f64),
            kw.get("masks_only")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        );
        let out = full_tree_json(&got);
        let name = s(&c["name"]);
        if c["sha"].is_null() {
            assert_eq!(out, c["tree"], "{name}");
        } else {
            assert_eq!(
                got.nodes.len() as u64,
                c["counts"]["nodes"].as_u64().unwrap(),
                "{name}"
            );
            assert_eq!(json!(got.next_offset), c["counts"]["next_offset"], "{name}");
            assert_eq!(py_sha(&out), s(&c["sha"]), "{name}");
        }
        assert_eq!(tree.now(), f(&c["clock_after"]), "{name}: clock reads");
    }
}

#[test]
fn accessibility_helpers_match_python() {
    let fx = load("pure.json");
    let h = &fx["ax_helpers"];
    let tree = Arc::new(FakeTree::from_spec(&fx["walks"]["specs"]["chat"], 0.0));
    let ax = Ax::new(Src(tree.clone()));
    let id = |e: Option<El>| e.map(|e| json!(FakeTree::id_of(&e))).unwrap_or(Value::Null);
    let browser_page = |a: Option<&str>| {
        let t = ferb_sanitize::parity::casefold(ferb_py::value::strip(a.unwrap_or("")));
        [
            "chrome://",
            "brave://",
            "edge://",
            "arc://",
            "vivaldi://",
            "opera://",
            "chrome-extension://",
            "extension://",
            "moz-extension://",
            "safari-web-extension://",
            "about:",
        ]
        .iter()
        .any(|p| t.starts_with(p))
            && !["about:blank", "about:srcdoc"]
                .iter()
                .any(|p| t.starts_with(p))
    };
    assert_eq!(
        json!(ax
            .ax_windows(21)
            .iter()
            .map(FakeTree::id_of)
            .collect::<Vec<_>>()),
        h["ax_windows"]
    );
    assert_eq!(id(ax.app_window(21)), h["app_window"]);
    let win = tree.el("win");
    assert_eq!(json!(ax.windows(21, Some(&win))), h["windows"]);
    for (k, v) in h["window_named"].as_object().unwrap() {
        assert_eq!(id(ax.window_named(21, k)), *v, "window_named {k:?}");
    }
    for c in h["window_with_frame"].as_array().unwrap() {
        assert_eq!(
            id(ax.window_with_frame(21, rect(&c["bounds"]), c["title"].as_str(), None)),
            c["out"]
        );
    }
    assert_eq!(json!(ax.menus(21)), h["menus"]);
    for c in h["menu_item_for_shortcut"].as_array().unwrap() {
        let mods: Vec<String> = c["mods"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| s(m).to_string())
            .collect();
        let got = ax
            .menu_item_for_shortcut(21, s(&c["key"]), &mods)
            .map(|(e, p, en)| json!([FakeTree::id_of(&e), p, en]));
        assert_eq!(got.unwrap_or(Value::Null), c["out"]);
    }
    for c in h["menu_item_shortcut"].as_array().unwrap() {
        let path: Vec<String> = c["path"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| s(m).to_string())
            .collect();
        assert_eq!(
            ax.menu_item_shortcut(21, &path)
                .map(|(a, b)| json!([a, b]))
                .unwrap_or(Value::Null),
            c["out"]
        );
    }
    for (k, v) in h["surface_facts"].as_object().unwrap() {
        assert_eq!(
            json!(ax.surface_facts(Some(&tree.el(k)))),
            *v,
            "surface_facts {k}"
        );
    }
    for (k, v) in h["page_url"].as_object().unwrap() {
        assert_eq!(
            json!(ax.page_url(&tree.el(k), &browser_page)),
            *v,
            "page_url {k}"
        );
    }
    for (k, v) in h["place_of"].as_object().unwrap() {
        let (a, b) = ax.place_of(&tree.el(k));
        assert_eq!(json!([a, b]), *v);
    }
    for (k, v) in h["secure_element"].as_object().unwrap() {
        assert_eq!(
            json!(ax.secure_element(Some(&tree.el(k)))),
            *v,
            "secure {k}"
        );
    }
    let facts: Vec<Value> = ax
        .window_facts(&tree.el("box"))
        .into_iter()
        .take(6)
        .map(|f| json!(f))
        .collect();
    assert_eq!(Value::from(facts), h["window_facts"]);
    for (k, v) in h["has_focus"].as_object().unwrap() {
        assert_eq!(json!(ax.has_focus(21, &tree.el(k))), *v, "has_focus {k}");
    }
}

#[test]
fn browser_list_and_app_lookup_match_python() {
    let fx = load("pure.json");
    let a = &fx["apps"];
    let handlers = apps::WebHandlers::default();
    let running = |pid: Option<i32>, _name: Option<&str>| -> Vec<String> {
        if pid == Some(77) {
            vec!["org.mozilla.firefox".into()]
        } else {
            vec![]
        }
    };
    for (k, v) in a["known"].as_object().unwrap() {
        assert_eq!(json!(apps::known(k)), *v, "known {k:?}");
    }
    for (k, v) in a["is_browser_app"].as_object().unwrap() {
        assert_eq!(
            json!(apps::is_browser_app(&handlers, k, &running)),
            *v,
            "is_browser_app {k:?}"
        );
    }
    for (k, v) in a["clean"].as_object().unwrap() {
        assert_eq!(json!(apps::clean(k)), *v);
    }
    for (k, v) in a["pair_key"].as_object().unwrap() {
        assert_eq!(json!(apps::pair_key(k)), *v, "pair_key {k:?}");
    }
    assert_eq!(json!(apps::data_folders()), a["data_folders"]);
    let list = vec![
        apps::RunningApp::new("net.whatsapp.WhatsApp", "\u{200e}WhatsApp", 21, false),
        apps::RunningApp::new("com.apple.TextEdit", "TextEdit", 22, true),
        apps::RunningApp::new("com.apple.TextEdit", "TextEdit", 23, false),
        apps::RunningApp::new("com.microsoft.VSCode", "Code", 24, false),
    ];
    for (k, v) in a["candidates"].as_object().unwrap() {
        let got: Vec<i32> = apps::candidates(&list, k).iter().map(|x| x.pid).collect();
        assert_eq!(json!(got), *v, "candidates {k:?}");
    }
    let conforms_set: Vec<(String, String)> = a["conforms"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (s(&p[0]).into(), s(&p[1]).into()))
        .collect();
    let conforms = |x: &str, y: &str| conforms_set.iter().any(|(a, b)| a == x && b == y);
    for (i, p) in a["plists"].as_array().unwrap().iter().enumerate() {
        let info = if p.is_null() { None } else { Some(p) };
        assert_eq!(json!(apps::shell_role(info)), a["shell_role"][i]);
        let cases = [
            ("public.shell-script", ""),
            ("com.apple.terminal.shell-script", "x"),
            ("", "sh"),
            ("public.data", "zsh"),
        ];
        for (j, (kind, ext)) in cases.iter().enumerate() {
            assert_eq!(
                json!(apps::editor_role(info, kind, ext, &conforms)),
                a["editor_role"][i][j],
                "editor {i} {j}"
            );
        }
    }
}

#[test]
fn permission_rows_match_python() {
    let fx = load("pure.json");
    let p = &fx["permissions"];
    let grants = [Map::new(), json!({"accessibility": "granted", "screen_recording": "denied", "full_disk_access": "granted",
                                     "input_monitoring": "not_determined"}).as_object().unwrap().clone()];
    let contain = [
        Map::new(),
        json!({"profile_ok": true, "shell": true, "proxy": true})
            .as_object()
            .unwrap()
            .clone(),
        json!({"profile_ok": true, "shell": true, "proxy": true, "proxy_alive": false})
            .as_object()
            .unwrap()
            .clone(),
    ];
    let mut i = 0;
    for (gi, g) in grants.iter().enumerate() {
        for c in &contain {
            let restart: Vec<String> = if gi > 0 {
                vec!["screen_recording".into()]
            } else {
                vec![]
            };
            assert_eq!(json!(permissions::rows(g, c, &restart)), p["rows"][i]);
            i += 1;
        }
    }
    for (j, st) in [
        Some("allow"),
        Some("unrestricted"),
        Some("default"),
        Some("ask"),
        Some("deny"),
        None,
        Some("odd"),
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(permissions::clipboard_row(*st), p["clipboard_rows"][j]);
    }
    let fresh: [&[u8]; 4] = [
        b"{\"screen_recording\": \"granted\", \"x\": \"y\", \"input_monitoring\": 3}",
        b"[1]",
        b"nope",
        b"{\"input_monitoring\": \"denied\"}",
    ];
    for (j, x) in fresh.iter().enumerate() {
        assert_eq!(Value::Object(permissions::fresh_answer(x)), p["fresh"][j]);
    }
    let restart = permissions::restart_needed(
        json!({"screen_recording": "denied", "input_monitoring": "granted"})
            .as_object()
            .unwrap(),
        json!({"screen_recording": "granted", "input_monitoring": "granted"})
            .as_object()
            .unwrap(),
    );
    assert_eq!(json!(restart), p["restart"]);
    let report = permissions::report_rows(&json!({"permissions": {"accessibility": "granted"},
                                                  "containment": {"profile_ok": true}, "clipboard": "ask"}));
    assert_eq!(json!(report), p["report_rows"]);
    assert_eq!(
        json!(permissions::missing(&report)
            .iter()
            .map(|(a, b)| json!([a, b]))
            .collect::<Vec<_>>()),
        p["missing"]
    );
    for (k, v) in p["panes"].as_object().unwrap() {
        assert_eq!(json!(permissions::pane_url(k)), *v);
    }
    assert_eq!(
        Value::Object(permissions::app_panes("com.ferb.ferb")),
        p["app_panes"]
    );
    for (k, v) in p["ae"].as_object().unwrap() {
        assert_eq!(permissions::ae_status(k.parse().unwrap()).value(), s(v));
    }
    for (k, v) in p["hid"].as_object().unwrap() {
        assert_eq!(permissions::hid_status(k.parse().unwrap()).value(), s(v));
    }
    for (j, c) in contain.iter().enumerate() {
        assert_eq!(json!(permissions::fda_ready(c)), p["fda_ready"][j]);
    }
}

#[test]
fn locks_after_wav_profiles_scripts_match_python() {
    let fx = load("pure.json");
    for c in fx["locks"]["ordered"].as_array().unwrap() {
        let keys: Vec<String> = c["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| s(k).to_string())
            .collect();
        match locks::ordered(&keys) {
            Ok(o) => assert_eq!(json!(o), c["out"]),
            Err(e) => assert_eq!(e, s(&c["error"])),
        }
    }
    for c in fx["after"].as_array().unwrap() {
        if let Some(facts) = c.get("facts") {
            let content = json!({"facts": facts});
            let after = aftercheck::after_of(&aftercheck::facts_of(content.as_object().unwrap()));
            assert_eq!(after, c["after"], "{facts}");
            let expects = [
                json!({"value_contains": "X"}),
                json!({"changed": true}),
                json!({"changed": false}),
                json!({}),
            ];
            for (i, e) in expects.iter().enumerate() {
                assert_eq!(
                    aftercheck::compare(e.as_object().unwrap(), &after),
                    c["compare"][i]
                );
            }
        } else {
            assert_eq!(
                Value::Object(aftercheck::facts_of(c["content"].as_object().unwrap())),
                c["facts_of"]
            );
        }
    }
    let dir = tempfile::tempdir().unwrap();
    for (i, c) in fx["wav"].as_array().unwrap().iter().enumerate() {
        let path = dir.path().join(format!("{i}.wav"));
        let w = wav::WavWriter::create(&path).unwrap();
        for ch in c["chunks"].as_array().unwrap() {
            let value = f(&ch[0]) as f32;
            let vals: Vec<f32> = (0..3)
                .flat_map(|_| [value, -value / 2.0, value / 4.0])
                .collect();
            let mut bytes: Vec<u8> = vals.iter().flat_map(|v| v.to_ne_bytes()).collect();
            bytes.push(1);
            w.write(&bytes, ch[1].as_u64().unwrap() as u32, ch[2].as_f64())
                .unwrap();
        }
        let facts = w.close();
        assert_eq!(
            json!({"frames": facts.frames, "sample_rate": facts.sample_rate, "audio_s": facts.audio_s, "peak": facts.peak}),
            c["facts"]
        );
        let data = std::fs::read(&path).unwrap();
        assert_eq!(hex::encode(&data[..44]), s(&c["head"]));
        use sha2::Digest;
        assert_eq!(hex::encode(sha2::Sha256::digest(&data)), s(&c["sha"]));
    }
    // profiles: the same files and dicts
    let pdir = tempfile::tempdir().unwrap();
    let stamps = std::sync::Mutex::new(1);
    let now: profile::Now = Arc::new(move || {
        let mut n = stamps.lock().unwrap();
        let t = format!("2026-01-0{}T00:00:00+00:00", *n);
        *n += 1;
        t
    });
    let p = profile::Profiles::new(Some(pdir.path()), now.clone());
    p.record("com.x/y z", "ax_press", true);
    p.record("com.x/y z", "ax_press", false);
    p.record("com.x/y z", "pid_keys", false);
    let pf = &fx["profiles"];
    for (name, text) in pf["files"].as_object().unwrap() {
        assert_eq!(
            std::fs::read_to_string(pdir.path().join(name)).unwrap(),
            s(text)
        );
    }
    assert_eq!(p.as_dict("com.x/y z"), pf["as_dict"]);
    assert_eq!(
        json!([
            p.status("com.x/y z", "ax_press"),
            p.status("com.x/y z", "nope")
        ]),
        pf["status"]
    );
    std::fs::write(pdir.path().join("bad.json"), "{nope").unwrap();
    let q = profile::Profiles::new(Some(pdir.path()), now);
    assert_eq!(q.as_dict("com.x/y z"), pf["reload"]);
    let mut bad = q.as_dict("bad");
    let diags = bad.as_object_mut().unwrap().remove("diagnostics").unwrap();
    assert_eq!(bad, pf["bad"]);
    assert_eq!(
        json!(diags
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["operation"].clone())
            .collect::<Vec<_>>()),
        pf["bad_diag_ops"]
    );
    // AppleScript
    let sc = &fx["scripts"];
    let vols = tempfile::tempdir().unwrap();
    for v in sc["volumes_present"].as_array().unwrap() {
        std::fs::create_dir(vols.path().join(s(v))).unwrap();
    }
    for c in sc["scripts"].as_array().unwrap() {
        let script = s(&c["script"]);
        assert_eq!(
            json!(applescript::runs_shell(script)),
            c["runs_shell"],
            "{script:?}"
        );
        assert_eq!(
            json!(applescript::names_own_app(script)),
            c["own_app"],
            "{script:?}"
        );
        let paths: Vec<String> = applescript::named_paths(script, s(&sc["home"]), vols.path())
            .into_iter()
            .map(|p| p.replace(&vols.path().to_string_lossy().into_owned(), "<VOLUMES>"))
            .collect();
        assert_eq!(json!(paths), c["named_paths"], "{script:?}");
    }
    assert_eq!(
        applescript::wrap("com.apple.Notes", "get x").unwrap(),
        s(&sc["wrap"][0])
    );
    assert!(applescript::wrap("com.apple.Notes\"", "x").is_err());
}
