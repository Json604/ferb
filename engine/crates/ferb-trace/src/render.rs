//! The text of `ferb trace`'s story (`ferb/trace/render.py`; P26). It adds nothing the records don't say. Every
//! number is exact except the "≈ $ API-equivalent" estimate, which says it is one (RQ4).

use serde_json::Value;

use crate::lines::{clip_value, duration_value, plural, point_checks};
use crate::py::{
    day_month_time, g, gd, hms, int_of, items, num, obj, round_int, squeeze, str_of, string, truthy,
};

/// V4/V5: how a task's status reads.
fn status_label(status: &str) -> Option<&'static str> {
    Some(match status {
        "done" => "done",
        "done_unverified" => "done (not confirmed)",
        "failed" => "failed ✗",
        "needs_user" => "waiting for you",
        "cancelled" => "stopped",
        "interrupted" => "interrupted by a restart",
        "running" => "running",
        "paused" => "paused, waiting for you",
        "unknown" => "outcome unknown",
        _ => return None,
    })
}

fn at_text(at: &Value) -> String {
    if truthy(at) {
        hms(&str_of(at))
    } else {
        "--:--:--".to_owned()
    }
}

fn kilo(n: i64) -> String {
    let x = n as f64 / 1000.0;
    if n >= 10000 {
        format!("{x:.0}k")
    } else if n >= 1000 {
        format!("{x:.1}k")
    } else {
        n.to_string()
    }
}

fn status(ep: &Value) -> String {
    let s = g(ep, "status");
    if s.as_str() == Some("done") && truthy(g(ep, "verified")) {
        return "done ✓ verified".to_owned();
    }
    let text = str_of(s);
    status_label(&text).map_or(text, str::to_owned)
}

fn cost(u: &Value) -> String {
    let records = int_of(g(u, "usage_records"));
    let calls = int_of(g(u, "model_calls"));
    if !truthy(g(u, "usage_records")) {
        let mut s = "Tokens: no usage recorded".to_owned();
        if truthy(g(u, "model_calls")) {
            s.push_str(&format!(" for {} model calls", str_of(g(u, "model_calls"))));
        }
        return s;
    }
    let (inp, cached) = (
        int_of(g(u, "input_tokens")),
        int_of(g(u, "cached_input_tokens")),
    );
    let percent = if inp != 0 {
        round_int(100.0 * cached as f64 / inp as f64)
    } else {
        0
    };
    let mut line = format!(
        "Tokens: {} in ({percent}% cached) · {} out",
        kilo(inp),
        kilo(int_of(g(u, "output_tokens")))
    );
    if !g(u, "cost_usd").is_null() {
        let subscription = g(u, "billing")
            .as_array()
            .is_some_and(|b| b.len() == 1 && b[0].as_str() == Some("subscription"));
        let note = if subscription {
            "estimate; your subscription covers it"
        } else {
            "estimate"
        };
        let part = if truthy(g(u, "unpriced_models")) {
            " for the priced calls"
        } else {
            ""
        };
        line.push_str(&format!(
            " · ≈ ${:.2} API-equivalent{part} ({note})",
            num(g(u, "cost_usd"))
        ));
    }
    if truthy(g(u, "unpriced_models")) {
        let names: Vec<String> = items(u, "unpriced_models").iter().map(str_of).collect();
        line.push_str(&format!(" · cost unknown for {}", names.join(", ")));
    }
    if records < int_of(gd(u, "model_calls", &Value::from(0))) {
        line.push_str(&format!(
            " · usage recorded for {records} of {calls} model calls"
        ));
    }
    line
}

/// `story(ep, verdict)`: the lines of the default `ferb trace`.
pub fn story(ep: &Value, verdict: &Value) -> Vec<String> {
    let u = obj(ep, "usage");
    let task_id = str_of(g(ep, "task_id"));
    let mut out = vec![format!(
        "#{} · \"{}\" · {} · {} · {}",
        crate::py::prefix(&task_id, 8),
        clip_value(gd(ep, "request_text", &string("")), 80),
        status(ep),
        duration_value(g(ep, "seconds")),
        plural(
            int_of(gd(u, "model_calls", &Value::from(0))),
            "model call",
            "model calls"
        )
    )];
    for (i, s) in items(ep, "steps").iter().enumerate() {
        let mark = if truthy(g(s, "mark")) {
            format!("{} ", str_of(g(s, "mark")))
        } else {
            String::new()
        };
        out.push(format!(
            "{}. {} {} ({}): {mark}{}",
            i + 1,
            at_text(g(s, "at")),
            str_of(g(s, "role")),
            duration_value(g(s, "seconds")),
            str_of(g(s, "text"))
        ));
    }
    if truthy(g(ep, "answer")) {
        out.push(format!("Answer: {}", squeeze(&str_of(g(ep, "answer")))));
    }
    if truthy(g(ep, "problems")) {
        let problems: Vec<String> = items(ep, "problems").iter().map(str_of).collect();
        out.push(problems.join(" · "));
    }
    if truthy(g(ep, "point_checks")) {
        out.push(point_checks(g(ep, "point_checks")));
    }
    out.push(cost(u));
    let d = obj(ep, "desktop");
    let near = items(d, "near_ferb");
    let others = items(d, "no_on_screen_action");
    let mut line = format!(
        "Desktop: {} changes during the task, ",
        str_of(gd(d, "changes", &Value::from(0)))
    );
    line.push_str(&if near.is_empty() {
        "none near a FERB on-screen action".to_owned()
    } else {
        format!("{} near a FERB on-screen action", near.len())
    });
    if !others.is_empty() {
        line.push_str(&format!(
            "; {} with no FERB on-screen action in progress",
            plural(
                others.len() as i64,
                "front-app or Space change",
                "front-app or Space changes"
            )
        ));
    }
    out.push(line);
    let mut shown: Vec<(String, String)> = Vec::new();
    for n in near {
        let mut tools: Vec<String> = items(n, "tools").iter().map(str_of).collect();
        tools.extend(
            items(n, "other_task_tools")
                .iter()
                .map(|t| format!("{} (another FERB task's call)", str_of(t))),
        );
        let changed: Vec<String> = items(n, "changed").iter().map(str_of).collect();
        shown.push((
            str_of(g(n, "at")),
            format!(
                "  · {} {} near {}",
                at_text(g(n, "at")),
                changed.join(", "),
                tools.join(", ")
            ),
        ));
    }
    for n in others {
        shown.push((
            str_of(g(n, "at")),
            format!("  · {} {}", at_text(g(n, "at")), str_of(g(n, "text"))),
        ));
    }
    for n in items(d, "paused") {
        let mut s = format!("  · {} desktop not observed (paused)", at_text(g(n, "at")));
        if truthy(g(n, "until")) {
            s.push_str(&format!(" until {}", at_text(g(n, "until"))));
        }
        shown.push((str_of(g(n, "at")), s));
    }
    shown.sort_by(|a, b| a.0.cmp(&b.0));
    out.extend(shown.into_iter().map(|(_, l)| l));
    if truthy(verdict) {
        let when = day_month_time(&str_of(g(verdict, "at")));
        let mut s = format!("Your verdict: {} ({when})", str_of(g(verdict, "verdict")));
        if truthy(g(verdict, "note")) {
            s.push_str(&format!(" — {}", str_of(g(verdict, "note"))));
        }
        out.push(s);
    }
    out
}
