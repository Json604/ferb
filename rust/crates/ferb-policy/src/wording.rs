//! policy/confirm.py's words: FERB's question for a set of needs, as its one line (`summary`), its card (`card`,
//! GATE-513/566), the full question the record keeps (`line`) and the reason line. Display only: it decides nothing.
//! A sentence that needs a fact the need doesn't carry is Err(Missing), where Python raises KeyError.

use crate::confirm::{
    app_label, shown_quoted, Apps, Missing, Need, Values, ACCOUNT_SECURITY, BROWSER_PAGE, BULK,
    FOREGROUND, PAIR, QUOTED_CHARS, TERMINAL_INPUT, TLS_OFF, TLS_QUESTION, UNREAD_AMOUNT,
};
use crate::py;

type R<T> = Result<T, Missing>;

fn account(v: &Values) -> String {
    if v.has("email") {
        format!(" ({})", v.opt("email"))
    } else {
        String::new()
    }
}

fn terminal(v: &Values) -> R<String> {
    let mut where_ = (if v.has("label") {
        format!("the terminal {}", v.opt("label"))
    } else {
        "the terminal".to_string()
    }) + &format!(" in {}", v.req("app")?)
        + &(if v.has("window") {
            format!(" (window {})", v.opt("window"))
        } else {
            String::new()
        });
    if v.has("runs") {
        let mut facts = if v.has("selection") {
            vec![
                format!("the editor's selection is {}", v.opt("selection"))
                    + &(if v.has("selection_size") {
                        format!(" ({}, as FERB read it)", v.opt("selection_size"))
                    } else {
                        String::new()
                    }),
            ]
        } else {
            let file = v.req("file")?;
            vec![format!(
                "the selection or current line in {}, which FERB can't read",
                if file.is_empty() {
                    "the active editor"
                } else {
                    file
                }
            )]
        };
        if v.has("file") && v.has("selection") {
            facts.push(format!("the active file is {}", v.opt("file")));
        }
        let kind = if v.has("menu") { "menu" } else { "control" };
        where_ = if v.has("runs_where") {
            v.opt("runs_where").to_string()
        } else {
            v.req("app")?.to_string()
        };
        let opens = if where_.ends_with("terminal") {
            ", and a terminal it opens may run its profile's startup command"
        } else {
            ""
        };
        // `v.get('menu') or v.get('control')`: Python writes "None" when neither key is there
        let mut pressed = if v.has("menu") {
            v.opt("menu").to_string()
        } else {
            v.get("control").unwrap_or("None").to_string()
        };
        if v.has("keys") {
            pressed = format!("{} ({pressed})", v.opt("keys"));
        }
        return Ok(format!(
            "press {pressed} in {} (this {kind} runs code in {where_}{opens}; {})",
            v.req("app")?,
            facts.join(", ")
        ));
    }
    if v.has("script") {
        let mut said = format!(
            "run the AppleScript {} in {} (it runs with that app's own access, outside FERB's sandbox: {})",
            v.req("script")?,
            v.req("app")?,
            v.req("surface")?
        );
        let named: Vec<String> = [v.opt("files"), v.opt("named")]
            .iter()
            .filter(|f| !f.is_empty())
            .map(|f| format!("it names {f}"))
            .collect();
        if !named.is_empty() {
            said += &format!("; {}", named.join("; "));
            if v.has("opens") {
                said += "; FERB can't see any other file it opens";
            }
        } else if v.has("opens") {
            said += "; FERB can't see which file this opens, if it opens one";
        }
        return Ok(said);
    }
    if v.has("files") {
        return Ok(format!("open {} (in {})", v.opt("files"), v.req("app")?));
    }
    if v.has("menu") {
        return Ok(format!(
            "press {} in {where_} (a terminal app's menu command can type into its terminal)",
            v.opt("menu")
        ));
    }
    if v.has("secret") {
        return Ok(format!(
            "fill your secret {} (FERB never shows it) into {where_}",
            v.opt("secret")
        ));
    }
    if v.has("drag") {
        let text = v.req("text")?;
        let dragged = if text.is_empty() {
            "something FERB can't read"
        } else {
            text
        };
        return Ok(format!(
            "drag {dragged} onto {where_} (dropped text goes into the terminal)"
        ));
    }
    let said = if v.has("paste") {
        let what = if v.has("concealed") {
            "the clipboard's item, which an app marked secret (FERB doesn't show it)".to_string()
        } else {
            format!("the clipboard's text {}", v.req("text")?)
        };
        format!("paste {what} into {where_}")
    } else if v.has("text") {
        format!("type {} into {where_}", v.opt("text"))
    } else if matches!(v.get("keys"), Some("return") | Some("enter")) {
        return Ok(format!(
            "press Return in {where_} (it runs what is on its command line, which FERB didn't type there)"
        ));
    } else {
        return Ok(format!("press {} in {where_}", v.req("keys")?));
    };
    Ok(said
        + if v.has("enter") {
            ", then press Return there"
        } else {
            ""
        }
        + " (it runs as you, outside FERB's sandbox)")
}

/// `ai_amount` (GATE-578): the AI's stated amount when code read none; "" otherwise.
pub fn ai_amount(v: &Values) -> String {
    let read = v.opt("amount");
    if !v.has("declared") || (!read.is_empty() && !read.starts_with(UNREAD_AMOUNT)) {
        return String::new();
    }
    v.opt("declared").to_string()
}

/// `unseen_line` (GATE-578).
pub fn unseen_line(v: &Values) -> String {
    let amount = ai_amount(v);
    if !amount.is_empty() && v.has("unseen") {
        format!("FERB couldn't find {amount} on this page.")
    } else {
        String::new()
    }
}

