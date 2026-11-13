//! One usage record for every runtime (ferb/runtimes/usage.py): tokens in, cached, written, out, reasoning, and the
//! cost basis. A cost FERB has no price for stays unknown (`None`), never zero; a cache-write count the provider
//! doesn't report is marked not reported (RUN-438).

use serde_json::{Map, Value};

use crate::error::Raised;
use crate::prices::prices;
use crate::pyjson::py_int;

#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    pub model: String,
    /// Every prompt token, cached or not.
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    /// Every generated token, reasoning included.
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    /// None when the model has no listed price.
    pub cost_usd: Option<f64>,
    /// "provider" | "price_table" | "unknown".
    pub cost_basis: String,
    /// "api" | "subscription".
    pub billing: String,
    pub cache_write_5m_tokens: i64,
    pub cache_write_1h_tokens: i64,
    pub cache_write_reported: bool,
}

impl Usage {
    /// `dataclasses.asdict(usage)`, fields in the dataclass's order.
    pub fn as_dict(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("model".into(), self.model.clone().into());
        m.insert("input_tokens".into(), self.input_tokens.into());
        m.insert(
            "cached_input_tokens".into(),
            self.cached_input_tokens.into(),
        );
        m.insert("cache_write_tokens".into(), self.cache_write_tokens.into());
        m.insert("output_tokens".into(), self.output_tokens.into());
        m.insert("reasoning_tokens".into(), self.reasoning_tokens.into());
        m.insert(
            "cost_usd".into(),
            self.cost_usd
                .and_then(serde_json::Number::from_f64)
                .map_or(Value::Null, Value::Number),
        );
        m.insert("cost_basis".into(), self.cost_basis.clone().into());
        m.insert("billing".into(), self.billing.clone().into());
        m.insert(
            "cache_write_5m_tokens".into(),
            self.cache_write_5m_tokens.into(),
        );
        m.insert(
            "cache_write_1h_tokens".into(),
            self.cache_write_1h_tokens.into(),
        );
        m.insert(
            "cache_write_reported".into(),
            self.cache_write_reported.into(),
        );
        m
    }

    /// A usage with no price and no cache split (the shape tests' fake providers build).
    pub fn plain(model: &str, input: i64, output: i64, billing: &str) -> Usage {
        Usage {
            model: model.into(),
            input_tokens: input,
            cached_input_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: output,
            reasoning_tokens: 0,
            cost_usd: None,
            cost_basis: "unknown".into(),
            billing: billing.into(),
            cache_write_5m_tokens: 0,
            cache_write_1h_tokens: 0,
            cache_write_reported: true,
        }
    }
}

/// usage.py `cost_usd`: `day` is the ISO date the price applies on (Python: `date.today()` when not given).
#[allow(clippy::too_many_arguments)]
pub fn cost_usd(
    model: &str,
    input_tokens: i64,
    cached: i64,
    cache_write: i64,
    output: i64,
    day: &str,
    cache_write_1h: i64,
) -> Option<f64> {
    let mut chosen = None;
    for entry in prices(model) {
        if entry.since <= day && chosen.is_none_or(|c: &crate::prices::Price| entry.since > c.since)
        {
            chosen = Some(entry);
        }
    }
    let p = chosen?;
    let long = p.long_above.is_some_and(|above| input_tokens > above);
    let (fi, fo) = if long {
        (p.long_input, p.long_output)
    } else {
        (1.0, 1.0)
    };
    let fresh = (input_tokens - cached - cache_write).max(0);
    let hour = cache_write_1h.min(cache_write);
    let hour_price = if p.cache_write_1h != 0.0 {
        p.cache_write_1h
    } else {
        p.cache_write
    };
    let writes = (cache_write - hour) as f64 * p.cache_write + hour as f64 * hour_price;
    Some(
        (fresh as f64 * p.input * fi
            + cached as f64 * p.cached * fi
            + writes * fi
            + output as f64 * p.output * fo)
            / 1e6,
    )
}

