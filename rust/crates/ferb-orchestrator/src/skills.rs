//! ferb/orchestrator/skills.py: P21 owner-written skills, a small catalog and confined body reads. Each path part is
//! opened relative to the folder already open, without following a link, so no link swapped in can leave the folder.

use std::ffi::CString;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

use serde_json::{json, Map, Value};

/// Python's `Refused`: the owner-facing text.
pub const REFUSED_PARTS: &str = "Choose a skill and a file inside its folder.";
pub const REFUSED_FILE: &str = "That skill file is unavailable or is a link outside its folder.";

/// `Path(relative).parts` (POSIX).
fn path_parts(p: &str) -> Vec<String> {
    let mut out = Vec::new();
    if p.starts_with('/') {
        out.push("/".to_string());
    }
    out.extend(
        p.split('/')
            .filter(|x| !x.is_empty() && *x != ".")
            .map(String::from),
    );
    out
}

fn open_at(dir: Option<RawFd>, name: &str, directory: bool) -> Option<OwnedFd> {
    let c = CString::new(name).ok()?;
    let mut flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    if directory {
        flags |= libc::O_DIRECTORY;
    }
    // SAFETY: `c` is a valid NUL-terminated path; `dir` is an open descriptor this function's caller owns.
    let fd = unsafe {
        match dir {
            Some(d) => libc::openat(d, c.as_ptr(), flags),
            None => libc::open(c.as_ptr(), flags),
        }
    };
    if fd < 0 {
        None
    } else {
        // SAFETY: `fd` was just opened and is owned by nothing else.
        Some(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

/// skills.py:12 `Skills`.
pub struct Skills {
    pub directory: String,
}

impl Skills {
    /// skills.py:16 `_read`: the file's text (universal newlines, as Python's text mode reads it).
    pub fn read_file(&self, skill: &str, relative: &str) -> Result<String, String> {
        let mut parts = vec![skill.to_string()];
        parts.extend(path_parts(relative));
        if skill.is_empty()
            || ferb_fs::pypath::name(skill) != skill
            || parts
                .iter()
                .any(|p| p.is_empty() || p == "." || p == ".." || p.contains('/'))
        {
            return Err(REFUSED_PARTS.into());
        }
        let mut held: Vec<OwnedFd> = Vec::new();
        held.push(open_at(None, &self.directory, true).ok_or_else(|| REFUSED_FILE.to_string())?);
        for part in &parts[..parts.len() - 1] {
            use std::os::fd::AsRawFd;
            let next = open_at(Some(held.last().expect("one open").as_raw_fd()), part, true)
                .ok_or_else(|| REFUSED_FILE.to_string())?;
            held.push(next);
        }
        use std::os::fd::AsRawFd;
        let fd = open_at(
            Some(held.last().expect("one open").as_raw_fd()),
            &parts[parts.len() - 1],
            false,
        )
        .ok_or_else(|| REFUSED_FILE.to_string())?;
        let mut file = std::fs::File::from(fd);
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut bytes).map_err(|_| REFUSED_FILE.to_string())?;
        let text = String::from_utf8(bytes).map_err(|_| REFUSED_FILE.to_string())?;
        Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
    }

    /// skills.py:37 `_parse`: (front matter's name and description, body).
    pub fn parse(text: &str) -> (Vec<(String, String)>, String) {
        let Some(rest) = text.strip_prefix("---\n") else {
            return (Vec::new(), text.to_string());
        };
        let Some((head, body)) = rest.split_once("\n---\n") else {
            return (Vec::new(), text.to_string());
        };
        let mut fields: Vec<(String, String)> = Vec::new();
        for line in crate::py::splitlines(head) {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            if key == "name" || key == "description" {
                let v = crate::py::strip_chars(crate::py::strip(value), "\"'").to_string();
                match fields.iter_mut().find(|(k, _)| k == key) {
                    Some(slot) => slot.1 = v,
                    None => fields.push((key.to_string(), v)),
                }
            }
        }
        (fields, body.to_string())
    }

    /// skills.py:51 `catalog`.
    pub fn catalog(&self) -> Vec<Value> {
        if !ferb_fs::pypath::is_dir(&self.directory) || ferb_fs::pypath::is_symlink(&self.directory)
        {
            return Vec::new();
        }
        let Ok(listing) = std::fs::read_dir(&self.directory) else {
            return Vec::new();
        };
        let mut names: Vec<String> = listing
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let mut items = Vec::new();
        for name in names {
            let Ok(text) = self.read_file(&name, "SKILL.md") else {
                continue;
            };
            let (fields, _) = Skills::parse(&text);
            let get = |k: &str| {
                fields
                    .iter()
                    .find(|(f, _)| f == k)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            };
            if !get("name").is_empty() && !get("description").is_empty() {
                let mut item = Map::new();
                item.insert("id".into(), json!(name));
                for (k, v) in &fields {
                    item.insert(k.clone(), json!(v));
                }
                items.push(Value::Object(item));
            }
        }
        items
    }

    /// skills.py:64 `read`.
    pub fn read(&self, skill: &str, file: &str) -> Result<Value, String> {
        let text = self.read_file(skill, file)?;
        let (fields, body) = if file == "SKILL.md" {
            Skills::parse(&text)
        } else {
            (Vec::new(), text)
        };
        let mut out = Map::new();
        out.insert("id".into(), json!(skill));
        out.insert("file".into(), json!(file));
        for (k, v) in fields {
            out.insert(k, json!(v));
        }
        out.insert("body".into(), json!(body));
        Ok(Value::Object(out))
    }
}
