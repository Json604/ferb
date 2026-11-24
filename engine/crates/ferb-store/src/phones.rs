//! The phones paired with this Mac and their audit (migration 0065; stage 3 S3, kb T-1202). No Python counterpart: the
//! phone lab kept these in the link's own sealed file (`link.json`) and an `audit.jsonl`; in FERB the daemon holds them
//! (PHN-124, PHN-441). Public keys only. The rules about them (at most 5, the 30-day warning, who may remove) are the
//! daemon's phone area's (ferb-daemon `phone`); this module only reads and writes rows.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::Value;

use crate::{pyjson, Error, Result};

/// The schema version of migration 0065.
pub const SCHEMA_VERSION: i64 = 65;
/// The schema version of migration 0068: each pairing's FERB account (PHN-828).
pub const ACCOUNT_VERSION: i64 = 68;
/// The tables migration 0065 adds.
pub const TABLES: [&str; 2] = ["phone_devices", "phone_audit"];

/// One paired phone (a live row unless `removed_at` is set).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phone {
    pub id: i64,
    /// The phone's link identity: what the link's hello names as the device.
    pub endpoint: String,
    pub name: String,
    pub app: String,
    /// The lock key (PIN, face or fingerprint), when the phone has a screen lock.
    pub unlock_pub: Option<String>,
    /// The device key (no prompt).
    pub device_pub: Option<String>,
    /// The fingerprint-only key, when the phone had a strong biometric.
    pub strong_pub: Option<String>,
    pub paired_at: String,
    pub last_seen: String,
    pub warned_at: Option<String>,
    pub removed_at: Option<String>,
    pub removed_by: Option<String>,
    /// The FERB account the phone paired under (PHN-828, migration 0068); None for a phone paired before accounts.
    pub account_id: Option<String>,
}

/// What a pairing adds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewPhone {
    pub endpoint: String,
    pub name: String,
    pub app: String,
    pub unlock_pub: Option<String>,
    pub device_pub: Option<String>,
    pub strong_pub: Option<String>,
    /// The FERB account the server's signed statement named, which the daemon matched with the Mac's (PHN-828).
    pub account_id: Option<String>,
}

/// Why a pairing was not added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddRefused {
    /// `max` live phones already.
    Full { max: usize },
    /// This endpoint is a live paired phone already.
    AlreadyPaired,
}

const COLUMNS: &str = "id, endpoint, name, app, unlock_pub, device_pub, strong_pub, paired_at, last_seen, warned_at, \
                       removed_at, removed_by, account_id";

fn phone(r: &Row<'_>) -> rusqlite::Result<Phone> {
    Ok(Phone {
        id: r.get(0)?,
        endpoint: r.get(1)?,
        name: r.get(2)?,
        app: r.get(3)?,
        unlock_pub: r.get(4)?,
        device_pub: r.get(5)?,
        strong_pub: r.get(6)?,
        paired_at: r.get(7)?,
        last_seen: r.get(8)?,
        warned_at: r.get(9)?,
        removed_at: r.get(10)?,
        removed_by: r.get(11)?,
        account_id: r.get(12)?,
    })
}

/// The live phones, oldest pairing first.
pub fn live(conn: &Connection) -> Result<Vec<Phone>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM phone_devices WHERE removed_at IS NULL ORDER BY id"
    ))?;
    let rows = st.query_map([], phone)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The live phone with this endpoint.
pub fn by_endpoint(conn: &Connection, endpoint: &str) -> Result<Option<Phone>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM phone_devices WHERE endpoint = ? AND removed_at IS NULL"
            ),
            [endpoint],
            phone,
        )
        .optional()?)
}

/// Every row, removed ones included, oldest first (the owner's history of pairings).
pub fn all(conn: &Connection) -> Result<Vec<Phone>> {
    let mut st = conn.prepare(&format!("SELECT {COLUMNS} FROM phone_devices ORDER BY id"))?;
    let rows = st.query_map([], phone)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Adds a pairing when fewer than `max` phones are live and the endpoint is not one of them, in one transaction
/// (`BEGIN IMMEDIATE`, STORE-415), so two pairings at once can't both pass the count.
pub fn add(
    db: &crate::Db,
    new: &NewPhone,
    now: &str,
    max: usize,
) -> Result<std::result::Result<Phone, AddRefused>> {
    db.transaction(|conn| -> Result<_> {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM phone_devices WHERE removed_at IS NULL",
            [],
            |r| r.get(0),
        )?;
        if n as usize >= max {
            return Ok(Err(AddRefused::Full { max }));
        }
        if by_endpoint(conn, &new.endpoint)?.is_some() {
            return Ok(Err(AddRefused::AlreadyPaired));
        }
        conn.execute(
            "INSERT INTO phone_devices (endpoint, name, app, unlock_pub, device_pub, strong_pub, paired_at, last_seen, \
             account_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                new.endpoint,
                new.name,
                new.app,
                new.unlock_pub,
                new.device_pub,
                new.strong_pub,
                now,
                now,
                new.account_id
            ],
        )?;
        let added = by_endpoint(conn, &new.endpoint)?
            .ok_or_else(|| Error::Value("the phone just added is not there".into()))?;
        Ok(Ok(added))
    })
}

/// Renames a live phone; false when there is none.
pub fn rename(conn: &Connection, endpoint: &str, name: &str) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE phone_devices SET name = ? WHERE endpoint = ? AND removed_at IS NULL",
        params![name, endpoint],
    )? == 1)
}