fn pays(verb: &str, v: &Values) -> String {
    let to = if v.has("merchant") {
        v.opt("merchant")
    } else {
        v.opt("recipients")
    };
    let stated = ai_amount(v);
    let amount = if !stated.is_empty() {
        stated
    } else if v.has("amount") {
        v.opt("amount").to_string()
    } else {
        UNREAD_AMOUNT.to_string()
    };
    let mut text = format!("{verb} {amount}")
        + &(if to.is_empty() {
            String::new()
        } else {
            format!(" to {to}")
        })
        + &(if v.has("site") {
            format!(" on {}", v.opt("site"))
        } else {
            String::new()
        })
        + &(if v.has("account") {
            format!(" from {}", v.opt("account"))
        } else {
            String::new()
        });
    let unseen = unseen_line(v);
    if !unseen.is_empty() {
        text += &format!(" ({})", py::prefix(&unseen, py::char_len(&unseen) - 1));
    }
    if v.has("approved") {
        let bank = if v.has("bank") {
            format!(" on {}", v.opt("bank"))
        } else {
            String::new()
        };
        text += &format!(
            " (the bank page{bank} shows this, but you approved {})",
            v.opt("approved")
        );
    }
    text
}

fn tls_line(v: &Values) -> R<String> {
    let site = if v.has("hosts") {
        format!("the site is really {}", v.opt("hosts"))
    } else {
        "each site it reaches is really that site".to_string()
    };
    Ok(TLS_QUESTION
        .replace("{command}", v.req("command")?)
        .replace("{site}", &site))
}

fn clicking(v: &Values) -> String {
    if v.has("control") {
        format!(" by clicking {}", shown_quoted(v.opt("control")))
    } else {
        String::new()
    }
}

fn carried(v: &Values) -> String {
    let value = v.opt("value");
    let matched = if v.has("matched") {
        v.opt("matched")
    } else {
        value
    };
    let same = py::casefold(matched) == py::casefold(value);
    let inside = if same {
        String::new()
    } else {
        format!(" (inside {})", shown_quoted(value))
    };
    let where_ = if v.has("source") {
        v.opt("source")
    } else {
        "your own data"
    };
    format!(
        "{}{inside}, which this task read from {where_}",
        shown_quoted(if same { value } else { matched })
    )
}

fn part(n: &Need, alongside: &[&str]) -> R<String> {
    let v = &n.values;
    let rule = n.rule.as_str();
    Ok(match rule {
        TLS_OFF => tls_line(v)?,
        FOREGROUND if v.has("script") => format!(
            "this would run a script in {0}, your own browser, which can act in any of its windows and tabs; one Yes \
             covers {0} for the rest of this task, and a No keeps FERB to its own tabs",
            v.req("app")?
        ),
        FOREGROUND => format!(
            "this would bring {0} to the front and use your screen; one Yes covers {0} for the rest of this task, and a \
             No keeps FERB in the background",
            v.req("app")?
        ),
        BROWSER_PAGE => format!(
            "this would act on {}'s own page {} (its settings or extensions); FERB asks before every input there, and \
             this Yes covers this one input only",
            v.req("app")?,
            v.req("page")?
        ),
        PAIR => format!(
            "this pairs your {} profile “{}”{} with FERB, so FERB can open tabs and work in that profile's signed-in \
             accounts; its pairing page opens there and closes itself",
            v.req("browser")?,
            v.req("name")?,
            account(v)
        ),
        TERMINAL_INPUT if v.has("wide") => format!(
            "this would {}; FERB counts it a terminal because it is {}. FERB asks before it first types into a \
             terminal app in a task, and this Yes lets FERB type in {}'s terminal for the rest of this task",
            terminal(v)?,
            v.req("surface")?,
            v.req("app")?
        ),
        TERMINAL_INPUT => format!(
            "this would {}; FERB counts it a terminal because it is {}. FERB asks before every input like this, and \
             this Yes covers this one input only",
            terminal(v)?,
            v.req("surface")?
        ),
        "floor:pay" => pays("this pays", v),
        "floor:account_security" => {
            format!("this changes account security ({ACCOUNT_SECURITY})")
                + &(if !v.req("site")?.is_empty() { format!(" on {}", v.opt("site")) } else { String::new() })
                + &(if !v.req("account")?.is_empty() { format!(" for {}", v.opt("account")) } else { String::new() })
        }
        "floor:delete_permanent" if v.has("place") => format!(
            "this permanently deletes {} on {} (it can't be undone)",
            v.req("path")?,
            v.opt("place")
        ),
        "floor:delete_permanent" => format!("this permanently deletes {} (it skips the Trash)", v.req("path")?),
        "floor:new_recipient" if v.req("recipients")?.is_empty() => {
            let where_ = (if v.has("place") { format!(" in {}", v.opt("place")) } else { String::new() })
                + &(if v.has("account") { format!(" as {}", v.opt("account")) } else { String::new() });
            if v.req("verb")? == "calls" {
                format!("this starts a call{where_} with a contact it doesn't name")
            } else {
                format!("this sends{where_} to a contact it doesn't name")
            }
        }
        "floor:new_recipient" => format!(
            "this {} {}, whom you didn't name in this request",
            v.req("verb")?,
            v.req("recipients")?
        ),
        "sink:new_destination" if v.has("form") => {
            format!(
                "this sends the page's answers to {}{}, a place you haven't named: {}",
                v.req("destination")?,
                clicking(v),
                if v.has("sent") { v.opt("sent") } else { "your own data" }
            ) + &(if v.has("read") { format!("; this task read {}", v.opt("read")) } else { String::new() })
        }
        "sink:new_destination" => format!(
            "this sends to {}, a place you haven't named, after this task read your own data and outside content ({})",
            v.req("destination")?,
            v.req("read")?
        ),
        "sink:private_url" => {
            let read = if v.has("read") { format!(" ({})", v.opt("read")) } else { String::new() };
            let dest = v.req("destination")?;
            format!(
                "this would open {dest}, a place you haven't named, and send it {} in the address, after this task \
                 read outside content{read}; saying yes lets FERB reach {dest} from the browser, now and in later tasks",
                carried(v)
            )
        }
        "sink:outside_value" if v.req("what")? == "typed" => format!(
            "this types {}, which FERB found only in outside content ({}), not in your words, your answers or your own \
             files",
            v.req("value")?,
            v.req("origin")?
        ),
        "sink:outside_value" => format!(
            "FERB found the {} {} only in outside content ({}), not in your words, your answers or your own files",
            v.req("what")?,
            v.req("value")?,
            v.req("origin")?
        ),
        BULK if v.get("what") == Some("delete") => {
            let counted = if v.has("counted") { format!("that is {}; ", v.opt("counted")) } else { String::new() };
            format!(
                "this permanently deletes {} ({counted}it skips the Trash): {} in {}, {}",
                v.req("path")?,
                v.req("files")?,
                v.req("folders")?,
                v.req("bytes")?
            ) + &(if !v.req("sample")?.is_empty() { format!(", for example {}", v.opt("sample")) } else { String::new() })
        }
        BULK if alongside.contains(&"floor:new_recipient") => {
            if v.get("what") == Some("call") {
                format!("that is {} recipients at once", v.req("count")?)
            } else {
                format!("that is {} to {} recipients at once", v.req("bytes")?, v.req("count")?)
            }
        }
        BULK if v.get("what") == Some("call") => format!(
            "this calls {} recipients you didn't name in this request at once: {}",
            v.req("count")?,
            v.req("sample")?
        ),
        BULK if v.get("what") == Some("send") => format!(
            "this sends {} to {} recipients you didn't name in this request at once: {}",
            v.req("bytes")?,
            v.req("count")?,
            v.req("sample")?
        ),
        _ => v.get("text").unwrap_or(rule).to_string(),
    })
}

