//! Chat capture: what FERB keeps from a chat turn (`ferb/daemon/areas/memory.py` `_remember`, `_until`,
//! `_save_login_name`). The planner names what the owner said about themselves with the exact words it rests on;
//! code checks the quote is whole words of what the owner typed (`items.capture`, L36), saves it (the owner's own
//! clause as theirs, any other wording as FERB's understanding, MEM-12/MEM-419), works out an end day from the owner's
//! own date words (L36 §6: the model never computes dates), and gives a site's login record its username only from the
//! owner's own words for a site they named (L36 §5). Memory never authorizes an action or saves a secret (MEM-12).
//!
//! Each item comes as `Remember.model_dump()` (orchestrator/schemas.py:438): `kind`, `subject`, `key`, `content`,
//! `quote`, `login_origin`, `until` {`quote`, `day`, `date`, `in_days`}, `replaces`, `review_in_days`.

use std::collections::BTreeSet;
use std::rc::Rc;

use ferb_learning::items::origin_label;
use serde_json::{json, Value};

use super::seams::LoginError;
use super::{to_py, State};

/// worker.py:179 `REASON_CHARS`: an exception's own words in the owner's line, at most.
const REASON_CHARS: usize = 300;
/// worker.py:180 `BUG`.
const BUG: &str = "That's a bug in FERB, not something you asked wrong.";

fn s(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        other => ferb_py::value::py_str(other),
    }
}

/// worker.py `_ended(_words(...))` for a FERB error's own message, else the bug line (`reason(e, redact)` for the
/// errors `memory.save` raises).
fn reason(st: &State, e: &ferb_learning::Error) -> String {
    use ferb_learning::Error as E;
    let words = |text: &str| {
        let text = ferb_py::value::squeeze(&st.redact_text(text));
        if ferb_py::value::char_len(&text) <= REASON_CHARS {
            text
        } else {
            ferb_py::text::shorten(&text, REASON_CHARS, true)
        }
    };
    let said = e.to_string();
    let ferb = matches!(
        e,
        E::MemoryRefused(_)
            | E::LearningRefused(_)
            | E::ImportUnreadable(_)
            | E::Ferb(_)
            | E::InvalidAnswer(_)
    );
    if ferb && !ferb_py::value::strip(&said).is_empty() {
        let t = words(&said);
        return if t.ends_with(['.', '!', '?', '…']) {
            t
        } else {
            format!("{t}.")
        };
    }
    let detail = words(&said);
    let sep = if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    };
    format!("something broke inside FERB ({}{sep}). {BUG}", e.class())
}

/// Whether `memory.save`'s error is a `FerbError` (caught and said) rather than a crash.
fn is_ferb_error(e: &ferb_learning::Error) -> bool {
    use ferb_learning::Error as E;
    matches!(
        e,
        E::MemoryRefused(_)
            | E::LearningRefused(_)
            | E::ImportUnreadable(_)
            | E::ImportBusy(_)
            | E::Ferb(_)
            | E::InvalidAnswer(_)
    )
}

