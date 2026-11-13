//! The owner's exclusion list (`ferb/sense/exclusions.py`), applied before anything is written, published or tagged.
//! An excluded observation is dropped completely: no row, no bus event, no count (decision: SENSE-403). Private or
//! incognito browsing is never recorded.

use std::collections::BTreeSet;

use ferb_py::text::lower;
use ferb_py::url::hostname;
use serde_json::Value;

use crate::observation::{truthy, Dict};
use crate::paths;

/// exclusions.py:15.
pub const DEFAULT_EXCLUDE_PATHS: [&str; 3] = ["~/.ssh", "~/Library/Keychains", "~/.ferb"];

/// exclusions.py:19.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Exclusions {
    pub apps: BTreeSet<String>,
    pub domains: BTreeSet<String>,
    /// expanded and resolved, as Python's `Path` objects (their text)
    pub paths: Vec<String>,
}

fn resolved(text: &str) -> String {
    let p = paths::expanduser(text);
    // a link loop: kept unresolved (Python raises RuntimeError here; see `paths`)
    paths::resolve(&p).unwrap_or(p)
}

impl Exclusions {
    /// exclusions.py:25 `build`.
    pub fn build<A: AsRef<str>, D: AsRef<str>, P: AsRef<str>>(
        apps: &[A],
        domains: &[D],
        paths: &[P],
    ) -> Exclusions {
        Exclusions {
            apps: apps.iter().map(|a| lower(a.as_ref())).collect(),
            domains: domains
                .iter()
                .map(|d| lower(d.as_ref()).trim_start_matches('.').to_string())
                .collect(),
            paths: paths.iter().map(|p| resolved(p.as_ref())).collect(),
        }
    }

    /// `Exclusions.build()` with the default paths.
    pub fn defaults() -> Exclusions {
        Exclusions::build::<&str, &str, &str>(&[], &[], &DEFAULT_EXCLUDE_PATHS)
    }

    /// exclusions.py:30.
    pub fn app(&self, bundle_id: Option<&str>, name: Option<&str>) -> bool {
        [bundle_id, name]
            .into_iter()
            .flatten()
            .any(|v| !v.is_empty() && self.apps.contains(&lower(v)))
    }

    /// exclusions.py:33.
    pub fn host(&self, host: Option<&str>) -> bool {
        let h = lower(host.unwrap_or(""));
        let h = h.trim_end_matches('.');
        !h.is_empty()
            && self
                .domains
                .iter()
                .any(|d| h == d || h.ends_with(&format!(".{d}")))
    }

    /// exclusions.py:37: a URL that `urlsplit` refuses is not excluded.
    pub fn url(&self, url: Option<&str>) -> bool {
        match hostname(url.unwrap_or("")) {
            Ok(h) => self.host(h.as_deref()),
            Err(_) => false,
        }
    }

    /// exclusions.py:43.
    pub fn path(&self, path: Option<&str>) -> bool {
        let Some(path) = path.filter(|p| !p.is_empty()) else {
            return false;
        };
        let p = resolved(path);
        self.paths.iter().any(|x| paths::within(&p, x))
    }

    /// exclusions.py:53: every rule applies to every source.
    pub fn excluded(&self, _source: &str, data: &Dict) -> bool {
        if truthy(data.get("incognito")) {
            return true;
        }
        let s = |k: &str| match data.get(k) {
            Some(Value::String(s)) => Some(s.as_str()),
            _ => None,
        };
        if self.app(s("bundle_id"), s("app")) {
            return true;
        }
        if self.host(s("host")) || self.url(s("url")) {
            return true;
        }
        self.path(s("path")) || self.path(s("from_path"))
    }
}