/// `shown_sent` (GATE-557): the values a form would send, each with where this task read it. Display only.
pub fn shown_sent(sent: &[(String, String)], total: usize) -> String {
    let parts: Vec<String> = sent
        .iter()
        .map(|(value, where_)| format!("{} (from {where_})", shown_quoted(value)))
        .collect();
    let more = total as i64 - sent.len() as i64;
    parts.join("; ")
        + &(if more > 0 {
            format!("; and {more} more")
        } else {
            String::new()
        })
}

fn lifted(lift: &str) -> Option<&'static str> {
    Some(match lift {
        "read" => "read {path} for this task (normally off-limits: a private store FERB stays out of)",
        "write" => "change {path} once (normally off-limits: FERB never changes it on its own)",
        "admin" => "run one command as administrator (sudo), with your Mac password (normally off-limits: FERB's shell \
                    never uses admin rights)",
        "service" => "let one command use macOS's certificate service, trustd (normally off: it reaches certificate \
                      addresses outside FERB's proxy)",
        _ => return None,
    })
}

fn short(n: &Need, alongside: &[&str]) -> R<Option<String>> {
    let v = &n.values;
    let rule = n.rule.as_str();
    Ok(Some(match rule {
        TLS_OFF => tls_line(v)?,
        FOREGROUND if v.has("script") => format!(
            "run scripts in {}, your own browser, for this task",
            v.req("app")?
        ),
        FOREGROUND => format!(
            "use your screen in {} (it comes to the front) for this task",
            v.req("app")?
        ),
        BROWSER_PAGE => format!(
            "change something on {}'s own page {} (its settings or extensions)",
            v.req("app")?,
            v.req("page")?
        ),
        PAIR => format!(
            "pair your {} profile “{}”{} with FERB, so it can work in that profile",
            v.req("browser")?,
            v.req("name")?,
            account(v)
        ),
        TERMINAL_INPUT => terminal(v)?,
        "floor:pay" => pays("pay", v),
        "floor:account_security" => {
            format!("change account security ({ACCOUNT_SECURITY})")
                + &(if !v.req("site")?.is_empty() {
                    format!(" on {}", v.opt("site"))
                } else {
                    String::new()
                })
                + &(if !v.req("account")?.is_empty() {
                    format!(" for {}", v.opt("account"))
                } else {
                    String::new()
                })
        }
        "floor:delete_permanent" if v.has("place") => {
            format!(
                "permanently delete {} on {}",
                v.req("path")?,
                v.opt("place")
            )
        }
        "floor:delete_permanent" => {
            format!("permanently delete {}, skipping the Trash", v.req("path")?)
        }
        "floor:new_recipient" if v.req("recipients")?.is_empty() => {
            let where_ = (if v.has("place") {
                format!(" in {}", v.opt("place"))
            } else {
                String::new()
            }) + &(if v.has("account") {
                format!(" as {}", v.opt("account"))
            } else {
                String::new()
            });
            if v.req("verb")? == "calls" {
                format!("start a call{where_}, with someone the step doesn't name")
            } else {
                format!("send something{where_}, to someone the step doesn't name")
            }
        }
        "floor:new_recipient" => format!(
            "{} {}, whom you didn't name",
            if v.req("verb")? == "calls" {
                "call"
            } else {
                "send to"
            },
            v.req("recipients")?
        ),
        "sink:new_destination" if v.has("form") => format!(
            "send {} to {}{}",
            if v.has("sent") {
                v.opt("sent")
            } else {
                "your own data"
            },
            v.req("destination")?,
            clicking(v)
        ),
        "sink:new_destination" => {
            let read = v.req("read")?;
            format!(
                "send to {}, a place you haven't named, after reading {}",
                v.req("destination")?,
                if read.is_empty() {
                    "outside content"
                } else {
                    read
                }
            )
        }
        "sink:private_url" => format!("open {}, sending it {}", v.req("destination")?, carried(v)),
        "sink:outside_value" if v.req("what")? == "typed" => format!(
            "type {}, which FERB found only in {}",
            v.req("value")?,
            v.req("origin")?
        ),
        "sink:outside_value" => format!(
            "use the {} {}, which FERB found only in {}",
            v.req("what")?,
            v.req("value")?,
            v.req("origin")?
        ),
        BULK if v.get("what") == Some("delete") => format!(
            "permanently delete {}: {} in {}, {}",
            v.req("path")?,
            v.req("files")?,
            v.req("folders")?,
            v.req("bytes")?
        ),
        BULK if alongside.contains(&"floor:new_recipient") => return Ok(None),
        BULK => format!(
            "{} {} recipients at once: {}",
            if v.get("what") == Some("call") {
                "call"
            } else {
                "send to"
            },
            v.req("count")?,
            v.req("sample")?
        ),
        r if r.starts_with("escape:") && v.get("lift").and_then(lifted).is_some() => {
            lifted(v.opt("lift"))
                .unwrap_or_default()
                .replace("{path}", v.get("path").unwrap_or(""))
        }
        _ => v.get("text").unwrap_or(rule).to_string(),
    }))
}

