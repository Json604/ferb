//! policy/payments.py: "one payment, one question" (GATE-204, GATE-216, GATE-217, GATE-578, GATE-580) for payments in
//! a task's browser tab: which Yes covers a payment-class call, from the facts code read on the page (never the
//! worker's numbers). Approvals live in memory: after a restart FERB asks again.

use ferb_core::when::DateTime;
use serde_json::{json, Value};

use crate::confirm::{key, Need, Values, ALWAYS_PAY};
use crate::{py, pyurl};

pub const PAY_WINDOW_S: f64 = 600.0;
pub const UNREAD: &str = "an amount FERB couldn't read on the page";
/// Currency signs as the same currency's code, longest first (ties in Python's dict order).
const SIGNS: &[(&str, &str)] = &[
    ("rs.", "INR"),
    ("us$", "USD"),
    ("aed", "AED"),
    ("rs", "INR"),
    ("a$", "AUD"),
    ("c$", "CAD"),
    ("s$", "SGD"),
    ("₹", "INR"),
    ("$", "USD"),
    ("€", "EUR"),
    ("£", "GBP"),
    ("¥", "JPY"),
];

/// `_NUMBER.search(text)`: `\d[\d,\s]*(?:\.\d+)?`, as a byte span.
fn number_span(text: &str) -> Option<(usize, usize)> {
    let (start, _) = text.char_indices().find(|&(_, c)| py::is_decimal(c))?;
    let mut end = start;
    for (i, c) in text[start..].char_indices() {
        if py::is_decimal(c) || c == ',' || py::is_space(c) {
            end = start + i + c.len_utf8();
        } else {
            break;
        }
    }
    if text[end..].starts_with('.') {
        let frac: usize = text[end + 1..]
            .chars()
            .take_while(|&c| py::is_decimal(c))
            .map(char::len_utf8)
            .sum();
        if frac > 0 {
            end += 1 + frac;
        }
    }
    Some((start, end))
}

/// `format(Decimal(text).normalize(), "f")` for digits with at most one point (28 significant digits, half-even).
fn decimal_text(text: &str) -> Option<String> {
    let t = py::strip(text);
    let (int, frac) = match t.split_once('.') {
        Some((a, b)) => (a, b),
        None => (t, ""),
    };
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    if !int.chars().all(py::is_decimal)
        || !frac.chars().all(py::is_decimal)
        || (t.contains('.') && frac.is_empty() && int.is_empty())
    {
        return None;
    }
    let mut coeff: Vec<u8> = py::ascii_digits(&format!("{int}{frac}"))
        .bytes()
        .map(|b| b - b'0')
        .collect();
    let mut exp: i64 = -(py::char_len(frac) as i64);
    while coeff.len() > 1 && coeff[0] == 0 {
        coeff.remove(0);
    }
    if coeff.iter().all(|&d| d == 0) {
        return Some("0".to_string());
    }
    if coeff.len() > 28 {
        let drop = coeff.len() - 28;
        let rest: Vec<u8> = coeff.split_off(28);
        exp += drop as i64;
        let first = rest[0];
        let tail_nonzero = rest[1..].iter().any(|&d| d != 0);
        let round_up = first > 5 || (first == 5 && (tail_nonzero || coeff[27] % 2 == 1));
        if round_up {
            let mut i = coeff.len();
            loop {
                if i == 0 {
                    coeff.insert(0, 1);
                    break;
                }
                i -= 1;
                if coeff[i] == 9 {
                    coeff[i] = 0;
                } else {
                    coeff[i] += 1;
                    break;
                }
            }
        }
    }
    while coeff.len() > 1 && coeff[coeff.len() - 1] == 0 {
        coeff.pop();
        exp += 1;
    }
    let digits: String = coeff.iter().map(|d| (b'0' + d) as char).collect();
    Some(if exp >= 0 {
        format!("{digits}{}", "0".repeat(exp as usize))
    } else {
        let point = -exp as usize;
        if digits.len() > point {
            format!(
                "{}.{}",
                &digits[..digits.len() - point],
                &digits[digits.len() - point..]
            )
        } else {
            format!("0.{}{digits}", "0".repeat(point - digits.len()))
        }
    })
}

