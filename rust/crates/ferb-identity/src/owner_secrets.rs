//! The owner's saved secrets: values in the login Keychain, names bound to their exact place.
//! A port of `ferb/identity/owner_secrets.py`; its module text explains the record's design.
//!
//! Compatible with what the Python build left on a Mac (decision: STORE-403):
//! - `<home>/owner-secret-handles.json`, version 2, is read and written with the same bytes
//!   (`json.dump`'s spelling, the same key order, fields this build doesn't know kept); a
//!   version-1 file (handle -> place) is read as version 2 and never rewritten by a read; a file
//!   that can't be read or isn't in either form is never overwritten: every call refuses.
//! - Each value is the Keychain item with service `ferb.owner-secrets` and the account the file
//!   names; a new entry's account is `"<place> · <name>"` (`owner_secrets.py:342`).
//!
//! Decisions: GATE-548 (the helper makes and reads the items), GATE-2 and APP-2 (delete, never
//! reading), GATE-206 and GATE-219 (a card detail fills only its own kind of field).
//!
//! Seams: [`Places`] stands for `ferb.act.mac.browsers.is_browser` and `ferb.policy.urls.parse`;
//! [`Keychain`] for the helper (Python's `keychain=` / `delete=` arguments); `typed_site` takes
//! the owner's typed words as a lookup (`ferb.policy.provenance.prepare`'s tokens and hosts).

use std::{
    collections::BTreeSet,
    fs,
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};

use serde_json::{json, Map, Value};

use crate::{
    helper::{Helper, Service},
    origin::normalize_origin,
    py, Error, Result, Secret,
};

/// `owner_secrets.py:41`.
pub const SERVICE: Service = Service::OwnerSecrets;
/// `owner_secrets.py:42`.
pub const VERSION: i64 = 2;
/// `owner_secrets.py:43`.
pub const WHOLE_BROWSER: &str =
    "it was saved for a whole web browser, which FERB no longer allows; ask the owner again";
/// `owner_secrets.py:114` (decision: GATE-206): the kinds a card field reports.
pub const CARD_KINDS: [&str; 3] = ["card_number", "card_csc", "card_expiry"];
/// The file's name in FERB's home (`owner_secrets.py:213`).
pub const FILE_NAME: &str = "owner-secret-handles.json";

/// `owner_secrets.py:44`: one lock for every change, since stores are short-lived and worker
/// threads share the file.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// What this module asks of the parts of FERB it does not own.
pub trait Places: Send + Sync {
    /// `ferb.act.mac.browsers.is_browser(bundle_id)`: a web browser, by FERB's one browser list.
    fn is_browser(&self, bundle_id: &str) -> bool;
    /// `ferb.policy.urls.parse(url).host`: the host as a browser reads it (IDNA, lowercase), or
    /// `None` when a browser couldn't parse the address or it names no host.
    fn url_host(&self, url: &str) -> Option<String>;
}

/// Where the values are kept: Python's `keychain` and `delete` arguments
/// (`owner_secrets.py:47-59`, `212-215`).
pub trait Keychain: Send + Sync {
    fn store(&self, account: &str, value: &Secret) -> Result<()>;
    /// `None` when the reply carried no value.
    fn lookup(&self, account: &str) -> Result<Option<Secret>>;
    /// Never reads the value; `false` when there was none.
    fn delete(&self, account: &str) -> Result<bool>;
}

/// The real one: the signed helper, service `ferb.owner-secrets`.
impl Keychain for Helper {
    fn store(&self, account: &str, value: &Secret) -> Result<()> {
        Helper::store(self, SERVICE, account, value)
    }

    fn lookup(&self, account: &str) -> Result<Option<Secret>> {
        Helper::lookup(self, SERVICE, account)
    }

    fn delete(&self, account: &str) -> Result<bool> {
        Helper::delete(self, SERVICE, account)
    }
}

/// The memory sources a username may still come from (`owner_secrets.py:189-194`, L36 §6):
/// `None` when the caller has no memory to ask.
pub type Holding<'a> = Option<&'a dyn Fn(&str) -> bool>;