fn rules_of(needs: &[Need]) -> Vec<&str> {
    let mut r: Vec<&str> = needs.iter().map(|n| n.rule.as_str()).collect();
    r.sort_unstable();
    r.dedup();
    r
}

fn covered(needs: &[Need]) -> Vec<String> {
    needs
        .iter()
        .filter(|n| n.rule == BULK)
        .filter_map(|n| n.values.get("covers").map(str::to_string))
        .collect()
}

/// `summary`: FERB's question, the one line its card shows (P27, GATE-225). Display only.
pub fn summary(needs: &[Need], typed: &str) -> R<String> {
    let said = covered(needs);
    let rules = rules_of(needs);
    if rules == [TLS_OFF] {
        let parts: Vec<String> = needs
            .iter()
            .map(|n| tls_line(&n.values))
            .collect::<R<_>>()?;
        return Ok(parts.join("; ") + ".");
    }
    let mut parts = Vec::new();
    for n in needs {
        if said.contains(&n.rule) {
            continue;
        }
        if let Some(p) = short(n, &rules)? {
            if !p.is_empty() {
                parts.push(p);
            }
        }
    }
    Ok(format!("FERB wants to {}.", parts.join("; "))
        + &(if typed.is_empty() {
            String::new()
        } else {
            format!(" Text last typed there: \"{typed}\".")
        }))
}

/// confirm.py:1494 `Card`: FERB's question as its card shows it (GATE-513, GATE-566). Display only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Card {
    pub question: String,
    pub detail: String,
    pub exact: String,
    pub yes: String,
    pub no: String,
    pub aside: String,
    pub clause: String,
    pub tail: String,
}

fn c(question: String, detail: String, exact: String, yes: &str, no: &str) -> Card {
    Card {
        question,
        detail,
        exact,
        yes: yes.to_string(),
        no: no.to_string(),
        ..Card::default()
    }
}

fn bare(text: &str) -> String {
    let n = py::char_len(text);
    if n >= 2 && text.starts_with('"') && text.ends_with('"') {
        let inner = &text[1..text.len() - 1];
        if !inner.is_empty() && inner == py::strip(inner) {
            return inner.to_string();
        }
    }
    text.to_string()
}

fn unquoted(text: &str) -> String {
    if !text.starts_with('"') {
        return text.to_string();
    }
    match ferb_py::json::loads(text) {
        Ok(serde_json::Value::String(s)) => s,
        _ => text.to_string(),
    }
}

fn word(text: &str) -> String {
    let one = py::squeeze(&unquoted(text));
    let clipped = if py::char_len(&one) <= QUOTED_CHARS {
        one
    } else {
        format!("{}…", py::prefix(&one, QUOTED_CHARS - 1))
    };
    format!("“{clipped}”")
}

fn in_terminal(v: &Values) -> String {
    format!(
        "{}'s terminal",
        if v.has("app") {
            v.opt("app")
        } else {
            "the app"
        }
    )
}

fn which(v: &Values) -> String {
    let parts: Vec<String> = [
        if v.has("label") {
            format!("terminal {}", v.opt("label"))
        } else {
            String::new()
        },
        if v.has("window") {
            format!("window {}", v.opt("window"))
        } else {
            String::new()
        },
    ]
    .into_iter()
    .filter(|p| !p.is_empty())
    .collect();
    if parts.is_empty() {
        return String::new();
    }
    format!(
        " It's {}'s {}.",
        if v.has("app") {
            v.opt("app")
        } else {
            "the app"
        },
        parts.join(", ")
    )
}

fn covers(v: &Values, only: &str) -> String {
    if v.has("wide") {
        format!(
            "Yes lets FERB type in {} for the rest of this task.",
            in_terminal(v)
        )
    } else {
        only.to_string()
    }
}

const ONLY: &str = "Yes covers only this.";

