//! The context readers and lookups through the real binary: a run token's call over the MCP shim's socket path
//! (`tools.call`), the gateway's whole path, on a temp home. ferbd's own ports (abilities, memory.read, about.read,
//! skills.read's result) are compared with engine/fixtures/ferbd/tools.json (make_tools.py); the others are the
//! ported crates' runs, checked for what each must return on invented data.

mod shim;
mod support;

use serde_json::{json, Value};
use shim::*;
use support::*;

fn package() -> std::path::PathBuf {
    repo().join("engine/runtime")
}

/// PRM-376: the browser verification ability's reason, as Python wrote it and as FERB writes it now.
const REASON_PYTHON: &str =
    "The browser verification workflow and held-input lifecycle are not installed; the \
                             procedure is available to read, but ability execution is unavailable.";
const REASON_NOW: &str = "No one-call runner; follow the procedure with the browser tools.";

/// A recorded abilities result as FERB gives it now: the named differences since the recording (PRM-376's reason, and
/// a procedure replaced whole, engine/fixtures/catalog/prompt_replacements.json: PRM-382's browser_verification
/// SKILL.md, whose recorded body must be the text Python read). Every other byte is compared as recorded.
fn as_now(recorded: &Value) -> Value {
    let text = serde_json::to_string(recorded)
        .unwrap()
        .replace(REASON_PYTHON, REASON_NOW);
    let mut v: Value = serde_json::from_str(&text).unwrap();
    let replacements = fixture("catalog/prompt_replacements.json");
    if let Some(body) = v.get("body").and_then(Value::as_str).map(str::to_string) {
        for r in replacements["replacements"].as_array().unwrap() {
            let prompt = r["prompt"].as_str().unwrap();
            let Some(id) = prompt
                .strip_prefix("abilities/")
                .and_then(|p| p.strip_suffix("/SKILL.md"))
            else {
                continue;
            };
            if v["id"] == id {
                assert_eq!(
                    body.trim_end(),
                    r["old"].as_str().unwrap(),
                    "{prompt}: the recorded body"
                );
                v["body"] = json!(std::fs::read_to_string(package().join(prompt)).unwrap());
            }
        }
    }
    v
}

// decision: PRM-376, PRM-382
#[test]
fn abilities_and_about_match_python() {
    let f = fixture("ferbd/tools.json");
    let catalog = fixture("catalog/tools.json");
    let all: Vec<String> = catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    use ferbd::compose::tools::context as c;
    let mut compared = 0;
    for set in f["abilities"].as_array().unwrap() {
        let installed: std::collections::BTreeSet<String> = match set["installed"].as_str().unwrap()
        {
            "all" => all.iter().cloned().collect(),
            "no_browser_wait" => all
                .iter()
                .filter(|n| *n != "browser.wait")
                .cloned()
                .collect(),
            _ => Default::default(),
        };
        let got = c::abilities_list(&package(), &installed);
        assert_eq!(
            Value::Object(got.content),
            as_now(&set["list"]["content"]),
            "list {}",
            set["installed"]
        );
        compared += 1;
        for (name, want) in set["read"].as_object().unwrap() {
            match c::abilities_read(&package(), &installed, name) {
                Ok(r) => assert_eq!(
                    Value::Object(r.content),
                    as_now(&want["content"]),
                    "read {name}"
                ),
                Err(e) => {
                    assert_eq!(want["raised"], "Refused", "{name}");
                    assert_eq!(e.message, want["message"].as_str().unwrap(), "{name}");
                }
            }
            compared += 1;
        }
    }
    let doc = f["about_read"]["document"].as_object().unwrap().clone();
    for call in f["about_read"]["calls"].as_array().unwrap() {
        let r = c::about_read(call["args"]["section"].as_str(), Ok(doc.clone())).unwrap();
        assert_eq!(
            Value::Object(r.content),
            call["out"]["content"],
            "{}",
            call["args"]
        );
        assert_eq!(r.is_error, call["out"]["is_error"].as_bool().unwrap());
        compared += 1;
    }
    assert_eq!(compared, 3 * 5 + 4);
}