fn refused<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Refused(message.into()))
}

/// `owner_secrets.py:62-69`: a secret is bound to a web page's HTTPS origin or a native app,
/// never to a web browser as an app (owner 2026-10-24: "at places only ive approved").
pub fn refuse_whole_browser(places: &dyn Places, target: &str) -> Result<()> {
    match target.strip_prefix("app:") {
        Some(bundle) if places.is_browser(bundle) => refused(
            "a secret is never bound to a whole web browser; a web page's secret is bound to its HTTPS \
             address through the browser connection",
        ),
        _ => Ok(()),
    }
}

/// `owner_secrets.py:72-83`: `app:<bundle id>` as given, else the address's HTTPS origin; ""
/// for anything else.
pub fn login_place(target: &str) -> String {
    let target = py::strip(target);
    if target.starts_with("app:") {
        return if target.len() > 4 {
            target.to_owned()
        } else {
            String::new()
        };
    }
    match normalize_origin(target) {
        Ok(origin) if origin.starts_with("https://") && origin != "https://" => origin,
        _ => String::new(),
    }
}

/// `owner_secrets.py:86-93`: a web place's or a task site's host as a browser reads it, without
/// a trailing dot; "" for an app.
pub fn site_host(places: &dyn Places, place_or_site: &str) -> String {
    let text = py::strip(place_or_site);
    if text.is_empty() || text.starts_with("app:") {
        return String::new();
    }
    let host = if text.contains("://") {
        places.url_host(text)
    } else {
        places.url_host(&format!("https://{text}"))
    };
    host.map(|h| h.trim_end_matches('.').to_owned())
        .unwrap_or_default()
}

/// `owner_secrets.py:96-101`: one site when equal, a leading "www." aside; never a subdomain.
pub fn same_site(a: &str, b: &str) -> bool {
    let (a, b) = (
        a.strip_prefix("www.").unwrap_or(a),
        b.strip_prefix("www.").unwrap_or(b),
    );
    !a.is_empty() && a == b
}

/// `owner_secrets.py:104-108`: whether the owner typed `host`'s site. `typed(text)` answers
/// whether `text` is a whole token or a URL's host of their words.
pub fn typed_site(host: &str, typed: &dyn Fn(&str) -> bool) -> bool {
    let bare = host.strip_prefix("www.").unwrap_or(host);
    !bare.is_empty() && (typed(bare) || typed(&format!("www.{bare}")))
}

/// `owner_secrets.py:121-128` (decisions: GATE-206, GATE-219): why a saved entry of kind `saved`
/// can't fill a field of kind `field`, when either is a card kind and they differ.
pub fn card_mismatch(saved: Option<&str>, field: Option<&str>) -> Option<String> {
    let card = |kind: Option<&str>| kind.is_some_and(|k| CARD_KINDS.contains(&k));
    if !(card(saved) || card(field)) || saved == field {
        return None;
    }
    let what = |kind: Option<&str>| match kind {
        Some("card_number") => Some("a card number"),
        Some("card_csc") => Some("a card's security code"),
        Some("card_expiry") => Some("a card's expiry date"),
        _ => None,
    };
    Some(format!(
        "that saved secret is {} and this field takes {}; a card detail fills only its own kind of field",
        what(saved).unwrap_or("not a card detail"),
        what(field).unwrap_or("no card detail"),
    ))
}

/// `owner_secrets.py:117-118`.
fn empty() -> Map<String, Value> {
    match json!({"username": null, "username_source": null, "sites": [], "entries": {}, "kinds": {}})
    {
        Value::Object(map) => map,
        _ => unreachable!(),
    }
}

fn fresh() -> Map<String, Value> {
    let mut data = Map::new();
    data.insert("version".into(), VERSION.into());
    data.insert("records".into(), Value::Object(Map::new()));
    data.insert("unusable".into(), Value::Array(Vec::new()));
    data
}

