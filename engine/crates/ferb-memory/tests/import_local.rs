//! The coding assistants' prompt history, claude.ai's manifest export and the export checks (MEM-432..438), on
//! invented data in the shapes the memory-import lab recorded (its tests/fakes.py and tests/test_exports.py; values
//! made up). Temporary folders only; nothing under the real home is read.

use std::fs;
use std::path::Path;

use ferb_memory::import_exports as exports;
use ferb_memory::import_local as local;
use ferb_memory::import_sources::{self as sources, ReadOptions};
use ferb_memory::times::{self, DateTime, FixedZone};
use ferb_memory::Error;
use serde_json::{json, Value};

const ZONE: FixedZone = FixedZone(0);
const S1: &str = "11111111-1111-4111-8111-111111111111";
const S2: &str = "22222222-2222-4222-8222-222222222222";

fn at(text: &str) -> DateTime {
    times::parse_iso(text).unwrap()
}

fn now() -> DateTime {
    at("2026-11-10T12:00:00+00:00")
}

fn ms(text: &str) -> i64 {
    times::instant(&at(text)) / 1000
}

/// A prompt-history file: rows of (time, session, folder, text), in each tool's own keys.
fn history(home: &Path, tool: &str, rows: &[(&str, &str, &str, &str)]) {
    let rel = if tool == "claude_code" {
        ".claude/history.jsonl"
    } else {
        ".codex/history.jsonl"
    };
    let path = home.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    for (when, session, folder, said) in rows {
        let t = ms(&format!("{when}+00:00"));
        let row = if tool == "claude_code" {
            json!({"display": said, "timestamp": t, "sessionId": session,
                   "project": format!("/Users/someone/code/{folder}")})
        } else {
            json!({"text": said, "ts": t / 1000, "session_id": session})
        };
        text.push_str(&row.to_string());
        text.push('\n');
    }
    fs::write(path, text).unwrap();
}

fn redact(t: &str) -> String {
    t.to_owned()
}

fn read(
    source: &str,
    path: Option<&Path>,
    home: &Path,
) -> Result<Vec<sources::Conversation>, Error> {
    let r = redact;
    let opts = ReadOptions {
        now: now(),
        redact: &r,
        home: Some(home),
        zone: &ZONE,
        unzstd: None,
        months: ferb_memory::import_sources::WINDOW_MONTHS,
    };
    sources::read(source, path, &opts)
}