#[test]
fn context_readers_run_through_the_binary() {
    let f = fixture("ferbd/tools.json");
    let tmp = root();
    let (run, mut shim) = started(tmp.path(), "01");

    // abilities.list: the registry holds every browser tool the abilities need, as the "all" recording
    let listed = shim.call("abilities.list", json!({}));
    assert_eq!(
        listed["content"],
        as_now(&f["abilities"][0]["list"]["content"]),
        "{listed}"
    );
    let read = shim.call("abilities.read", json!({"name": "google_docs"}));
    assert_eq!(
        read["content"]["body"],
        f["abilities"][0]["read"]["google_docs"]["content"]["body"]
    );
    let refused = shim.call("abilities.read", json!({"name": "../google_docs"}));
    assert_eq!(refused["is_error"], true, "{refused}");
    assert!(refused["content"]
        .to_string()
        .contains("Choose an installed ability ID"));

    // skills.read: the owner's skills folder in FERB's home
    for (rel, text) in f["skills"]["files"].as_object().unwrap() {
        let p = run.home.join("skills").join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text.as_str().unwrap()).unwrap();
    }
    std::fs::write(tmp.path().join("outside.txt"), "not a skill\n").unwrap();
    std::os::unix::fs::symlink(
        tmp.path().join("outside.txt"),
        run.home.join("skills/packing/link.txt"),
    )
    .unwrap();
    for call in f["skills"]["calls"].as_array().unwrap() {
        let got = shim.call("skills.read", call["args"].clone());
        match call["out"].get("raised") {
            None => assert_eq!(got["content"], call["out"]["content"], "{}", call["args"]),
            Some(_) => {
                assert_eq!(got["is_error"], true, "{got}");
                assert!(
                    got["content"]
                        .to_string()
                        .contains(call["out"]["message"].as_str().unwrap()),
                    "{} {got}",
                    call["args"]
                );
            }
        }
    }

    // about.read: the About area's document; a section narrows it
    let about = shim.call("about.read", json!({"section": "version"}));
    assert_eq!(
        about["content"],
        json!({"version": {"ferb": "0.28.0"}}),
        "{about}"
    );
    let bad = shim.call("about.read", json!({"section": "bogus"}));
    assert_eq!(bad["is_error"], true);

    // trail.query / trail.days on an empty trail
    let q = shim.call("trail.query", json!({}));
    assert_eq!(q["is_error"], false, "{q}");
    assert_eq!(q["content"]["events"], json!([]), "{q}");
    let d = shim.call("trail.days", json!({}));
    assert_eq!(d["content"]["count"], 0, "{d}");

    // task.evidence: the live task's own records
    let ev = shim.call("task.evidence", json!({}));
    assert_eq!(ev["is_error"], false, "{ev}");

    // chat.history: the task's tree has no entries yet
    let ch = shim.call("chat.history", json!({}));
    assert_eq!(
        ch["content"],
        json!({"records": [], "next_after": null, "total_records": 0}),
        "{ch}"
    );

    // conversation.read: none in this tree
    let cr = shim.call("conversation.read", json!({}));
    assert_eq!(cr["is_error"], false, "{cr}");
    assert!(cr["content"]["text"]
        .as_str()
        .unwrap()
        .contains("conversations"));

    // secrets.list: nothing saved for this place (the helper is off in tests: the store's own handle files)
    let sl = shim.call("secrets.list", json!({"place": "https://shop.example"}));
    assert!(sl.get("content").is_some(), "{sl}");
    let bad_place = shim.call("secrets.list", json!({"place": "nowhere"}));
    assert_eq!(bad_place["is_error"], true, "{bad_place}");
    // the task's one owner-typed site is read as a link at a sign-in page elsewhere (`HiddenInput._question_sites`):
    // the record saved at that site shows as linked (its entry names only, never a value)
    std::fs::write(
        run.home.join(ferb_identity::owner_secrets::FILE_NAME),
        serde_json::to_vec(&json!({"version": 2, "records": {"https://bookslots.example": {
            "username": null, "sites": [], "entries": {"password": "an invented account"}}}, "unusable": []}))
        .unwrap(),
    )
    .unwrap();
    run.control(json!({"do": "record_request", "request_id": "req-1",
                        "words": "book a slot on bookslots.example",
                        "targets": {"sites": ["bookslots.example"]}}));
    let linked = shim.call(
        "secrets.list",
        json!({"place": "https://login.example.org"}),
    );
    assert_eq!(linked["is_error"], false, "{linked}");
    assert!(
        linked["content"].to_string().contains("bookslots.example"),
        "{linked}"
    );

    // an owner browser tool with no profile paired: the resolver's refusal (areas/browsers.py:260 `Resolver`), not a
    // fixed stand-in; profiles.json's case "bindings 0, named null" has these words
    let opened = shim.call("browser.open", json!({"url": "https://shop.example/"}));
    assert_eq!(opened["is_error"], true, "{opened}");
    assert!(
        opened["content"].to_string().contains(
            "no browser profile is paired with FERB yet: pick the owner's profile from browser_profiles and pair it \
             with browser_pair (FERB asks the owner once)"
        ),
        "{opened}"
    );

    // memory.learned on an empty learning store
    let ml = shim.call("memory.learned", json!({"query": "bicycle"}));
    assert_eq!(ml["is_error"], false, "{ml}");
    assert!(
        ml["content"]["text"]
            .as_str()
            .unwrap()
            .contains("\"total\": 0"),
        "{ml}"
    );
}

