//! The seams this program deliberately leaves off.
//!
//! Every seam `ferbd` fills for an engine crate is either a real implementation or goes through [`off`], which names
//! it. Each name is listed in `rust/crates/ferbd/seams-off.txt` with why it is off (the Python line that is also empty
//! there, or what makes it unneeded), so leaving a seam unwired is a reviewed line in that file, never a silent
//! `|| false`. The guard test (`tests/seams_wired.rs`) fails when:
//!
//! - a placeholder-shaped value (`|| false`, a closure that answers `None`, `{}`, an empty string or list, a `No…`
//!   stand-in, `..Default::default()`, a field set to `None`) appears in the composition outside an `off(…)` call;
//! - an `off("id", …)` names an id the file doesn't list, or the file lists one no code uses;
//! - a trait the file names (`trait <crate file> <Trait> for <ferbd file>`) has a method with a default body that
//!   ferbd's impl doesn't override and the file doesn't list as `default <Trait>::<method>`.

/// `value`, a seam deliberately left off; `id` names its line in `seams-off.txt`.
#[inline]
pub fn off<T>(id: &'static str, value: T) -> T {
    let _ = id;
    value
}