fn terminal_card(v: &Values) -> Option<Card> {
    let app = if v.has("app") {
        v.opt("app")
    } else {
        "the app"
    };
    let (where_, which_) = (in_terminal(v), which(v));
    if v.has("runs") {
        let mut pressed = if v.has("menu") {
            v.opt("menu").to_string()
        } else if v.has("control") {
            v.opt("control").to_string()
        } else {
            "this".to_string()
        };
        if v.has("keys") {
            pressed = format!("{} ({pressed})", v.opt("keys"));
        }
        let (what, unread) = if v.has("selection") {
            ("the selected text".to_string(), "")
        } else {
            (
                if v.has("file") {
                    format!("the selection or current line in {}", v.opt("file"))
                } else {
                    "the selection or current line".to_string()
                },
                "; FERB can't read what it runs",
            )
        };
        return Some(c(
            format!("Press {pressed} in {app}? It runs code"),
            format!(
                "It runs {what} in {} with your full access{unread}. {}",
                if v.has("runs_where") {
                    v.opt("runs_where")
                } else {
                    app
                },
                covers(v, ONLY)
            ),
            unquoted(v.opt("selection")),
            "Do it",
            "Don't",
        ));
    }
    if v.has("script") {
        let named = v.opt("names");
        let files = if !named.is_empty() {
            format!(" Files it names: {named}")
                + if v.has("opens") {
                    "; FERB can't see others it may open"
                } else {
                    ""
                }
                + "."
        } else if v.has("opens") {
            " FERB can't see which file this opens, if it opens one.".to_string()
        } else {
            String::new()
        };
        return Some(c(
            format!("Let {app} run this script?"),
            format!(
                "{app} acts with its own access, outside FERB's safety limits.{files} {}",
                covers(v, "Yes covers only this script.")
            ),
            unquoted(v.opt("script")),
            "Run script",
            "Don't run",
        ));
    }
    if v.has("files") {
        let path = unquoted(v.opt("open_path"));
        let path = if path.is_empty() {
            "this file".to_string()
        } else {
            path
        };
        let trimmed = path.trim_end_matches('/');
        let last = trimmed.rsplit_once('/').map_or(trimmed, |(_, l)| l);
        let name = if last.is_empty() { path.as_str() } else { last };
        let exact: Vec<String> = [path.clone(), unquoted(v.opt("open_text"))]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect();
        return Some(c(
            format!("Open {name}? It runs when opened"),
            format!(
                "Opening it in {app} runs what's inside with your access. Its contents are below. Yes covers these exact \
                 contents."
            ),
            exact.join("\n"),
            "Open it",
            "Don't open",
        ));
    }
    if v.has("menu") {
        return Some(Card {
            question: format!("Press {} in {app}? It can type into its terminal", unquoted(v.opt("menu"))),
            detail: "A terminal app's menu command can type into its terminal, which runs with your full access. ".to_string()
                + &covers(v, ONLY),
            yes: "Do it".into(),
            no: "Don't".into(),
            ..Card::default()
        });
    }
    if v.has("secret") {
        return Some(Card {
            question: format!("Fill your secret {} into {where_}?", word(v.opt("secret"))),
            detail: "FERB never shows it. Text in a terminal can run with your full access. Yes covers only this.".to_string()
                + &which_,
            yes: "Fill it in".into(),
            no: "Don't".into(),
            ..Card::default()
        });
    }
    if v.has("drag") {
        return Some(c(
            format!("Drag this onto {where_}?"),
            "Dropped text goes into the terminal, where it can run with your full access. "
                .to_string()
                + &covers(v, ONLY)
                + &which_,
            unquoted(v.opt("text")),
            "Drag it",
            "Don't",
        ));
    }
    if v.has("paste") {
        let then = if v.has("enter") { " and run it" } else { "" };
        if v.has("concealed") {
            return Some(Card {
                question: format!("Paste a secret item into {where_}{then}?"),
                detail: "An app marked it secret, so FERB doesn't show it here. Pasted text can run with your full \
                         access. Yes covers only this paste."
                    .to_string()
                    + &which_,
                yes: "Paste it".into(),
                no: "Don't paste".into(),
                ..Card::default()
            });
        }
        return Some(c(
            format!("Paste this into {where_}{then}?"),
            "Pasted text can run with your full access. ".to_string()
                + &covers(v, "Yes covers only this paste.")
                + &which_,
            bare(v.opt("text")),
            "Paste it",
            "Don't paste",
        ));
    }
    if v.has("text") {
        if v.has("enter") {
            return Some(Card {
                tail: ". ".to_string() + &covers(v, "Yes covers only this command.") + &which_,
                ..c(
                    format!("Run this command in {app}?"),
                    "It runs with full access to your Mac and files".to_string(),
                    bare(v.opt("text")),
                    "Run it",
                    "Don't run",
                )
            });
        }
        return Some(c(
            format!("Type this into {where_}?"),
            "FERB won't press Return. If Return gets pressed, it runs with your full access. "
                .to_string()
                + &covers(v, "Yes covers only this typing.")
                + &which_,
            bare(v.opt("text")),
            "Type it",
            "Don't type",
        ));
    }
    if matches!(v.get("keys"), Some("return") | Some("enter")) {
        return Some(Card {
            question: format!("Press Return in {where_}?"),
            detail: "That runs whatever is on its command line now. FERB didn't type it and can't show it to you."
                .to_string()
                + &(if v.has("wide") { format!(" {}", covers(v, ONLY)) } else { String::new() })
                + &which_,
            yes: "Press Return".into(),
            no: "Don't".into(),
            ..Card::default()
        });
    }
    if v.has("keys") {
        return Some(Card {
            question: format!("Press {} in {where_}?", v.opt("keys")),
            detail: covers(v, "Yes covers only this key press.") + &which_,
            yes: format!("Press {}", v.opt("keys")),
            no: "Don't".into(),
            ..Card::default()
        });
    }
    None
}

fn page_name(page: &str) -> String {
    let (scheme, _, rest) = py::partition(py::strip(page), ":");
    if py::casefold(scheme).ends_with("extension") {
        return "extension".into();
    }
    let rest = rest.trim_start_matches('/');
    let first = rest.split('/').next().unwrap_or("");
    let first = first.split('?').next().unwrap_or("");
    let first = first.split('#').next().unwrap_or("");
    if first.is_empty() {
        return "settings".into();
    }
    let mut chars = first.chars();
    let head = chars
        .next()
        .map(|c| py::upper(&c.to_string()))
        .unwrap_or_default();
    head + chars.as_str()
}