/// `amount_key`: an amount as shown, compared: (currency code or "", the number without grouping).
pub fn amount_key(shown: Option<&str>) -> Option<(String, String)> {
    let shown = shown.filter(|s| !s.is_empty())?;
    let text = py::squeeze(shown);
    let (a, b) = number_span(&text)?;
    let number = decimal_text(&text[a..b].replace([',', ' '], ""))?;
    let head = &text[..a];
    let tail = &text[b..];
    let before = if py::strip(head).is_empty() {
        String::new()
    } else {
        py::rstrip(head)
            .rsplit(' ')
            .next()
            .unwrap_or("")
            .to_string()
    };
    let after = if py::strip(tail).is_empty() {
        String::new()
    } else {
        py::lstrip(tail).split(' ').next().unwrap_or("").to_string()
    };
    let mut unit = String::new();
    for (token, at_end) in [(before, true), (after, false)] {
        let low = py::casefold(&token);
        unit = SIGNS
            .iter()
            .find(|(sign, _)| {
                low == *sign
                    || (!sign.chars().next().is_some_and(py::is_alpha)
                        && if at_end {
                            low.ends_with(sign)
                        } else {
                            low.starts_with(sign)
                        })
            })
            .map_or(String::new(), |(_, code)| code.to_string());
        if unit.is_empty() && token.len() == 3 && token.bytes().all(|b| b.is_ascii_uppercase()) {
            unit = token.clone();
        }
        if !unit.is_empty() {
            break;
        }
    }
    Some((unit, number))
}

/// `_same(a, b, amount_key)`: whether two amounts agree; None when either is unread.
pub fn same_amount(a: Option<&str>, b: Option<&str>) -> Option<bool> {
    let (ka, kb) = (amount_key(a)?, amount_key(b)?);
    if ka.0.is_empty() || kb.0.is_empty() {
        return Some(ka.1 == kb.1);
    }
    Some(ka == kb)
}

/// `merchant_key`.
pub fn merchant_key(shown: Option<&str>) -> Option<String> {
    let text = py::casefold(&py::squeeze(shown.unwrap_or("")));
    (!text.is_empty()).then_some(text)
}

/// `_same(a, b, merchant_key)`.
pub fn same_merchant(a: Option<&str>, b: Option<&str>) -> Option<bool> {
    Some(merchant_key(a)? == merchant_key(b)?)
}

/// payments.py:113 `PayFacts`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PayFacts {
    pub chain: String,
    pub host: String,
    pub frame_host: Option<String>,
    pub amount: Option<String>,
    pub merchant: Option<String>,
    pub submits_card: bool,
    pub submits: bool,
    pub shown: Vec<String>,
    pub declared: Option<String>,
    pub page: Option<String>,
    pub control: Option<String>,
}

/// payments.py:130 `Pressed` (GATE-580): the page just before a covered press, read fresh.
#[derive(Debug, Clone, PartialEq)]
pub struct Pressed {
    pub url: Option<String>,
    pub amounts: Option<Vec<String>>,
}

impl Pressed {
    /// `Pressed(None, None)` (pay_press.py:44, 59): the tab is gone or the read failed, so nothing fresher is known.
    pub fn unread() -> Pressed {
        Pressed {
            url: None,
            amounts: None,
        }
    }
}

/// `page_key`: a page's address without its fragment; Err where urlsplit raises.
pub fn page_key(url: Option<&str>) -> Result<Option<String>, pyurl::InvalidUrl> {
    let Some(url) = url.filter(|u| !u.is_empty()) else {
        return Ok(None);
    };
    let mut split = pyurl::urlsplit(url)?;
    split.fragment.clear();
    Ok(Some(pyurl::urlunsplit(&split)))
}

/// `digits_of` (GATE-580): the last (up to three) digits of an amount's whole part.
pub fn digits_of(amount: Option<&str>) -> String {
    let whole = amount_key(amount).map_or(String::new(), |k| {
        k.1.split('.').next().unwrap_or("").to_string()
    });
    let n = whole.len();
    whole[n.saturating_sub(3)..].to_string()
}

/// payments.py:149 `Approval`.
#[derive(Debug, Clone, PartialEq)]
pub struct Approval {
    pub cid: String,
    pub chain: String,
    pub host: String,
    pub amount: Option<String>,
    pub merchant: Option<String>,
    pub at: DateTime,
    pub bank_host: Option<String>,
    pub bank_amount: Option<String>,
    pub bank_merchant: Option<String>,
    pub bank_done: bool,
    pub cards: u32,
    pub paid_one_click: bool,
    pub done: bool,
    pub declared: Option<String>,
    pub seen: bool,
    pub page: Option<String>,
    pub control: Option<String>,
}

/// payments.py:172 `Check`: covered by `approval`, else a Need to ask.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub approval: Option<Approval>,
    pub need: Option<Need>,
}

