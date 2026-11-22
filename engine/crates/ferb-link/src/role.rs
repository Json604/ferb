//! What a paired phone may ask FERB's daemon to do (kb PHN-111, PHN-112). The owner's rule: the phone is his full
//! remote. The engine's own table decides it: every method of `ferb_rpc::auth::METHODS` has one row in
//! `ferb_rpc::classes` (kb PHN-465), whose class decides what the Mac checks before the call goes on, never whether a
//! feature exists:
//!
//!   plain      the phone unlocked towards the Mac within the unlock window (`link.unlock`).
//!   private    the same window; it shows the owner's data, and the audit names it so.
//!   sensitive  an unlock proof signed for this exact call and a fresh one-time challenge (`link.challenge`).
//!   never      would break something if a phone called it; the row says what.
//!
//! The daemon makes each class's check (the unlock window, a proof for the exact call: ferb-daemon's phone gate). The
//! bridge reads the same table to refuse what is not a phone's at all before anything reaches the Mac, to name the
//! class in its audit, to know the streams, and to send a sensitive call's parameters on the sealed path. Since S4
//! (kb T-1204) there is no table of the link's own: the lab's (`role.rs` at ferb-phone-link@57b3f34) is gone, and the
//! one authority is the engine's, which `engine/fixtures/phone/classes.json` holds as data (tests/classes.rs).
//!
//! What stays here are facts the engine's table does not hold: which methods answer with a picture and which ask for
//! an older chat page (the public relays carry neither, kb PHN-444), and FERB's question kinds the bridge names.
//! Structure only: method names, never a judgment of content.

use crate::errors::{self, LinkError, LinkResult};
pub use ferb_rpc::classes::Class;
use ferb_rpc::classes::{self, PhoneMethod};
use serde_json::Value;

/// The method's row in the engine's table, when the engine has such a method for a phone at all.
pub fn row(method: &str) -> Option<&'static PhoneMethod> {
    classes::phone_method(method)
}

/// The class of a method the engine knows; `None` for one it does not (a newer or older build's, a misspelling).
pub fn class(method: &str) -> Option<Class> {
    row(method).map(|r| r.class)
}

/// The daemon answers it as a stream.
pub fn is_stream(method: &str) -> bool {
    row(method).is_some_and(|r| r.stream)
}

/// It opens a prompt or a window on the Mac's screen (kb PHN-117, PHN-120).
pub fn on_mac_screen(method: &str) -> bool {
    row(method).is_some_and(|r| r.on_mac_screen)
}

/// Why a phone may not call it (a `never` row's reason).
pub fn why(method: &str) -> &'static str {
    row(method).map_or("", |r| r.why)
}

/// FERB's own consequential checks (the gateway's OWN_CHECKS): a Yes to one needs the phone's proof.
pub const OWN_CHECKS: &[&str] = &[
    "ferb.confirm",
    "ferb.retry",
    "conversation.approve",
    "memory.forget",
    "downloads.delete",
    "browsing.choose",
    // HAND-663: a coding agent's permission prompt, and turning on FERB's sight of the sessions (its hook)
    "agent_session.permission",
    "agent_session.enable",
];
/// Hidden inputs (the gateway's SECRET_KINDS): answered only through the sealed path (kb PHN-109, PHN-110): chat.answer
/// with the phone's unlock proof; the value is never logged, stored or echoed by the bridge.
pub const SECRET_KINDS: &[&str] = &["owner_login_code", "owner_admin_password", "owner_secret"];

/// Methods whose answer is a picture (a screenshot of FERB's work, a question's picture, a remembered image): never
/// on the public relays (kb PHN-444). Structure only: what the method returns, never what it shows.
pub const PICTURES: &[&str] = &[
    "insomnia.camera",
    "questions.picture",
    "seen.image",
    "tasks.preview",
];
/// Pages of the chat, and the parameters that ask for an older page than the latest: on the public relays only the
/// latest page is given (kb PHN-444), at most `Limits::shared_page_max` entries.
pub const PAGES: &[(&str, &[&str])] = &[
    ("chat.entries.page", &["before_pos", "day"]),
    ("chat.history.page", &["before"]),
];

/// The class of a call, or why it cannot be made from a phone.
/// `phone_level`: the feature level the phone said in its hello (0: an app from before levels).
pub fn check(method: &str, params: &Value, phone_level: u32) -> LinkResult<Class> {
    let row = match row(method) {
        // a method only one side knows: the builds differ, and the refusal says which side is behind
        None if phone_level > crate::LEVEL => return Err(errors::mac_link_older(method)),
        None if phone_level < crate::LEVEL => return Err(errors::app_older(method)),
        None => {
            return Err(LinkError::new(
                errors::METHOD_NOT_ALLOWED,
                format!("\"{method}\" is not something FERB's phone link knows."),
            )
            .allow("Do it in FERB on the Mac; a new FERB feature may need a newer phone link"));
        }
        Some(r) if r.class == Class::Never => {
            return Err(LinkError::new(
                errors::NOT_FROM_PHONE,
                format!("\"{method}\" cannot be done from a phone: it {}.", r.why),
            )
            .allow("Do it in FERB on the Mac"));
        }
        Some(r) => r,
    };
    let Some(map) = params.as_object() else {
        return Err(LinkError::new(
            errors::BAD_REQUEST,
            "params must be an object",
        ));
    };
    if let Some(k) = map
        .keys()
        .find(|k| row.refused_params.contains(&k.as_str()))
    {
        return Err(LinkError::new(
            errors::PARAM_NOT_ALLOWED,
            format!("\"{k}\" cannot be sent with {method} from a phone."),
        ));
    }
    Ok(row.class)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every name the bridge keeps for itself names a method the engine's table has (kb PHN-465): a renamed method
    /// can't silently drop out of the public relays' rules.
    // decision: PHN-465
    // decision: PHN-444
    #[test]
    fn the_link_s_own_lists_name_rows_of_the_engine_s_table() {
        for m in PICTURES {
            assert!(row(m).is_some(), "{m}");
        }
        for (m, _) in PAGES {
            assert!(row(m).is_some(), "{m}");
        }
        assert!(is_stream("chat.send") && !is_stream("chat.status"));
        assert_eq!(class("no.such.method"), None);
    }
}
