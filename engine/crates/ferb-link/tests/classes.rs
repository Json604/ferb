//! The link reads the engine's one class table (ferb-rpc `classes`, kb PHN-465): what the bridge refuses before the
//! Mac is asked, the streams, the on-screen methods and the parameters a phone may not send are the same as
//! `engine/fixtures/phone/classes.json`, the table as data the phone apps compare against. The lab's own table (and
//! its check against FERB's Python method table) is gone since S4.

use ferb_link::{errors, role};
use serde_json::{Value, json};

fn table() -> Value {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/phone/classes.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

// decision: PHN-465
// decision: PHN-112
#[test]
fn the_link_s_classes_are_the_engine_s_table() {
    let t = table();
    let rows = t["methods"].as_object().unwrap();
    assert!(rows.len() > 100);
    for (m, row) in rows {
        let class = row["class"].as_str().unwrap();
        assert_eq!(role::class(m).map(|c| c.as_str()), Some(class), "{m}");
        assert_eq!(role::is_stream(m), row["stream"] == json!(true), "{m}");
        assert_eq!(
            role::on_mac_screen(m),
            row["on_mac_screen"] == json!(true),
            "{m}"
        );
        let checked = role::check(m, &json!({}), ferb_link::LEVEL);
        if class == "never" {
            let e = checked.unwrap_err();
            assert_eq!(e.code, errors::NOT_FROM_PHONE, "{m}");
            assert!(e.message.contains(row["why"].as_str().unwrap()), "{m}: {e}");
        } else {
            assert_eq!(checked.unwrap().as_str(), class, "{m}");
            for p in row["refused_params"].as_array().unwrap() {
                let p = p.as_str().unwrap();
                let e = role::check(m, &json!({p: 1}), ferb_link::LEVEL).unwrap_err();
                assert_eq!(e.code, errors::PARAM_NOT_ALLOWED, "{m} {p}");
            }
        }
    }
    // a method the engine has no row for is not the link's to guess at: the refusal names the side that is behind
    assert_eq!(
        role::check("made.up", &json!({}), ferb_link::LEVEL + 1)
            .unwrap_err()
            .code,
        errors::MAC_LINK_OLDER
    );
    assert_eq!(
        role::check("made.up", &json!({}), 0).unwrap_err().code,
        errors::APP_OLDER
    );
}
