//! Each watcher's logic (`ferb/sense/watchers`): what it does with what macOS reports, over seams for the native
//! reads (implemented in [`crate::mac`]). None of them activates, focuses or moves anything (rule 10); Accessibility
//! is read only where Python reads it (events the observers deliver, the focused field at a key press), never polled
//! for content.

pub mod accessibility;
pub mod app_focus;
pub mod app_installs;
pub mod base;
pub mod browser_tabs;
pub mod clipboard;
pub mod files;
pub mod idle;
pub mod notifications;
pub mod owner_input;
pub mod presence;
pub mod software;
pub mod text_input;
pub mod windows;
pub mod workspace;
