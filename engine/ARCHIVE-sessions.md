# Archived: the owner's coding-agent sessions (2026-11-22..24)

FERB's sessions feature was removed on 2026-11-24 (kb HAND-717, journal 198). Its code is kept only in git, at the tag
`archive/sessions-2026-11-24` (commit `f01e1d44`, the last commit with sessions in it). This page says what it was, why
it went, what was removed and what stayed, and how to bring any part back. A new design starts from the owner and a
plan (kb T-1272), not from restoring this as it was.

## What it was

Every Claude Code and Codex session on the Mac (and later every terminal and every open window of a known agent or chat
app), in FERB.app's Sessions page and a Sessions tab on both phones: listed, read, answered, sent to, started, watched
live and handed to FERB to keep going. Designs tried, in order (`engine/JOURNAL.md`):

| Journal | What was built | What happened |
|---|---|---|
| 170 | The agent remote lab built in: Claude Code's transcripts by allowlist and FERB's hook in Claude Code's settings (held permission prompts as FERB's own questions), Codex through its app-server daemon, hosts from the process tree, send paths (the agent's own interface, tmux, Terminal's scripting, else foreground typing proven in the session's terminal), New session in Terminal, Hand to FERB as a chat message, Watch as a live preview of the host window; portable crate `ferb-sessions` | Offline only (stand-in agents and Codex daemon) |
| 172 | Desktop chat apps (Claude app, ChatGPT, Codex app, Cursor's agent pane) as sessions, read from their Accessibility trees by code, waited on by code | Stand-in trees only |
| 181 | Sending into an editor's terminal: proven by title, or program and folder; else the owner picks the terminal; refused sends shown as not sent; the phones' tab layout B | His VS Code send was refused (the proof used only the title) |
| 185 | Watch finds the session's window; a send shows at once; the session's conversation uses the main chat's list; every phone page follows the screenshot switch | Watch said "isn't open" for VS Code |
| 186 | Every terminal is a session (process table, argv[0], tty activity), its text read while watched; New session is a terminal plus an optional command; chat panes proven by structure, one model look per unknown layout | agy in Terminal never showed |
| 189 | Fixes from his live run: terminals under root's `login`, a chat reply as typed parts, Hand to FERB said in the session, Take it back, one banner per finished turn | Terminal tabs still missing (see 196) |
| 191 | Hand to FERB as a session driver: code waits for the agent, one fresh worker step per reply with the usual tools and lookups, 20 rounds then ask | Not run live |
| 196 | Redesign (HAND-710): only Claude Code's and Codex's records read; terminals and agent apps' windows only watched live, with "Type here" into the window; Watch switches desktop and unminimizes; the driver for Claude Code and Codex only; the app-chat and terminal-text readers removed; `proc_pidinfo` EPERM for root processes found | Installed; he didn't like it |

The live findings that drove each change are in those entries ("What the owner found live", "Why").

## Why it was archived

The owner, after the redesigned build was installed: "i still don't like it, I have to think of this later with a fresh
mind and add it. as of now just archive this" (HAND-717). He had first asked to remove it before the redesign ("lets
remove adn archive eveyrhitgn built in sessions - BUT do a complete audit whats there to archive, is ti only sessions or
other things that are separate from it that we need anywyas") and then kept it for one more try (HAND-710).

## What was removed

About 25,000 lines in 143 files deleted, and the sessions parts of 123 more (+374 −3,892), plus 83 golden images
deleted and 159 recorded again (the phones' top bar lost a tab).

**Engine**
- Crate `ferb-sessions` whole (`engine/crates/ferb-sessions`, 3,457 lines): the model, Claude Code's transcript and
  hooks, Codex's app-server messages, hosts, terminals, windows, send-path proofs, the WebSocket frames, the refusal
  codes. Out of the workspace, `engine/gate.sh portable` and both `Cargo.lock` files.
- `ferb-daemon/src/sessions/` (mod, driver, terminals, codex_link, rpc, windows: 3,912) and `tests/sessions.rs`
  (2,321); the `sessions.enabled` setting; the `phone.locked` event (only sessions read it).
- `ferbd`: `compose/sessions.rs` (1,375), `compose/tools/sessions.rs` (271), `tests/sessions_hook.rs` (199), the
  `agent_session.*` tools' slot, `tasks.preview`'s `session`, the loops, the "Session driver steps" line of `ferb stats`
  and its test, the sessions' lines in `spawns.txt` and `seams-off.txt`, the settings rows in the replay tests.
  `agent_hook.rs` became a stub (below).
- `ferb-supervisor/src/flow/driver.rs` (408), `tests/session_driver.rs` (273), the `Fresh.driver` field,
  `DRIVER_HIDDEN_TOOLS` and the driver filters in `host.rs`, `restart.rs` and `chat_rpc.rs`; `ferb-core/src/driver.rs`.
  The temporary chat's fresh switch (ORCH-699..704) is unchanged.
- `ferb-core` bus topics `sessions.changed`, `sessions.card`, `sessions.card_gone`, `sessions.finished`,
  `phone.locked`; `ferb-orchestrator` waits' NOT_WAITED entries for them.
- `ferb-rpc`: the 12 `sessions.*` methods (`SESSIONS_ADDITIONS`, `SESSIONS_HOOK`, their phone classes, the
  `CANCEL_WHEN_LEFT` entry) and their rows in `engine/fixtures/phone/classes.json`: journal 170's named differences
  undone.
- `ferb-gateway`: `agent_session.list/read/send/answer` (catalog additions and their test) and the own checks
  `agent_session.permission`, `agent_session.enable` (also in `ferb-link`'s role and its test stand-in).
- `ferb-policy`: the app-chat wording of the terminal question and `TerminalInput.chat` (HAND-675), `is_terminal_app`
  (HAND-682), `tests/chat_card.rs`.
- `ferb-platform-macos/src/sessions.rs` (587) and its test (132), and its `plist` dependency.
- `ferb-mac-hands`: `show_window`, `focused_window_frame`, `same_frame` and the `Ax` trait's `show_window`,
  `window_minimized`, `focused_window_frame` (added only for sessions; the capture's own `window_minimized` stays).
- `ferb-link`: the refusal codes -32040..-32049 (a comment keeps them reserved: never reuse them) and the `session`
  forward on a preview.
- `ferb-mobile/src/sessions.rs` (678); `ferb-mobile-ffi`'s session calls, `watch_session`, the preview target and
  the 11 session records.
- `engine/runtime/orchestrator/prompts/session_driver.md`.

**FERB.app** (`apps/mac/FERBNotch`): `SessionsPage.swift`, `SessionsModel.swift`, `SessionTranscript.swift`,
`NotchCore/SessionRows.swift`, `Playground/PlaygroundSessions.swift`, four test files; the Sessions page in the window,
the Settings switch, the uninstall's `sessions.remove_hooks` step, `TaskPreview`'s session, the transcript's item row,
the daemon events.

**Android** (`apps/android`): `ui/Sessions.kt`, `model/Sessions.kt`, design `components/Sessions.kt` and
`screens/SessionScreens.kt`, three test files, 62 `sessions_*` strings, the Sessions tab, the folder picker
(`MacFolderPicker`, only New session used it), the session parts of the view model, phone API, Watch, sheets,
showcase and gallery; 42 golden images.

**iPhone** (`apps/ios`): `CodingSessions.swift`, `FerbAppUI/Sessions.swift`, design `Sessions.swift` and
`SessionParts.swift`, three test files, the Sessions tab and sheets, the session parts of the app model, phone API,
Watch, the gallery; the string catalog regenerated from Android's; 41 golden images.

## What stayed

- The chat's shared list (Android `ChatList`, `rememberChatFollow`, `JumpPill`; iPhone `FollowingList`) and a
  whole-screen page taking every touch (`coversPage`): PHN-826.
- The screenshot switch on every page (PHN-810, PHN-811); Watch live of a task.
- The temporary chat and its fresh switch (ORCH-699..704), ORCH-703's planner repair, the checker, learning and limits
  work done alongside, `LearningSpans`.
- The memory import's index of his coding sessions (MEM-466: `worklog_index`, "Bring in what's new"): memory, not this
  feature.
- The phones' adaptive top bar (PHN-801, PHN-820), now with four tabs (PHN-825).
- `ferb agent-hook …`, for good, as a stub: it reads its event, prints nothing, exits 0. Claude Code sessions started
  before the upgrade keep calling it until they restart, and an unknown command exits 2, which for UserPromptSubmit
  would block his prompts (HAND-718).
- The cleanup (HAND-718, ferb-device `claude_hooks`): every daemon start and `ferb uninstall` take FERB's own entries
  out of `~/.claude/settings.json` (nothing else in it changes), delete `~/.ferb/sessions/` and drop config.toml's
  `[sessions]`; journaled as `sessions.retired` when something was there.

## How to restore

The tag holds everything as it was:

```
git show archive/sessions-2026-11-24:engine/JOURNAL.md        # the whole story, journals 170-196
git checkout archive/sessions-2026-11-24 -- engine/crates/ferb-sessions engine/crates/ferb-daemon/src/sessions
git revert <the removal commit>                               # everything at once, onto today's tree
```

Restoring a part also needs its wiring back (the workspace member, the daemon's `mod sessions`, the RPC methods and
their phone classes, the bus topics, the catalog additions): `git show <the removal commit>` lists every place. The
kb decisions are superseded by HAND-717, not deleted: `tools/kb/run kb show HAND-710` and the rest still read. A
restored feature needs new decisions from the owner, and the start's cleanup (HAND-718) would remove a hook the
restored code installs: change or remove it in the same commit.
