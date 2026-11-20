//! T-1274: insomnia mode from the paired phone, through the real phone gate on a temp store (the phone tests' world)
//! with the real `insomnia.*` handlers over stand-ins for the Mac's power. The start is signed with the phone's
//! fingerprint like a setting (APP-980); its end, Lock my Mac and It's fine work from a locked phone (APP-983); the
//! state rides `chat.status` (so a locked phone notifies); Show me's camera never crosses the shared relay.

mod insomnia_support;
mod phone_support;

use ferb_rpc::phone_gate as code;
use phone_support::*;
use serde_json::json;

const P1: &str = "endpoint-one";

async fn world_with_insomnia() -> (
    World,
    std::sync::Arc<insomnia_support::Mac>,
    ferb_daemon::insomnia::Insomnia,
) {
    let w = world(None).await;
    let (mac, ins) = insomnia_support::insomnia();
    ins.register(&w.server);
    let i = ins.clone();
    w.phones
        .set_insomnia(std::sync::Arc::new(move || i.status()));
    (w, mac, ins)
}

/// The start needs the phone's signature for this exact call; the end, Lock my Mac and It's fine work while locked.
// decision: DMN-609
// decision: APP-980
// decision: APP-983
// decision: PHN-112
#[tokio::test]
async fn the_phone_starts_it_signed_and_ends_or_locks_it_even_locked() {
    let (w, mac, ins) = world_with_insomnia().await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, false, false).await;
    let mut c = Conn::phone(&w, P1, "a").await;
    let params = json!({"variant": "shield", "screen_yes": true});
    assert_eq!(
        c.call("insomnia.start", params.clone(), None)
            .await
            .unwrap_err()
            .0,
        code::UNLOCK_REQUIRED,
        "no proof: refused"
    );
    assert!(!ins.on());
    let started = c
        .signed_call(&keys.lock, "insomnia.start", params)
        .await
        .unwrap();
    assert_eq!(started["on"], true);
    assert_eq!(started["by"], P1);
    // the state rides chat.status, also to a locked phone (its notifications)
    let st = c.ok("chat.status", json!({})).await;
    assert_eq!(st["insomnia"]["on"], true, "{st}");
    // someone at the Mac: the notice is in the status
    let p: serde_json::Map<String, serde_json::Value> = json!({"event": "shield", "state": "up"})
        .as_object()
        .unwrap()
        .clone();
    ins.report(&p).unwrap();
    let p = json!({"event": "person_input", "input": "keyboard"})
        .as_object()
        .unwrap()
        .clone();
    ins.report(&p).unwrap();
    let st = c.ok("chat.status", json!({})).await;
    let notice = st["insomnia"]["notice"]["id"].as_str().unwrap().to_owned();
    // a locked session (a new one, not unlocked) may say It's fine, lock the Mac and end the mode
    let mut locked = Conn::phone(&w, P1, "b").await;
    let fine = locked
        .ok("insomnia.dismiss", json!({"notice_id": notice}))
        .await;
    assert_eq!(fine["dismissed"], true);
    let lockd = locked.ok("insomnia.lock", json!({})).await;
    assert_eq!(lockd["locked"], true);
    assert_eq!(*mac.locks.lock().unwrap(), 1, "the owner's own Lock my Mac");
    let ended = locked.ok("insomnia.end", json!({"why": "back"})).await;
    assert_eq!(ended["ended"], true);
    assert_eq!(ended["by"], P1);
    assert_eq!(
        ended["last_end"]["why"], "turned_off",
        "a phone never says the owner is back at the Mac"
    );
    // the cover's reports and frames are never a phone's
    assert!(c
        .call(
            "insomnia.report",
            json!({"event": "shield", "state": "down"}),
            None
        )
        .await
        .is_err());
    assert!(c
        .call("insomnia.camera_frame", json!({"seq": 1, "jpeg": ""}), None)
        .await
        .is_err());
}

/// Show me: the camera reaches the phone only unlocked and on a direct path or the owner's relay; the shared relay
/// and an unknown path are refused with the stable code, and no frame is taken for them.
// decision: DMN-607
// decision: APP-983
// decision: PHN-454
// decision: PHN-444
#[tokio::test]
async fn the_camera_never_crosses_the_shared_relay() {
    let (w, _mac, ins) = world_with_insomnia().await;
    let keys = Keys::new(1);
    pair(&w, P1, &keys, false, false).await;
    ins.start(ferb_daemon::insomnia::Variant::Shield, true, "mac");
    for (session, tier) in [("s", Some("shared_relay")), ("u", None)] {
        let mut hello = json!({"role": "phone", "device": P1, "session": session});
        if let Some(t) = tier {
            hello["tier"] = json!(t);
        }
        let mut c = Conn::open(&w, hello).await.unwrap();
        c.unlock(&keys).await;
        let (n, why) = c
            .call("insomnia.camera", json!({}), None)
            .await
            .unwrap_err();
        assert_eq!(n, code::FILE_NOT_ALLOWED, "{tier:?}: {why}");
        assert!(!ins.camera_holds_frame());
        assert_eq!(ins.status()["camera"]["on"], false, "{tier:?}: not started");
    }
    let mut direct = Conn::open(
        &w,
        json!({"role": "phone", "device": P1, "session": "d", "tier": "direct"}),
    )
    .await
    .unwrap();
    // locked: refused before anything
    assert_eq!(
        direct
            .call("insomnia.camera", json!({}), None)
            .await
            .unwrap_err()
            .0,
        code::LOCKED
    );
    direct.unlock(&keys).await;
    let first = direct.ok("insomnia.camera", json!({})).await;
    assert_eq!(first["state"], "starting");
    assert_eq!(ins.status()["camera"]["on"], true);
}