fn nonempty(x: &Option<String>) -> Option<&str> {
    x.as_deref().filter(|s| !s.is_empty())
}

/// GATE-580: the page changes already asked about, by (tree, chain).
type Changes = Vec<((String, String), Vec<Vec<Value>>)>;

/// payments.py:178 `Payments`: the tree's approvals, by (tree, tab chain), in the order first approved.
pub struct Payments {
    pub now: Box<dyn Fn() -> DateTime + Send + Sync>,
    by: Vec<((String, String), Approval)>,
    changes: Changes,
}

fn seconds(d: chrono::TimeDelta) -> f64 {
    d.num_microseconds()
        .map_or(d.num_seconds() as f64, |us| us as f64 / 1e6)
}

impl Payments {
    pub fn new(now: Box<dyn Fn() -> DateTime + Send + Sync>) -> Payments {
        Payments {
            now,
            by: Vec::new(),
            changes: Vec::new(),
        }
    }

    fn index(&self, tree: &str, chain: &str) -> Option<usize> {
        self.by
            .iter()
            .position(|((t, c), _)| t == tree && c == chain)
    }

    fn active_index(&self, tree: &str, chain: &str) -> Option<usize> {
        let i = self.index(tree, chain)?;
        let a = &self.by[i].1;
        if a.done || seconds((self.now)() - a.at) > PAY_WINDOW_S {
            return None;
        }
        Some(i)
    }

    /// Whether this call is covered; `values` are the question's other facts (site, account, declared).
    pub fn check(&self, tree: &str, facts: &PayFacts, values: &Values) -> Check {
        let Some(i) = self.active_index(tree, &facts.chain) else {
            return Check {
                approval: None,
                need: Some(need(facts, values, None)),
            };
        };
        let a = &self.by[i].1;
        let ask = |approved: Option<&Approval>| Check {
            approval: None,
            need: Some(need(facts, values, approved)),
        };
        let bank = facts.host != a.host || nonempty(&facts.frame_host).is_some_and(|f| f != a.host);
        if bank {
            let amount = same_amount(facts.amount.as_deref(), a.amount.as_deref());
            let merchant = same_merchant(facts.merchant.as_deref(), a.merchant.as_deref());
            if amount == Some(false) || merchant == Some(false) {
                return ask(Some(a));
            }
            return Check {
                approval: Some(a.clone()),
                need: None,
            };
        }
        if a.bank_done || a.paid_one_click {
            return ask(None);
        }
        if facts.submits_card && a.cards > 0 {
            return ask(None);
        }
        if facts.amount.is_some()
            && same_amount(facts.amount.as_deref(), a.amount.as_deref()) != Some(true)
        {
            return ask(None);
        }
        Check {
            approval: Some(a.clone()),
            need: None,
        }
    }

    /// The owner's Yes to a payment question.
    pub fn approve(&mut self, tree: &str, facts: &PayFacts, cid: &str) -> Approval {
        let declared = if facts.amount.is_none() {
            facts.declared.clone()
        } else {
            None
        };
        let seen = nonempty(&declared).is_some()
            && facts
                .shown
                .iter()
                .any(|s| same_amount(Some(s), declared.as_deref()) == Some(true));
        let a = Approval {
            cid: cid.to_string(),
            chain: facts.chain.clone(),
            host: facts.host.clone(),
            amount: facts.amount.clone(),
            merchant: facts.merchant.clone(),
            at: (self.now)(),
            bank_host: None,
            bank_amount: None,
            bank_merchant: None,
            bank_done: false,
            cards: 0,
            paid_one_click: false,
            done: false,
            declared,
            seen,
            page: facts.page.clone(),
            control: facts.control.clone(),
        };
        match self.index(tree, &facts.chain) {
            Some(i) => self.by[i].1 = a.clone(),
            None => self
                .by
                .push(((tree.to_string(), facts.chain.clone()), a.clone())),
        }
        a
    }

    pub fn active(&self, tree: &str, chain: &str) -> Option<Approval> {
        self.active_index(tree, chain).map(|i| self.by[i].1.clone())
    }

    /// GATE-580: the page changed after the owner's Yes: that Yes no longer covers the chain (not a No).
    pub fn set_aside(&mut self, tree: &str, chain: &str) {
        if let Some(i) = self.index(tree, chain) {
            self.by.remove(i);
        }
    }

