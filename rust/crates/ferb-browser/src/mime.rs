//! `mimetypes.guess_type(name)[0]` as Python 3.12 computes it for a file's name: Python's built-in
//! tables ([`crate::mime_table`], generated) and then each `mime.types` file Python's `init` reads
//! (`knownfiles`: on the Mac, /etc/apache2/mime.types), later lines winning. The upload through the
//! page tells the page this type (HAND-501).

use crate::mime_table::{ENCODINGS, SUFFIXES, TYPES};

/// `mimetypes.knownfiles`, read in order when present.
pub const KNOWN_FILES: [&str; 9] = [
    "/etc/mime.types",
    "/etc/httpd/mime.types",
    "/etc/httpd/conf/mime.types",
    "/etc/apache/mime.types",
    "/etc/apache2/mime.types",
    "/usr/local/etc/httpd/conf/mime.types",
    "/usr/local/lib/netscape/mime.types",
    "/usr/local/etc/httpd/conf/mime.types",
    "/usr/local/etc/mime.types",
];

/// A `MimeTypes` database: the defaults and what files added.
#[derive(Clone, Debug, Default)]
pub struct MimeTypes {
    /// Extension -> type, from files (they override the defaults).
    added: Vec<(String, String)>,
}

/// `posixpath.splitext`.
fn splitext(p: &str) -> (&str, &str) {
    let sep = p.rfind('/').map_or(0, |i| i + 1);
    let dot = p.rfind('.');
    match dot {
        Some(d) if d >= sep => {
            // a name that is all dots before this one has no extension
            if p[sep..d].chars().all(|c| c == '.') {
                (p, "")
            } else {
                (&p[..d], &p[d..])
            }
        }
        _ => (p, ""),
    }
}

impl MimeTypes {
    /// The built-in tables only (`MimeTypes(filenames=())`).
    pub fn defaults() -> MimeTypes {
        MimeTypes::default()
    }

    /// The database `mimetypes.init()` makes: the defaults, then each known file present.
    pub fn system(read: impl Fn(&str) -> Option<String>) -> MimeTypes {
        let mut db = MimeTypes::defaults();
        for f in KNOWN_FILES {
            if let Some(text) = read(f) {
                db.read(&text);
            }
        }
        db
    }

    /// `MimeTypes.readfp`: each line's first word is a type, the rest its suffixes, up to a word that
    /// starts with `#`.
    pub fn read(&mut self, text: &str) {
        for line in text.lines() {
            let mut words: Vec<&str> = line
                .split(ferb_py::value::is_py_space)
                .filter(|w| !w.is_empty())
                .collect();
            if let Some(i) = words.iter().position(|w| w.starts_with('#')) {
                words.truncate(i);
            }
            let Some((kind, suffixes)) = words.split_first() else {
                continue;
            };
            for s in suffixes {
                let ext = format!(".{s}");
                self.added.retain(|(e, _)| *e != ext);
                self.added.push((ext, (*kind).to_string()));
            }
        }
    }

    fn lookup(&self, ext: &str) -> Option<String> {
        if let Some((_, t)) = self.added.iter().find(|(e, _)| e == ext) {
            return Some(t.clone());
        }
        TYPES
            .binary_search_by_key(&ext, |(e, _)| e)
            .ok()
            .map(|i| TYPES[i].1.to_string())
    }

    /// `guess_type(name)[0]`, strict.
    pub fn guess_type(&self, name: &str) -> Option<String> {
        // `urllib.parse._splittype`: a leading "scheme:" is dropped; "data:" gives its own type
        let mut url = name;
        if let Some(i) = name.find(':') {
            let scheme = &name[..i];
            if !scheme.is_empty() && !scheme.contains('/') {
                if scheme.to_lowercase() == "data" {
                    let rest = &name[i + 1..];
                    let comma = rest.find(',')?;
                    let semi = rest[..comma].find(';').unwrap_or(comma);
                    let kind = &rest[..semi];
                    return Some(if kind.contains('=') || !kind.contains('/') {
                        "text/plain".into()
                    } else {
                        kind.into()
                    });
                }
                url = &name[i + 1..];
            }
        }
        let (b, e) = splitext(url);
        let (mut base, mut ext) = (b.to_string(), e.to_string());
        while let Some((_, to)) = SUFFIXES.iter().find(|(x, _)| *x == ext.to_lowercase()) {
            let joined = format!("{base}{to}");
            let (b, e) = splitext(&joined);
            (base, ext) = (b.to_string(), e.to_string());
        }
        if ENCODINGS.iter().any(|(x, _)| *x == ext) {
            ext = splitext(&base).1.to_string();
        }
        self.lookup(&ext.to_lowercase())
    }
}