/// `_remember(items, own_words, source_id, sites, elsewhere, about)`: per saved item its `receipt`, `line` and
/// `card`; per refused item `refused` (and `guard` when the quote check refused it).
pub async fn remember(
    st: &Rc<State>,
    items: &[Value],
    own_words: &str,
    source_id: &str,
    sites: &[String],
    elsewhere: &[String],
    about: &str,
) -> ferb_learning::Result<Vec<Value>> {
    let mut saved = Vec::new();
    for it in items {
        let (kind, subject, key) = (s(&it["kind"]), s(&it["subject"]), s(&it["key"]));
        let got = ferb_learning::text::capture(own_words, &s(&it["content"]), &s(&it["quote"]));
        if !got.refused.is_empty() {
            st.journal.append(
                "memory.capture_refused",
                Some(json!({"key": key, "why": got.refused})),
                None,
            )?;
            let told = ferb_learning::text::capture_told(&got.refused).unwrap_or_default();
            saved.push(json!({"refused": told, "guard": got.refused}));
            continue;
        }
        let topic_src = if about.is_empty() {
            key.replace(['_', '-'], " ")
        } else {
            about.to_owned()
        };
        let topic = ferb_py::value::squeeze(&topic_src);
        let replaces: Vec<String> = it["replaces"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|r| super::memory::memory_id(st, &s(r)))
                    .collect()
            })
            .unwrap_or_default();
        let confidence = if got.origin == "owner_stated" {
            1.0
        } else {
            0.7
        };
        let done = match st.memory.save(
            &kind,
            &subject,
            &key,
            &got.content,
            &got.origin,
            &[("journal".to_owned(), source_id.to_owned())],
            confidence,
            &replaces,
            &topic,
            Some(true),
        ) {
            Ok(done) => done,
            Err(e) if is_ferb_error(&e) => {
                st.journal.append(
                    "memory.capture_refused",
                    Some(json!({"key": key, "why": e.to_string()})),
                    None,
                )?;
                saved.push(json!({"refused": reason(st, &e)}));
                continue;
            }
            Err(e) => return Err(e),
        };
        let m = &done.item;
        if !done.not_replaced.is_empty() {
            st.journal.append(
                "memory.replace_refused",
                Some(json!({"item": m.id, "ids": done.not_replaced})),
                None,
            )?;
        }
        let days = review_days(&it["review_in_days"]);
        if days > 0 && m.valid_to.is_none() {
            let at = st.now() + chrono::Duration::days(days);
            st.memory.set_review(&m.id, Some(&to_py(&at)))?;
        }
        let text = ferb_py::value::squeeze(&m.content);
        let mut ends = match it.get("until").filter(|u| !u.is_null()) {
            Some(until) => format!(" {}", until_line(st, until, &m.id, own_words)?),
            None => String::new(),
        };
        // a safety check's own line
        let stopped = if ends.starts_with(" \n") {
            let line = ferb_py::value::strip(&ends).to_owned();
            ends.clear();
            line
        } else {
            String::new()
        };
        let login = s(&it["login_origin"]);
        let told = if login.is_empty() {
            String::new()
        } else {
            save_login_name(st, it, &m.id, own_words, sites, &m.origin, elsewhere).await?
        };
        let told: Vec<&str> = [told.as_str(), stopped.as_str()]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect();
        let told = told.join("\n");
        let label = origin_label(&m.origin);
        let replaced = done
            .replaced
            .iter()
            .map(|o| format!("“{}”", ferb_py::value::squeeze(&o.content)))
            .collect::<Vec<_>>()
            .join("; ");
        let line = format!(
            "Memory: noted “{text}” ({label}){ends}{}",
            if replaced.is_empty() {
                String::new()
            } else {
                format!(" — replaces {replaced}")
            }
        );
        let replaced_ids: Vec<String> = done.replaced.iter().map(|o| o.id.clone()).collect();
        let receipt = format!(
            "remembered ({}, {label}): {}{}",
            m.id,
            m.content,
            if replaced_ids.is_empty() {
                String::new()
            } else {
                format!(" (replaces {})", replaced_ids.join(", "))
            }
        );
        // step 8e: each saved item's note card, from the stored item (its id is what Correct / Forget act on)
        let mut card = ferb_orchestrator::cards::note(&m.id, &text, "saved", &line);
        card["label"] = json!(label);
        card["origin"] = json!(m.origin);
        card["replaces"] = json!(replaced_ids);
        let full = if told.is_empty() {
            line
        } else {
            format!("{line}\n{told}")
        };
        saved.push(json!({"receipt": receipt, "line": full, "card": card}));
    }
    Ok(saved)
}

/// `int(getattr(it, "review_in_days", 0) or 0)`.
fn review_days(v: &Value) -> i64 {
    match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f.trunc() as i64))
            .unwrap_or(0),
        Value::Bool(b) => i64::from(*b),
        _ => 0,
    }
}

