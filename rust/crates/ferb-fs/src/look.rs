//! look.py (HAND-517): a picture or one PDF page on disk as an image the AI sees. Opens nothing on screen, writes no
//! file, caps no file's size. The file is opened once and checked as fs.read checks it; decoding (ImageIO, PDFKit,
//! the screenshot's JPEG helper) is a [`Renderer`] that `ferb-platform-macos` supplies.

use ferb_core::types::{Dict, ToolContext, ToolResult};
use serde::Deserialize;
use serde_json::Value;

use crate::files::{self, Fail, FsEnv, Out};
use crate::pypath as pp;
use crate::sys::{self, At};

pub const NAME: &str = "fs.look";
pub const DESCRIPTION: &str = "See a picture or one page of a PDF on disk, as an image: a photo, screenshot or scan \
    (PNG, JPEG, HEIC, TIFF, GIF, WebP) or how a PDF page looks; a PDF page also comes with its text, when it has any. A \
    file the owner attached in chat is a path you can look at with this. It opens nothing on screen.";
/// mac/screenshot.py MAX_EDGE: the long side of the image the model gets.
pub const MAX_EDGE: u32 = 1568;
pub const PDF_MAGIC: &[u8] = b"%PDF-";
/// The PDF header may follow some bytes; readers look in the first KiB.
pub const PDF_HEAD: usize = 1024;

/// Why a file's bytes gave no picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LookError {
    /// `Undecodable(why)`: no picture or PDF FERB can decode.
    Undecodable(String),
    /// `IndexError(pages)`: the PDF has this many pages, not the one asked.
    NoPage(i64),
}

/// ImageIO and PDFKit (look.py `picture` and `pdf_page`): the JPEG and its facts, in look.py's key order
/// (`format, pixels, width, height[, frames, shown]` for a picture; `format, page, pages, width, height, text` for a
/// page).
pub trait Renderer: Send + Sync {
    fn picture(&self, data: &[u8], max_edge: u32) -> Result<(Vec<u8>, Dict), LookError>;
    fn pdf_page(
        &self,
        data: &[u8],
        number: i64,
        max_edge: u32,
    ) -> Result<(Vec<u8>, Dict), LookError>;
}

fn default_page() -> i64 {
    1
}

#[derive(Clone, Debug, Deserialize)]
pub struct Args {
    pub path: String,
    #[serde(default = "default_page")]
    pub page: i64,
}

/// fs.look.
pub fn fs_look(a: &Args, ctx: &ToolContext, env: &FsEnv, renderer: &dyn Renderer) -> Out {
    match look(a, ctx, env, renderer) {
        Ok(r) => Ok(r),
        Err(Fail::Refused(m)) => Ok(files::err(&m, env)),
        Err(Fail::Decode(start)) => Ok(files::err(
            &format!("the file isn't UTF-8 text (byte {start}): nothing was read; use a binary-aware tool"),
            env,
        )),
        Err(Fail::Os(e)) => Ok(files::os_error(&e, "nothing was read", env)),
        Err(Fail::Raised(m)) => Err(files::Raised::Error(m)),
    }
}

fn look(
    a: &Args,
    ctx: &ToolContext,
    env: &FsEnv,
    renderer: &dyn Renderer,
) -> Result<ToolResult, Fail> {
    let p = files::resolve(&a.path, &ctx.workspace, &env.home, true)?;
    if !files::pl_is_file(&p)? {
        return Ok(files::err(&format!("{p} is not a file"), env));
    }
    let fd = sys::open(At::Cwd, &p, sys::O_RDONLY | sys::O_CLOEXEC, 0)?;
    if let Some(refusal) = files::opened_closed(ctx, fd.raw(), "read").filter(|r| !r.is_empty()) {
        return Ok(files::err(&refusal, env));
    }
    let data = sys::read_to_end(fd.raw())?;
    drop(fd);
    let name = pp::name(&p);
    let page = a.page;
    let head = &data[..data.len().min(PDF_HEAD)];
    let decoded = if head.windows(PDF_MAGIC.len()).any(|w| w == PDF_MAGIC) {
        renderer.pdf_page(&data, page, MAX_EDGE)
    } else if page != 1 {
        return Ok(files::err(
            &format!("{name} is a picture, not a PDF: it has no page {page}"),
            env,
        ));
    } else {
        renderer.picture(&data, MAX_EDGE)
    };
    let (jpeg, facts) = match decoded {
        Ok(x) => x,
        Err(LookError::NoPage(pages)) => {
            let s = if pages != 1 { "s" } else { "" };
            return Ok(files::err(
                &format!("{name} has {pages} page{s}; there is no page {page}"),
                env,
            ));
        }
        Err(LookError::Undecodable(why)) => {
            return Ok(files::err(
                &format!(
                    "FERB couldn't open {name} as a picture or a PDF ({why}); nothing was shown"
                ),
                env,
            ))
        }
    };
    let mut d = files::identity(&p)?;
    for (k, v) in facts {
        d.insert(k, v);
    }
    let source = files::source_of(&p, ctx, &env.home)?;
    Ok(ToolResult {
        content: d,
        image: Some(jpeg),
        read_sources: vec![source],
        ..Default::default()
    })
}

/// A `Value` helper for renderers: `[w, h]`.
pub fn pair(a: i64, b: i64) -> Value {
    Value::Array(vec![a.into(), b.into()])
}
