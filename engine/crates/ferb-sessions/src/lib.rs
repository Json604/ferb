//! The owner's sessions on his Mac (HAND-710): every terminal whatever runs in it (kb HAND-691..695), his Claude Code and
//! Codex sessions (found, read and signalled through the agent itself, HAND-652..664) and every open window of an agent
//! or chat app (HAND-711), whatever app shows them. Only Claude Code's and Codex's own records are read; everything
//! else is watched live and typed into.
//!
//! This crate is portable (it builds for the phones too) and does no input or output of its own: the daemon hands it
//! bytes (a transcript's new lines, a hook's JSON, an app-server message, a process table) and takes back facts
//! (sessions, timeline items, cards, what to send, what to write). Everything here is structure: a record's type, a
//! field's presence, a process's parent. Nothing judges what a message means (AGENTS.md rule 4); what the agents
//! write is data, never instructions (rule 9).
//!
//! - [`model`]: a session, its host, its state, a timeline item, a question card, the send paths.
//! - [`apps`]: the agent and chat apps whose windows are sessions, listed by structure (HAND-711).
//! - [`claude`]: Claude Code's transcript (an allowlist), its hook events and replies, `claude agents --json`.
//! - [`codex`]: Codex's app-server messages (threads, items, status, approval requests) and the requests FERB sends.
//! - [`host`]: the app that shows an agent, from a process table (HAND-654).
//! - [`paths`]: which send paths a session has, in order (HAND-656).
//! - [`proof`]: whether the terminal FERB would type into is the session's (HAND-660, HAND-682).
//! - [`window`]: which of the host app's windows shows a session, for sending and for Watch (HAND-683, PHN-797).
//! - [`terminals`]: every terminal on the Mac as a session (HAND-692).
//! - [`hooks`]: FERB's entries in Claude Code's settings, added and removed without touching anything else.
//! - [`ws`]: the WebSocket frames Codex's local socket speaks.
//! - [`codes`]: the stable refusal codes of the `sessions.*` calls.

pub mod apps;
pub mod claude;
pub mod codes;
pub mod codex;
pub mod hooks;
pub mod host;
pub mod model;
pub mod paths;
pub mod proof;
pub mod terminals;
pub mod window;
pub mod ws;

pub use model::{
    Agent, Card, CardKind, Choice, Host, Item, ItemKind, SendPath, Session, Source, Status,
    TerminalChoice, Via, Waiting,
};

/// Cuts `text` to at most `n` characters on a character boundary, adding "…" when it was cut.
pub fn clip(text: &str, n: usize) -> String {
    let t = text.trim();
    if t.chars().count() <= n {
        return t.to_owned();
    }
    let mut out: String = t.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// One line: whitespace runs (newlines included) become one space.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