// decision: MEM-432, MEM-433
#[test]
fn what_was_typed_is_read_by_session_and_day_with_noise_and_repeats_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    history(
        home,
        "claude_code",
        &[
            (
                "2026-11-01T10:00:00",
                S1,
                "garden",
                "I grow tomatoes on the balcony",
            ),
            ("2026-11-01T10:01:00", S1, "garden", "/clear"),
            (
                "2026-11-01T10:02:00",
                S1,
                "garden",
                "[Request interrupted by user]",
            ),
            (
                "2026-11-01T10:03:00",
                S1,
                "garden",
                "<local-command-stdout>ran</local-command-stdout>",
            ),
            (
                "2026-11-02T10:00:00",
                S2,
                "shed",
                "I grow tomatoes on the balcony",
            ),
            ("2026-11-02T10:05:00", S2, "shed", "make the shed door blue"),
            ("2026-11-03T09:00:00", S2, "shed", "I keep bees too"),
            ("2026-05-01T10:00:00", S2, "shed", "far too old to be read"),
        ],
    );
    let convs = read("claude_code", None, home).unwrap();
    let got: Vec<(String, String, Vec<String>)> = convs
        .iter()
        .map(|c| {
            (
                c.id.clone(),
                c.title.clone(),
                c.messages.iter().map(|m| m.text.clone()).collect(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (
                format!("{S1}@2026-11-01"),
                "in the folder garden".to_owned(),
                vec!["I grow tomatoes on the balcony".to_owned()]
            ),
            (
                format!("{S2}@2026-11-02"),
                "in the folder shed".to_owned(),
                vec!["make the shed door blue".to_owned()]
            ),
            (
                format!("{S2}@2026-11-03"),
                "in the folder shed".to_owned(),
                vec!["I keep bees too".to_owned()]
            ),
        ]
    );
    assert!(convs
        .iter()
        .all(|c| c.messages.iter().all(|m| m.role == "owner")));
    assert_eq!(convs[1].place, "shed");
    // a later day only adds a conversation: the earlier ones read the same, so their parts are not read again
    history(
        home,
        "claude_code",
        &[("2026-11-10T11:00:00", S2, "shed", "I also sell honey")],
    );
    let later = read("claude_code", None, home).unwrap();
    assert_eq!(later[..3], convs[..]);
    assert_eq!(later.len(), 4);
}

// decision: MEM-432
#[test]
fn codex_folder_comes_from_the_session_files_first_line_only() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    history(
        home,
        "codex",
        &[
            ("2026-11-03T09:00:00", S1, "", "I use a split keyboard"),
            ("2026-11-03T09:10:00", S2, "", "this session's file is gone"),
        ],
    );
    let day = home.join(".codex/sessions/2026/11/03");
    fs::create_dir_all(&day).unwrap();
    fs::write(
        day.join(format!("rollout-2026-11-03T09-00-00-{S1}.jsonl")),
        format!(
            "{}\n{}\n",
            json!({"type": "session_meta", "payload": {"id": S1, "cwd": "/Users/someone/code/key board"}}),
            json!({"type": "response_item", "payload": {"cwd": "/somewhere/else"}})
        ),
    )
    .unwrap();
    assert_eq!(
        local::codex_folders(home),
        vec![(S1.to_owned(), "key board".to_owned())]
    );
    let rows = local::lines(home, "codex", &at("2026-10-01T00:00:00+00:00"), &now());
    let folders: Vec<(&str, &str)> = rows
        .iter()
        .map(|l| (l.session.as_str(), l.folder.as_str()))
        .collect();
    assert_eq!(folders, vec![(S1, "key board"), (S2, "")]);
    let convs = read("codex", None, home).unwrap();
    assert_eq!(convs[0].title, "in the folder key board");
    assert_eq!(convs[1].title, "");
    assert_eq!(local::found(home), vec!["codex"]);
}

// decision: MEM-432, MEM-458
#[test]
fn claude_codes_saved_notes_are_claims_within_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    history(
        home,
        "claude_code",
        &[(
            "2026-11-01T10:00:00",
            S1,
            "garden",
            "by the way I live in Lisbon these days",
        )],
    );
    let notes = home.join(".claude/projects/-code-garden/memory");
    fs::create_dir_all(&notes).unwrap();
    for (name, text, days_ago) in [
        ("home.md", "I live in Lisbon", 1),
        ("pet.md", "I own a horse", 1),
        ("old.md", "I live in Lisbon", 200),
    ] {
        let p = notes.join(name);
        fs::write(&p, text).unwrap();
        let t = std::time::UNIX_EPOCH
            + std::time::Duration::from_secs(
                (times::instant(&now()) / 1_000_000 - days_ago * 86_400) as u64,
            );
        fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }
    let convs = read("claude_code", None, home).unwrap();
    let mut claims: Vec<String> = convs
        .iter()
        .filter(|c| c.claims)
        .map(|c| c.id.clone())
        .collect();
    claims.sort();
    assert_eq!(
        claims,
        vec!["memory--code-garden-home", "memory--code-garden-pet"]
    );
    assert!(convs
        .iter()
        .filter(|c| c.claims)
        .all(|c| c.messages.iter().all(|m| m.role == "assistant")));
}

// decision: MEM-432
#[test]
fn no_history_says_where_it_looked_and_a_folder_given_is_still_read_as_transcripts() {
    let dir = tempfile::tempdir().unwrap();
    let Err(Error::Unreadable(m)) = read("codex", None, dir.path()) else {
        panic!("a missing history must say so")
    };
    assert!(m.starts_with("no Codex CLI prompt history at "), "{m}");
    let Err(Error::Unreadable(m)) = read("codex", Some(&dir.path().join("nope")), dir.path())
    else {
        panic!()
    };
    assert!(m.starts_with("no Codex sessions at"), "{m}");
}

