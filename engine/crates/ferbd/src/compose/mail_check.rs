//! MEM-476: a website watch's code check of the export mail, with no model. In the watch's paired browser profile,
//! only when that profile is already connected (never launched for this), FERB opens one background tab on Gmail's
//! search for mail from the site's own domains after the watch began, reads the result rows' markup, and closes the
//! tab. What it takes from them is structural: each row's sender address (its `email` attribute) and its thread id;
//! Gmail's own search does the date filter, so no localized date is read, and no subject or body is looked at (rule 4).
//! Anything else (the profile isn't open, the page isn't Gmail, the results don't show) is `Unchecked`, and the watch
//! keeps its 12-hour model run.

use std::sync::Arc;
use std::time::Duration;

use ferb_browser::bridge::Hub;
use ferb_browser::driver::Driver;
use ferb_core::types::{Identity, WaitCondition, WaitKind};
use ferb_daemon::areas_a::{MailQuery, MailSeen};

/// The task-like owner of the one tab a check opens (its tabs are FERB's, closed when the check ends).
pub const OWNER: &str = "memory-mail-check";
/// Gmail's results region: present once the list (or its "no results") has drawn.
pub const SELECTOR: &str = "div[role=\"main\"]";
/// Looks at the drawn results, a pause apart; two in a row that agree are the answer.
const LOOKS: usize = 8;
const PAUSE: Duration = Duration::from_millis(2500);
/// Characters read per page of the markup, and pages at most.
const PAGE_CHARS: usize = 1_000_000;
const PAGES: usize = 8;

fn encode(q: &str) -> String {
    let mut out = String::new();
    for b in q.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'-' | b'_' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Gmail's search, in the profile's first account, for mail from any of `domains` (an address there or at a
/// subdomain) that came after `after` (Unix seconds; Gmail's `after:` takes them).
pub fn search_url(domains: &[String], after: i64) -> String {
    let from = if domains.len() == 1 {
        domains[0].clone()
    } else {
        format!("({})", domains.join(" OR "))
    };
    format!(
        "https://mail.google.com/mail/u/0/#search/{}",
        encode(&format!("from:{from} after:{after}"))
    )
}

fn attr<'a>(chunk: &'a str, name: &str) -> Vec<&'a str> {
    let key = format!(" {name}=\"");
    let mut out = Vec::new();
    let mut rest = chunk;
    while let Some(i) = rest.find(&key) {
        let v = &rest[i + key.len()..];
        let Some(end) = v.find('"') else { break };
        out.push(&v[..end]);
        rest = &v[end..];
    }
    out
}

fn in_domains(address: &str, domains: &[String]) -> bool {
    let Some((_, host)) = address.rsplit_once('@') else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    domains.iter().any(|d| {
        let d = d.to_ascii_lowercase();
        host == d || host.ends_with(&format!(".{d}"))
    })
}

/// The rows of Gmail's results region whose sender is in `domains`: each row's thread id (`data-legacy-thread-id`,
/// else `data-thread-id`), else the address. None: the results region isn't in the markup (not drawn yet, or not
/// Gmail's page).
pub fn found_in(markup: &str, domains: &[String]) -> Option<Vec<String>> {
    if !markup.contains(" role=\"main\"") {
        return None;
    }
    let mut out: Vec<String> = Vec::new();
    for row in markup.split("<tr").skip(1) {
        let Some(address) = attr(row, "email")
            .into_iter()
            .find(|a| in_domains(a, domains))
        else {
            continue;
        };
        let key = attr(row, "data-legacy-thread-id")
            .into_iter()
            .chain(attr(row, "data-thread-id"))
            .find(|t| !t.is_empty())
            .unwrap_or(address)
            .to_string();
        if !out.contains(&key) {
            out.push(key);
        }
    }
    Some(out)
}

fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '#', '?']).next().unwrap_or("")
}

/// The check, run in the daemon's runtime: open, wait for the results, read them twice, close.
pub async fn check(hub: Arc<Hub>, router: Arc<Driver>, q: MailQuery) -> MailSeen {
    let Some((browser, folder)) = q.profile.split_once('/') else {
        return MailSeen::Unchecked("the watch's browser profile isn't known".into());
    };
    let bridge = match hub.bridge(browser, folder) {
        Ok(Some(b)) if b.is_connected() && b.binding.identity == Identity::Owner => b,
        _ => return MailSeen::Unchecked("that browser profile isn't open now".into()),
    };
    let url = search_url(&q.domains, q.after);
    let tab = match router
        .open_in(bridge, &url, Some(OWNER), None, false, false, None, None)
        .await
    {
        Ok(t) => t,
        Err(e) => return MailSeen::Unchecked(format!("the tab didn't open: {e}")),
    };
    let seen = read(&router, &tab, &q.domains).await;
    let _ = router.close(&tab, None).await;
    let _ = router.release(OWNER).await;
    seen
}