    /// GATE-580: whether this observed change of the page is new for the chain.
    pub fn first_change(&mut self, tree: &str, chain: &str, state: &[Value]) -> bool {
        let k = (tree.to_string(), chain.to_string());
        let i = match self.changes.iter().position(|(key, _)| *key == k) {
            Some(i) => i,
            None => {
                self.changes.push((k, Vec::new()));
                self.changes.len() - 1
            }
        };
        let seen = &mut self.changes[i].1;
        let same = |x: &Vec<Value>| {
            x.len() == state.len()
                && x.iter()
                    .zip(state)
                    .all(|(a, b)| ferb_py::value::py_eq(a, b))
        };
        if seen.iter().any(same) {
            return false;
        }
        seen.push(state.to_vec());
        true
    }

    /// Any call in an approved chain: the bank page it reached and what that page showed.
    pub fn observe(
        &mut self,
        tree: &str,
        chain: Option<&str>,
        host: Option<&str>,
        amount: Option<&str>,
        merchant: Option<&str>,
    ) {
        let Some(chain) = chain.filter(|c| !c.is_empty()) else {
            return;
        };
        let Some(i) = self.active_index(tree, chain) else {
            return;
        };
        let a = &mut self.by[i].1;
        let Some(host) = host.filter(|h| !h.is_empty()) else {
            return;
        };
        if host == a.host {
            return;
        }
        a.bank_host = Some(host.to_string());
        if let Some(x) = amount.filter(|x| !x.is_empty()) {
            a.bank_amount = Some(x.to_string());
        }
        if let Some(x) = merchant.filter(|x| !x.is_empty()) {
            a.bank_merchant = Some(x.to_string());
        }
    }

    /// A covered or approved call ran.
    pub fn ran(&mut self, tree: &str, facts: &PayFacts, ok: bool) {
        let Some(i) = self.active_index(tree, &facts.chain) else {
            return;
        };
        if !ok {
            return;
        }
        let a = &mut self.by[i].1;
        if facts.submits_card && facts.host == a.host {
            a.cards += 1;
        } else if facts.submits
            && facts.host == a.host
            && nonempty(&facts.frame_host).is_none()
            && a.cards == 0
            && !a.bank_done
        {
            a.paid_one_click = true;
        }
        if facts.host != a.host {
            let chain = facts.chain.clone();
            self.observe(
                tree,
                Some(&chain),
                Some(&facts.host),
                facts.amount.as_deref(),
                facts.merchant.as_deref(),
            );
            self.by[i].1.bank_done = true;
        }
    }

    /// The approval an OTP question at `host` belongs to.
    pub fn for_otp(&self, tree: &str, host: &str) -> Option<Approval> {
        let active: Vec<&Approval> = self
            .by
            .iter()
            .filter(|((t, c), _)| t == tree && self.active_index(t, c).is_some())
            .map(|(_, a)| a)
            .collect();
        let exact: Vec<&&Approval> = active
            .iter()
            .filter(|a| a.bank_host.as_deref() == Some(host) || a.host == host)
            .collect();
        if let Some(a) = exact.last() {
            return Some((**a).clone());
        }
        (active.len() == 1).then(|| active[0].clone())
    }

    pub fn forget(&mut self, trees: &[String]) {
        self.by.retain(|((t, _), _)| !trees.contains(t));
        self.changes.retain(|((t, _), _)| !trees.contains(t));
    }
}

/// `rechecks` (GATE-580).
pub fn rechecks(a: &Approval, facts: &PayFacts) -> bool {
    a.amount.is_none() && facts.host == a.host && nonempty(&facts.frame_host).is_none()
}

/// `pressed_change` (GATE-580): why a press the approval covers must not go now; None when nothing changed.
pub fn pressed_change(
    a: &Approval,
    facts: &PayFacts,
    now: Option<&Pressed>,
    asked_here: bool,
) -> Result<Option<&'static str>, pyurl::InvalidUrl> {
    let Some(now) = now else {
        return Ok(None);
    };
    if !rechecks(a, facts) {
        return Ok(None);
    }
    let same_control = asked_here || (nonempty(&a.control).is_some() && facts.control == a.control);
    if same_control && nonempty(&a.page).is_some() && page_key(now.url.as_deref())? != a.page {
        return Ok(Some("page"));
    }
    if nonempty(&a.declared).is_some()
        && a.seen
        && match &now.amounts {
            None => true,
            Some(amounts) => !amounts
                .iter()
                .any(|s| same_amount(Some(s), a.declared.as_deref()) == Some(true)),
        }
    {
        return Ok(Some("amount"));
    }
    Ok(None)
}

