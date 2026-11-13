//! Both ends of the crypto: rust/fixtures/browser/crypto.json holds the MACs and seals made by Python
//! (ferb/act/browser/ext_protocol.py, ferb/identity/seal.py) and by the unchanged protocol.js under node.

mod common;

use ferb_browser::protocol;
use ferb_browser::seal::{self, Sealed};
use ferb_identity::Secret;
use serde_json::Value;

use common::{b64d, fixture};

fn sealed(v: &Value) -> Sealed {
    seal::from_wire(v).expect("a seal")
}

// decision: HAND-222
#[test]
fn macs_agree_with_python_and_the_extension() {
    let f = fixture("crypto.json");
    for c in f["daemon"].as_array().unwrap() {
        let key = b64d(c["key"].as_str().unwrap());
        let (nonce, seq, ty, body) = (
            c["nonce"].as_str().unwrap(),
            c["seq"].as_i64().unwrap(),
            c["type"].as_str().unwrap(),
            c["body"].as_str().unwrap(),
        );
        assert_eq!(
            c["js_text"].as_str().unwrap(),
            format!("{nonce}|{seq}|{ty}|{body}")
        );
        let mac = protocol::daemon_mac(&key, nonce, seq, ty, body);
        assert_eq!(mac, c["mac"].as_str().unwrap(), "Python");
        assert_eq!(mac, c["js_mac"].as_str().unwrap(), "protocol.js");
    }
    for c in f["extension"].as_array().unwrap() {
        let key = b64d(c["key"].as_str().unwrap());
        let body = c["js_body_json"].as_str().unwrap(); // JSON.stringify's text, checked as it came
        let mac = protocol::sign(
            &key,
            c["install_id"].as_str().unwrap(),
            c["seq"].as_i64().unwrap(),
            c["type"].as_str().unwrap(),
            body,
        );
        assert_eq!(mac, c["js_mac"].as_str().unwrap(), "protocol.js");
        assert_eq!(mac, c["py_mac_of_js_body"].as_str().unwrap(), "Python");
    }
}

#[test]
fn seals_are_the_same_bytes_on_every_side() {
    let f = fixture("crypto.json");
    let py = f["python_seals"].as_array().unwrap();
    let js = f["js_seals"].as_array().unwrap();
    assert_eq!(py.len(), js.len());
    for (p, j) in py.iter().zip(js) {
        let key = b64d(p["key"].as_str().unwrap());
        let skey = seal::seal_key(&key);
        assert_eq!(
            ferb_identity::py::b64encode(&skey[..]),
            p["seal_key"].as_str().unwrap()
        );
        let iv: [u8; 12] = b64d(p["iv"].as_str().unwrap()).try_into().unwrap();
        let (text, cid, origin) = (
            p["text"].as_str().unwrap(),
            p["command"].as_str().unwrap(),
            p["origin"].as_str().unwrap(),
        );
        let ours = seal::seal_with_nonce(&skey[..], &iv, text, cid, origin).unwrap();
        assert_eq!(ours, sealed(&p["sealed"]), "Python");
        assert_eq!(ours, sealed(&j["sealed"]), "protocol.js");
        assert_eq!(j["python_opens"].as_str().unwrap(), text);
        // Rust opens both; a different command or origin opens nothing.
        for s in [&p["sealed"], &j["sealed"]] {
            let opened = seal::unseal(&skey[..], &sealed(s), cid, origin).unwrap();
            assert_eq!(opened.expose(), text);
            assert!(seal::unseal(&skey[..], &sealed(s), &format!("{cid}x"), origin).is_err());
            assert!(seal::unseal(&skey[..], &sealed(s), cid, &format!("{origin}/")).is_err());
        }
        // A fresh seal (random nonce) opens on Rust's side and is new each time.
        let a = seal::seal_fill(&key, &Secret::new(text.into()), cid, origin).unwrap();
        let b = seal::seal_fill(&key, &Secret::new(text.into()), cid, origin).unwrap();
        assert_ne!(a.nonce, b.nonce);
        assert_eq!(
            seal::unseal(&skey[..], &a, cid, origin).unwrap().expose(),
            text
        );
    }
    // protocol.js opened Python's seals, and refused them for another command or origin
    let opened: Vec<bool> = f["js_unseal"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["ok"].as_bool().unwrap())
        .collect();
    assert_eq!(opened, [true, true, true, true, false, false]);
    for u in f["js_unseal"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|u| u["ok"] == true)
    {
        let p = py
            .iter()
            .find(|p| p["sealed"] == u["case"]["sealed"])
            .unwrap();
        assert_eq!(u["text"], p["text"]);
    }
}

/// The daemon refuses exactly the key presses the extension would put text in with (the other end):
/// every spec of crypto.json's list agrees with protocol.js's entersText (decision: HAND-218).
#[test]
fn enters_text_follows_protocol_js() {
    let f = fixture("crypto.json");
    let p = fixture("protocol.json");
    for (js, py) in f["js_enters_text"]
        .as_array()
        .unwrap()
        .iter()
        .zip(p["text"]["enters_text"].as_array().unwrap())
    {
        assert_eq!(
            ferb_browser::rules::enters_text(py["spec"].as_str()),
            js["enters"].as_bool().unwrap(),
            "{}",
            py["spec"]
        );
    }
}
