//! Prompt context and structured answers (rust/PLAN.md stage 1, part 3): what FERB's prompts are measured with,
//! how a context is fitted into a model's window, where prompt files come from, every answer schema FERB asks a
//! model for, each provider's form of it, and the check every answer passes. Everything here reaches a model, so
//! the bytes are the contract (decision ENG-3); `rust/fixtures/context/make.py` and `rust/fixtures/catalog` hold
//! what the Python build produces.
//!
//! - [`tokens`]: ferb/core/context.py's token counts (o200k_base, 8,192-character slices, the UTF-8 fallback).
//! - [`assemble`]: ferb/core/context.py's context assembly and budget report.
//! - [`prompts`]: ferb/core/prompts.py's `load_prompt`.
//! - [`schemas`]: ferb/orchestrator/schemas.py and the other answer schemas, and the JSON-level answer readers.
//! - [`shape`]: ferb/runtimes/schema_shape.py: per-provider forms, the stated form, answers mapped back.
//! - [`structured`]: ferb/core/structured.py's `check_final`, with jsonschema's error choice and words.
//! - [`pyvalue`]: an answer as Python holds it, and Python's `repr` and `json.dumps` of it.
//!
//! Portable: no file but a prompt file is read, nothing touches a platform API.

pub mod assemble;
mod generated;
pub mod prompts;
pub mod pyvalue;
pub mod schemas;
pub mod shape;
pub mod structured;
pub mod tokens;

pub use assemble::{assemble, assemble_with, AssembleError, Assembly, ContextBlock};
pub use prompts::{PromptError, PromptRoot};
pub use pyvalue::Py;
pub use shape::{for_anthropic, for_gemini, repair_note, stated, unfenced, Refusals, Shape};
pub use structured::{check_final, CheckError, InvalidAnswer};
pub use tokens::{count_tokens, token_method, GrowingCount, Tokenizer, TOKEN_SLICE};