/// `owner_secrets.py:143-144`: a dict whose values are all text.
fn strings(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Object(map)) if map.values().all(Value::is_string))
}

fn text<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(Value::as_str)
}

fn object<'a>(map: &'a Map<String, Value>, key: &str) -> &'a Map<String, Value> {
    static NONE: LazyLock<Map<String, Value>> = LazyLock::new(Map::new);
    map.get(key).and_then(Value::as_object).unwrap_or(&NONE)
}

fn object_mut<'a>(map: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    map.get_mut(key)
        .and_then(Value::as_object_mut)
        .expect("a loaded record has this dict")
}

fn list<'a>(map: &'a Map<String, Value>, key: &str) -> &'a [Value] {
    map.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn sorted_keys(map: &Map<String, Value>) -> Vec<String> {
    let mut keys: Vec<String> = map.keys().cloned().collect();
    keys.sort();
    keys
}

/// `owner_secrets.py:147-164`: a version-2 file's data with every record's fields present, or
/// `None` when its shape or a field's type is not version 2's.
fn version_2(data: &Map<String, Value>) -> Option<Map<String, Value>> {
    let records = data.get("records")?.as_object()?;
    let unusable = match data.get("unusable") {
        None => Vec::new(),
        Some(Value::Array(items)) => items.clone(),
        Some(_) => return None,
    };
    let mut shaped = Map::new();
    for (place, record) in records {
        let record = record.as_object()?;
        if !strings(record.get("entries"))
            || !(record.get("kinds").is_none() || strings(record.get("kinds")))
        {
            return None;
        }
        let mut whole = empty();
        for (key, value) in record {
            whole.insert(key.clone(), value.clone());
        }
        let text_or_none =
            |key: &str| matches!(whole.get(key), Some(Value::String(_) | Value::Null));
        let sites_ok = matches!(whole.get("sites"), Some(Value::Array(sites)) if sites.iter().all(Value::is_string));
        if !sites_ok || !text_or_none("username") || !text_or_none("username_source") {
            return None;
        }
        shaped.insert(place.clone(), Value::Object(whole));
    }
    let known =
        |u: &Value| strings(Some(u)) && ["name", "place", "why"].iter().all(|k| u.get(k).is_some());
    if !unusable.iter().all(known) {
        return None;
    }
    let mut out = Map::new();
    out.insert("version".into(), VERSION.into());
    out.insert("records".into(), Value::Object(shaped));
    out.insert("unusable".into(), Value::Array(unusable));
    Some(out)
}

/// `owner_secrets.py:183-186`: every Keychain account a saved secret uses.
fn accounts(data: &Map<String, Value>) -> Vec<&str> {
    let mut out: Vec<&str> = object(data, "records")
        .values()
        .flat_map(|record| record.get("entries").and_then(Value::as_object))
        .flat_map(|entries| entries.values().filter_map(Value::as_str))
        .collect();
    out.extend(
        list(data, "unusable")
            .iter()
            .filter_map(|u| u.get("name").and_then(Value::as_str)),
    );
    out
}

/// `owner_secrets.py:189-194`.
fn held(record: Option<&Value>, holding: Holding<'_>) -> Option<String> {
    let record = record?.as_object()?;
    let name = text(record, "username").filter(|name| !name.is_empty())?;
    let holds = match holding {
        None => true,
        Some(holds) => text(record, "username_source").is_some_and(holds),
    };
    holds.then(|| name.to_owned())
}

/// The owner's saved secrets of one FERB home (`owner_secrets.py:211-215`).
pub struct OwnerSecrets {
    path: PathBuf,
    keychain: Arc<dyn Keychain>,
    places: Arc<dyn Places>,
}