async fn read(router: &Driver, tab: &ferb_core::types::TabRef, domains: &[String]) -> MailSeen {
    let loaded = WaitCondition {
        kind: WaitKind::Load,
        value: None,
    };
    let _ = router.wait(tab, &loaded, 30.0).await;
    let mut last: Option<Vec<String>> = None;
    for _ in 0..LOOKS {
        tokio::time::sleep(PAUSE).await;
        let host = router.tab_url(&tab.tab_id).unwrap_or_default();
        if host_of(&host) != "mail.google.com" {
            // a sign-in page, or a mailbox that isn't Gmail
            last = None;
            continue;
        }
        let mut markup = String::new();
        let mut offset = 0;
        for _ in 0..PAGES {
            let Ok(page) = router
                .html(tab, None, Some(SELECTOR), None, false, offset, PAGE_CHARS)
                .await
            else {
                break;
            };
            markup.push_str(page.get("text").and_then(|t| t.as_str()).unwrap_or(""));
            match page.get("next_offset").and_then(|n| n.as_u64()) {
                Some(n) => offset = n as usize,
                None => break,
            }
        }
        let now = found_in(&markup, domains);
        if now.is_some() && now == last {
            return match now {
                Some(found) if !found.is_empty() => MailSeen::Found(found),
                _ => MailSeen::Nothing,
            };
        }
        last = now;
    }
    MailSeen::Unchecked("Gmail's results didn't show in the watch's profile".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domains(d: &[&str]) -> Vec<String> {
        d.iter().map(|s| s.to_string()).collect()
    }

    // decision: MEM-476
    #[test]
    fn the_search_names_the_site_s_domains_and_the_watch_s_start() {
        assert_eq!(
            search_url(&domains(&["openai.com"]), 1_764_000_000),
            "https://mail.google.com/mail/u/0/#search/from%3Aopenai.com+after%3A1764000000"
        );
        assert_eq!(
            search_url(&domains(&["anthropic.com", "claude.ai"]), 5),
            "https://mail.google.com/mail/u/0/#search/from%3A%28anthropic.com+OR+claude.ai%29+after%3A5"
        );
    }

    // decision: MEM-476
    #[test]
    fn only_rows_from_the_site_s_own_domains_count_by_their_thread() {
        // invented markup in the shape browser.html gives (attributes as the page has them)
        let markup = r##"<!-- frame 0: https://mail.google.com/mail/u/0/ -->
<div class="AO" role="main"><table><tbody>
<tr class="zA zE"><td><span class="bA4"><span email="noreply@tm.openai.com" name="OpenAI">OpenAI</span></span></td>
<td><span data-thread-id="#thread-f:111" data-legacy-thread-id="18aa01">Your data export is ready</span></td></tr>
<tr class="zA"><td><span email="someone@openai.com.example.net" name="Look-alike">x</span></td>
<td><span data-legacy-thread-id="18aa02">y</span></td></tr>
<tr class="zA"><td><span email="news@mail.openai.com">OpenAI</span></td><td>no thread id here</td></tr>
<tr class="zA"><td><span email="noreply@tm.openai.com">OpenAI</span></td>
<td><span data-thread-id="#thread-f:111" data-legacy-thread-id="18aa01">again</span></td></tr>
</tbody></table></div>"##;
        assert_eq!(
            found_in(markup, &domains(&["openai.com"])),
            Some(vec![
                "18aa01".to_string(),
                "news@mail.openai.com".to_string()
            ])
        );
        assert_eq!(found_in(markup, &domains(&["anthropic.com"])), Some(vec![]));
        // Gmail's "no results" region: read, nothing found
        assert_eq!(
            found_in(
                r#"<div role="main"><div>No messages matched your search.</div></div>"#,
                &domains(&["openai.com"])
            ),
            Some(vec![])
        );
        // not drawn yet, or another page: no answer
        assert_eq!(
            found_in("<div>Loading…</div>", &domains(&["openai.com"])),
            None
        );
    }

    // decision: MEM-476
    #[test]
    fn a_page_that_isn_t_gmail_is_told_by_its_host() {
        assert_eq!(
            host_of("https://mail.google.com/mail/u/0/#search/x"),
            "mail.google.com"
        );
        assert_eq!(
            host_of("https://accounts.google.com/signin?x"),
            "accounts.google.com"
        );
        assert_eq!(
            host_of("https://mail.google.com.evil.example/"),
            "mail.google.com.evil.example"
        );
    }
}