fn card_of(n: &Need, alongside: &[&str]) -> Option<Card> {
    let v = &n.values;
    let rule = n.rule.as_str();
    if rule == BULK && alongside.contains(&"floor:new_recipient") {
        return None;
    }
    match rule {
        "sink:private_url" => {
            let dest = v.opt("destination");
            let value = if v.has("matched") { v.opt("matched") } else { v.opt("value") };
            let piece = v.opt("value");
            let inside = if !piece.is_empty() && py::casefold(piece) != py::casefold(value) {
                format!(" (inside {})", word(piece))
            } else {
                String::new()
            };
            let source = if v.has("source_plain") { v.opt("source_plain") } else { "your own data" };
            Some(c(
                format!("Open {dest}? The link includes {}", word(value)),
                format!(
                    "FERB treats {}{inside} as yours because it read it in {source}. Opening this link shows it to \
                     {dest}. Yes also lets FERB open {dest} in later tasks.",
                    word(value)
                ),
                if v.has("url") { v.opt("url").to_string() } else { dest.to_string() },
                "Open it",
                "Don't open",
            ))
        }
        "sink:new_destination" if v.has("form") => {
            let dest = v.opt("destination");
            let mut values: Vec<String> = v.opt("values").split('\n').filter(|x| !x.is_empty()).map(str::to_string).collect();
            if values.is_empty() {
                values = vec![unquoted(v.opt("value"))];
            }
            let sources = if v.has("sources_plain") { v.opt("sources_plain") } else { "your own data" };
            let which = if values.len() > 1 { "the values below" } else { "the value below" };
            let click = if v.has("control") { format!("Clicking {}", word(v.opt("control"))) } else { "This click".into() };
            let found = if v.has("found_via") { format!("; it found it {}", v.opt("found_via")) } else { String::new() };
            let more = if v.has("more") { format!(" (and {} more)", v.opt("more")) } else { String::new() };
            Some(c(
                format!("Send your details to {dest}?"),
                format!(
                    "{click} submits the form to {dest}, including {which}{more} from {sources}. You haven't named this \
                     site to FERB{found}. Yes covers {dest} for this task."
                ),
                values.join("\n"),
                "Send",
                "Don't send",
            ))
        }
        "sink:new_destination" => {
            let dest = v.opt("destination");
            Some(c(
                format!("Send your details to {dest}?"),
                format!(
                    "This sends to {dest}, and this task has read your own data and outside content. You haven't named \
                     this site to FERB. Yes also lets FERB send to {dest} in later tasks."
                ),
                dest.to_string(),
                "Send",
                "Don't send",
            ))
        }
        "sink:outside_value" => {
            let what = v.get("what").unwrap_or("value");
            let value = v.opt("value");
            let places = if v.has("places") {
                v.opt("places")
            } else if v.has("origin") {
                v.opt("origin")
            } else {
                "something FERB read"
            };
            let later = !n.destinations.is_empty();
            if what == "typed" {
                let kind = crate::confirm::value_kind(value);
                let on = if v.has("site") { format!(" on {}", v.opt("site")) } else { String::new() };
                return Some(c(
                    format!("Type this {kind}{on}?"),
                    format!(
                        "You didn't give FERB this {kind}; it only saw it on {places}. A page can plant an address to \
                         get your data, so check it's right."
                    ) + if later { " Yes also lets FERB use it again later." } else { "" },
                    value.to_string(),
                    "Type it",
                    "Don't type it",
                ));
            }
            if what == "recipient" {
                let said = format!(
                    "You never gave FERB this address; it came from {places}. A page can plant an address to get your \
                     data."
                );
                let title = if py::char_len(value) <= 40 { format!("Send to {value}?") } else { "Send to this address?".into() };
                return Some(Card {
                    aside: said.clone(),
                    ..c(
                        title,
                        said + if later { " Yes also lets FERB send to it later." } else { "" },
                        value.to_string(),
                        "Send",
                        "Don't send",
                    )
                });
            }
            Some(c(
                format!("Use {value}, from {places}?"),
                format!("You didn't give FERB this amount; it was on {places}. Check it's the amount you want."),
                value.to_string(),
                "Use it",
                "Don't",
            ))
        }
        "floor:new_recipient" => {
            let calls = v.get("verb") == Some("calls");
            let (verb, act) = if calls { ("Start a call", "starts a call") } else { ("Send", "sends") };
            if !v.has("recipients") {
                let place = v.opt("place");
                let account = if v.has("account") { format!(" as {}", v.opt("account")) } else { String::new() };
                return Some(Card {
                    question: format!(
                        "{verb}{} without seeing who gets it?",
                        if place.is_empty() { String::new() } else { format!(" in {place}") }
                    ),
                    detail: format!(
                        "This button {act}{account}, but FERB can't tell to whom. Say {} only if that's what you asked \
                         for. Yes covers {} for this task.",
                        if calls { "Call" } else { "Send" },
                        if place.is_empty() { "it" } else { place }
                    ),
                    yes: (if calls { "Call" } else { "Send" }).into(),
                    no: (if calls { "Don't call" } else { "Don't send" }).into(),
                    ..Card::default()
                });
            }
            let people = v.opt("recipients");
            let them = if people.contains(',') { "them" } else { "this person" };
            Some(c(
                format!("{} {people} for the first time?", if calls { "Call" } else { "Send to" }),
                format!(
                    "You haven't asked FERB to contact {them} before; check it's who you meant. Yes also lets FERB {} \
                     {them} in later tasks.",
                    if calls { "call" } else { "send to" }
                ),
                format!("{}: {people}", if calls { "Call" } else { "To" })
                    + &(if v.has("place") { format!(" · in {}", v.opt("place")) } else { String::new() })
                    + &(if v.has("account") { format!(" · from {}", v.opt("account")) } else { String::new() }),
                if calls { "Call" } else { "Send" },
                if calls { "Don't call" } else { "Don't send" },
            ))
        }
        "floor:pay" => {
            let amount = if v.has("amount") { v.opt("amount") } else { UNREAD_AMOUNT };
            let to = if v.has("merchant") { v.opt("merchant") } else { v.opt("recipients") };
            let mut detail = "Money leaves your account. Yes covers only this payment.".to_string();
            if v.has("approved") {
                detail = format!(
                    "This is different from what you approved ({}): the bank page{} shows {amount}. ",
                    v.opt("approved"),
                    if v.has("bank") { format!(" on {}", v.opt("bank")) } else { String::new() }
                ) + &detail;
            }
            let stated = ai_amount(v);
            let shown = if stated.is_empty() { amount.to_string() } else { stated.clone() };
            let to_part = if to.is_empty() { String::new() } else { format!(" to {to}") };
            let exact: Vec<String> = [
                if to.is_empty() { shown.clone() } else { format!("{shown} → {to}") },
                v.opt("site").to_string(),
                if v.has("account") { format!("from {}", v.opt("account")) } else { String::new() },
            ]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect();
            let exact = exact.join(" · ");
            if !stated.is_empty() {
                let unseen = unseen_line(v);
                return Some(c(
                    format!("Pay {stated}{to_part}?"),
                    if unseen.is_empty() { detail } else { format!("{unseen} {detail}") },
                    exact,
                    if unseen.is_empty() { "" } else { "Pay" },
                    "Don't pay",
                ))
                .map(|mut card| {
                    if card.yes.is_empty() {
                        card.yes = format!("Pay {stated}");
                    }
                    card
                });
            }
            if !v.has("amount") || amount.starts_with(UNREAD_AMOUNT) {
                return Some(c(
                    format!("Pay{}? FERB couldn't read the amount", if to.is_empty() { String::new() } else { format!(" {to}") }),
                    detail,
                    exact,
                    "Pay",
                    "Don't pay",
                ));
            }
            Some(Card {
                yes: format!("Pay {amount}"),
                ..c(format!("Pay {amount}{to_part}?"), detail, exact, "", "Don't pay")
            })
        }
        "floor:account_security" => {
            let account = if v.has("account") { v.opt("account") } else { "your account" };
            let exact: Vec<&str> = [v.opt("account"), v.opt("site")].into_iter().filter(|p| !p.is_empty()).collect();
            Some(c(
                "Change sign-in security".to_string()
                    + &(if v.has("site") { format!(" on {}", v.opt("site")) } else { String::new() })
                    + "?",
                format!(
                    "This changes how you sign in to {account} (password, two-factor or recovery) or deletes the \
                     account; a mistake can lock you out. Yes covers this one change."
                ),
                exact.join(" · "),
                "Change it",
                "Don't change",
            ))
        }
        "floor:delete_permanent" => {
            let detail = if v.has("place") {
                format!("It's on {}, and it can't be undone.", v.opt("place"))
            } else {
                "It skips the Trash and can't be undone.".to_string()
            };
            Some(Card {
                question: "Delete this for good?".into(),
                detail,
                exact: v.opt("path").to_string(),
                clause: format!("it permanently deletes {}", if v.has("path") { v.opt("path") } else { "something" }),
                ..Card::default()
            })
        }
        BULK if v.get("what") == Some("delete") => {
            let path = v.opt("path");
            let trimmed = path.trim_end_matches('/');
            let last = trimmed.rsplit_once('/').map_or(trimmed, |(_, l)| l);
            let name = if !last.is_empty() {
                last
            } else if !path.is_empty() {
                path
            } else {
                "this folder"
            };
            Some(c(
                format!("Delete {name} and all {} in it?", v.get("files").unwrap_or("the files")),
                format!("{} in {}, {}", v.opt("files"), v.opt("folders"), v.opt("bytes"))
                    + &(if v.has("sample") { format!(", e.g. {}", v.opt("sample")) } else { String::new() })
                    + ". They skip the Trash, so you can't get them back.",
                path.to_string(),
                "Delete all",
                "Keep them",
            ))
        }
        BULK => {
            let calls = v.get("what") == Some("call");
            let count = v.get("count").unwrap_or("several");
            Some(Card {
                yes: format!("{} all {count}", if calls { "Call" } else { "Send to" }),
                ..c(
                    format!("{} {count} people at once?", if calls { "Call" } else { "Send to" }),
                    "You didn't name them".to_string()
                        + &(if calls {
                            ".".to_string()
                        } else {
                            format!(", and once {count} people have it you can't take it back.")
                        }),
                    v.opt("sample").to_string(),
                    "",
                    if calls { "Don't call" } else { "Don't send" },
                )
            })
        }
        TERMINAL_INPUT => terminal_card(v),
        r if r.starts_with("escape:") && v.get("lift") == Some("read") => {
            let path = v.get("path").unwrap_or("this folder");
            Some(c(
                format!("Let FERB read {path}?"),
                "It's on your private list, so FERB normally stays out. Yes lets it read there (not change anything) \
                 until this task ends."
                    .into(),
                v.opt("path").to_string(),
                "Allow reading",
                "Keep out",
            ))
        }
        r if r.starts_with("escape:") && v.get("lift") == Some("write") => {
            let exact: Vec<&str> = [v.opt("path"), v.opt("command")].into_iter().filter(|p| !p.is_empty()).collect();
            Some(c(
                format!("Let FERB change {} once?", v.get("path").unwrap_or("this")),
                "It's on your do-not-change list. Yes covers only this one change.".into(),
                exact.join("\n"),
                "Change it once",
                "Don't",
            ))
        }
        r if r.starts_with("escape:") && v.get("lift") == Some("admin") => Some(c(
            "Run this command as administrator?".into(),
            "Admin commands can change anything on your Mac. Next, macOS needs your Mac password. Yes covers only this \
             command."
                .into(),
            v.opt("command").to_string(),
            "Run as admin",
            "Don't",
        )),
        r if r.starts_with("escape:") && v.get("lift") == Some("service") => Some(c(
            "Let this command check certificates directly?".into(),
            "macOS will contact certificate servers outside FERB's network filter for it. Yes covers only this command."
                .into(),
            v.opt("command").to_string(),
            "Allow once",
            "Don't",
        )),
        FOREGROUND if v.has("script") => {
            let app = v.get("app").unwrap_or("your browser");
            Some(c(
                format!("Let FERB run a script in {app}?"),
                format!(
                    "A script can read and act in every {app} window and tab, including sites you're signed in to. Yes \
                     covers scripts in {app} until this task ends."
                ),
                v.opt("script_text").to_string(),
                "Allow scripts",
                "Don't",
            ))
        }
        FOREGROUND => {
            let app = v.get("app").unwrap_or("the app");
            Some(Card {
                question: format!("Let FERB use your screen in {app} for this task?"),
                detail: format!("{app} will come to the front while FERB works in it. One Yes covers {app} until this task ends."),
                clause: format!("{app} comes to the front while FERB types"),
                ..Card::default()
            })
        }
        BROWSER_PAGE => {
            let (app, page) = (v.get("app").unwrap_or("the browser"), v.opt("page"));
            Some(c(
                format!("{} on {app}'s {} page?", if v.has("verb") { v.opt("verb") } else { "Click" }, page_name(page)),
                format!(
                    "It's {app}'s own settings area, where one change affects all your browsing (extensions, \
                     passwords, privacy). Yes covers this one action."
                ),
                page.to_string(),
                "Allow",
                "Don't",
            ))
        }
        PAIR => {
            let (browser, name, account) = (v.opt("browser"), v.opt("name"), v.opt("email"));
            let exact: Vec<&str> = [browser, name, account].into_iter().filter(|p| !p.is_empty()).collect();
            Some(c(
                format!("Let FERB work in your {browser} “{name}” profile?"),
                "FERB could then open tabs there and act as you on sites you're signed in to".to_string()
                    + &(if account.is_empty() { String::new() } else { format!(" ({account})") })
                    + ". A pairing page opens and closes itself.",
                exact.join(" · "),
                "Pair it",
                "Don't pair",
            ))
        }
        TLS_OFF => {
            let whom = if v.has("hosts") { v.opt("hosts") } else { "each site it reaches" };
            Some(c(
                "Run this without checking the site is real?".into(),
                format!(
                    "It skips the check that it's really talking to {whom}, so someone on your network could swap what \
                     it downloads. Yes covers only this command."
                ),
                v.opt("command").to_string(),
                "Run it",
                "Don't run",
            ))
        }
        _ => None,
    }
}

