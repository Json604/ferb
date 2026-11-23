//! `ferb agent-hook …` (kb HAND-718): the command FERB's archived sessions put in Claude Code's settings for each hook
//! event (engine/ARCHIVE-sessions.md). It stays as a stub that does nothing: it reads the event Claude Code sends on
//! stdin, prints nothing and exits 0. Every daemon start takes FERB's entries out of Claude Code's settings
//! (ferb-device `claude_hooks`), but a Claude Code session started before that keeps calling the hook until it
//! restarts. Without this command `ferb` would answer an unknown command with exit 2, which Claude Code reads as a
//! blocking hook error: for UserPromptSubmit that blocks the owner's prompts.

use std::io::Read;

/// The subcommand.
pub const COMMAND: &str = "agent-hook";
/// The most of stdin read (a hook's JSON; a tool's input can be large); the rest is left unread.
pub const MAX_INPUT: u64 = 16 * 1024 * 1024;

/// Reads the event and does nothing with it; always exits 0 with no output.
pub fn main(_args: &[String]) -> i32 {
    let _ = std::io::copy(&mut std::io::stdin().take(MAX_INPUT), &mut std::io::sink());
    0
}