// ---------- claude.ai's export ----------

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for b in data {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A zip of stored members (name -> JSON value).
fn zipped(path: &Path, members: &[(&str, Value)]) {
    let (mut out, mut dir) = (Vec::new(), Vec::new());
    for (name, value) in members {
        let data = value.to_string().into_bytes();
        let crc = crc32(&data);
        let offset = out.len() as u32;
        let head = |sig: &[u8], central: bool| {
            let mut h = sig.to_vec();
            if central {
                h.extend(20u16.to_le_bytes());
            }
            h.extend(20u16.to_le_bytes());
            h.extend([0, 0, 0, 0, 0, 0, 0, 0]);
            h.extend(crc.to_le_bytes());
            h.extend((data.len() as u32).to_le_bytes());
            h.extend((data.len() as u32).to_le_bytes());
            h.extend((name.len() as u16).to_le_bytes());
            h.extend(0u16.to_le_bytes());
            if central {
                h.extend([0u8; 6]);
                h.extend(0u32.to_le_bytes());
                h.extend(offset.to_le_bytes());
            }
            h.extend(name.as_bytes());
            h
        };
        out.extend(head(b"PK\x03\x04", false));
        out.extend(&data);
        dir.extend(head(b"PK\x01\x02", true));
    }
    let start = out.len() as u32;
    out.extend(&dir);
    out.extend(b"PK\x05\x06\0\0\0\0");
    out.extend((members.len() as u16).to_le_bytes());
    out.extend((members.len() as u16).to_le_bytes());
    out.extend((dir.len() as u32).to_le_bytes());
    out.extend(start.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    fs::write(path, out).unwrap();
}

/// A conversation as claude.ai's export holds it, every human turn linked to the one before.
fn real_chat(ident: &str, texts: &[&str]) -> Value {
    let mut messages = Vec::new();
    let mut parent = "00000000-0000-4000-8000-000000000000".to_owned();
    for (n, text) in texts.iter().enumerate() {
        let when = format!("2026-11-01T10:{n:02}:00Z");
        messages.push(json!({"uuid": format!("{ident}-m{n}"), "text": text,
            "content": [{"type": "text", "text": text}], "sender": if n % 2 == 0 {"human"} else {"assistant"},
            "created_at": when, "updated_at": when, "attachments": [], "files": [], "parent_message_uuid": parent}));
        parent = format!("{ident}-m{n}");
    }
    json!({"uuid": ident, "name": format!("chat {ident}"), "summary": "", "created_at": "2026-11-01T10:00:00Z",
           "updated_at": "2026-11-01T10:00:00Z", "account": {"uuid": "acc-1"}, "chat_messages": messages})
}

/// claude.ai's export as it comes since late 2026: a manifest of links and one zip per category.
fn new_export(folder: &Path, parts: &[&str], created: &str, with_manifest: bool) {
    fs::create_dir_all(folder).unwrap();
    let names = [
        "light_metadata",
        "projects",
        "memories",
        "feedback",
        "frames",
        "conversations",
    ];
    if with_manifest {
        let files: Vec<Value> = names.iter().enumerate().map(|(n, name)| json!({"batch_index": n,
            "export_url": format!("https://example.invalid/export/x/{name}"), "category": name, "part": 0,
            "filename": format!("{name}-000.zip")})).collect();
        fs::write(
            folder.join("manifest-abc-2026-11-10.json"),
            json!({"instructions": "Download each file using the export_url.", "created_at": created,
                   "total_files": 6, "version": "1.0", "data_files": files})
            .to_string(),
        )
        .unwrap();
    }
    let stamp = "2026-11-01T10:00:00Z";
    for part in parts {
        let members: Vec<(&str, Value)> = match *part {
            "conversations" => vec![(
                "conversations.json",
                json!([real_chat("c1", &["I play the cello", "Noted."])]),
            )],
            "memories" => vec![(
                "memories/mem-1.json",
                json!({"account_uuid": "acc-1", "conversations_memory": "I play the cello",
                       "memory_files": [{"path": "notes/music.md", "content": "I own a horse", "updated_at": stamp}]}),
            )],
            "projects" => vec![
                (
                    "projects/p1.json",
                    json!({"uuid": "p1", "name": "Kite", "description": "I am building a kite app",
                       "is_private": true, "is_starter_project": false, "prompt_template": "", "created_at": stamp,
                       "updated_at": stamp, "docs": [{"uuid": "d1", "filename": "plan.md",
                       "content": "I am a document, not read", "created_at": stamp}]}),
                ),
                (
                    "projects/p0.json",
                    json!({"uuid": "p0", "name": "How to use Claude", "description": "An example project the site made",
                       "is_starter_project": true, "prompt_template": "", "created_at": stamp, "updated_at": stamp}),
                ),
            ],
            "light_metadata" => vec![(
                "users.json",
                json!([{"uuid": "acc-1", "email_address": "someone@example.invalid"}]),
            )],
            "feedback" => vec![(
                "reflections/acc-1.json",
                json!({"reflections": [{"content": "I am a reflection, not read"}]}),
            )],
            _ => vec![(
                "artifacts/a1/versions/v1/memories.json",
                json!({"note": "an artifact's own file that happens to be named memories"}),
            )],
        };
        zipped(&folder.join(format!("{part}-000.zip")), &members);
    }
}

const ALL: [&str; 6] = [
    "conversations",
    "memories",
    "projects",
    "light_metadata",
    "feedback",
    "frames",
];

// decision: MEM-438, MEM-437
#[test]
fn claude_ais_manifest_export_is_read_from_exactly_the_files_it_names() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Downloads");
    new_export(&folder, &ALL, "2026-11-10T08:00:00Z", true);
    zipped(
        &folder.join("someone-elses.zip"),
        &[(
            "conversations.json",
            json!([real_chat("x9", &["I am from another export entirely"])]),
        )],
    );
    for given in [folder.join("manifest-abc-2026-11-10.json"), folder.clone()] {
        let convs = read("claude", Some(&given), dir.path()).unwrap();
        let mut ids: Vec<&str> = convs.iter().map(|c| c.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["c1", "memory-mem-1.json", "project-p1"]);
        let text = format!("{convs:?}");
        for never in [
            "another export",
            "document, not read",
            "reflection",
            "example.invalid",
            "example project",
        ] {
            assert!(!text.contains(never), "{never}");
        }
    }
    // the part zips alone, moved away from the manifest, read the same
    let loose = dir.path().join("Claude export");
    new_export(&loose, &ALL, "", false);
    let convs = read("claude", Some(&loose), dir.path()).unwrap();
    assert_eq!(convs.len(), 3);
}

// decision: MEM-438
#[test]
fn a_manifest_whose_parts_are_missing_or_expired_says_what_to_do_and_never_a_link() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = dir.path().join("fresh");
    new_export(
        &fresh,
        &["projects", "frames"],
        "2026-11-10T09:00:00Z",
        true,
    );
    let Err(Error::Unreadable(m)) = read("claude", Some(&fresh), dir.path()) else {
        panic!()
    };
    for part in [
        "2 of its parts aren't downloaded yet: memories-000.zip, conversations-000.zip",
        "the links stop working in about 21 hours",
        "A link may work only once",
    ] {
        assert!(m.contains(part), "{part}: {m}");
    }
    assert!(!m.contains("example.invalid"));
    let stale = dir.path().join("stale");
    new_export(&stale, &[], "2026-11-09T06:00:00Z", true);
    let Err(Error::Unreadable(m)) = read(
        "claude",
        Some(&stale.join("manifest-abc-2026-11-10.json")),
        dir.path(),
    ) else {
        panic!()
    };
    assert!(
        m.contains("so they have expired: ask for a new export"),
        "{m}"
    );
    let cut = dir.path().join("cut");
    new_export(&cut, &ALL, "2026-11-10T08:00:00Z", true);
    let whole = fs::read(cut.join("conversations-000.zip")).unwrap();
    fs::write(cut.join("conversations-000.zip"), &whole[..50]).unwrap();
    let Err(Error::Unreadable(m)) = read("claude", Some(&cut), dir.path()) else {
        panic!()
    };
    assert!(
        m.contains("conversations-000.zip isn't a complete zip file"),
        "{m}"
    );
}

