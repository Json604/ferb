//! Traces: a task's recorded spans composed into the story, the span tree and the timing breakdown a developer reads
//! (`ferb trace TASK`, `--full`, `--json`), ported from `ferb/trace/` and the trace commands of `ferb/cli/admin.py`.
//!
//! Everything here is pure: the span tree and the outcome come in as decoded JSON (`ferb_store::spans::node_json`
//! and `SpanStore::outcome` produce it; the store itself is Mac-side), and text and JSON come out byte for byte as
//! the Python code writes them (`ferb-py` for `json.dumps`, `str` and `repr`; `ferb-sanitize` for origins and the
//! wrappers around pasted text).
//!
//! - [`compose`]: the episode of one task (`compose.py`).
//! - [`render`]: the story's lines (`render.py`).
//! - [`timing`]: where a turn's time went (`timing.py`).
//! - [`lines`]: one-line summaries and step lines (`lines.py`).
//! - [`tree`]: the span tree, the output modes of `ferb trace` (`admin.py`).
//!
//! Where Python raises on a record that is not the shape FERB writes (a missing field, a list where an object
//! belongs, a time that cannot be read), these functions read the missing part as empty; the one exception FERB's
//! own records can reach is listed in the journal.

pub mod compose;
pub mod lines;
pub mod py;
pub mod render;
pub mod timing;
pub mod tree;

mod printable;

pub use compose::compose;
pub use render::story;
pub use timing::{breakdown, lines as timing_lines};
pub use tree::{render_trace, trace_output, Mode};
