//! What the `secrets.list` tool answers: a port of `ferb/identity/secret_tools.py`. The read-only
//! listing of the owner's saved logins (L36 §5): never a value, never a Keychain account.
//!
//! Seam: the tool's registration (scopes `task:read`, the owner identity only, low risk, an
//! observation of private origin, read sources private and inferred: `secret_tools.py:22-23`,
//! `49`) belongs to the gateway and `ferb-core`'s tool types; this is the tool's `run`.

use serde_json::{json, Value};

use crate::owner_secrets::{login_place, Holding, OwnerSecrets};

/// `secret_tools.py:15`.
pub const NAME: &str = "secrets.list";
/// `secret_tools.py:16-21`: executable input for the model, kept word for word.
pub const DESCRIPTION: &str = "List what is saved for one login place: the username (plain text, from the owner's \
own words) and the names of its sealed entries (the password, security answers). Never a value. Pass the login \
page's address or origin (https://…), or app:<bundle id>. Call it on a login page before asking the owner; fill a \
listed entry with browser_secret or mac_secret secret=<name>. `linked` lists the other login places that serve this \
site (a sign-in form on another address, or the site the owner typed for this task), each by the place its entries \
fill.";
/// `secret_tools.py:22`.
pub const SCOPE: &str = "task:read";

/// A tool's answer (`ferb/core/types.py:362`): the content and whether it is an error.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    pub content: Value,
    pub is_error: bool,
}

/// `secret_tools.py:38-49`. `sites(place)`: the calling task's owner-typed sites for this place,
/// read as links (`HiddenInput._login_sites`); `holding`: the memory a username may still come
/// from (L36 §6).
pub fn secrets_list(
    store: &OwnerSecrets,
    place: &str,
    sites: &dyn Fn(&str) -> Vec<String>,
    holding: Holding<'_>,
) -> Listing {
    let place = login_place(place);
    if place.is_empty() {
        return Listing {
            content: json!({"error": "give an https:// address or app:<bundle id>"}),
            is_error: true,
        };
    }
    match store.listing(&place, &sites(&place), holding) {
        Ok(content) => Listing {
            content,
            is_error: false,
        },
        // an unreadable handle file: say why, change nothing
        Err(e) => Listing {
            content: json!({"error": e.to_string()}),
            is_error: true,
        },
    }
}