// decision: MEM-437 (fixed against Python)
#[test]
fn an_edited_away_prompt_is_not_read_as_the_owners_last_word() {
    let dir = tempfile::tempdir().unwrap();
    let mut chat = real_chat("c1", &["I live in Porto", "Noted."]);
    // the owner edited the first prompt: a second branch from the same parent, newer
    chat["chat_messages"].as_array_mut().unwrap().push(json!({"uuid": "c1-e0", "text": "I live in Lisbon",
        "content": [{"type": "text", "text": "I live in Lisbon"}], "sender": "human",
        "created_at": "2026-11-01T11:00:00Z", "parent_message_uuid": "00000000-0000-4000-8000-000000000000"}));
    let file = dir.path().join("conversations.json");
    fs::write(&file, json!([chat]).to_string()).unwrap();
    let convs = read("claude", Some(&file), dir.path()).unwrap();
    let said: Vec<&str> = convs[0].messages.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(said, vec!["I live in Lisbon"]);
}

fn check_says(source: &str, path: &Path, words: &str) {
    let got = exports::check(source, path)
        .and_then(|()| read(source, Some(path), path.parent().unwrap()).map(|_| ()))
        .map_err(|e| exports::read_error(source, path, e));
    let Err(Error::Unreadable(m)) = got else {
        panic!("{}: {got:?}", path.display())
    };
    assert!(m.contains(words), "{words}: {m}");
}