/// Removes a live phone (the row stays, marked); the phone as it was, or `None` when there is none.
pub fn remove(conn: &Connection, endpoint: &str, by: &str, now: &str) -> Result<Option<Phone>> {
    let Some(found) = by_endpoint(conn, endpoint)? else {
        return Ok(None);
    };
    conn.execute(
        "UPDATE phone_devices SET removed_at = ?, removed_by = ? WHERE id = ?",
        params![now, by, found.id],
    )?;
    Ok(Some(found))
}

/// PHN-828: the live phone's FERB account, recorded when a phone paired before accounts signs in (or bound again to the
/// Mac's account); false when there is no such live phone.
pub fn bind_account(conn: &Connection, endpoint: &str, account_id: &str) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE phone_devices SET account_id = ? WHERE endpoint = ? AND removed_at IS NULL",
        params![account_id, endpoint],
    )? == 1)
}

/// The phone was seen now: its last use moves and an unused warning given before is cleared.
pub fn seen(conn: &Connection, endpoint: &str, now: &str) -> Result<()> {
    conn.execute(
        "UPDATE phone_devices SET last_seen = ?, warned_at = NULL WHERE endpoint = ? AND removed_at IS NULL",
        params![now, endpoint],
    )?;
    Ok(())
}

/// Registers the device key and the fingerprint-only key of a phone paired without them, each only when none is
/// registered yet (a key already there is replaced only by pairing again: PHN-133). The phone as it is now.
pub fn add_keys(
    conn: &Connection,
    endpoint: &str,
    device_pub: Option<&str>,
    strong_pub: Option<&str>,
) -> Result<Option<Phone>> {
    conn.execute(
        "UPDATE phone_devices SET device_pub = COALESCE(device_pub, ?), strong_pub = COALESCE(strong_pub, ?) \
         WHERE endpoint = ? AND removed_at IS NULL",
        params![device_pub, strong_pub, endpoint],
    )?;
    by_endpoint(conn, endpoint)
}

/// The unused warning was given now.
pub fn mark_warned(conn: &Connection, id: i64, now: &str) -> Result<()> {
    conn.execute(
        "UPDATE phone_devices SET warned_at = ? WHERE id = ? AND removed_at IS NULL",
        params![now, id],
    )?;
    Ok(())
}

/// One audit row: what a phone did. `detail` holds structural facts only and is passed through the redactor by the
/// caller (rule 7).
#[derive(Clone, Debug, PartialEq)]
pub struct AuditRow {
    pub at: String,
    pub device: String,
    pub method: String,
    pub class: String,
    pub outcome: String,
    pub detail: Value,
}

pub fn audit(conn: &Connection, row: &AuditRow) -> Result<()> {
    conn.execute(
        "INSERT INTO phone_audit (at, device, method, class, outcome, detail_json) VALUES (?, ?, ?, ?, ?, ?)",
        params![
            row.at,
            row.device,
            row.method,
            row.class,
            row.outcome,
            pyjson::dumps_utf8(&row.detail)
        ],
    )?;
    Ok(())
}

/// The newest `limit` audit rows, newest last.
pub fn audit_tail(conn: &Connection, limit: usize) -> Result<Vec<AuditRow>> {
    let mut st = conn.prepare(
        "SELECT at, device, method, class, outcome, detail_json FROM \
         (SELECT * FROM phone_audit ORDER BY seq DESC LIMIT ?) ORDER BY seq",
    )?;
    let rows = st.query_map([limit as i64], |r| {
        let detail: String = r.get(5)?;
        Ok(AuditRow {
            at: r.get(0)?,
            device: r.get(1)?,
            method: r.get(2)?,
            class: r.get(3)?,
            outcome: r.get(4)?,
            detail: serde_json::from_str(&detail).unwrap_or(Value::Null),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// One phone's audit rows of these methods that ended `ok`, newest first, at most `limit` (T-1211: the files a phone
/// fetched or sent, for its recent files).
pub fn audit_ok_of(
    conn: &Connection,
    device: &str,
    methods: &[&str],
    limit: usize,
) -> Result<Vec<AuditRow>> {
    let marks = vec!["?"; methods.len().max(1)].join(", ");
    let mut st = conn.prepare(&format!(
        "SELECT at, device, method, class, outcome, detail_json FROM phone_audit \
         WHERE device = ? AND outcome = 'ok' AND method IN ({marks}) ORDER BY seq DESC LIMIT ?"
    ))?;
    let mut args: Vec<rusqlite::types::Value> = vec![device.to_owned().into()];
    args.extend(
        methods
            .iter()
            .map(|m| rusqlite::types::Value::from((*m).to_owned())),
    );
    if methods.is_empty() {
        args.push(String::new().into());
    }
    args.push((limit as i64).into());
    let rows = st.query_map(rusqlite::params_from_iter(args), |r| {
        let detail: String = r.get(5)?;
        Ok(AuditRow {
            at: r.get(0)?,
            device: r.get(1)?,
            method: r.get(2)?,
            class: r.get(3)?,
            outcome: r.get(4)?,
            detail: serde_json::from_str(&detail).unwrap_or(Value::Null),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Removes the audit rows written before `before` (an ISO time of the same form as `at`); how many went.
pub fn prune_audit(conn: &Connection, before: &str) -> Result<usize> {
    Ok(conn.execute("DELETE FROM phone_audit WHERE at < ?", [before])?)
}
