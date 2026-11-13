//! Versioned prompt loading (ferb/core/prompts.py): prompt files are executable input, read whole from the package
//! and never composed in code. Where the package is (FERB's `ferb/` folder in the Python build; the Rust daemon's
//! bundled copy) is the caller's: `PromptRoot` is the seam.

use std::io;
use std::path::{Component, Path, PathBuf};

/// Why `load_prompt` returned no text.
#[derive(Debug)]
pub enum PromptError {
    /// prompts.py:13's `ValueError`: not a `.md` file, or outside the package once links are followed.
    NotInPackage,
    /// The file couldn't be read (missing, a folder, not allowed).
    Io(io::Error),
    /// The file isn't UTF-8 (Python's `UnicodeDecodeError`).
    NotUtf8,
    /// Links that lead back to themselves (Python 3.12's `Path.resolve` raises `RuntimeError("Symlink loop …")`).
    LinkLoop,
}

impl std::fmt::Display for PromptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PromptError::NotInPackage => {
                f.write_str("prompt must be a Markdown file inside the FERB package")
            }
            PromptError::Io(e) => write!(f, "{e}"),
            PromptError::NotUtf8 => f.write_str("the prompt file is not UTF-8"),
            PromptError::LinkLoop => f.write_str("the prompt path is a link loop"),
        }
    }
}

impl std::error::Error for PromptError {}

/// The marker `resolve` fails with on a link loop.
#[derive(Debug)]
pub struct LinkLoop;

impl std::fmt::Display for LinkLoop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("symlink loop")
    }
}

impl std::error::Error for LinkLoop {}

/// The package folder prompt paths are relative to (prompts.py:7 `PACKAGE`).
#[derive(Clone, Debug)]
pub struct PromptRoot {
    package: PathBuf,
}

impl PromptRoot {
    /// `package` is resolved (links followed) once, as `Path(__file__).resolve().parents[1]` is.
    pub fn new(package: impl AsRef<Path>) -> io::Result<PromptRoot> {
        Ok(PromptRoot {
            package: resolve(package.as_ref())?,
        })
    }

    pub fn package(&self) -> &Path {
        &self.package
    }

    /// prompts.py:10 `load_prompt`: the file's text, `.strip()`ped, then each `{key}` replaced by its value, in the
    /// order given (a later replacement also applies inside an earlier one's value).
    pub fn load(
        &self,
        relative_path: &str,
        replacements: &[(&str, &str)],
    ) -> Result<String, PromptError> {
        let path = resolve(&self.package.join(relative_path)).map_err(|e| {
            if e.get_ref().is_some_and(|inner| inner.is::<LinkLoop>()) {
                PromptError::LinkLoop
            } else {
                PromptError::Io(e)
            }
        })?;
        let inside = path.ancestors().skip(1).any(|p| p == self.package);
        if !inside || python_suffix(&path) != ".md" {
            return Err(PromptError::NotInPackage);
        }
        let bytes = std::fs::read(&path).map_err(PromptError::Io)?;
        let text = String::from_utf8(bytes).map_err(|_| PromptError::NotUtf8)?;
        // read_text opens in text mode: "\r\n" and a lone "\r" read as "\n" (universal newlines)
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut text = ferb_py::value::strip(&text).to_string();
        for (key, value) in replacements {
            text = text.replace(&format!("{{{key}}}"), value);
        }
        Ok(text)
    }
}

/// `PurePath.suffix`: the final component's last `.` and what follows, when the dot is neither first nor last.
fn python_suffix(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name.rfind('.') {
        Some(at) if at > 0 && at < name.len() - 1 => name[at..].to_string(),
        _ => String::new(),
    }
}

/// `Path.resolve()` (strict=False, posixpath.realpath): absolute, `.` and `..` worked out component by component,
/// each link followed where the path exists; a part that doesn't exist is kept as written.
pub fn resolve(path: &Path) -> io::Result<PathBuf> {
    let start = if path.is_absolute() {
        PathBuf::from("/")
    } else {
        std::env::current_dir()?
    };
    let mut seen = 0usize;
    join_real(start, path, &mut seen)
}

fn join_real(mut out: PathBuf, rest: &Path, links: &mut usize) -> io::Result<PathBuf> {
    for part in rest.components() {
        match part {
            Component::RootDir | Component::Prefix(_) => out = PathBuf::from("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => {
                let next = out.join(name);
                match std::fs::symlink_metadata(&next) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        *links += 1;
                        if *links > 40 {
                            return Err(io::Error::other(LinkLoop));
                        }
                        let target = std::fs::read_link(&next)?;
                        let base = if target.is_absolute() {
                            PathBuf::from("/")
                        } else {
                            out.clone()
                        };
                        out = join_real(base, &target, links)?;
                    }
                    _ => out = next,
                }
            }
        }
    }
    Ok(out)
}
