//! FERB's dated table of provider prices, USD per 1M tokens (ferb/runtimes/prices.py). An unlisted model has no
//! price. Checked against the official pages on the dates prices.py names; add a dated entry, never edit an old one.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    /// Uncached input.
    pub input: f64,
    /// Read from the provider's cache.
    pub cached: f64,
    /// Written to the cache (equal to input where writes aren't billed extra).
    pub cache_write: f64,
    /// Generated tokens, reasoning included.
    pub output: f64,
    /// Prompt size above which the long-context factors apply to the whole request.
    pub long_above: Option<i64>,
    pub long_input: f64,
    pub long_output: f64,
    /// ISO date this price starts.
    pub since: &'static str,
    /// Written with the 1-hour TTL; 0: as cache_write (RUN-436).
    pub cache_write_1h: f64,
}

const fn p(input: f64, cached: f64, cache_write: f64, output: f64) -> Price {
    Price {
        input,
        cached,
        cache_write,
        output,
        long_above: None,
        long_input: 1.0,
        long_output: 1.0,
        since: "2000-01-01",
        cache_write_1h: 0.0,
    }
}

const fn long(mut price: Price, above: i64, fi: f64, fo: f64) -> Price {
    price.long_above = Some(above);
    price.long_input = fi;
    price.long_output = fo;
    price
}

const fn hour(mut price: Price, write_1h: f64) -> Price {
    price.cache_write_1h = write_1h;
    price
}

const fn since(mut price: Price, day: &'static str) -> Price {
    price.since = day;
    price
}

/// prices.py PRICES, in its order.
pub fn prices(model: &str) -> &'static [Price] {
    static GPT6_ASTRA: [Price; 1] = [long(p(10.00, 1.00, 12.50, 50.00), 272_000, 2.0, 1.5)];
    static GPT61_SOL: [Price; 1] = [long(p(2.00, 0.10, 2.50, 10.00), 272_000, 2.0, 1.5)];
    static GPT6_SOL: [Price; 1] = [long(p(2.00, 0.20, 2.50, 10.00), 272_000, 2.0, 1.5)];
    static GPT6_LUNA: [Price; 1] = [p(0.10, 0.01, 0.125, 0.50)];
    static GPT55: [Price; 1] = [long(p(5.00, 0.50, 5.00, 30.00), 272_000, 2.0, 1.5)];
    static GPT56_SOL: [Price; 1] = [long(p(4.00, 0.40, 5.00, 20.00), 272_000, 2.0, 1.5)];
    static GPT56_LUNA: [Price; 1] = [long(p(0.20, 0.02, 0.25, 1.20), 272_000, 2.0, 1.5)];
    static FABLE51: [Price; 1] = [hour(p(10.00, 0.25, 12.50, 50.00), 20.00)];
    static OPUS55: [Price; 1] = [hour(p(4.00, 0.20, 5.00, 20.00), 8.00)];
    static SONNET55: [Price; 1] = [hour(p(2.00, 0.10, 2.50, 10.00), 4.00)];
    static HAIKU55: [Price; 1] = [hour(
        long(p(0.10, 0.01, 0.125, 0.50), 100_000, 5.0, 5.0),
        0.20,
    )];
    static HAIKU45: [Price; 1] = [hour(p(1.00, 0.10, 1.25, 5.00), 2.00)];
    static GEMINI38: [Price; 2] = [
        p(0.75, 0.075, 0.75, 3.75),
        since(p(1.50, 0.15, 1.50, 7.50), "2027-01-01"),
    ];
    static GEMINI31: [Price; 1] = [long(p(2.00, 0.20, 2.00, 12.00), 200_000, 2.0, 1.5)];
    match model {
        "gpt-6-astra" => &GPT6_ASTRA,
        "gpt-6.1-sol" => &GPT61_SOL,
        "gpt-6-sol" => &GPT6_SOL,
        "gpt-6-luna" => &GPT6_LUNA,
        "gpt-5.5" => &GPT55,
        "gpt-5.6-sol" => &GPT56_SOL,
        "gpt-5.6-luna" => &GPT56_LUNA,
        "claude-fable-5-1" => &FABLE51,
        "claude-opus-5-5" => &OPUS55,
        "claude-sonnet-5-5" => &SONNET55,
        "claude-haiku-5-5" => &HAIKU55,
        "claude-haiku-4-5" | "claude-haiku-4-5-20251001" => &HAIKU45,
        "gemini-3.8-flash" => &GEMINI38,
        "gemini-3.1-pro-preview" => &GEMINI31,
        _ => &[],
    }
}

/// Every model with a price, in prices.py's order.
pub const PRICED: [&str; 15] = [
    "gpt-6-astra",
    "gpt-6.1-sol",
    "gpt-6-sol",
    "gpt-6-luna",
    "gpt-5.5",
    "gpt-5.6-sol",
    "gpt-5.6-luna",
    "claude-fable-5-1",
    "claude-opus-5-5",
    "claude-sonnet-5-5",
    "claude-haiku-5-5",
    "claude-haiku-4-5",
    "claude-haiku-4-5-20251001",
    "gemini-3.8-flash",
    "gemini-3.1-pro-preview",
];