/// `stated` (GATE-578): the payment question's values for the AI's stated amount.
pub fn stated(read: Option<&str>, declared: Option<&str>, shown: &[String]) -> Values {
    let mut v = Values::new();
    let (Some(declared), false) = (
        declared.filter(|d| !d.is_empty()),
        read.is_some_and(|r| !r.is_empty()),
    ) else {
        return v;
    };
    v.set("declared", declared);
    if !shown
        .iter()
        .any(|a| same_amount(Some(a), Some(declared)) == Some(true))
    {
        v.set("unseen", "1");
    }
    v
}

/// `need`: the payment question (fresh: asked every time).
pub fn need(facts: &PayFacts, values: &Values, approved: Option<&Approval>) -> Need {
    let mut shown = Values::of(&[
        ("amount", nonempty(&facts.amount).unwrap_or(UNREAD)),
        ("merchant", facts.merchant.as_deref().unwrap_or("")),
        ("site", values.opt("site")),
        ("account", values.opt("account")),
        ("recipients", ""),
        ("chain", &facts.chain),
    ]);
    for (k, v) in stated(
        facts.amount.as_deref(),
        values.get("declared"),
        &facts.shown,
    )
    .0
    {
        shown.set(&k, &v);
    }
    if let Some(a) = approved {
        let to = nonempty(&a.merchant).map_or(String::new(), |m| format!(" to {m}"));
        shown.set(
            "approved",
            &format!("{}{to}", nonempty(&a.amount).unwrap_or(UNREAD)),
        );
        let bank = match nonempty(&facts.frame_host) {
            Some(f) if f != a.host => f.to_string(),
            _ => facts.host.clone(),
        };
        shown.set("bank", &bank);
    }
    let amount = match amount_key(facts.amount.as_deref()) {
        Some((u, n)) => json!([u, n]),
        None => match &facts.amount {
            Some(x) if !x.is_empty() => json!(x),
            Some(_) | None => facts.amount.as_ref().map_or(Value::Null, |x| json!(x)),
        },
    };
    let k = key(&[
        json!(ALWAYS_PAY),
        json!("tab"),
        json!(facts.chain),
        amount,
        merchant_key(facts.merchant.as_deref()).map_or(Value::Null, |m| json!(m)),
        shown.get("bank").map_or(Value::Null, |b| json!(b)),
    ]);
    let mut n = Need::new(ALWAYS_PAY, k, shown);
    n.fresh = true;
    n
}

/// `otp_card` (GATE-513, GATE-566): (question, why line, exact part) for a code asked on a payment's page.
pub fn otp_card(site: &str, a: &Approval) -> (String, String, String) {
    let said = "Your bank sent a one-time code to approve this payment. Type it only if it's the payment you approved. \
                FERB enters it on this page once; FERB's AI never sees it.";
    if let Some(bank_amount) = nonempty(&a.bank_amount) {
        let m = nonempty(&a.bank_merchant);
        let paid = format!(
            "{bank_amount}{}",
            m.map_or(String::new(), |m| format!(" to {m}"))
        );
        return (
            format!("{site}: code to pay {paid}"),
            said.to_string(),
            format!(
                "{bank_amount}{} · {site}",
                m.map_or(String::new(), |m| format!(" → {m}"))
            ),
        );
    }
    let amount = nonempty(&a.amount).unwrap_or(UNREAD);
    let m = nonempty(&a.merchant);
    let approved = format!(
        "{amount}{}",
        m.map_or(String::new(), |m| format!(" to {m}"))
    );
    (
        format!("{site}: code for the payment you approved ({approved})"),
        format!("FERB couldn't read the amount on this page. {said}"),
        format!(
            "{amount}{} · {site}",
            m.map_or(String::new(), |m| format!(" → {m}"))
        ),
    )
}

/// `otp_line`: what an OTP question on a payment's later page repeats.
pub fn otp_line(a: Option<&Approval>) -> String {
    let Some(a) = a else {
        return String::new();
    };
    if let Some(bank_amount) = nonempty(&a.bank_amount) {
        return format!(
            "This page shows {bank_amount}{}.",
            nonempty(&a.bank_merchant).map_or(String::new(), |m| format!(" to {m}"))
        );
    }
    let approved = format!(
        "{}{}",
        nonempty(&a.amount).unwrap_or(UNREAD),
        nonempty(&a.merchant).map_or(String::new(), |m| format!(" to {m}"))
    );
    format!(
        "For the payment you approved ({approved}); FERB couldn't read the amount on this page."
    )
}