#[allow(clippy::too_many_arguments)]
fn make(
    model: &str,
    inp: i64,
    cached: i64,
    write: i64,
    out: i64,
    reasoning: i64,
    billing: &str,
    provider_cost: Option<f64>,
    write_1h: i64,
    by_ttl: bool,
    write_reported: bool,
    day: &str,
) -> Usage {
    let (w5, w1) = if by_ttl {
        (write - write_1h.min(write), write_1h.min(write))
    } else {
        (0, 0)
    };
    let (cost, basis) = match provider_cost {
        Some(c) => (Some(c), "provider"),
        None => {
            let c = cost_usd(model, inp, cached, write, out, day, write_1h);
            (
                c,
                if c.is_some() {
                    "price_table"
                } else {
                    "unknown"
                },
            )
        }
    };
    Usage {
        model: model.into(),
        input_tokens: inp,
        cached_input_tokens: cached,
        cache_write_tokens: write,
        output_tokens: out,
        reasoning_tokens: reasoning,
        cost_usd: cost,
        cost_basis: basis.into(),
        billing: billing.into(),
        cache_write_5m_tokens: w5,
        cache_write_1h_tokens: w1,
        cache_write_reported: write_reported,
    }
}

fn int(d: &Map<String, Value>, key: &str) -> Result<i64, Raised> {
    py_int(d.get(key))
}

fn sub(d: &Map<String, Value>, key: &str) -> Map<String, Value> {
    match d.get(key) {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    }
}

/// usage.py `from_openai`.
pub fn from_openai(
    model: &str,
    usage: &Map<String, Value>,
    billing: &str,
    day: &str,
) -> Result<Usage, Raised> {
    let details = sub(usage, "input_tokens_details");
    let out = sub(usage, "output_tokens_details");
    Ok(make(
        model,
        int(usage, "input_tokens")?,
        int(&details, "cached_tokens")?,
        int(&details, "cache_write_tokens")?,
        int(usage, "output_tokens")?,
        int(&out, "reasoning_tokens")?,
        billing,
        None,
        0,
        false,
        details.contains_key("cache_write_tokens"),
        day,
    ))
}

/// usage.py `from_codex` (OpenAI's meaning: input includes cached, output includes reasoning).
pub fn from_codex(model: &str, usage: &Map<String, Value>, day: &str) -> Result<Usage, Raised> {
    Ok(make(
        model,
        int(usage, "input_tokens")?,
        int(usage, "cached_input_tokens")?,
        int(usage, "cache_write_input_tokens")?,
        int(usage, "output_tokens")?,
        int(usage, "reasoning_output_tokens")?,
        "subscription",
        None,
        0,
        false,
        usage.contains_key("cache_write_input_tokens"),
        day,
    ))
}

/// usage.py `from_anthropic`.
pub fn from_anthropic(
    model: &str,
    usage: &Map<String, Value>,
    billing: &str,
    provider_cost: Option<f64>,
    day: &str,
) -> Result<Usage, Raised> {
    let read = int(usage, "cache_read_input_tokens")?;
    let write = int(usage, "cache_creation_input_tokens")?;
    let by_ttl = sub(usage, "cache_creation");
    Ok(make(
        model,
        int(usage, "input_tokens")? + read + write,
        read,
        write,
        int(usage, "output_tokens")?,
        0,
        billing,
        provider_cost,
        int(&by_ttl, "ephemeral_1h_input_tokens")?,
        true,
        usage.contains_key("cache_creation_input_tokens"),
        day,
    ))
}

/// usage.py `from_gemini`.
pub fn from_gemini(
    model: &str,
    meta: &Map<String, Value>,
    billing: &str,
    day: &str,
) -> Result<Usage, Raised> {
    let thoughts = int(meta, "thoughtsTokenCount")?;
    Ok(make(
        model,
        int(meta, "promptTokenCount")? + int(meta, "toolUsePromptTokenCount")?,
        int(meta, "cachedContentTokenCount")?,
        0,
        int(meta, "candidatesTokenCount")? + thoughts,
        thoughts,
        billing,
        None,
        0,
        false,
        false,
        day,
    ))
}

/// The local date as `date.today().isoformat()` writes it.
pub fn today() -> String {
    chrono::Local::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
}
