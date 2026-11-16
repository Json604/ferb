//! T-1221, PHN-454: the live preview on the owner's phone. A phone's `tasks.preview` gets the frame as captured only
//! under the phone gate's word (`Phones::preview_link`): the phone paired and not removed, its unlock window open now,
//! and the link naming a direct path or the owner's relay as the session's tier. The public relay is refused by the
//! gate before the handler runs (as files are, PHN-444); a session whose tier is unknown, locked, or of a removed phone
//! gets nothing. The handler here does what ferbd's (`areas_c/records.rs`) does with the word: the frame's bytes through
//! `OwnerOnlyFrame::for_paired_phone`, else none. And a phone's `phone.list` names its own row and the Mac.

mod phone_support;

use base64::Engine;
use ferb_mac_hands::point::OwnerOnlyFrame;
use ferb_rpc::phone_gate as code;
use ferb_rpc::{Params, Session};
use phone_support::*;
use serde_json::{json, Map, Value};

const P1: &str = "endpoint-one";
const FRAME: &[u8] = b"\xff\xd8 the Mac's window, as captured";

/// ferbd's handler's phone path, on this test's server: the gate's word, then the frame's bytes.
fn serve_preview(w: &World) {
    let phones = w.phones.clone();
    w.server
        .register_unary("tasks.preview", move |s: Session, _p: Params| {
            let phones = phones.clone();
            async move {
                let frame = OwnerOnlyFrame::captured(FRAME.to_vec());
                let mut out = Map::new();
                out.insert("state".into(), json!("working"));
                let bytes = match &s.phone {
                    Some(phone) => {
                        let link = phones.preview_link(phone).map_err(|r| {
                            ferb_rpc::server::HandlerError::coded(r.code, r.message)
                        })?;
                        frame.for_paired_phone(&link).to_vec()
                    }
                    None => frame.for_owner_app().to_vec(),
                };
                out.insert(
                    "image".into(),
                    json!(base64::engine::general_purpose::STANDARD.encode(bytes)),
                );
                Ok(out)
            }
        });
}

async fn phone_on(w: &World, session: &str, tier: Option<&str>) -> Conn {
    let mut hello = json!({"role": "phone", "device": P1, "session": session});
    if let Some(t) = tier {
        hello["tier"] = json!(t);
    }
    Conn::open(w, hello).await.expect("a phone session")
}

fn image(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(v["image"].as_str().unwrap())
        .unwrap()
}

// decision: PHN-454
// decision: PHN-444
// decision: PHN-125
// decision: PHN-512
#[tokio::test]
async fn a_phone_s_preview_needs_the_pairing_the_unlock_and_a_path_that_carries_it() {
    let w = world(None).await;
    serve_preview(&w);
    let keys = Keys::new(1);
    pair(&w, P1, &keys, false, true).await;

    // a direct path, unlocked: the frame as captured
    let mut direct = phone_on(&w, "d", Some("direct")).await;
    assert_eq!(
        direct
            .call("tasks.preview", json!({"task_id": "t1"}), None)
            .await
            .unwrap_err()
            .0,
        code::LOCKED,
        "locked: refused before anything is captured"
    );
    direct.unlock(&keys).await;
    let got = direct.ok("tasks.preview", json!({"task_id": "t1"})).await;
    assert_eq!(image(&got), FRAME);
    // the owner's relay too
    let mut own = phone_on(&w, "o", Some("own_relay")).await;
    own.unlock(&keys).await;
    assert_eq!(
        image(&own.ok("tasks.preview", json!({"task_id": "t1"})).await),
        FRAME
    );

    // the public relay: refused by the gate, unlocked or not
    let mut shared = phone_on(&w, "s", Some("shared_relay")).await;
    shared.unlock(&keys).await;
    assert_eq!(
        shared
            .call("tasks.preview", json!({"task_id": "t1"}), None)
            .await
            .unwrap_err()
            .0,
        code::FILE_NOT_ALLOWED
    );
    // a session whose tier nobody named: refused by the gate's word
    let mut unknown = phone_on(&w, "u", None).await;
    unknown.unlock(&keys).await;
    let (n, why) = unknown
        .call("tasks.preview", json!({"task_id": "t1"}), None)
        .await
        .unwrap_err();
    assert_eq!(n, code::FILE_NOT_ALLOWED, "{why}");

    // the app locks: nothing more
    direct.ok("phone.lock", json!({})).await;
    assert_eq!(
        direct
            .call("tasks.preview", json!({"task_id": "t1"}), None)
            .await
            .unwrap_err()
            .0,
        code::LOCKED
    );
    // the idle window closes by itself on the Mac's clock
    w.clock.advance(301.0);
    assert_eq!(
        own.call("tasks.preview", json!({"task_id": "t1"}), None)
            .await
            .unwrap_err()
            .0,
        code::LOCKED
    );

    // a removed phone: refused at once
    own.unlock(&keys).await;
    let mut id = Params::new();
    id.insert("id".into(), json!(P1));
    w.phones.remove(&app_session(), &id).unwrap();
    assert!(own
        .call("tasks.preview", json!({"task_id": "t1"}), None)
        .await
        .is_err());
}