/// `_until(until, item_id, own_words)`: a remembered item's last day from the owner's own date words. A line for the
/// owner; date words they didn't write are the quote check's, said on a line of its own (starting "\n").
pub fn until_line(
    st: &State,
    until: &Value,
    item_id: &str,
    own_words: &str,
) -> ferb_learning::Result<String> {
    use ferb_policy::small::blocked;
    if !ferb_learning::text::quoted(own_words, &s(&until["quote"]), false) {
        st.journal.append(
            "memory.until_refused",
            Some(json!({"item": item_id, "why": "date words not typed"})),
            None,
        )?;
        return Ok(format!(
            "\n{}",
            blocked::blocked(
                blocked::END_DATE,
                "the date words I quoted from you aren't in what you wrote",
                "it's saved without an end date",
                "tell me when it ends and I'll add it",
            )
        ));
    }
    let when = ferb_clock::when::When {
        day: until["day"].as_str().map(str::to_owned),
        date: until["date"].as_str().map(str::to_owned),
        in_days: until["in_days"].as_i64(),
        ..Default::default()
    };
    let zone = st.clock.zone();
    let end = match ferb_clock::when::end_of_day(&when, &st.now(), &zone) {
        Ok(end) => end,
        Err(e) => {
            st.journal.append(
                "memory.until_refused",
                Some(json!({"item": item_id, "why": e.to_string()})),
                None,
            )?;
            return Ok(format!("(saved without an end date: {e})"));
        }
    };
    st.memory.expire_at(item_id, &to_py(&end))?;
    let last = zone.local(&(end - chrono::Duration::seconds(1)));
    Ok(format!(
        "(until {})",
        ferb_clock::py::fmt(&last, "%a %d %b")
    ))
}

/// Which apps are web browsers and how a browser reads an address (`is_browser`, `urls.parse`).
struct Places;

impl ferb_identity::owner_secrets::Places for Places {
    fn is_browser(&self, bundle_id: &str) -> bool {
        ferb_mac_hands::apps::known(bundle_id)
    }

    fn url_host(&self, url: &str) -> Option<String> {
        ferb_policy::urls::parse(url)
            .map(|p| p.host)
            .filter(|h| !h.is_empty())
    }
}

/// `_save_login_name(entry, item_id, own_words, sites, origin, elsewhere)` (L36 §5): an owner-stated entry's username
/// for its site's login record, only from the owner's own words for an https:// site they named. A line for the owner,
/// or "".
pub async fn save_login_name(
    st: &Rc<State>,
    entry: &Value,
    item_id: &str,
    own_words: &str,
    sites: &[String],
    origin: &str,
    elsewhere: &[String],
) -> ferb_learning::Result<String> {
    use ferb_identity::owner_secrets::{login_place, same_site, site_host, typed_site};
    let place = login_place(&s(&entry["login_origin"]));
    let host = if place.starts_with("https://") {
        site_host(&Places, &place)
    } else {
        String::new()
    };
    let named = !host.is_empty() && {
        let typed = ferb_policy::provenance::prepare(own_words);
        let typed_fn = |t: &str| typed.tokens.contains(t) || typed.hosts.contains(t);
        typed_site(&host, &typed_fn)
            || sites
                .iter()
                .any(|site| same_site(&site_host(&Places, site), &host))
    };
    let span = if origin == "owner_stated" {
        ferb_learning::text::owner_span_at(own_words, &s(&entry["content"]))
    } else {
        None
    };
    let chars: Vec<char> = own_words.chars().collect();
    let hidden = span.is_some_and(|(a, b)| {
        redacted_marks(&chars)
            .into_iter()
            .any(|(start, end)| start <= b && a <= end)
    });
    let mut why = if host.is_empty() {
        "the site isn't an https:// address".to_owned()
    } else if !named {
        "the site isn't one you named".to_owned()
    } else if span.is_none() {
        "the username isn't in your words".to_owned()
    } else if hidden {
        "the username was hidden as a secret".to_owned()
    } else {
        String::new()
    };
    let name: String = match span {
        Some((a, b)) if !hidden => chars[a..b].iter().collect(),
        _ => String::new(),
    };
    let older: BTreeSet<String> = if name.is_empty() {
        BTreeSet::new()
    } else {
        st.memory
            .history(item_id)?
            .into_iter()
            .filter(|v| v.id != item_id)
            .map(|v| format!("memory:{}", v.id))
            .collect()
    };
    let mut followed: Vec<String> = Vec::new();
    if !name.is_empty() && (why.is_empty() || !older.is_empty()) {
        let save = why.is_empty();
        let source = format!("memory:{item_id}");
        let lock = st.login_names.clone();
        let _held = lock.lock().await;
        match st
            .logins
            .save_username(&place, &name, &source, save, &older)
        {
            Ok(places) => followed = places,
            Err(LoginError::Refused(e)) | Err(LoginError::Failed(e)) => why = e,
        }
    }
    if !followed.is_empty() {
        st.journal.append(
            "memory.login_updated",
            Some(json!({"item": item_id, "places": followed})),
            None,
        )?;
    }
    if why.is_empty() {
        return Ok(format!("Saved as your login name for {host}."));
    }
    let mut lines = Vec::new();
    if !followed.is_empty() {
        let hosts: Vec<String> = followed
            .iter()
            .map(|p| {
                let h = site_host(&Places, p);
                if h.is_empty() {
                    p.clone()
                } else {
                    h
                }
            })
            .collect();
        lines.push(format!("Updated your login name for {}.", hosts.join(", ")));
    }
    if !followed.contains(&place) {
        st.journal.append(
            "memory.login_refused",
            Some(json!({"item": item_id, "place": place, "why": why})),
            None,
        )?;
        if !host.is_empty() && !named {
            // the one refusal the owner can act on (final review I1)
            let here = if elsewhere
                .iter()
                .any(|e| same_site(&site_host(&Places, e), &host))
            {
                " in this chat"
            } else {
                ""
            };
            lines.push(format!(
                "Not saved as your username for {host}: you didn't name that site{here}."
            ));
        }
    }
    Ok(lines.join(" "))
}