#[test]
fn memory_read_matches_python_through_the_binary() {
    let f = fixture("ferbd/tools.json");
    let tmp = root();
    let (run, mut shim) = started(tmp.path(), "01");
    // the Python store's rows, put into this daemon's store (the journal's own sequence left to it)
    let mut statements = Vec::new();
    for (table, t) in f["memory"]["rows"].as_object().unwrap() {
        let mut cols: Vec<String> = t["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect();
        let skip = if table == "journal" {
            cols.iter().position(|c| c == "seq")
        } else {
            None
        };
        if let Some(i) = skip {
            cols.remove(i);
        }
        for row in t["rows"].as_array().unwrap() {
            let mut vals: Vec<Value> = row.as_array().unwrap().clone();
            if let Some(i) = skip {
                vals.remove(i);
            }
            let marks = vec!["?"; cols.len()].join(", ");
            statements.push(json!([
                format!(
                    "INSERT INTO \"{table}\" ({}) VALUES ({marks})",
                    cols.iter()
                        .map(|c| format!("\"{c}\""))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                vals
            ]));
        }
    }
    run.control(json!({"do": "sql", "statements": statements}));
    // the Python clock's time when the calls were made
    run.control(json!({"do": "clock", "advance_s": 9.0 * 60.0}));
    let mut compared = 0;
    for call in f["memory"]["calls"].as_array().unwrap() {
        let got = shim.call("memory.read", call["args"].clone());
        assert_eq!(got["content"], call["out"]["content"], "{}", call["args"]);
        assert_eq!(got["is_error"], call["out"]["is_error"], "{}", call["args"]);
        compared += 1;
    }
    assert_eq!(compared, 14);
}

fn b64(text: &str) -> Vec<u8> {
    // the fixture's files, standard base64 (no crate needed for a test this small)
    let table = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.bytes().filter(|c| *c != b'=') {
        let v = table.iter().position(|t| *t == c).unwrap() as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

#[test]
fn fs_look_decodes_as_python() {
    use ferb_fs::look::{LookError, Renderer};
    use sha2::Digest;
    let f = fixture("ferbd/tools.json");
    let r = ferbd::native::MacRenderer;
    let mut same_bytes = 0;
    for call in f["look"]["calls"].as_array().unwrap() {
        let data = b64(f["look"]["files"][call["file"].as_str().unwrap()]
            .as_str()
            .unwrap());
        let got = match call["page"].as_i64() {
            Some(n) => r.pdf_page(&data, n, 1568),
            None => r.picture(&data, 1568),
        };
        let want = &call["out"];
        match got {
            Ok((jpeg, facts)) => {
                assert_eq!(Value::Object(facts), want["facts"], "{call}");
                assert!(jpeg.starts_with(&[0xFF, 0xD8]), "a JPEG");
                if hex::encode(sha2::Sha256::digest(&jpeg)) == want["jpeg_sha256"] {
                    same_bytes += 1;
                }
            }
            Err(LookError::Undecodable(why)) => assert_eq!(why, want["undecodable"], "{call}"),
            Err(LookError::NoPage(n)) => assert_eq!(n, want["no_page"], "{call}"),
        }
    }
    // ImageIO's thumbnail, the white bitmap and the JPEG helper: the same bytes as Python's
    assert_eq!(same_bytes, 4, "pictures byte for byte as Python's");
}

/// Invented noise (a small LCG): the worst case for JPEG, so a picture of it is as large as one gets.
fn noise(len: usize) -> Vec<u8> {
    let mut x: u32 = 0x2545_F491;
    (0..len)
        .map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (x >> 24) as u8
        })
        .collect()
}

/// A 24-bit BMP of noise, `w`x`h`.
fn noise_bmp(w: u32, h: u32) -> Vec<u8> {
    let row = (w * 3).div_ceil(4) * 4;
    let size = 54 + row * h;
    let mut b = Vec::with_capacity(size as usize);
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&size.to_le_bytes());
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(&54u32.to_le_bytes());
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&w.to_le_bytes());
    b.extend_from_slice(&h.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    b.extend_from_slice(&[0; 24]);
    let pixels = noise((row * h) as usize);
    b.extend_from_slice(&pixels);
    b
}

/// A one-page PDF whose page is a noise image `w`x`h` drawn over the whole US Letter page.
fn noise_pdf(w: u32, h: u32) -> Vec<u8> {
    let image = noise((w * h * 3) as usize);
    let content = b"q 612 0 0 792 0 0 cm /Im1 Do Q\n";
    let mut out: Vec<u8> = b"%PDF-1.4\n".to_vec();
    let mut at = Vec::new();
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /XObject << /Im1 4 0 R >> >> /Contents 5 0 R >>"
            .to_vec(),
        [
            format!("<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length {} >>\nstream\n", image.len()).into_bytes(),
            image,
            b"\nendstream".to_vec(),
        ]
        .concat(),
        [
            format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(),
            content.to_vec(),
            b"endstream".to_vec(),
        ]
        .concat(),
    ];
    for (i, o) in objects.iter().enumerate() {
        at.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for a in at {
        out.extend_from_slice(format!("{a:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// RUN-473 (agent): a picture or PDF page as large as a JPEG gets (noise) comes back under the byte target, and the
/// width and height fs.look reports are the picture's own.
// decision: RUN-473
#[test]
fn fs_look_keeps_the_largest_picture_and_page_under_the_byte_target() {
    use ferb_fs::look::Renderer;
    use ferb_mac_hands::shots::IMAGE_BYTES_TARGET;
    let r = ferbd::native::MacRenderer;
    for (what, got) in [
        ("picture", r.picture(&noise_bmp(2000, 1500), 1568)),
        ("page", r.pdf_page(&noise_pdf(600, 800), 1, 1568)),
    ] {
        let (jpeg, facts) = got.unwrap_or_else(|e| panic!("{what}: {e:?}"));
        assert!(
            jpeg.len() <= IMAGE_BYTES_TARGET,
            "{what}: {} bytes",
            jpeg.len()
        );
        let (w, h) = ferb_mac_hands::native::paint::size(&jpeg).unwrap();
        assert_eq!(
            (facts["width"].as_i64(), facts["height"].as_i64()),
            (Some(w), Some(h)),
            "{what}: {facts:?}"
        );
        assert!(w.max(h) >= 320, "{what}: still a picture to read ({w}x{h})");
        eprintln!("{what}: {} bytes at {w}x{h}", jpeg.len());
    }
}

#[test]
fn fs_look_through_the_binary() {
    let f = fixture("ferbd/tools.json");
    let tmp = root();
    let (run, mut shim) = started(tmp.path(), "01");
    let ws = run.home.join("workspace");
    for (name, data) in f["look"]["files"].as_object().unwrap() {
        std::fs::write(ws.join(name), b64(data.as_str().unwrap())).unwrap();
    }
    let page = shim.call("fs.look", json!({"path": "doc.pdf", "page": 1}));
    assert_eq!(page["is_error"], false, "{page}");
    for k in ["format", "page", "pages", "width", "height", "text"] {
        assert_eq!(
            page["content"][k], f["look"]["calls"][3]["out"]["facts"][k],
            "{k}: {page}"
        );
    }
    // a daemon without the Mac's parts has no text recognition: the image is withheld, never sent unmasked
    assert!(page.get("image").is_none(), "{page}");
    let none = shim.call("fs.look", json!({"path": "doc.pdf", "page": 3}));
    assert!(
        none["content"]["error"]
            .as_str()
            .unwrap()
            .contains("has 2 pages; there is no page 3"),
        "{none}"
    );
    let text = shim.call("fs.look", json!({"path": "text.txt"}));
    assert!(
        text["content"]["error"]
            .as_str()
            .unwrap()
            .contains("as a picture or a PDF"),
        "{text}"
    );
    // recovery.snapshot: snapshots are off in a daemon without the Mac's parts, and it says so
    let snap = shim.call(
        "recovery.snapshot",
        json!({"before": "a test's bulk change"}),
    );
    assert_eq!(snap["is_error"], true, "{snap}");
    assert!(
        snap["content"]["error"]
            .as_str()
            .unwrap()
            .contains("turned off"),
        "{snap}"
    );
}

#[test]
fn browser_tools_without_a_browser_connected() {
    let tmp = root();
    let (run, mut shim) = started(tmp.path(), "01");
    // the owner's router: no browser connected, no tabs
    let tabs = shim.call("browser.tabs", json!({}));
    assert_eq!(tabs["content"], json!({"tabs": []}), "{tabs}");
    // FERB's own profile's driver: its tools, once attached, answer through it (no profile of its own here)
    run.control(json!({"do": "attach_identity"}));
    let token = run.control(
        json!({"do": "issue", "task_id": "task-1", "worker_id": "w-2",
                                   "identity": "ferb", "scopes": ["identity:browse"]}),
    );
    let mut ferb = Shim::open(&run, token.as_str().unwrap());
    let open = ferb.call("ferb_browser.open", json!({"url": "https://example.com"}));
    assert_eq!(open["is_error"], true, "{open}");
    assert!(
        !open.to_string().contains("not available in this build"),
        "{open}"
    );
    let handoff = shim.call(
        "browser.handoff",
        json!({"tab": "t-none", "message": "a check"}),
    );
    assert_eq!(handoff["is_error"], true, "{handoff}");
}