// decision: MEM-438
#[test]
fn a_file_that_isnt_a_usable_export_is_said_plainly() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let chat = json!([{"conversation_id": "g1", "title": "chat g1", "update_time": 1_793_000_000.0,
        "current_node": "n0", "mapping": {"n0": {"id": "n0", "parent": null, "message": {"author": {"role": "user"},
        "create_time": 1_793_000_000.0, "content": {"content_type": "text", "parts": ["I live in Lisbon"]}}}}}]);
    zipped(&d.join("good.zip"), &[("conversations.json", chat.clone())]);
    let whole = fs::read(d.join("good.zip")).unwrap();
    fs::write(d.join("cut.zip"), &whole[..60]).unwrap();
    fs::write(
        d.join("expired.zip"),
        "<html><body>This link has expired</body></html>",
    )
    .unwrap();
    fs::write(d.join("export.zip.crdownload"), "PK").unwrap();
    fs::write(d.join("empty.zip"), "").unwrap();
    check_says("chatgpt", &d.join("cut.zip"), "isn't a complete zip file");
    check_says(
        "chatgpt",
        &d.join("expired.zip"),
        "the export link had expired",
    );
    check_says(
        "chatgpt",
        &d.join("export.zip.crdownload"),
        "a download that hasn't finished",
    );
    check_says("chatgpt", &d.join("empty.zip"), "is empty");
    check_says("chatgpt", &d.join("nowhere.zip"), "doesn't exist");
    // another site's export: the claude reader finds nothing, and the reason names ChatGPT
    let convs = read("claude", Some(&d.join("good.zip")), d).unwrap_or_default();
    assert!(convs.is_empty());
    let Error::Unreadable(m) = exports::nothing("claude", &d.join("good.zip")) else {
        panic!()
    };
    assert_eq!(m, "good.zip looks like ChatGPT's export, not Claude's.");
    let Error::Unreadable(m) = exports::nothing("chatgpt", &d.join("good.zip")) else {
        panic!()
    };
    assert!(m.contains("holds nothing from the last month"), "{m}");
}