/// `re.finditer(re.escape(REDACTED), own_words)`: each marker's (start, end), in characters.
fn redacted_marks(chars: &[char]) -> Vec<(usize, usize)> {
    let mark: Vec<char> = ferb_sanitize::known::REDACTED.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + mark.len() <= chars.len() {
        if chars[i..i + mark.len()] == mark[..] {
            out.push((i, i + mark.len()));
            i += mark.len();
        } else {
            i += 1;
        }
    }
    out
}

/// areas/schedules.py:397 `_typed_behind_memory(request_id)`: what the owner typed behind each active owner-stated
/// memory item: the text of the `memory.add` / `memory.edit` entry it rests on, or the owner's own words (pasted
/// parts left out) of the chat turn it was captured in; never the item's content. The current request's turn is
/// left out, and so is the setup persona (FERB built it from app names; it never names a send destination).
pub fn typed_behind_memory(st: &State, request_id: &str) -> Vec<String> {
    let persona = ferb_memory::terms::slug(ferb_device::persona::PERSONA_KEY, None);
    let Ok(items) = st.memory.items(Some("active"), None, true) else {
        return Vec::new();
    };
    let ids: BTreeSet<String> = items
        .iter()
        .filter(|it| it.origin == "owner_stated" && it.key != persona)
        .flat_map(|it| it.source_ids.iter())
        .filter_map(|src| src.strip_prefix("journal:").map(str::to_owned))
        .collect();
    if ids.is_empty() {
        return Vec::new();
    }
    let ids: Vec<String> = ids.into_iter().collect();
    let mut typed = Vec::new();
    for entry in st.journal.by_ids(&ids).unwrap_or_default() {
        let data = &entry.data;
        match entry.kind.as_str() {
            "memory.added" | "memory.edited" => typed.push(s(&data["content"])),
            "chat.message" if s(&data["request_id"]) != request_id => {
                let pasted = match data.get("untrusted_parts") {
                    Some(v) if !v.is_null() => {
                        let n = v.as_i64().unwrap_or(0);
                        let paths = data.get("input_paths").is_some_and(ferb_py::value::truthy);
                        Some((n - i64::from(paths)).max(0) as usize)
                    }
                    _ => None,
                };
                if let Some(own) =
                    ferb_sanitize::untrusted::recorded_own_words(&s(&data["text"]), pasted)
                {
                    if !own.is_empty() {
                        typed.push(own);
                    }
                }
            }
            _ => {}
        }
    }
    typed
}
