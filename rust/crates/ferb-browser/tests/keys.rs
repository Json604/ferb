//! The extension's key table, the amounts a page shows and the capture masking's geometry against
//! rust/fixtures/browser/{keys,money,mask}.json (made by hands.py: protocol.js under node, and the
//! Python build).

mod common;

use std::sync::Mutex;

use common::fixture;
use ferb_browser::keys;
use ferb_browser::mask::{self, ImageBox, Images, PageState};
use ferb_browser::money;
use serde_json::{json, Value};

/// Fixed against Python (ENG-6, the extension is the other end): Python's enters_text disagrees with
/// protocol.js's entersText. These are the cases of keys.json where they differ, as Python had them.
const PYTHON_DIFFERS: usize = 155;

// decision: HAND-218
#[test]
fn key_events_are_the_extensions() {
    let f = fixture("keys.json");
    let (mut differ, mut throws) = (0, 0);
    for case in f.as_array().unwrap() {
        let spec = case["spec"].as_str();
        let got = keys::key_event(spec);
        if case.get("throws").is_some() {
            assert_eq!(got, Err(keys::Throws), "{spec:?}");
            assert!(
                keys::enters_text(spec),
                "{spec:?}: refused before it reaches a secret field"
            );
            throws += 1;
            continue;
        }
        let event = got.unwrap_or_else(|_| panic!("{spec:?} threw"));
        assert_eq!(
            event.map_or(Value::Null, |e| e.to_json()),
            case["event"],
            "{spec:?}"
        );
        let enters = case["enters"].as_bool().unwrap();
        assert_eq!(keys::enters_text(spec), enters, "{spec:?}");
        differ += usize::from(enters != case["python"].as_bool().unwrap());
    }
    assert!(throws > 50, "{throws}");
    assert_eq!(differ, PYTHON_DIFFERS);
}

// decision: GATE-578
#[test]
fn amounts_are_pythons() {
    let f = fixture("money.json");
    for case in f["label_amount"].as_array().unwrap() {
        assert_eq!(
            json!(money::label_amount(case["label"].as_str())),
            case["amount"],
            "{}",
            case["label"]
        );
    }
    for case in f["page_amounts"].as_array().unwrap() {
        let texts: Vec<Option<&str>> = case["texts"]
            .as_array()
            .unwrap()
            .iter()
            .map(Value::as_str)
            .collect();
        assert_eq!(
            json!(money::page_amounts(texts)),
            case["amounts"],
            "{}",
            case["texts"]
        );
    }
}

struct Recorder(Mutex<Vec<Value>>);

impl Images for Recorder {
    fn size(&self, image: &[u8]) -> Result<(i64, i64), String> {
        if image.starts_with(b"bad") {
            return Err("not an image".into());
        }
        Ok((2560, 1800))
    }
    fn mask(&self, image: &[u8], boxes: &[ImageBox]) -> Result<Vec<u8>, String> {
        let text = String::from_utf8(image.to_vec()).unwrap();
        self.0.lock().unwrap().push(json!([
            text,
            boxes
                .iter()
                .map(|b| json!([b.0, b.1, b.2, b.3]))
                .collect::<Vec<_>>()
        ]));
        if image.starts_with(b"unpaintable") {
            return Err("no".into());
        }
        Ok([b"painted:".as_slice(), image].concat())
    }
}

fn r4(r: mask::Rect) -> Value {
    json!([r.0, r.1, r.2, r.3])
}

// decision: L13
#[test]
fn masking_geometry_is_pythons() {
    let f = fixture("mask.json");
    for case in f["parse"].as_array().unwrap() {
        let got = PageState::parse(Some(&case["raw"])).map(|s| {
            json!({"top": s.top, "viewport": r4(s.viewport), "items": s.items.iter().map(|i| json!({
                "key": i.key, "rect": r4(i.rect), "paint": i.paint, "child": i.child})).collect::<Vec<_>>()})
        });
        assert_eq!(json!(got), case["state"], "{}", case["raw"]);
    }
    for case in f["page_masks"].as_array().unwrap() {
        let (b, a) = (
            PageState::parse(Some(&case["before"])).unwrap(),
            PageState::parse(Some(&case["after"])).unwrap(),
        );
        let got = match mask::page_masks(&b, &a) {
            Ok((rects, viewport)) => {
                json!({"rects": rects.into_iter().map(r4).collect::<Vec<_>>(), "viewport": r4(viewport)})
            }
            Err(e) => json!({"error": e.class(), "message": e.message()}),
        };
        assert_eq!(got, case["out"], "{case}");
    }
    for case in f["to_image_boxes"].as_array().unwrap() {
        let rects: Vec<mask::Rect> = case["rects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| mask::rect(Some(r)).unwrap())
            .collect();
        let size = (
            case["size"][0].as_i64().unwrap(),
            case["size"][1].as_i64().unwrap(),
        );
        let boxes = mask::to_image_boxes(&rects, mask::rect(Some(&case["bounds"])).unwrap(), size);
        let got: Vec<Value> = boxes.iter().map(|b| json!([b.0, b.1, b.2, b.3])).collect();
        assert_eq!(json!(got), case["boxes"], "{case}");
    }
    for case in f["painted"].as_array().unwrap() {
        let images = Recorder(Mutex::default());
        let image = case["image"].as_str().unwrap().as_bytes().to_vec();
        let got = match mask::painted(&images, image, &case["reply"]) {
            Ok(v) => json!({"ok": String::from_utf8(v).unwrap()}),
            Err(e) => json!({"error": e.class(), "message": e.message()}),
        };
        assert_eq!(got, case["out"], "{case}");
        assert_eq!(json!(*images.0.lock().unwrap()), case["calls"], "{case}");
    }
}

/// `mimetypes.guess_type` as the upload through the page names a file's type (HAND-501): Python's
/// built-in tables, then a mime.types file read as `mimetypes.init` reads the Mac's.
#[test]
fn file_types_are_pythons() {
    let f = fixture("mime.json");
    let bare = ferb_browser::mime::MimeTypes::defaults();
    let mut read = ferb_browser::mime::MimeTypes::defaults();
    read.read(f["file"].as_str().unwrap());
    for (db, key) in [(&bare, "defaults"), (&read, "with_file")] {
        for case in f[key].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            assert_eq!(json!(db.guess_type(name)), case["type"], "{key}: {name}");
        }
    }
}