impl OwnerSecrets {
    /// The store of `home` with its values in `keychain`: the signed helper
    /// (`ferb_platform_macos::identity::owner_secrets` is Python's `OwnerSecrets(home)`), a helper
    /// with a failure hook, or a stand-in.
    pub fn with_keychain(
        home: &Path,
        keychain: Arc<dyn Keychain>,
        places: Arc<dyn Places>,
    ) -> Self {
        OwnerSecrets {
            path: home.join(FILE_NAME),
            keychain,
            places,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `owner_secrets.py:253-267`.
    fn load(&self) -> Result<Map<String, Value>> {
        if !self.path.exists() {
            return Ok(fresh());
        }
        let name = FILE_NAME;
        let unreadable = |kind: &str| {
            refused(format!(
                "FERB's saved secret names ({name}) can't be read ({kind}); FERB leaves the file as it is"
            ))
        };
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) => {
                return unreadable(match e.kind() {
                    io::ErrorKind::NotFound => "FileNotFoundError",
                    io::ErrorKind::PermissionDenied => "PermissionError",
                    io::ErrorKind::IsADirectory => "IsADirectoryError",
                    _ => "OSError",
                })
            }
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            return unreadable("UnicodeDecodeError");
        };
        let Ok(data) = serde_json::from_str::<Value>(text) else {
            return unreadable("JSONDecodeError");
        };
        if let Value::Object(map) = &data {
            let is_version_2 = map.get("version").and_then(Value::as_f64) == Some(VERSION as f64)
                && !matches!(map.get("version"), Some(Value::Bool(_)));
            if let Some(shaped) = is_version_2.then(|| version_2(map)).flatten() {
                return Ok(shaped);
            }
            if strings(Some(&data)) {
                // version 1: handle -> place (version 2's number is no string)
                return Ok(self.migrated(map));
            }
        }
        refused(format!(
            "FERB's saved secret names ({name}) are in a form this FERB doesn't know; FERB leaves the file as it is"
        ))
    }

    /// `owner_secrets.py:131-140`: each version-1 handle becomes an entry of its place under its
    /// own name, its Keychain account unchanged; one bound to a whole browser is unusable.
    fn migrated(&self, old: &Map<String, Value>) -> Map<String, Value> {
        let mut records = Map::new();
        let mut unusable = Vec::new();
        for (handle, place) in old {
            let place = place.as_str().unwrap_or_default();
            if refuse_whole_browser(self.places.as_ref(), place).is_err() {
                unusable.push(json!({"name": handle, "place": place, "why": WHOLE_BROWSER}));
                continue;
            }
            let record = records
                .entry(place)
                .or_insert_with(|| Value::Object(empty()));
            object_mut(record.as_object_mut().expect("made above"), "entries")
                .insert(handle.clone(), Value::String(handle.clone()));
        }
        let mut data = fresh();
        data.insert("records".into(), Value::Object(records));
        data.insert("unusable".into(), Value::Array(unusable));
        data
    }

    /// `owner_secrets.py:269-279`: every change goes through here, under one lock, on the file as
    /// read, written back only when something changed and nothing refused.
    fn edit<T>(&self, change: impl FnOnce(&mut Map<String, Value>) -> Result<T>) -> Result<T> {
        let _held = WRITE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut data = self.load()?;
        let before = data.clone();
        let out = change(&mut data)?;
        if data != before {
            self.save(&data)?;
        }
        Ok(out)
    }