/// `card`: FERB's question card (GATE-513), built from the needs' facts.
pub fn card(needs: &[Need], typed: &str, apps: &dyn Apps) -> R<Card> {
    let rules = rules_of(needs);
    let said = covered(needs);
    let mut ordered: Vec<&Need> = needs.iter().collect();
    ordered.sort_by_key(|n| n.rule != TERMINAL_INPUT);
    let mut cards: Vec<Card> = Vec::new();
    for n in ordered {
        if said.contains(&n.rule) {
            continue;
        }
        let mut n = n.clone();
        let mut changed = false;
        let mut values = n.values.clone();
        for k in ["app", "place"] {
            if let Some(v) = n.values.get(k) {
                let label = app_label(v, apps);
                if label != v {
                    values.set(k, &label);
                    changed = true;
                }
            }
        }
        if changed {
            n.values = values;
        }
        let made = match card_of(&n, &rules) {
            Some(made) => made,
            None => {
                let short = match short(&n, &rules) {
                    Ok(s) => s,
                    Err(_) => {
                        if n.rule == BULK && rules.contains(&"floor:new_recipient") {
                            None
                        } else {
                            Some("go ahead with this step".to_string())
                        }
                    }
                };
                match short {
                    None => continue,
                    Some(s) => Card {
                        question: format!("FERB wants to {s}."),
                        ..Card::default()
                    },
                }
            }
        };
        cards.push(made);
    }
    if cards.is_empty() {
        return Ok(Card {
            question: summary(needs, typed)?,
            ..Card::default()
        });
    }
    let first = cards[0].clone();
    let mut rest: Vec<Card> = cards[1..].to_vec();
    let mut lead = first.detail.clone();
    if !first.tail.is_empty() {
        for x in &rest {
            if !x.clause.is_empty() {
                lead += &format!("; {}", x.clause);
            }
        }
        lead += &first.tail;
        rest.retain(|x| x.clause.is_empty());
    }
    let mut parts: Vec<String> = vec![lead];
    for x in &rest {
        parts.push(if x.aside.is_empty() {
            x.question.clone()
                + &(if x.detail.is_empty() {
                    String::new()
                } else {
                    format!(" {}", x.detail)
                })
        } else {
            x.aside.clone()
        });
    }
    if !typed.is_empty() {
        parts.push(format!("Text last typed there: \"{typed}\"."));
    }
    let detail = parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let exact = cards
        .iter()
        .map(|x| x.exact.as_str())
        .filter(|e| !e.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Card {
        question: first.question,
        detail,
        exact,
        yes: first.yes,
        no: first.no,
        ..Card::default()
    })
}