/// The gate's word itself: only a paired phone's session, unlocked now, on a direct path or the owner's relay.
// decision: PHN-454
// decision: PHN-512
#[tokio::test]
async fn the_preview_link_is_given_only_under_its_three_conditions() {
    let w = world(None).await;
    let keys = Keys::new(2);
    pair(&w, P1, &keys, false, false).await;
    let session = |tier: Option<&str>, s: &str| ferb_rpc::phone_gate::PhoneSession {
        device: P1.into(),
        session: s.into(),
        tier: tier.map(str::to_string),
        ..Default::default()
    };
    // locked
    assert_eq!(
        w.phones
            .preview_link(&session(Some("direct"), "a"))
            .unwrap_err()
            .code,
        code::LOCKED
    );
    let mut a = phone_on(&w, "a", Some("direct")).await;
    a.unlock(&keys).await;
    assert!(w.phones.preview_link(&session(Some("direct"), "a")).is_ok());
    for tier in [Some("shared_relay"), None, Some("something new")] {
        assert_eq!(
            w.phones.preview_link(&session(tier, "a")).unwrap_err().code,
            code::FILE_NOT_ALLOWED,
            "{tier:?}"
        );
    }
    // not a paired phone
    let stranger = ferb_rpc::phone_gate::PhoneSession {
        device: "stranger".into(),
        session: "a".into(),
        tier: Some("direct".into()),
        ..Default::default()
    };
    assert!(w.phones.preview_link(&stranger).is_err());
}

/// A phone's `phone.list` (behind its unlock) says which row is its own and the Mac's name; FERB.app's has neither.
// decision: PHN-513
#[tokio::test]
async fn a_phone_s_list_names_its_own_row_and_the_mac() {
    let w = world(None).await;
    w.phones
        .set_mac_name(std::sync::Arc::new(|| "Invented Mac".to_string()));
    let keys = Keys::new(3);
    pair(&w, P1, &keys, false, false).await;
    pair(&w, "endpoint-two", &Keys::new(4), false, false).await;
    let mut c = phone_on(&w, "a", Some("direct")).await;
    assert_eq!(
        c.call("phone.list", json!({}), None).await.unwrap_err().0,
        code::LOCKED
    );
    c.unlock(&keys).await;
    let listed = c.ok("phone.list", json!({})).await;
    assert_eq!(listed["mac"]["name"], "Invented Mac");
    let mine: Vec<&Value> = listed["phones"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["this"] == json!(true))
        .collect();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0]["id"], P1);
    assert!(mine[0]["paired_at"].as_str().is_some_and(|s| !s.is_empty()));
    let app = w.phones.list(&app_session()).unwrap();
    assert!(app.get("mac").is_none());
    assert!(app["phones"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p.get("this").is_none()));
}