    /// `owner_secrets.py:380-387`: written whole beside the file (mode 0600), then renamed.
    fn save(&self, data: &Map<String, Value>) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("tmp");
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        out.set_permissions(fs::Permissions::from_mode(0o600))?;
        let mut bytes = Vec::new();
        py::dumps_into(&Value::Object(data.clone()), &mut bytes);
        out.write_all(&bytes)?;
        drop(out);
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// `owner_secrets.py:167-180`: the other records that serve `place` (I14).
    fn linked(&self, records: &Map<String, Value>, place: &str, sites: &[String]) -> Vec<String> {
        let places = self.places.as_ref();
        let host = site_host(places, place);
        let mut own_sites: Vec<&str> = records
            .get(place)
            .and_then(Value::as_object)
            .map(|record| {
                list(record, "sites")
                    .iter()
                    .filter_map(Value::as_str)
                    .collect()
            })
            .unwrap_or_default();
        own_sites.extend(sites.iter().map(String::as_str));
        let mut found = Vec::new();
        for (other, record) in records {
            if other == place {
                continue;
            }
            let other_host = site_host(places, other);
            let lists_this = record.as_object().is_some_and(|r| {
                list(r, "sites")
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|s| same_site(s, &host))
            });
            if same_site(&host, &other_host)
                || lists_this
                || own_sites.iter().any(|s| same_site(s, &other_host))
            {
                found.push(other.clone());
            }
        }
        found.sort();
        found
    }

    /// `owner_secrets.py:197-208`: a place's username and, when it comes through a link, the
    /// place of the first record holding it; never one of two that differ.
    fn account_in(
        &self,
        records: &Map<String, Value>,
        place: &str,
        sites: &[String],
        holding: Holding<'_>,
    ) -> (Option<String>, Option<String>) {
        if let Some(own) = held(records.get(place), holding) {
            return (Some(own), None);
        }
        let holders: Vec<(String, String)> = self
            .linked(records, place, sites)
            .into_iter()
            .filter_map(|p| held(records.get(&p), holding).map(|name| (p, name)))
            .collect();
        let names: BTreeSet<&str> = holders.iter().map(|(_, name)| name.as_str()).collect();
        match (names.len(), holders.first()) {
            (1, Some((first, name))) => (Some(name.clone()), Some(first.clone())), // `linked` is sorted
            _ => (None, None),
        }
    }

    /// `owner_secrets.py:217-225` (decision: APP-2): every saved entry by name: its place, its
    /// name, its kind and whether it is unusable. Never a value, an account or a username.
    pub fn inventory(&self) -> Result<Vec<Value>> {
        let data = self.load()?;
        let records = object(&data, "records");
        let mut out = Vec::new();
        for place in sorted_keys(records) {
            let record = records[&place]
                .as_object()
                .expect("a loaded record is a dict");
            for name in sorted_keys(object(record, "entries")) {
                let kind = text(object(record, "kinds"), &name).unwrap_or("secret");
                out.push(json!({"place": place, "name": name, "kind": kind, "unusable": false}));
            }
        }
        for u in list(&data, "unusable") {
            out.push(
                json!({"place": u["place"], "name": u["name"], "kind": "secret", "unusable": true}),
            );
        }
        Ok(out)
    }

    /// `owner_secrets.py:227-251` (decisions: APP-2, GATE-2): the owner's Delete on one saved
    /// entry. Its Keychain item goes first (through the helper, never read), then its name; an
    /// item another entry also names stays; a record left with nothing goes too.
    pub fn delete(&self, place: &str, name: &str) -> Result<Value> {
        self.edit(|data| {
            let in_record = object(data, "records")
                .get(place)
                .and_then(Value::as_object)
                .and_then(|record| text(object(record, "entries"), name))
                .map(str::to_owned);
            let unusable = list(data, "unusable").iter().position(|u| {
                u.get("place").and_then(Value::as_str) == Some(place)
                    && u.get("name").and_then(Value::as_str) == Some(name)
            });
            let account = match (&in_record, unusable) {
                (Some(account), _) => account.clone(),
                (None, Some(_)) => name.to_owned(),
                (None, None) => return refused("nothing is saved under that name for that place"),
            };
            if accounts(data).iter().filter(|a| **a == account).count() == 1 {
                self.keychain.delete(&account)?;
            }
            if in_record.is_some() {
                let records = object_mut(data, "records");
                let record = object_mut(records, place);
                object_mut(record, "entries").shift_remove(name);
                object_mut(record, "kinds").shift_remove(name);
                let bare = object(record, "entries").is_empty()
                    && !record.get("username").is_some_and(py::truthy)
                    && !record.get("sites").is_some_and(py::truthy);
                if bare {
                    records.shift_remove(place);
                }
            } else if let (Some(i), Some(Value::Array(items))) =
                (unusable, data.get_mut("unusable"))
            {
                items.remove(i);
            }
            Ok(json!({"place": place, "name": name, "deleted": true}))
        })
    }

    /// `owner_secrets.py:281-282`.
    pub fn records(&self) -> Result<Map<String, Value>> {
        Ok(object(&self.load()?, "records").clone())
    }

    /// `owner_secrets.py:284-285`.
    pub fn unusable(&self) -> Result<Vec<Value>> {
        Ok(list(&self.load()?, "unusable").to_vec())
    }

    /// `owner_secrets.py:287-289`. `sites`: the current task's owner-typed site, read as a link.
    pub fn username(&self, target: &str, sites: &[String]) -> Result<Option<String>> {
        Ok(self.account(target, sites, None)?.0)
    }

    /// `owner_secrets.py:291-295`: the username for `target` and, when it comes through a link,
    /// the place whose record holds it.
    pub fn account(
        &self,
        target: &str,
        sites: &[String],
        holding: Holding<'_>,
    ) -> Result<(Option<String>, Option<String>)> {
        let data = self.load()?;
        Ok(self.account_in(object(&data, "records"), target, sites, holding))
    }

    /// `owner_secrets.py:297-314`: what `secrets.list` shows for one place: its username and its
    /// entries' names, never a value or a Keychain account (L36 §5).
    pub fn listing(&self, target: &str, sites: &[String], holding: Holding<'_>) -> Result<Value> {
        let data = self.load()?;
        let records = object(&data, "records");
        let (username, source) = self.account_in(records, target, sites, holding);
        let own = records.get(target).and_then(Value::as_object);
        let mut out = Map::new();
        out.insert("place".into(), target.into());
        out.insert("username".into(), username.into());
        if let Some(source) = source {
            out.insert(
                "username_site".into(),
                site_host(self.places.as_ref(), &source).into(),
            );
        }
        out.insert(
            "entries".into(),
            own.map(|r| sorted_keys(object(r, "entries")))
                .unwrap_or_default()
                .into(),
        );
        if let Some(kinds) = own
            .map(|r| object(r, "kinds"))
            .filter(|kinds| !kinds.is_empty())
        {
            out.insert("card_details".into(), Value::Object(kinds.clone()));
        }
        let unusable: Vec<Value> = list(&data, "unusable")
            .iter()
            .map(|u| json!({"name": u["name"], "why": u["why"]}))
            .collect();
        out.insert("unusable".into(), unusable.into());
        let linked: Vec<Value> = self
            .linked(records, target, sites)
            .into_iter()
            .map(|p| {
                let entries = records[&p]
                    .as_object()
                    .map(|r| sorted_keys(object(r, "entries")))
                    .unwrap_or_default();
                json!({"place": p, "username": held(records.get(&p), holding), "entries": entries})
            })
            .collect();
        if !linked.is_empty() {
            out.insert("linked".into(), linked.into());
        }
        Ok(Value::Object(out))
    }

    /// `owner_secrets.py:316-329`: the value saved as `handle` for exactly `target`, for a field
    /// of kind `kind` (decisions: GATE-206, GATE-219).
    pub fn reveal(&self, handle: &str, target: &str, kind: Option<&str>) -> Result<Secret> {
        refuse_whole_browser(self.places.as_ref(), target)?; // one saved that way before the rule is never filled
        let data = self.load()?;
        let record = object(&data, "records")
            .get(target)
            .and_then(Value::as_object);
        let Some(account) = record.and_then(|r| text(object(r, "entries"), handle)) else {
            return refused("the secret handle is not bound to this exact destination");
        };
        let saved = record.and_then(|r| text(object(r, "kinds"), handle));
        if let Some(why) = card_mismatch(saved, kind) {
            return refused(why);
        }
        match self.keychain.lookup(account)? {
            Some(value) => Ok(value),
            None => refused("the saved secret is unavailable"),
        }
    }

    /// `owner_secrets.py:331-352`: save `value` under the name `handle` for the place `target`,
    /// replacing that name's value. `sites`: the sites the record lists from then on (I14). A
    /// new entry's Keychain account is never one another entry already uses.
    pub fn remember(
        &self,
        handle: &str,
        target: &str,
        value: &Secret,
        sites: &[String],
        kind: Option<&str>,
    ) -> Result<()> {
        let places = self.places.as_ref();
        refuse_whole_browser(places, target)?;
        let own = site_host(places, target);
        let hosts: BTreeSet<String> = sites
            .iter()
            .map(|site| site_host(places, site))
            .filter(|host| !host.is_empty() && !same_site(host, &own))
            .collect();
        self.edit(|data| {
            let known = object(data, "records")
                .get(target)
                .and_then(Value::as_object)
                .and_then(|record| text(object(record, "entries"), handle))
                .map(str::to_owned);
            let account = match known {
                Some(account) => account,
                None => {
                    let account = format!("{target} \u{b7} {handle}");
                    if accounts(data).contains(&account.as_str()) {
                        return refused(
                            "that name would reuse the Keychain item of another saved secret; save it under \
                             another name",
                        );
                    }
                    account
                }
            };
            self.keychain.store(&account, value)?;
            let record = object_mut(data, "records").entry(target).or_insert_with(|| Value::Object(empty()));
            let record = record.as_object_mut().expect("a loaded record is a dict");
            object_mut(record, "entries").insert(handle.to_owned(), account.into());
            match kind.filter(|k| CARD_KINDS.contains(k)) {
                Some(kind) => {
                    object_mut(record, "kinds").insert(handle.to_owned(), kind.into());
                }
                None => {
                    object_mut(record, "kinds").shift_remove(handle);
                }
            }
            let mut all: BTreeSet<String> =
                list(record, "sites").iter().filter_map(Value::as_str).map(str::to_owned).collect();
            all.extend(hosts.iter().cloned());
            record.insert("sites".into(), all.into_iter().collect::<Vec<_>>().into());
            Ok(())
        })
    }

    /// `owner_secrets.py:354-362`: the record's username, from an owner-stated memory item
    /// (`source`: "memory:<id>"), kept as the owner spelled it; `target` must be a login place.
    pub fn set_username(&self, target: &str, username: &str, source: &str) -> Result<()> {
        if login_place(target) != target {
            return refused("a login record is kept for an https:// origin or app:<bundle id>");
        }
        refuse_whole_browser(self.places.as_ref(), target)?;
        self.edit(|data| {
            let record = object_mut(data, "records")
                .entry(target)
                .or_insert_with(|| Value::Object(empty()));
            let record = record.as_object_mut().expect("a loaded record is a dict");
            record.insert("username".into(), username.into());
            record.insert("username_source".into(), source.into());
            Ok(())
        })
    }

    /// `owner_secrets.py:364-373`: every username that came from one of `sources` gets
    /// `username` from `source` instead, or is cleared when `username` is none or empty; the
    /// places, sorted.
    pub fn replace_usernames(
        &self,
        sources: &BTreeSet<String>,
        username: Option<&str>,
        source: Option<&str>,
    ) -> Result<Vec<String>> {
        self.edit(|data| {
            let records = object_mut(data, "records");
            let mut changed: Vec<String> = records
                .iter()
                .filter(|(_, record)| {
                    record
                        .get("username_source")
                        .and_then(Value::as_str)
                        .is_some_and(|s| sources.contains(s))
                })
                .map(|(place, _)| place.clone())
                .collect();
            changed.sort();
            let (name, from) = match username.filter(|name| !name.is_empty()) {
                Some(name) => (Value::from(name), source.map_or(Value::Null, Value::from)),
                None => (Value::Null, Value::Null),
            };
            for place in &changed {
                let record = object_mut(records, place);
                record.insert("username".into(), name.clone());
                record.insert("username_source".into(), from.clone());
            }
            Ok(changed)
        })
    }

    /// `owner_secrets.py:375-378`.
    pub fn clear_usernames(&self, sources: &BTreeSet<String>) -> Result<Vec<String>> {
        self.replace_usernames(sources, None, None)
    }
}