/// `reason_line`: the task's reason as the small line under a question.
pub fn reason_line(why: &str) -> String {
    if py::strip(why).is_empty() {
        String::new()
    } else {
        crate::confirm::from_task_lines(&py::squeeze(why))
    }
}

/// `line`: FERB's own words, first and labelled (P27), then the call, the text last typed, the reason and the
/// question.
pub fn line(needs: &[Need], call: &str, why: &str, typed: &str) -> R<String> {
    let said = covered(needs);
    let rules = rules_of(needs);
    if rules == [TLS_OFF] {
        let parts: Vec<String> = needs
            .iter()
            .map(|n| tls_line(&n.values))
            .collect::<R<_>>()?;
        return Ok(format!("FERB: {}.", parts.join("; "))
            + &(if why.is_empty() {
                String::new()
            } else {
                format!(" Reason given: \"{why}\".")
            }));
    }
    let mut parts = Vec::new();
    for n in needs {
        if !said.contains(&n.rule) {
            parts.push(part(n, &rules)?);
        }
    }
    let mut text = format!("FERB: {}", parts.join("; "))
        + &(if call.is_empty() {
            ".".to_string()
        } else {
            format!(". The step: {call}.")
        });
    if !typed.is_empty() {
        text += &format!(" Text last typed on this page: \"{typed}\".");
    }
    if !why.is_empty() {
        text += &format!(" Reason given: \"{why}\".");
    }
    Ok(text + " Go ahead?")
}
