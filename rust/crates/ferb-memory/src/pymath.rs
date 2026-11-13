//! Python 3.12's float arithmetic where results must match bit for bit: `sum()` of floats (Neumaier's compensated
//! sum since 3.12) and `round(x, n)` (correctly rounded, half to even on the exact binary value).

/// `sum(values)` with Python's start value 0: the first value as is, then Neumaier's compensated sum; the
/// compensation is added at the end only when it is finite and non-zero (`bltinmodule.c` `builtin_sum_impl`).
pub fn sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut it = values.into_iter();
    let Some(first) = it.next() else {
        return 0.0;
    };
    let mut f = 0.0 + first;
    let mut c = 0.0;
    for x in it {
        let t = f + x;
        if f.abs() >= x.abs() {
            c += (f - t) + x;
        } else {
            c += (x - t) + f;
        }
        f = t;
    }
    if c != 0.0 && c.is_finite() {
        f += c;
    }
    f
}

/// `round(x, digits)` for a float.
pub fn round(x: f64, digits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.digits$}").parse().unwrap_or(x)
}
