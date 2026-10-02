# FERB architecture (version 0.19.0)

This file describes what the code on this branch does, and only that. Every claim names the file (and where it
helps, the function) that implements it; a claim the author could not confirm in code is marked "unverified".
Future work is in sections 15 and 16 and nowhere else. Two platform references stand beside it:
`docs/PLATFORM_MACOS.md` (how Mac apps can be driven in the background, with probe results on this Mac) and
`docs/PLATFORM_BROWSER.md` (the same for Chromium). The one-worker design is `docs/DESIGN_ONE_WORKER.md`; the rework
that produced this version is `docs/REWORK_PLAN.md`.

Companion documents: `CODE_TOUR.md` (the code in the order it runs), `FILE_MAP.md` (every file), `CAPABILITIES.md`
(what works and how well it is tested), `docs/OWNER_ACCEPTANCE.md` (the owner's own test list), `CHANGELOG.md`.

This rewrite replaces the pre-rework numbering. Historical `§17` (spikes), `§18` (build steps), `§20` (units)
and `§21` (safety tests) references in source comments, changelogs and old findings refer to the previous contract,
available with `git show b993da5:ARCHITECTURE.md`. Use sections 6–7 for today's drivers, 5 for policy, and 14 for
testing; historical completion claims do not establish that the rebuilt version passed owner acceptance.

Sections: 1 Purpose and principles · 2 Processes and trust boundaries · 3 Identities and scopes · 4 The chat turn
and the one worker · 5 The gate at the action · 6 Mac control · 7 Browser control · 8 Shell and files ·
9 Verification, ledger and reconciliation · 10 Sense, memory, time, identity secrets · 11 Storage, bus, journal,
spans, redaction · 12 Install, permissions, setup, doctor, voice · 13 CLI reference · 14 Testing · 15 Known gaps and
what's next · 16 Future blocks.

---

## 1. Purpose and principles

FERB is a personal agent that runs on the owner's Mac and does what the owner asks in chat: reads and acts in Mac
apps, in the owner's own browser profiles, in FERB's own browser and accounts, in a shell and in files; sets
reminders and scheduled tasks; remembers what the owner tells it; keeps a record of the owner's activity (metadata
only). The owner's goal for this version: **control the Mac like a person would, in the background, learning by
doing**. Background means the owner keeps working in their own app while FERB works in another: nothing on their
screen moves, the pointer and keyboard are theirs, their desktop (Space) never switches. The owner's eyes are the
final verifier of that (section 14).

The principles the code follows:

- **The owner's instruction is the approval.** FERB asks only when a mistake would be costly and the owner didn't
  name the target, or when an action can't be undone (section 5). Everything else just runs.
- **Hardcoding is allowed for facts about the platform and for safety floors, never for judgement**
  (`docs/REWORK_PLAN.md`, "The rule on hardcoding"). Allowed: Accessibility role and attribute names, virtual key
  codes, browser bundle ids, CDP method names, destructive shell patterns, the hard floor (pay, permanent deletion,
  account security), the identity split, exposure after untrusted reads, the one floor word list. Not allowed:
  tables that turn the owner's verbs into action kinds, "risky label" lists used as permissions, "search box" label
  regexes, phrase lists for "the owner named their browser", service catalogues that decide app versus web, per-app
  behaviour written into generic branches. Where FERB needs to know how an app behaves, it learns by probing and
  stores a per-app profile (`ferb/act/mac/profile.py`); it ships none written by hand.
- **One model worker per request, with every tool its identity allows; the gate is at the action**, not at
  planning time (section 5). The worker declares what each act does; code raises the declaration to a floor derived
  from facts about the control; the floor only ever raises.
- **Everything a model does reaches the Mac through one gateway** (`ferb/daemon/gateway.py`), which checks the run
  token, the identity, the scopes, the effect, the policy engine, the ledger, and redacts the result. Failures are
  values: a worker always gets a result, never an exception.
- **Background first.** Per primitive, the Mac driver tries the mechanisms the platform probes and the owner's own
  runs proved, in a fixed order, and only brings an app in front when the call says it may (section 6). The browser
  driver works in a minimized window with a debugger session per tab (section 7).
- **Content is data.** Anything read from apps, pages, files or pasted into chat is marked untrusted, and a worker
  that read untrusted content loses shell, AppleScript and file-write for the rest of its run (section 5).
- **Code computes dates, never the model** (`ferb/clock/when.py`). Code checks every quote the model claims the
  owner said (reminders, schedules, memory).
- **Redact everywhere.** One pipeline (`ferb/sanitize/redact.py`) runs before anything reaches a model, a trace, a
  log, the journal, memory or an export.
- **No graphical UI.** The only interface is the CLI (`ferb chat`, `ferb logs`, the admin commands) and push-to-talk.
  FERB.app has no window, Dock icon or menu-bar item.

---

## 2. Processes and trust boundaries

```
 ferb chat / ferb logs / admin commands            FERB.app (voice, signed)
          │ Unix socket, JSON-RPC, role cli                 │ role voice
          ▼                                                 ▼
 ┌──────────────────────────────────────────────────────────────────────┐
 │ FERB daemon (launchd user agent, ~/.ferb)                            │
 │ supervisor · gateway · policy · ledger · bus · timers · schedules    │
 │ watchers · trail · memory · spans · journal · drivers (mac, browser) │
 └───────▲──────────────────────────────────────────────▲───────────────┘
         │ role mcp_shim + run token                     │ role native_host
 ┌───────┴─────────────┐                        ┌────────┴─────────────┐
 │ codex exec / Claude │ stdio MCP              │ ferb-native-host     │ native messaging
 │ Agent SDK  ◄────────┤ ferb mcp shim          │ (relay only)  ◄──────┤ FERB extension (Chrome / Brave
 └─────────────────────┘                        └──────────────────────┘ profiles, incl. FERB's own)
```

- **The daemon** (`ferb/daemon/main.py:Daemon`) is the only long-lived process. launchd starts it at login as
  `com.ferb.daemon` (`ferb/device/install.py:Installer.plist`) with `FERB_HOME`, `RunAtLoad`, `KeepAlive` on crash
  only, `LimitLoadToSessionType: Aqua`. It refuses to start if another daemon answers on the socket
  (`ferb/daemon/server.py:ensure_socket_free`). It owns the single SQLite connection, the bus, every driver and every
  loop (undo releases, timers, hourly distillation, a CoreFoundation run-loop tick every 0.2 s so macOS's running-app
  list stays fresh: `Daemon._macos_events`).
- **The socket** (`~/.ferb/run/ferb.sock`, mode 0600) speaks newline-delimited JSON-RPC 2.0, 1 MiB per message
  (`ferb/daemon/server.py:Server`). The first request must be `auth.hello {role}`. Roles and what proves them
  (`ferb/daemon/auth.py`): `cli` by the peer's uid being the owner's; `mcp_shim` by a run token (only its SHA-256 is
  stored; scoped to one worker; expiring; revocable; re-checked on every call so a revoke takes effect on open
  connections); `native_host` by the owner's uid plus a `chrome-extension://<id>/` origin in the known extension ids,
  after which the connection is a message channel authenticated per message by pairing (section 7); `voice` by the
  owner's uid, the peer executable being the installed `~/.ferb/FERB.app` and its code signature meeting the
  requirement recorded at `ferb setup --voice` (`auth.check_voice_peer`). Each role has a fixed method list
  (`auth.METHODS`); the shim may call only `ping`, `tools.list`, `tools.call`.
- **Runtimes** are the model processes FERB starts for the supervisor and the worker (`ferb/registry.py:build`
  registers `codex` and `claude`; `config.toml [runtimes] orchestrator / worker` picks per role, default codex).
  `ferb/runtimes/codex_exec.py:CodexRuntime` runs the pinned Codex 0.157.1 binary (`~/.ferb/node`, or the repo's
  `node_modules` in development) with `--ephemeral`, a read-only sandbox, `approval_policy=never`, every disableable
  native tool off (`DISABLED`), FERB's own locked model catalog (`locked_catalog`: code mode, sub-agents and
  experimental tools neutralised for every model), model `gpt-5.5` (`MODEL`), and FERB's own `CODEX_HOME`
  (`~/.ferb/codex-home`) once the owner has logged in there (`ferb setup --codex-login`). Worker mode's only tool
  source is the `ferb mcp` server; the run token goes to the shim in a 0600 file named by `FERB_RUN_TOKEN_FILE`, never
  on the command line. The request trace (`RUST_LOG=trace` on stderr) is scanned on every run
  (`ferb/runtimes/tooltrace.py:tools_in`); any tool outside the allowlist kills the run and the daemon unregisters
  that runtime (`Daemon._runtime_violation`). `ferb/runtimes/claude_sdk.py:ClaudeRuntime` does the same over the
  Claude Agent SDK 0.2.160: `tools=[]`, `permission_mode="dontAsk"`, no settings sources, strict MCP, no session
  persistence, an API-key login refused (subscription only), and the tool list the API really received checked
  through a local proxy that records only tool names (`ferb/runtimes/proxy.py:ToolListProxy`).
- **`ferb mcp`** (`ferb/cli/mcp.py:Shim`) is the stdio MCP server a runtime spawns. It has no logic: `tools/list`
  forwards to the daemon's `tools.list`, `tools/call` to `tools.call`, each with the run token, with a 1 h socket
  timeout because a tool call may rightly wait on the owner. Tool names the model sees use `_` for `.`
  (`mcp_name`).
- **`ferb-native-host`** (`ferb/native_host/main.py:relay`) is what Chrome or Brave launch for the extension
  (`~/.ferb/bin/ferb-native-host`, a shell launcher pointing at the venv; manifest in Chrome's and Brave's
  `NativeMessagingHosts` folders, `Installer.write_native_host`). It relays length-prefixed native messages to and
  from the daemon socket, checks nothing but size, logs nothing, and exits as soon as either side closes.
- **The extension** (`extensions/chromium/`, MV3, version 0.8.0, protocol 4) is FERB's hands in the browser: it
  executes daemon commands and reports tab events; it holds no policy. Its id is fixed by the key in
  `manifest.json` (`ferb/act/browser/ext_protocol.py:FERB_EXTENSION_ID`). The CDP allowlist is enforced in the
  extension itself (`protocol.js:CDP_ALLOW`), so the daemon can't ask for more.
- **FERB.app** (`macapp/FERB/main.swift`, built and signed by `ferb setup --voice` into `~/.ferb/FERB.app`) is a
  background Swift app (`LSUIElement`) that opens the microphone and runs on-device speech recognition for
  push-to-talk (section 12). It is the only part of FERB that opens a microphone.
- **Browsers are never launched under remote debugging** by FERB (profiles are opened with `open -g` or the
  browser binary with `--no-startup-window`, `ferb/act/browser/profiles.py:open_known_profile`); the only CDP is the
  extension's `chrome.debugger` with its allowlist. The lab Chrome used by real tests (`tests/chromelab.py`) is the
  exception and never touches the owner's browsers.

---

## 3. Identities and scopes

Two identities (`ferb/core/types.py:Identity`): `owner` (the owner's Mac, browsers, shell and files; never the
owner's credentials) and `ferb` (FERB's own browser profile, secrets, mailbox and workspace). Every task, run token
and capability carries one.

Scopes per identity (`ferb/policy/rules.py:SCOPES_BY_IDENTITY`):

| Identity | Scopes |
|---|---|
| `owner` | `browser:read`, `browser:act`, `mac:read`, `mac:act`, `applescript`, `shell`, `fs:read`, `fs:write`, `screen`, `trail:read`, `time` |
| `ferb` | `identity:browse`, `secrets:fill`, `mailbox:read`, `mailbox:send`, `fs:read`, `fs:write`, `time` |

A worker gets every scope its identity allows that FERB has right now (`ferb/orchestrator/dag.py:scopes_for`,
over the availability built by `Daemon._availability`), reads included (`rules.with_reads`: `browser:act` brings
`browser:read`, `mac:act` brings `mac:read`, `fs:write` brings `fs:read`). The gateway lists a tool only when its
scopes are within the token's and its `identities` include the token's identity (`Gateway.tools_for`). The policy
engine re-checks both on every call (`rules.check_identity`, `rules.check_scopes`); a `ferb` task may touch files
only inside FERB's workspace (`~/.ferb/workspace`). The supervisor's task is rejected once and repaired when a
`ferb` task names an owner folder (`dag.validate`); a second failure becomes a question to the owner.

Tools by identity (`ferb/registry.py`): owner tasks get `browser.*` (the owner's paired profiles), `mac.*`,
`shell.*`, `trail.query`, `fs.*`, `time.*`, `owner.ask`; ferb tasks get `ferb_browser.*` (FERB's profile, plus
`fill_secret`, `fill_otp`), `mailbox.send`, `fs.*`, `time.*`, `owner.ask`. `owner.ask` has no scopes and both
identities (`ferb/act/ask.py`). A task never switches identity.

FERB's credentials live in the macOS Keychain only (service `ferb`, account = the secret key such as
`ferb/email/password`; `ferb/identity/secrets.py:KeychainSecretStore`). Their `SecretSpec` (allowed origins,
account hint) is in `config.toml [secrets]`. There is no public reveal: `_reveal` is called only by the identity
browser driver's origin-checked fill (section 7). Owner passwords are never requested or typed. For an owner-requested OTP login, `browser.login_code` obtains
hidden CLI input through `Gateway.ask_login_code`, binds it to the selected HTTPS frame origin and code field,
and sends a sealed fill through `Router.owner_code`. The model receives only the result, and the transient approval
answer is cleared. The redactor learns the value to scrub page echoes. FERB's Keychain and inbox remain identity-only.

---

## 4. The chat turn and the one worker

**Entry.** `ferb chat` streams `chat.send {session_id, text}`; `Daemon._chat_send` runs
`Supervisor.send` as its own asyncio task and merges its events with bus events for the task it starts (tool calls,
approval requests, undo countdowns, blocks). Closing chat never stops a task (the answer waits in `task_results`
and arrives as a macOS notification, `Daemon._task_result`); Ctrl-C calls `task.stop` (`Daemon._task_stop`), which
cancels the tree and undoes any send held for it. Push-to-talk feeds the same path with session `voice`
(`Daemon._voice_turn`).

**Before the model** (`ferb/orchestrator/supervisor.py:Supervisor.send`): pasted or quoted text is wrapped as
untrusted data (`ferb/sanitize/untrusted.py:mark`: bracketed pastes, fenced blocks, `>` quotes; tags carry a random
id so pasted text can't forge a closing tag; `[policy] pasted_text_is_data = false` turns this off). The owner's own
words are kept apart for every later quote check. If the previous turn was a question (`clarify`), the new message
continues that request: both messages count as the owner's words, and a plain yes endorses FERB's question for
naming checks (`_affirmative`). Memory is recalled by keyword (`ferb/memory/recall.py:context_block`, up to 8
items), availability is built (`Daemon._availability`: what tools exist and which are usable now, with a plain
"Unavailable: … how to fix" line), and an environment line is built (`Daemon._environment`: macOS version, the
owner's paired profiles, the catalogue services the owner's words mention with "app installed / web domain"
(`ferb/policy/services.py:services_named`, `summary`), what is running, and the browser or profile the owner named
(`ferb/act/browser/profiles.py:named_in`) with an instruction to use only that).

**The supervisor model** (`Supervisor._supervise`; prompt `ferb/orchestrator/prompts/supervisor.md`; no tools;
supervisor mode of the runtime, low reasoning effort for Codex) sees: FERB's instructions with the availability
block, the last day's conversation and tasks from the journal (the last 8 verbatim, earlier ones as one-line gists,
`Supervisor.recent`), the recalled memory, the environment line, the owner's message, and a time block at the end
(`ferb/clock/context.py:time_block`: now, time of day, idle time, relative ages of the tagged items, and after a
gap of over an hour a "since we last talked" list, `Supervisor.since_last_talk`). It answers with exactly one of
(`ferb/orchestrator/schemas.py:SUPERVISOR_SCHEMA`, `SupervisorOutcome`):

- `reply`: text.
- `clarify`: one question; the next message continues the request.
- `task`: `goal` (the request in plain words with everything the owner said that matters), `identity`
  (`owner` for the owner's apps, files, browser profiles or accounts; `ferb` for FERB's own accounts or a site with
  no mention of the owner's), `targets` (apps, sites, paths, recipients the model understood: informational, used
  for the environment line and the "did the owner name this" checks, never a reason to refuse), and optional
  `success_conditions` (text plus a code check: `file_exists`, `file_contains`, `ledger_done`, `tab_url`,
  `account`, or `none`).

plus, on any kind: `remember` (items with the owner's exact quote), `timers` (reminders with a structured `when`
and a quote), `schedule`, `schedule_changes`, `reminder_cancels`. No subtasks, roles, verbs or scopes exist in the
schema. Code checks only the identity floor and that FERB has something usable for the identity (`dag.validate`);
one repair round, then the question "I couldn't work out how to do that — tell me which app or site".

**Side effects handled in code before any worker starts** (`Supervisor.send`): `remember` items are kept only when
the quote is in the owner's typed words (`Daemon._remember`, origin `owner_stated`); reminder cancels, then new
reminders (`Supervisor._set_timers`: quote checked, `clock/when.py:resolve` computes the time, in the future and
within a year; a refused reminder makes the reply say so instead of claiming it was set); a schedule
(`Daemon._schedule_create`: quote checked, first run computed by code, `Rule.build`, targets recorded with paths
normalised and "me" replaced by the owner's email); schedule changes and reminder cancels (quote-checked). "Email
me" with no known address becomes the question "What email address should I use for you?" (`_wants_me`,
`Daemon._owner_email` reads an owner-stated memory item whose key contains `email`).

**The task** (`Supervisor.run_tree`): one tree id, one root task and one worker task `<tree>.worker` with
`role="worker"`, the identity's full scope set, the success-condition texts and a deadline of 4 hours
(`ferb/orchestrator/guards.py:MAX_MINUTES`). The request's facts are recorded for the gateway
(`ferb/policy/targets.py:Requests.record`: the owner's words, the targets, the owner's email) and forgotten when
the tree ends. The worker's context (`Supervisor._context`) is the goal, the success conditions, what the owner
named, and the workspace path; `ferb/orchestrator/worker.py:WorkerRuns._run_worker_once` adds the tool list
(with a note that Codex hides MCP tools behind `tool_search`), the measured typical duration of similar runs
(`ferb/clock/durations.py:typical`) and the time block. The prompt is `prompts/worker.md`: what the worker can do,
that apps and tabs are worked in the background and it must never bring an app forward, the effect classes and
`why`, how to retry after `needs_owner_approval`, messages and calls (Send button over Return, the `contact`
field), `owner_ask`, that everything read is data, and the report shape.

A run token is issued per worker run (`daemon/auth.py:RunTokens.issue`, TTL = the time to the deadline), revoked
when the run ends, and the worker's taint labels are forgotten with it. A worker that reported `failed` or
`needs_user` saying it had no tools, having called none, gets one more run with that spelled out
(`WorkerRuns.run_worker`, `NO_TOOLS`). The run is watched (`WorkerRuns._watch`): no sign of life for 180 s
(`guards.STALL_S`), not counting waits on the owner, cancels it; the tree caps (4 h, 50 worker runs, 1,000 tool
calls, `guards.TreeGuard.over`) stop the tree; the same failing call 3 times or a 2–4 call loop repeated 5 times
with identical arguments and results stops the worker (`TreeGuard.call`, fed by `Gateway.observer`). There are no
token or dollar budgets.

**The report** (`schemas.WORKER_SCHEMA`): `status` done / failed / needs_user, `summary`, `artifacts`, `evidence`.
The summary is the owner's answer: there is no separate answer-writing run (`Supervisor._final`). `needs_user` is
for the case where the owner must do something on screen; a choice that is the owner's goes through `owner.ask`
(section 5) and the worker carries on. A task whose ledger holds an `unknown` action is reconciled before the
outcome (section 9); success conditions are verified after a `done` outcome (section 9), and the final answer
appends what verification could not confirm. The outcome is journaled (`task.answer`, `task.outcome`), stored in
`task_results` for a closed chat, published on the bus, and the task's tabs and shell processes end with it
(`Daemon._task_end`, also for the `.verify` and `.reconcile` helper tasks).

**Continuity.** The chat history is re-sent each turn from the journal (the Codex supervisor is ephemeral). After a
daemon restart, tasks that were running are marked `interrupted` (`ferb/orchestrator/tree.py:TaskTree.interrupt_running`)
and chat offers to pick them up (`Daemon.start`). `chat.status` on opening returns answers that arrived while chat
was closed, running tasks, pending approvals and queued questions (`Daemon._chat_status`).

**Scheduled runs** (`Supervisor.run_scheduled`, `ferb/orchestrator/schedules.py`): a schedule keeps the owner's
request, their words and the targets recorded at scheduling time; each run is decided afresh by the supervisor
with a note that the owner isn't there, then held to the recorded targets (`targets.within`: extra recipients,
sites, apps or paths refuse the run with a plain answer). Rules: once, daily, weekdays, weekly on a day, or every
N ≥ 15 minutes, in the Mac's time zone (a daily 09:00 stays 09:00 across daylight-saving changes). Each next run is a
durable timer of kind `schedule`. A run missed while the Mac slept runs late if it is less than half the interval
late (at most 12 h), else it is skipped and reported (`Daemon._run_schedule`). Three failures in a row pause the
schedule. During an unattended run questions can't be asked: `owner.ask` is declined at once, an approval waits an
hour then the step is skipped, and a folder-access question is queued for the owner's next chat
(`owner_questions`, `Daemon._queue_question`).

---

## 5. The gate at the action

Every tool call goes through `ferb/daemon/gateway.py:Gateway._call`, in this order:

1. The tool must be in this worker's list (scopes, identity, not lost to exposure) and the task running.
2. `why` (≤ 400 chars) and `effect` are stripped from the arguments; both are recorded in the call's span. The
   gateway adds both fields to the schema of every tool whose `reversible` is false (`Gateway.tools_for`,
   `Gateway._acts`): `mac.menu`, `mac.act`, `mac.click`, `mac.applescript`, `browser.act`, `ferb_browser.act`,
   `ferb_browser.fill_secret`, `fill_otp`, `mailbox.send`, `shell.run`, `shell.send`, `fs.write`, `fs.edit`. A
   bad `effect` value is refused. The remaining arguments are validated against the tool's `Input` model.
3. `cap.precheck` runs before any policy step (today only `mac.click` in a messaging or calling app without a
   declared `purpose`).
4. `owner.ask` is answered here and never reaches a tool (below).
5. `cap.describe(args)` gives the `ActionSpec` (kind, target, content, data labels). The **floor** is
   `ferb/policy/effects.py:floor_of(kind)` and `effective = raise_to(declared, floor)` = max of the two; an absent
   declaration is the floor. A `send`, `pay` or `destroy` call without `why` is sent back for one, saying what the
   floor is.
6. Taint labels: a worker that read owner-account data carries its labels onto any call with content
   (`ferb/policy/taint.py`).
7. The contact check for calls, and for sends when the request names a person rather than an address
   (`Gateway._contact`, `ferb/policy/contacts.py`, below).
8. `owner_named` is computed (`ferb/policy/targets.py:owner_named`): every recipient (or the owner's own email)
   is named in the owner's words, else, for kinds that go to people (`send`, `call`, `post`, `reply`,
   `mailbox_send`), false; else the site or app is named. "Named" (`targets.named`) means the target itself, a
   distinctive label of its domain or bundle id, or a catalogue name or alias for that service ("Gmail" names
   `mail.google.com`), in the owner's own words only.
9. The duplicate-send check: a second `send` to the same contact in one task asks, showing both texts
   (`Gateway._send_again`); a no says the first message stands.
10. The span starts and `tool.called` is published.
11. `PolicyEngine.evaluate` (below). `ask` waits for the owner (`Gateway._await_owner`): a yes also records the
    action's destination as approved for data flow, then policy runs again; a no, expiry or `ferb stop` blocks. A
    `block` returns "blocked by FERB policy: …" and the worker is told not to work around it.
12. Jev, when configured, for non-navigate effects (`Gateway._jev_action`): only a stricter answer acts (block, or
    ask once more).
13. `hold`: the undo window (`Gateway._wait_hold`); the owner's time doesn't count as a stall.
14. Resource locks (`ferb/act/locks.py`: account, app, file, tab, always taken in that order) are held across the
    call only, then `Gateway._execute`: a ledger row before any non-reversible kind (section 9), the tool's `run`,
    the folder-access question for `fs.*` outside the allowed roots (below), read sources into taint and exposure,
    the result redacted, the snapshot recorded for the contact check (`contacts.Seen`), Jev's result checks
    (injection, signed-out page), the span finished, the time footer appended. An image reaches the model only,
    never a span, log or file.

**Effect classes** (`effects.EFFECTS`, in rank order): `navigate` (opens, scrolls, switches, searches, reads;
nothing of the owner's changes), `edit` (changes something that can be changed back or only affects the task's own
targets: typing, filling, saving a draft, a setting, a file in a granted folder, liking a song), `send` (anything
that leaves the Mac or reaches another person or service), `pay` (money), `destroy` (permanent deletion, account
security, anything irreversible). The help text the worker sees is `gateway.EFFECT_HELP`.

**Floors from kinds** (`effects.FLOOR_BY_KIND`): read, navigate, search, open_tab, mailbox_read → navigate;
draft, fill_form, click, write_file, mac_input, applescript, run_shell, trash, timer, fill_secret → edit; send,
post, reply, submit, mailbox_send, call → send; pay → pay; delete_permanent, account_security → destroy. Unknown
kinds count as edit.

**The one floor word list** (`effects.FLOOR_WORDS`, `label_kind`): a control whose label literally contains, as a
whole word, case-insensitively, `send` → send, `submit` → submit, `call` → call, `pay` / `buy` / `order` → pay,
`delete` / `remove` / `unsubscribe` → delete_permanent, `sign out` → account_security. It is a floor, not a
permission: any other label is left to the declaration; labels in other languages don't hit it. It is used by
`mac.act` press on a labelled control, `mac.menu` on the last path item, `mac.click` on the control Accessibility
finds under the point, and `browser.act` on an element's label.

**Kinds are facts about the control**, decided in the tools' `describe` (`ferb/act/mac/tools.py:MacAct._kind`,
`ferb/act/browser/tools.py:act_kind`): Mac typing is `mac_input` (a search box: `search`); Return is `submit`;
focus is `navigate`; a press is `mac_input` unless the label hits the floor. On a page: scroll and hover are `read`;
upload or a file input is `fill_form`; type or select is `search` in a search form else `fill_form`; Enter is
`search` in a search form else `submit`; a floor-word label pins the kind; a link with an href, a tab or tree item
is `navigate`; form roles are `fill_form`; a form's button or submit input in a non-search form is `submit`; else
`click` (or `search` in a search form). A non-reversible browser action on an account or security page
(`browser/tools.py:security_page`: password, 2FA, recovery, delete-account paths under an account context) is
`account_security`, so it always asks.

**The policy engine** (`ferb/policy/engine.py:PolicyEngine.evaluate`), fixed order, first decision wins:

1. Stop: blocked (`ferb stop`, lifted by `ferb resume-tasks`; survives restarts through `~/.ferb/STOPPED`).
2. Identity and scopes (`rules.check_identity`, `check_scopes`).
3. Exposure (`ferb/policy/exposure.py`): an exposed worker loses `shell`, `applescript`, `fs:write`
   (`LOST_WHEN_EXPOSED`; the gateway also drops those tools from its list). Exposure comes from reading sources of
   kind web, email, message, download or third_party, or from an injection signal (Jev). A `trusted_sources` set
   exists in the tracker but nothing populates it from config (unverified use).
4. `navigate` runs.
5. Data flow (`ferb/policy/dataflow.py:check`): owner-account data may go only to a destination the request
   involves (`targets.involves`: a target of the request, something the owner named, or something they approved in
   an ask) or back where it came from; it never crosses into the `ferb` identity. Applies to `OUTBOUND` kinds
   (send, post, reply, fill_form, submit, mailbox_send, write_file, pay, click, mac_input).
6. `ferb/policy/autonomy.py:ask_reason`: an irreversible action (`rules.irreversible`: kind in the hard floor
   `pay`, `delete_permanent`, `account_security`, or effect `pay` / `destroy`, or flagged by Jev) asks every time,
   bound to the exact action; a `send` the owner didn't name asks once per (request, effect, target); an injection
   signal on the worker's path asks for anything but navigate; `careful` mode asks for every non-navigate kind not
   in `[policy] trusted_kinds`. An unverifiable account (section 7) asks once per site and profile and the yes is
   remembered in `[policy] trusted_accounts`.
7. Approvals (`ferb/policy/approvals.py`): a grant for an irreversible action matches the exact payload hash
   (`rules.payload_hash`: tool, kind, effect, identity, target, amount, content) and is used once; any other grant
   matches the scope key (`rules.scope_key`: request, effect, identity, account, site, recipients, path, amount;
   never the content) and covers later calls for the rest of the request (4 h). An unanswered question expires
   after 10 minutes (1 hour for an unattended run). Chat shows it as "OK for FERB to … ? [y/N]" with the worker's
   `why` (`ferb/cli/chat.py:question`); when no chat is attached to the task, a macOS notification says to open
   chat (`Daemon._approval_notice`).
8. The undo window: `send`, `post`, `reply`, `mailbox_send` and `call` kinds are held 10 s
   (`rules.UNDO_WINDOW`, `approvals.Holds`); chat shows "✉️ Sending to … in 10 s… (Ctrl-C to cancel)"; `ferb undo`,
   Ctrl-C, a spoken "stop" or `ferb stop` cancel it; the daemon releases due holds every 0.2 s
   (`Daemon._release_loop`). A `submit` (a form's button, Enter in a non-search form) is `send` for asking purposes
   but is not in the undo window.

**Autonomy levels** (`autonomy.Level`, `ferb autonomy`): `careful`, `balanced` (default), `hands-off`. Only
`careful` changes anything today (it asks for every consequential kind); the hard floor applies at every level.

**`owner.ask`** (`ferb/act/ask.py`, `Gateway._ask_owner`): a question (≤ 300 chars) with 0–6 options. The gateway
files it as an approval of kind `owner_question` (`Approvals.question`), publishes `approval.requested` so chat
shows numbered options or a free-text prompt, and blocks the call until the owner answers through `chat.answer`
(a number picks an option, other text is the answer, Enter declines). Result `{answer}` or `{declined}`; a scheduled
run gets `{declined, reason}` at once.

**Contacts are never guessed** (`ferb/policy/contacts.py`, `Gateway._contact`): a call, or a send when the request
names a person, must carry `contact = {name, selected, detail}`. Code checks that the selected name and its detail
were on screen in this task's snapshots of that app or tab (the last 10, in memory); if one screen showed several
entries matching the name and nothing in the owner's words picks one (this request, the owner's messages of the
last day, and what they explicitly asked FERB to remember about that name: `Daemon._owner_words`), FERB asks "Which
Rahul did you mean?" with numbered options; the answer applies to that one action only. The undo countdown names
the exact contact and `contact.selected` is journaled. In messaging and calling apps (`services.COMMUNICATION`:
WhatsApp, Slack, Zoom, Teams, FaceTime, Messages, Telegram, Discord, Signal, Skype) a `mac.click` must declare
`purpose` call / send (with contact) / other, and a `mac.screenshot` there also records the window's Accessibility
text as evidence of who was on screen (never sent to the model).

**Folder access** (`Gateway._folder_access`): an `fs.*` call refused as outside the allowed folders triggers "Allow
FERB to access ~/X?" only when the folder is among the request's targets or named in the owner's own words
(`_named_folder`; never a bare home folder, never from content FERB read); a yes adds it to `[fs] owner_roots` at
once (`Daemon._grant_folder`) and the call is retried.

**Jev** (`ferb/decide/decider.py:ShadowDecider`, `questions.py`; set up by `ferb setup --jev`, key in the Keychain
only) is asked after FERB's own checks and only ever acts stricter: `action.matches_request` (no → block, unsure →
ask), `action.risk` (high → ask), `content.injection_suspected` (yes → the reader is exposed),
`browser.login_state` (signed_out → the worker is told to stop), `loop.control` (stop / escalate → the worker is
stopped, checked every 25 calls), `memory.worth_keeping` (no → the proposal is dropped), `trail.relevance` /
`trail.sensitive` (stored on the row; sensitive → extra redaction), `browser.profile_match` (a suggestion the owner
confirms), `task.route` (logged). In shadow mode (the default) a sample of calls is logged next to FERB's decision
in the `decisions` table and nothing changes; `ferb decide enable <question>` turns one on. Jev unavailable leaves
FERB's decision in place and fails nothing.

---

## 6. Mac control

Owner identity only. Tools (`ferb/act/mac/tools.py`): `mac.apps` (list, frontmost marked; `mac:read`), `mac.open`
(`mac:read`: opening to look is reading), `mac.snapshot` (`mac:read`, plus an image with `screen`), `mac.menu`
(`mac:read` to list; pressing needs `mac:act`), `mac.act` (`mac:act`), `mac.screenshot` (`screen`), `mac.click`
(`mac:act`), `mac.applescript` (`applescript`). Apps on `[privacy] exclude_apps` (default: password managers,
`ferb/config.py:DEFAULT_EXCLUDE_APPS`) are never read, opened, captured or acted on
(`ferb/act/mac/driver.py:MacComputerDriver.app`).

**Opening** (`tools.MacOpen`): `open -g -b <bundle>` (`ferb/act/mac/apps.py:launch`), never activation. If macOS
switched the owner to the new app anyway, the owner's app is activated again (`driver.on_activate` tells the
trail it was FERB). Then `driver.ensure_window` waits up to 10 s for a titled window, pressing the app's own "Open
main window" or Window-menu item through Accessibility if it has none (`accessibility.open_window`). Electron apps
are asked once per process for their full tree (`accessibility.expose_tree` sets `AXManualAccessibility`; other
apps refuse it harmlessly).

**The snapshot** (`driver.snapshot` → `accessibility.read_tree`): one Accessibility walk of the app's window (the
focused or main window, the first listed, or the focused element's window: `accessibility.app_window`, the only
routes to a window on another Space), plus any sheet in it and the app's open menus, popovers and dialogs (children
of the application element). A control is anything that exposes an action (`AXUIElementCopyActionNames`: press,
menu, increment, decrement, confirm, cancel, pick) or has a known role or the search-field subrole; the raw AX role
is reported as a fact alongside the kind (button, textbox, searchbox, popup, combobox, checkbox, radio, slider,
stepper, tab, menuitem, link, or the role's own name for rows, groups, cells and images). Each element carries its
actions, value, disabled / selected / expanded state and, for a control in a sheet, menu, dialog or popover, `in`.
The snapshot also returns the window title, visible text, headings, `open` (what panels are up), `windows` (the
window server's list for the app, each with id, title, on this Space or not, and whether Accessibility can reach it
now), and `truncated` with the reason (400 elements, depth 40, 5,000 visited). A password field (role or subrole
`AXSecureTextField`) is reported only as sensitive and filled or empty; its value is never read. Element ids are
`s<gen>e<n>` and stay valid until the next snapshot of that app (`driver.node` refuses older ones). The same walk
serves the before / after signature that tells whether a background event did anything (`AxTree.signature`), and
the password scan for a screenshot. A browser window can be read this way but not acted on
(`ferb/act/mac/ax_browser.py:ACT_REFUSED`); its web area is re-read for up to 2 s while empty. `mac.snapshot`
passes `window` (a title or id from the list) to read another window. With the `screen` scope and
`[mac] snapshot_image` on, the snapshot also returns the window image with each control's number drawn on it
(`screenshot.capture` with marks), under the screenshot rules below.

**Acting** (`driver.act`, `ComputerAction`): per primitive, the rungs below, each tried only after the one before
verifiably did nothing (a value read back, or the window signature compared after 0.4 s), with the per-app profile
recording what landed. The order comes from `docs/PLATFORM_MACOS.md` section 12 and the owner-verified runs there.

| Primitive | Rungs, in order |
|---|---|
| press / click (an element) | `AXPress` → `AXPress` on the nearest ancestor that lists it (Electron wraps controls in groups, `accessibility.press_ancestor`) → `AXFocused` then Return (Space for a checkbox or radio) posted to the process → foreground |
| type | `AXFocused` the box; refused if something else holds the app's keyboard focus (a sheet or dialog: `FOCUS_ELSEWHERE`) → unicode keystrokes posted to the process (`CGEventPostToPid`, `input.type_steps`), read back → the Accessibility keyboard (`AXUIElementPostKeyboardEvent`, `accessibility.post_text`; tried where the profile says it works or is untried) → the box's whole value set to old + new (`AXValue`) → foreground (a box that isn't a text field is clicked and typed into in front, since set values there don't search: WhatsApp) |
| set_value | `AXValue` on a real text field, else as type |
| edit | `AXSelectedTextRange` + `AXSelectedText` at an exact range (`select` = `[start, length]`, `all`, `end`, `start`, or the caret) → the whole value rewritten with the edit applied. Never arrow keys |
| key | a ⌘ shortcut runs the menu item it maps to (`accessibility.menu_item_for_shortcut`), and is refused with the Accessibility alternative while the app reports that item disabled (a background app often has no key window: the key would only beep). `escape` cancels the open sheet, dialog, popover or menu with `AXCancel`, never blind. Other keys are posted to the process and judged by what changed; an unchanged window is still reported sent when the app is known to take keys. Arrow keys in a text box are refused (use edit); Return where an enabled Send button exists, or in a messaging app, is refused (press the button). ⌘Tab, Control+arrows, ⌘Space, F3 / F4 (`input.switches_apps`) and ⌘Q / ⌘H / ⌘M (`input.quits_or_hides`) are refused |
| scroll | `AXScrollToVisible` on the element → `AXIncrement` / `AXDecrement` on a slider or stepper → Page Up / Down keys with the scroll area's content focused → the scroll bar's `AXIncrement` / `AXDecrement` or `AXValue` → foreground wheel |
| select (a pop-up button) | `AXPress` the button, then `AXPress` the menu item by title; `AXValue` where settable |
| click at a point (`mac.click`) | the app's own hit test turns the point into an element (`accessibility.hit_element`; it answers the menu bar for a window on another Space, which doesn't count) and presses it → a click posted to the process, only for a window on the current Space, kept if the window changed → foreground |

Every Mac action runs under the GUI lock (`ferb/act/gui_lock.py`) so two workers' keystrokes never interleave.
The background event path targets the app's process and does not wait for the owner to be idle.

**The learned per-app profile** (`ferb/act/mac/profile.py:Profiles`, `~/.ferb/device/apps/<bundle>.json`): for
each mechanism (`ax_press`, `ax_value`, `ax_selected_text`, `ax_focus`, `ax_keyboard`, `pid_keys`,
`pid_keycodes`, `pid_mouse`, `pid_scroll`, `menu_press`, `hit_test`) a count of landed and failed outcomes, the
failure streak and the last outcome, written by every real action. A mechanism that failed 3 times in a row is
skipped (`Profiles.skips`) until a success is recorded, for example by a new probe seed; there is no timed retry
for skipped mechanisms. `prefers` and `untried` steer the Accessibility keyboard rung.
`ferb.act.mac.probe --seed` writes a probe result into it (`ferb/act/mac/probe.py`, allowed targets TextEdit,
Calculator, Finder only; it never quits an app). Nothing per app is written by hand.

**The foreground path** (`driver._foreground` → `_real`) runs only when the call carries
`allow_foreground=true`; otherwise the result says what was tried and that the owner's screen would move
(`FOREGROUND_HINT`). It is the only way any `mac.*` call brings an app in front. `_real` waits until the owner is
quiet, brings the app forward once per stretch of work (`_forward`: remembers the owner's app, engages the input
guard, activates the app, raises the captured window for a click), posts real events through the HID tap
(`input.RealInput.run`: one event per step, 20 ms apart), and schedules the owner's app back after 3 s without
foreground work (`STRETCH_IDLE_S`, `restore_owner`). The owner wins: `RealInput` checks the owner-input tap before
each step and waits while they are active (quiet period 1.5 s, `sense/watchers/owner_input.py:OwnerInput`); if that
takes over 30 s the action stops and reports where (`input.Yielded`; the gateway treats it as outcome unknown).
While FERB's app is forward a filtering tap (`owner_input.InputGuard`) holds back the owner's key presses and
clicks so none lands in FERB's app, and their first touch puts their app back at once; mouse movement only makes
FERB step aside. Real input is refused while `ferb pause` is on (no tap to yield with).

**Screenshots** (`driver.screenshot`, `ferb/act/mac/screenshot.py`): the app's front window only, through
ScreenCaptureKit's desktop-independent window filter (never the rest of the screen, never windows on top; a window
on another Space can be captured), JPEG in memory, long side ≤ 1568 px, never written to a file. Refused for
excluded apps and when the window's Accessibility tree shows a password field before or after the capture; for a
browser window the extension is asked whether the active tab at those bounds has a password field
(`Daemon._web_password`, extension op `window_password`). The frame reaches the model only. `mac.screenshot` and
`mac.click` on a browser with a paired profile connected need a `fallback_reason` (`tools._Base._extension_first`).

**AppleScript** (`mac.applescript`, `ferb/act/mac/applescript.py`): one scriptable app per script (scriptability
from the device profile, `ferb/device/scan.py:read_app`), wrapped in FERB's own `tell application id` block.
Refused: `do shell script`, `do script` (Terminal), run / load / store script, any `tell`, System Events, `using
terms from`, `osascript`, `curl`, raw event codes, `activate`, `reopen`. 30 s timeout. macOS asks for Automation
per app on first use.

**Untrusted apps** (`driver.UNTRUSTED_APPS`): a snapshot or AppleScript result from Mail, Messages, WhatsApp,
Slack, Discord, Telegram or Teams counts as untrusted content and owner-account data; a browser window counts as
web content (`tools._sources`).

---

## 7. Browser control

Two tool families over two drivers (`ferb/act/browser/tools.py`): `browser.*` for owner tasks over the owner's
paired Chrome and Brave profiles (`ext_bridge.Router`), and `ferb_browser.*` for ferb tasks over FERB's identity
profile (`ext_bridge.FerbIdentity`). Verbs: open, navigate, snapshot, act, wait, close, scroll, find; the owner
family adds `tabs` and `use_tab`; the FERB family adds `fill_secret`, `fill_otp` and `mailbox.send`. Only http(s)
URLs to public hosts open (`tools.check_url`). A task sees only the tabs it opened or took over
(`_TabDriver.owned_by`); its tabs close when it ends, except a taken-over tab, which FERB lets go of and never
closes.

**Profiles and pairing.** Discovery reads each browser's `Local State` only (`profiles.discover`: folder, display
name, email; never cookies). A profile name the owner gives resolves by display name, email or local part, or a
saved alias; several matches ask (`profiles.resolve`). Pairing (`ferb/act/browser/pairing.py:Pairings`,
`ferb browsers pair`): the daemon issues a one-time token (5 min) for (browser, folder, identity), opens the
extension's own blank pairing page in that profile with the token in the URL fragment (`pair.html`, `pair.js`
hands it to the service worker and closes), and the extension's `pair` message binds its install id to the profile
and receives a per-install HMAC key. From then on every message carries a strictly increasing `seq` and an
HMAC-SHA256 over install id, seq, type and body (`ext_protocol.authentic`); a replay or bad MAC is dropped. An
unpaired install may only say hello and pair; it can't act or write trail events. A hello with an older protocol or
version than `MIN_EXTENSION_VERSION` (0.8.0) is refused until the extension is reloaded. If the extension isn't
installed in a profile (`profiles.has_extension` reads its preferences on disk), pairing opens that profile's
extensions page and `ferb/cli/admin.py:_guide_install` walks the owner through Load unpacked (Chrome 137+ allows
no other way), then pairs. The identity profile is paired as `ferb` by `ferb setup --identity` after
`ferb/identity/seed.py:verify` confirms it holds exactly FERB's Google account (`Daemon._identity_pair`).

**Connections** (`ext_bridge.Hub`, `Bridge`): one bridge per paired profile; a new connection bumps the
generation; commands carry it and a command id; the extension answers an old generation "stale" and a seen id
"duplicate", and records an id as started before running it (`background.js:onCommand`, in `storage.session`).
A command in flight when the connection drops or times out (30 s) raises `OutcomeUnknown`; the next hello's seen
and owned lists settle it (`Bridge._reconcile`: done → done, failed → failed, never seen → failed, started/unknown → stays unknown; never
resent). A profile whose extension is asleep is woken by loading it with no window (`profiles.open_known_profile`:
`open -g -b … --no-startup-window`, or the binary when the browser already runs) and, only if that doesn't connect
within 8 s, a unique blank local wake page (`ext_bridge.WakeServer`) closed once FERB's own tab is open. If a
browser took the front from the owner anyway, the owner's app is activated again (`Daemon._put_owner_back`,
journaled as `browser.focus_returned`).

**Choosing the profile** (`ferb/act/browser/resolver.py:Resolver.choose`): a profile the owner named wins and
must be paired; otherwise a paired profile that already has the site open (several browsers: `[browser]
prefer_for_actions`; several profiles in one: ask), then the profile where the owner last used that site in the
trail (owner rows only, `Trail.owner_site_profile`), then the default browser's last-used profile if paired, else
ask. The browser or profile the owner named for the request is enforced: `Router.open_for`, `open_tabs` and
`attach` refuse any other (`Daemon._named_for_task`). FERB's identity profile is never used for an owner task.

**FERB's window and tabs** (`background.js:ferbWindow`, `commands.open`): one FERB window per profile, created
`state: "minimized", focused: false` with a blank placeholder tab and kept for the browser session (re-minimized
if the owner restored it); every task tab is created in it with `active: false`, opted out of Memory Saver, and
grouped under "FERB". FERB never shows, focuses or restores that window. `tabs` lists the owner's open tabs
without FERB's window; `use_tab` takes one over (its real viewport is kept).

**The debugger session** (`background.js:attach`): on a task tab's first action `chrome.debugger.attach` (CDP
1.3), then `Page.enable`, `Emulation.setFocusEmulationEnabled true` (the page sees itself visible and focused;
input lands only with it), `Target.setAutoAttach {flatten}` (the one `Target.*` call allowed, for out-of-process
frames), `Emulation.setDeviceMetricsOverride 1280×900` for tabs in FERB's own window, then a readiness probe (a
trusted `mouseMoved` must be seen in the isolated world, `content.js:probeArm`). The session stays attached for
the tab's life and is detached on close. The owner pressing Cancel on the "started debugging" bar sends
`event detached {reason: canceled_by_user}`; the act in flight becomes outcome unknown and the bus publishes
`browser.debugger_detached`. While a task tab is open the daemon pings the extension every 15 s
(`Bridge._keepalive_loop`) so the MV3 worker (30 s idle limit) stays alive. The bar is invisible in a minimized
window and in FERB's identity profile (no owner windows); in an owner profile it shows in their windows while FERB
is attached (`docs/PLATFORM_BROWSER.md` 2.1).

**Input** (`background.js:trusted`, `protocol.js:keyEvent`): real input by default for click, dblclick,
rightclick, hover, type and press (`ext_bridge.TRUSTED_KINDS`), dispatched by `Input.dispatchMouseEvent` /
`dispatchKeyEvent` / `insertText`, so every DOM event is `isTrusted`. A click is move, press, release at the
element's centre in top-viewport coordinates (child-frame rects are composed from the iframe elements' rects,
`composeFrames`). Type: a real click to focus if editable, the active element checked in the isolated world
(never typed into whatever else has focus), ⌘A to replace, then `Input.insertText` (per-key `dispatchKeyEvent` on
request). Keys carry `key`, `code`, Windows and native key codes, modifiers (Alt 1, Ctrl 2, Meta 4, Shift 8) and
the mac editing `commands` the renderer needs for shortcuts (⌘A `selectAll`, ⌘← `moveToBeginningOfLine`, ⌫
`deleteBackward`, …; `protocol.js:MAC_COMMANDS`, allowlisted in `EDIT_COMMANDS`). `page_mode=true`, `select` and
`scroll` use scripted (untrusted) events in the isolated world (`content.js:act`); a page the debugger can't attach
to, or a frame whose offset is unknown, returns a failure with its diagnostic. Synthetic page mode is explicit.
Covered/disabled controls and unconfirmed keyboard focus are refused. AX targets use the same DOM references and checks.

**Snapshots** (`content.js:snapshot`, composed in `commands.snapshot`, shaped by
`ferb/act/browser/snapshot.py:for_model`): a DOM walk in the isolated world of every frame, open shadow roots
included; up to 400 elements, in-view ones kept first; each with role, label (aria, labels, placeholder, title,
alt, text), value (password and one-time-code fields: filled or empty only), rect, `in_view` (meets the viewport
and `elementFromPoint` at its centre hits it), `offscreen` above / below / left / right, `occluded`, `container`
(its nearest scrollable ancestor), `form` search / login / other, href (tokens masked), state and heading level;
the page's `scroll` and a `scrollers` list (id, label, y, height, viewport height, at_end); the visible text
nearest the viewport in document order (8,000 chars); dialogs dismissed since the last snapshot. `ax.js:accessibilitySnapshot` reads Chromium's AX tree on every snapshot, taking roles, names and ordering
from AX while preserving the isolated DOM walk's redacted values, forms and hit testing. Backend nodes are bound
to isolated-world element references using random `data-ferb-ax` markers and narrowly allowlisted DOM methods.
Additional AX nodes are adopted into the same DOM reference map; there is no separate blind-coordinate AX actuator.
CDP frame-tree/owner identities replace URL/name matching. Child debugger sessions are tracked and recursively
auto-attached; fresh frame-owner rects are read for input. If AX fails, the DOM view carries an explicit diagnostic.
Frame text is included in the daemon snapshot, so an embedded login dialog is not lost behind top-page text. Ids `s<gen>e<n>` go stale after a newer snapshot
or a navigation (`_TabDriver._resolve`). Password values never leave the page.

**Scrolling** (`commands.scroll`): a trusted `mouseMoved` then `mouseWheel` at the centre of the window or of a
container from the latest snapshot, about 85 % of its height, position read until it settles; `at_end` says
whether it reached the end. `find` counts and shows lines of the whole page's text containing a string. `wait`
waits (≤ 30 s) for load, text or URL and returns `met: false` on timeout.

**Dialogs** (`chrome.debugger.onEvent` on `Page.javascriptDialogOpening`): a background tab's dialog is invisible
to the owner, so the extension answers it: dismissed unless the act declared `expect_dialog: accept` (with
`dialog_text` for a prompt); `beforeunload` is accepted only while FERB itself navigates or closes the tab. Each
dialog is reported as an event (`browser.dialog` on the bus) and in the next snapshot.

**Uploads** (`commands.upload`): the element is stamped with a random `data-ferb-node` token in the isolated
world, found by `DOM.querySelector` and filled with `DOM.setFileInputFiles` (absolute paths of existing files,
checked in the daemon); top frame only. **Screenshots** (`commands.screenshot`): `Page.captureScreenshot` JPEG
quality 70, in parts under the 1 MB native-messaging line. This is exposed on `_TabDriver.screenshot` and
exposed to workers by `browser.screenshot` / `ferb_browser.screenshot`. A `point` resolves image coordinates to a
fresh element id without clicking; `act` then runs the normal policy and account gates. Shot references are tab-bound,
one-use, expire after 30 seconds and invalidate on newer snapshots/navigation. Capture refuses sensitive or truncated inventories.

**The CDP allowlist** (`protocol.js:CDP_ALLOW`) is exactly: `Page.enable`, `Emulation.setFocusEmulationEnabled`,
`Target.setAutoAttach` (that shape only), `Emulation.setDeviceMetricsOverride` (1280×900 only),
`Input.dispatchMouseEvent`, `Input.dispatchKeyEvent`, `Input.insertText`, `Page.handleJavaScriptDialog`,
`Page.captureScreenshot`, `Accessibility.getFullAXTree`, `DOM.getBoxModel`, `DOM.getDocument` (depth 0),
`DOM.querySelector` (the FERB stamp only), `DOM.setFileInputFiles`, `Page.getFrameTree`, `DOM.getFrameOwner`,
`DOM.describeNode` (depth 0/1), `DOM.pushNodesByBackendIdsToFrontend` (one backend id), `DOM.setAttributeValue`
(only the random FERB AX binding marker); each with a parameter-shape check, and return
values filtered to named fields (`CDP_RETURN_FIELDS`). Cookies, storage, free-form script evaluation and other
targets are refused in the extension.

**Account check before consequential actions in owner profiles** (`Router.check_account`,
`ferb/act/browser/accounts.py`, `content.js:ACCOUNT`): for sites with an extractor (Gmail, YouTube) and a profile
whose Chrome account is known, the page's signed-in account must equal the profile's; YouTube's is read once per
connection from its account page in a short-lived background tab; a mismatch or a signed-out page refuses the
action. Other sites can't be checked, so policy asks once per site and profile and remembers the yes.

**FERB's identity profile and sealed fills** (`ext_bridge.FerbIdentity`): a dedicated profile in the owner's
Chrome that holds only FERB's Google account (`[identity] browser / profile`, `[mailbox] address`). `fill_secret`
checks the target frame's origin (extension op `frame_origin`) against the secret's allowed origins before the
Keychain is read, seals the value (`ferb/identity/seal.py`: AES-GCM, key HKDF-SHA256 from the per-install pairing
key with label "ferb seal v1", fresh nonce, the fill command id and the origin as associated data), sends it to the
extension, which unseals it in the service worker for that command id and origin only (`protocol.js:unseal`) and
hands it to the isolated world of that frame, where `content.js:fill` re-checks `location.origin` in the same call
that sets the value on that exact element. The value is added to the redactor so a page echoing it is scrubbed.
`fill_otp` waits up to 2 minutes for a fresh code in FERB's inbox (Gmail's Atom feed of unread mail read in a
private background tab of FERB's profile, op `read_source`, the feed sealed back to the daemon;
`ferb/identity/mailbox.py`: one 4–8 digit code, refused when the newest message holds several or the inbox is
signed out) and fills it the same way; a code is never shown to a model, logged or stored. `mailbox.send` sends
from FERB's Gmail through its compose page with a real click on Send, confirmed by Gmail's "Message sent" toast or
by the message appearing in Sent; no confirmation leaves the ledger `unknown` (never sent twice). Owner profiles
never get FERB's secrets (`Router.fill_secret` refuses).

**Safari** is read-only: no extension, no acting; its front tab is recorded by the tab watcher through AppleScript
only when Automation is already granted (`sense/watchers/browser_tabs.py`), and its window can be read through
Accessibility. Firefox and other browsers are not supported.

---

## 8. Shell and files

**Shell** (`ferb/act/shell/tools.py`, owner identity, scope `shell`, lost when exposed): `shell.run` runs
`/bin/sh -c` in a pseudo-terminal (`pty_proc.PtyProc`) in a folder inside the allowed roots (default FERB's
workspace), waits up to `timeout_s` (≤ 120) and returns the last 8,000 chars with byte offsets; `shell.read` pages
the whole output from any offset; `shell.send` writes to its input; `shell.kill`, `shell.list`. Processes belong to
the task and die with it unless `background=true` (`manager.ShellManager.end_task`); `ferb ps` / `ferb kill`
list and end them; `ferb stop` kills them all. The environment is clean (`CLEAN_ENV_KEYS` only, `TERM=dumb`, no
FERB tokens) with Homebrew and user bins added to launchd's bare PATH. Output is kept in a 256 KiB ring and
appended, redacted, to `~/.ferb/logs/shell/<id>.log`. Refused outright (`guard.REFUSED`): Keychain `security`
subcommands, anything naming `.ferb` / `FERB_HOME` / `ferb.db`, `osascript`, `launchctl`, and the protected
locations below. Destructive commands (`rules.DESTRUCTIVE_SHELL`: `rm -rf`, `git push --force`, `git reset --hard`,
`git clean -f`, `mkfs`, `dd of=/dev/`, `diskutil erase`, `sudo`, fork bombs, writes to raw disks, `chmod -R 777`,
`srm` / `shred`) are kind `delete_permanent` and always ask. Network-fetching commands (`guard.fetches_network`:
curl, wget, ssh, git clone / fetch / pull, and the like, on each pipeline segment's command word) mark their output
untrusted, so running or reading one exposes the worker. Interactive terminal programs run here; `ferb trace`
renders their screens as plain text.

**Files** (`ferb/act/fs/files.py`, both identities): `fs.list`, `fs.read` (byte ranges, ≤ 2 MiB), `fs.write`
(atomic, `overwrite` required to replace), `fs.edit` (one exact occurrence), `fs.trash` (macOS Trash, never
permanent). Roots: FERB's workspace always; the owner's folders from `[fs] owner_roots` for owner tasks only.
Relative paths resolve in the workspace, except `Desktop/…`, `Documents/…`, `Downloads/…`, which are the owner's;
a name Finder shows with "/" is found by its on-disk ":" form (`finder_name`). `..` is refused; symlinks must
resolve inside a root; FERB's own home (outside the workspace) and `PROTECTED` (`~/.ssh`, `~/.gnupg`, `~/.aws`,
`~/Library/Keychains`, `~/.codex`, `~/.claude`, `~/.config/gh`) are refused even inside a root. Reads from
`[fs] untrusted_roots` (default `~/Downloads`) count as untrusted content.

---

## 9. Verification, ledger and reconciliation

**The ledger** (`ferb/orchestrator/ledger.py:Ledger`, table `action_ledger`): before a tool of a non-reversible
kind runs (`rules.REVERSIBLE` = read, navigate, search, open_tab, draft, trash, timer are exempt), the gateway
writes a row `intended` → `executing` with the target and the payload hash; if the write fails the action doesn't
run. The tool's result marks it `done` or `failed`; an exception marks it `unknown`. While an identical action
(same task, tool, payload) is `unknown`, a new intent for it is refused (`DuplicateUnresolved`): a lost
acknowledgement after Send can't become a second send. After a daemon restart every `intended` or `executing` row
becomes `unknown`. Browser drivers tag commands with the ledger id so the extension's seen / owned lists can settle
them (section 7).

**Reconciliation** (`Supervisor._reconcile`): when the worker finishes and its task has `unknown` rows, a
read-only run (`prompts/verifier.md`, read scopes only) re-reads the target and answers yes / no / unclear with
evidence; yes → `done`, no → `failed`, unclear → stays unknown and the outcome is `needs_user`. Never retried.

**Verification** (`Supervisor.verify`, `ferb/orchestrator/verify.py:Checks`): after a `done` outcome with success
conditions, each condition with a code-checkable form is checked by code: `file_exists`, `file_contains` (through
the fs resolver), `ledger_done` (legacy: always requires a fresh outcome read; completed input is not proof of the goal), `tab_url` (a task
tab's URL contains the text), `account` (the signed-in account on a task tab). Conditions needing judgement go to
one read-only verifier run (`.verify` task, read scopes plus `fs:read`) that must see evidence itself; the worker's
report is passed as data. A check code can't decide goes to the verifier instead of immediately failing the task. `_final`: an unmet
condition turns the outcome `failed` with the reason; an undecided one keeps `done` and says "I couldn't confirm";
`verified` requires a done worker, at least one success condition, and every supplied condition confirmed.
A report with no conditions stays unverified. The planner must express the requested result rather than an attempted action.

---

## 10. Sense (the trail), memory, time, identity secrets

**The trail** (`ferb/sense/trail.py:Trail.record`), one path for every observation in a fixed order: paused →
nothing; excluded (`sense/exclusions.py`: `[privacy] exclude_apps`, `exclude_domains` with subdomains,
`exclude_paths` default `~/.ssh`, `~/Library/Keychains`, `~/.ferb`; incognito always) → dropped completely (no
row, no event, no count); redaction; id and actor (`Attribution.decide`: owner, ferb or unknown; FERB's own app
switches, the paths its file tools touch, the tabs it opened and everything in its identity profile are `ferb`; a
file change under a running FERB shell's folder is `unknown`); persist to the append-only `trail` table; publish
`trail.event`; only then Jev tagging. Metadata only: watchers (`sense/watchers/`) report the frontmost app
(Accessibility's focused application, every 0.5 s), focused window titles (not browsers; `[sense] window_titles`),
browser tabs (extension tab events: page loads and tab switches, host, masked URL, title; AppleScript for Safari
and for Chrome / Brave with no connected profile, only when Automation is granted), file changes in
`[sense] watch_folders` only (FSEvents; path and kind of change), idle / active transitions (from the owner-input
tap, `[sense] idle_after_s` 120), app installs and removals (rescanned into the device profile). Never keystrokes,
clipboard, file contents or screen content. `ferb pause` stops every watcher and the owner-input tap, drops what
FERB knew about the owner's tabs, refuses real input, and survives restarts (`~/.ferb/PAUSED`); `ferb resume`
starts them again. Retention: raw rows are kept `[sense] trail_retention_days` (30); a day is purged only once its
permanent summary is in the journal (`Trail.purge`). Consumers: the resolver, distillation, `trail.query` (owner
tasks, read only; reading browser or window rows exposes the worker since titles are set by sites).

**Memory** (`ferb/memory/items.py:Memory`, SQLite is the source of truth; `~/.ferb/memory/*.md` is a generated
view, `memory/export.py`): items have kind (fact, preference, person, project, env), subject, key, content,
origin, status, validity and sources. Origins in code: `owner_stated` (chat capture with a verified quote,
`ferb memory add` / `edit`) is active at once and supersedes earlier versions; `observed` needs only trail rows
with actor owner and activates after evidence on 3 separate days; `inferred` waits for `ferb memory confirm`;
`third_party` can be a fact about the world, never a preference or about the owner. Nothing about the owner may
rest on FERB's own activity. Versions: a newer item closes the old (`valid_to`, `supersedes`); only the owner's
statement supersedes an owner statement, otherwise both stay and recall flags the conflict. `forget` purges the
item, its versions, items built on the same sources, the sources themselves (unless `--keep-sources`), rebuilds or
deletes the day summaries affected, the FTS index and the WAL, and keeps a one-way fingerprint so distillation
can't propose it again. Recall is FTS5 keyword search. Distillation (`memory/distill.py:Distiller`, hourly
`Daemon._distill_loop`): for each past day, the permanent summary by code first (`memory/summary.py:build`: time
per app, sites, folders, titles, idle, the owner's chats and FERB's tasks as a separate section, with evidence
handles E owner activity / C owner's words / P pasted text / F FERB's own), then one model run (`distill.md`,
supervisor mode, no tools) proposing a narrative and items citing handles, checked in code (`distill.check`), then
the purge. Three attempts per day.

**Time** (`ferb/clock/`): wall time for timestamps and schedules, monotonic for timeouts and guards. Timers
(`timers.py:Timers`, table `timers`) are durable: checked against the wall clock at least every 30 s, so a timer
due during sleep fires on wake with `overdue_s`; delivered exactly once (claimed with a token, acknowledged, and a
consumer record `timer_deliveries` so a redelivery after a crash is a no-op). Kinds: reminder, timer, sleep,
schedule. Delivery (`Daemon._timer_fired`): a macOS notification (osascript), chat (live, or on next opening),
`ferb logs`, the journal. Tools `time.remind_at`, `time.set_timer`, `time.sleep_until` (a durable sleep within the
task's limit, counted as waiting; cancelled by a restart). The model never computes a date: it fills `when`
(`in_minutes`, `in_days`, `day` incl. `next_<weekday>` and `end_of_month`, `date`, `time`) and `when.resolve` does
the rest in the Mac's zone; the owner's own meanings of fuzzy words are stored as `time:<phrase>` preferences and
listed in the supervisor's facts. Non-urgent notifications are held while the owner is idle (`Daemon._notify`).

**Identity secrets and mailbox** are in sections 3 and 7. `ferb/identity/seed.py` finds and verifies FERB's Chrome
profile from `Local State` and `Preferences` only.

---

## 11. Storage, bus, journal, spans, redaction

**Home** (`~/.ferb`, or `$FERB_HOME`; `ferb/paths.py`), mode 0700: `config.toml` (owner-only, written atomically
with comments kept, `ferb/config.py`), `ferb.db` (SQLite, WAL, `secure_delete`, mode 0600, `ferb/store/db.py`),
`run/ferb.sock`, `run/token-*` (worker run tokens, 0600, deleted after the run), `run/codex-models.json`,
`workspace/` (FERB's files), `logs/daemon.out.log`, `logs/daemon.err.log`, `logs/shell/`, `memory/*.md`,
`device/<device_id>.json` (the device profile with a local git history, no remote: `ferb/device/profile.py`),
`device/apps/<bundle>.json` (learned app profiles), `watch/` (`ferb watch` results), `FERB.app`,
`bin/ferb-native-host`, `python/`, `venv/`, `node/`, `codex-home/`, the markers `.ferb-home`, `STOPPED`, `PAUSED`.

**Tables** (`ferb/store/migrations/`): `spans`, `journal` (append-only by trigger), `action_ledger`,
`browser_profiles` (defined in 0003 but not used by code: discovery reads `Local State` live), `ext_installs`,
`tasks`, `trail` (append-only for what / when / who), `memory_items`, `memory_sources`, `memory_fts`,
`memory_forgotten`, `distill_runs`, `decisions`, `timers`, `timer_deliveries`, `task_results`, `schedules`,
`owner_questions`.

**Config keys read by code**: `[device] id`; `[runtimes] orchestrator`, `worker`, `claude_model`; `[policy]
autonomy`, `trusted_kinds`, `trusted_accounts`, `pasted_text_is_data`; `[fs] owner_roots`, `untrusted_roots`;
`[privacy] exclude_apps`, `exclude_domains`, `exclude_paths`; `[sense] window_titles`, `watch_folders`,
`idle_after_s`, `trail_retention_days`; `[mac] snapshot_image`; `[browser] extension_ids`, `prefer_for_actions`;
`[identity] browser`, `profile`; `[mailbox] address`; `[secrets] <key> allowed_origins / account_hint`; `[jev]
enabled`; `[voice] push_to_talk`, `requirement`, `mic`, `sounds`; `[gates] owner_profiles` (only changes the
wording of the "installed but unpaired" note, `Daemon._unpaired_note`; pairing itself is not gated in code).

**The bus** (`ferb/bus.py`): in-process, bounded per-subscriber queues, the publisher never blocks; an unknown topic
is a programming error. Critical topics are never dropped (a subscriber that can't take one is disconnected with
an overflow): `task.created`, `task.started`, `task.finished`, `task.retried`, `approval.requested`,
`approval.resolved`, `device.changed`, `browser.profile_paired`, `timer.fired`, `memory.committed`,
`policy.blocked`, `worker.exposed`, `action.held`, `action.released`, `action.undone`, `stop.engaged`,
`stop.lifted`, `sense.paused`, `sense.resumed`, `sense.watcher_failed`, `contact.selected`, `plan.extended`
(listed; nothing publishes it since the rework), `voice.heard`, `voice.said`, `browser.debugger_detached`.
Telemetry may be dropped oldest first: `tool.called`, `trail.event`, `browser.connected`, `browser.disconnected`,
`browser.profiles_changed`, `memory.proposed`, `browser.dialog`. `ferb logs` subscribes with filters
(`Daemon.LOG_FILTERS`).

**Journal** (`ferb/memory/journal.py`): append-only, redacted, from the first run: `daemon.started`,
`chat.message`, `chat.reply`, `chat.clarify`, `chat.failed`, `task.repair`, `task.answer`, `task.outcome`,
`task.stopped`, `approval.resolved`, `contact.selected`, `action.undone`, `timer.*`, `schedule.*`, `memory.*`,
`trail.day_summary`, `trail.day_narrative`, `trail.purged`, `sense.*`, `voice.*`, `browser.focus_returned`,
`runtime.unregistered`, `stop.*`, `setup.folder_granted`, and more. `ferb timeline` renders it.

**Spans** (`ferb/store/spans.py:SpanStore`): `task`, `worker`, `model.turn`, `model.usage`, `tool.call` (input:
tool, args, effect, why; output: the redacted result or the error). `ferb trace` renders the tree of a task and its
`.worker` / `.verify` / `.reconcile` steps.

**Redaction** (`ferb/sanitize/redact.py:Redactor`), applied by the journal, spans, trail, memory, the bus
publisher, tool results, shell logs and exception text: values under secret-named keys (password, token, cookie,
otp, session, …; FERB's own id fields exempt), password / one-time-code field values, text under CDP `Input.*`
params, known secret patterns (private keys, API keys, JWTs, bearer tokens), secret query and fragment parameters
in URLs, URL passwords, excluded domains and apps (replaced by `[excluded]`), and exact secret values added at fill
time. Runtimes keep no transcript on disk.

---

## 12. Install, permissions, setup, doctor, voice

**Install** (`ferb/device/install.py:Installer`, `ferb setup`): a dedicated Python 3.12 under `~/.ferb/python`
(uv-managed; TCC grants attach to it), a venv with a copy of FERB (never a link to the repo; `ferb setup --update`
reinstalls and restarts), the pinned Codex in `~/.ferb/node`, the native-host launcher and manifests, the
LaunchAgent plist, loaded with `launchctl bootstrap`. Setup then scans the Mac's apps from their `Info.plist`
(`ferb/device/scan.py`; scriptability from `NSAppleScriptEnabled` or an sdef; browsers present) into the device
profile, asks for **Accessibility** and **Screen Recording** (required; the daemon checks them on itself,
`ferb/device/permissions.py`; the grant goes to the venv's interpreter path setup prints; Screen Recording needs a
daemon restart to show), Input Monitoring only if the owner-input tap can't be made, and runs the self-test
(`ferb/device/selftest.py`: the Dock's AX tree readable, a listen-only tap can be made, a marked no-op mouse event
is seen). Everything else is asked when first needed: folder access, the owner's email, Automation per app (the
system prompt, requested from the daemon so the grant attaches to it), FERB's identity, Jev, voice.

**Doctor** (`ferb/cli/setup.py:doctor`): home marker, Codex home, config permissions, interpreter, venv,
LaunchAgent plist and load state, the daemon's executable (a dev daemon is flagged), installed code hash versus the
checkout, each permission, each self-test, push-to-talk, runtimes (pinned versions, unregistered ones), the
identity profile and session (through the extension), secrets in the Keychain, native-host manifests, paired
profiles' connection, the emergency stop, watchers and failed ones, Jev, watch folders, the device profile's drift.

**Uninstall** (`ferb uninstall [--purge]`): removes the LaunchAgent and native-host manifests; `--purge` deletes
`~/.ferb` after the owner types `delete` (refused without the `.ferb-home` marker) and offers to delete FERB's
Keychain items. macOS permission entries must be removed by hand.

**Voice** (push-to-talk; `ferb setup --voice`, `ferb/voice/`): FERB.app is built by `macapp/FERB/build.sh` and
signed with the local certificate "FERB Local Code Signing" so its Microphone and Speech Recognition grants
survive rebuilds; its designated requirement is recorded in `[voice] requirement` and checked on every voice
connection. The talk key is Right Option, read from the daemon's owner-input tap (`voice/push_to_talk.py:TalkKey`):
a hold under 0.3 s is a tap and ignored, another key while held makes it a shortcut and the recording is
discarded, release sends. The daemon picks the microphone from the device list FERB.app reports, never a
Bluetooth one (`voice/mic.py:choose`), tells FERB.app to start (a chime once the hold is real, `[voice] sounds`),
and runs the transcript as a chat turn in session `voice` (`voice/controller.py:Voice.transcript`): a spoken yes /
no / number answers the oldest waiting question, "stop" cancels a send or call countdown, anything else is a
request. FERB's own words are spoken with the Mac's `say` (`voice/speak.py:SaySpeaker`); a new press cuts it off.
Speech recognition is on-device (`SFSpeechRecognizer`); audio is never written anywhere. `ferb voice on|off`
switches it without a daemon restart (off quits FERB.app).

---

## 13. CLI reference

All commands are dispatched by `ferb/cli/__init__.py:main`. Chat and most admin commands use the socket as role
`cli`; `stats` reads SQLite directly, `watch` runs local probes, and setup/doctor/uninstall/device also do local work.
`daemon run` starts the service; `mcp` connects as `mcp_shim`.

| Command | What it does |
|---|---|
| `ferb chat [--verbose] [--speak]` | Talk to FERB (`cli/chat.py`): a prompt_toolkit line editor, Option-Enter for a new line, pastes kept whole and marked as data (a whole-message paste asks whether it's the owner's request), questions answered inline (y/N, a number, or free text for a worker's question), undo countdowns, answers that arrived while chat was closed, reminders delivered live. Ctrl-C stops the running task; closing chat doesn't. `--verbose` shows the plan, the worker, tools, blocks and checks; `--speak` says FERB's words aloud |
| `ferb logs [--tasks --trail --tools --browsers --policy --actions --timers --voice]` | Live, redacted stream from the bus (`cli/logs.py`) |
| `ferb trace [task] [--full]` | What a task did step by step, from its spans: each tool call with its arguments, effect, why and result; the latest task by default, a short id works (`cli/admin.py:render_trace`) |
| `ferb stats [--days N]` | Task outcomes, tool errors by reason, model runs, read straight from the database (works without the daemon) |
| `ferb watch [--mac] [--browser] [--only M4,M9] [--dry-run]` | The owner-verified background test (section 14) |
| `ferb stop` / `ferb resume-tasks` | Emergency stop: block the gateway, revoke every run token, cancel held sends, cancel runtimes, shut browser tabs and task processes (`orchestrator/stop.py:StopControl`); survives restarts / lift it |
| `ferb undo [hold_id]` | Cancel a pending send during its undo window |
| `ferb autonomy [careful\|balanced\|hands-off]` | Show or set the level |
| `ferb pause` / `ferb resume` | Stop / start every watcher (nothing recorded, no real input while paused) |
| `ferb browsers [pair\|unpair [--browser chrome\|brave] <profile>]` | List profiles with pairing and connection state; pair (guiding the extension install when missing); unpair |
| `ferb reminders [cancel <id>]` | Pending reminders and timers |
| `ferb schedules [list\|cancel\|pause\|resume <id>]` | Scheduled and recurring tasks |
| `ferb memory list [--proposed\|--all] \| show [subject] \| add <content> [--kind --subject --key] \| confirm <id> \| edit <id> <content> \| retract <id> \| forget <id> [--keep-sources] [--yes]` | Memory |
| `ferb timeline [--since YYYY-MM-DD]` | The journal |
| `ferb decide stats \| enable <question> \| disable <question>` | Jev's answers next to FERB's; enabling a question (asks for confirmation when FERB has no logic of its own there) |
| `ferb device [show\|scan]` | The device profile, with live browser status when the daemon runs |
| `ferb ps` / `ferb kill <id>` | FERB's shell processes |
| `ferb voice [on\|off]` | Push-to-talk live (what was heard and said), or switch it |
| `ferb setup [--update] [--identity [--profile NAME]] [--jev] [--codex-login] [--voice]` | Install and onboard; reinstall; FERB's identity profile; the Jev key; FERB's own Codex login; push-to-talk |
| `ferb doctor` | Health checks (section 12) |
| `ferb uninstall [--purge]` | Remove the agent; purge the home |
| `ferb daemon run` | Run the daemon in the foreground (development; a separate `FERB_HOME` keeps the installed one running) |
| `ferb mcp` | The stdio MCP shim (used by runtimes only; needs the run token) |

---

## 14. Testing

**The offline suite** (`tests/test_*.py` without the `real` marker, run by plain `pytest`): a real daemon
in a temporary home on a background thread (`tests/conftest.py:RunningDaemon`) with fake runtimes, a fake clock
(`clock.FakeClock`) and sequential ids; no watchers, no distillation, no notifications; a fake extension speaking
the real protocol through the real native host (`tests/fake_extension.py`, modelling the minimized window, the
debugger session, Cancel, dialogs, pings, chunked screenshots, uploads); stub Mac drivers. It proves the contract's
promises rather than coverage: the identity split and scope rules (`test_policy.py`), the effect gate, floors,
approvals by scope key, the hard floor, undo, stop, exposure, data flow and the duplicate-send rule
(`test_policy.py`, `test_gateway.py`), the supervisor's outcomes, repair, guards, stall, reconciliation and
verification (`test_supervisor.py`), the acceptance set that replays eight real failures from the run history with
scripted workers (`test_acceptance_one_worker.py`), the Mac snapshot and profile rules (`test_mac_snapshot.py`,
`test_background_input.py`), the browser bridge and extension behaviour (`test_browser_background.py`,
`test_ext_bridge.py`, including a node run of the real `background.js` against slow storage), the audit fixes
(`test_rework_fixes.py`), contacts, memory, timers, schedules, sense, redaction, the CLI and setup flows. Tests
named after safety promises still cover the 24 invariants of the pre-rework contract (gateway bypass, token
revocation, identity, origin-bound fills, secrets never visible, the CDP allowlist, exposure, data flow, approval
scope, lost acknowledgements, wrong accounts, stale refs, locks, reconnect replay, unpaired installs, forgotten
memory, ferb-actor trail rows, exactly-once timers, the identity profile check, sealed values, window-only
screenshots, exclusions and pause, Jev never approving); the intent-match test became the effect-gate tests.

**Real tests** (`pytest -m real`, Codex only; the Claude variants carry `real_claude` and run only when the owner
runs them): locked down so each test daemon gets only the tools that test needs. The two the rework added:
`tests/test_mac_background_real.py` drives TextEdit and Calculator through the driver (a new document by menu,
typed, edited in place, scrolled by keys, 7 × 6 = pressed, the documents closed through the close button and the
sheet's Delete from snapshots) while a thread polls the owner's frontmost app every 50 ms and asserts it never
changed; `tests/test_browser_lab_real.py` runs a throwaway Chrome with its own data dir (`tests/chromelab.py`,
never the owner's browsers) with the unpacked extension and a local page, and asserts trusted typing, clicks and
keys, page and container scrolls, a confirm() answered, a file upload, a screenshot, and no frontmost change. Other
real tests cover the runtimes' lockdown, the extension through a spare profile, FERB's identity session, contacts,
schedules and the launcher.

**`ferb watch`** (`ferb/device/watch.py`): the owner-verified background test. It runs every interaction type FERB
uses, one at a time, with the target in the background (M1–M13 on TextEdit and Calculator: open with `open -g`,
menu press, `AXValue`, keystrokes to the process, `AXSelectedText`, ⌘A only when its menu item is enabled,
Page Down keys, scroll-wheel and mouse events to the process (experimental), Calculator presses and typed keys,
cleanup; B1–B12 on a throwaway Chrome with a probe extension: windowless launch, a minimized window, an inactive
tab, debugger attach with focus emulation, trusted click, type, Enter, wheel scroll, screenshot, navigate, an
unfocused normal window (experimental), close), and after each asks the owner what they saw. The owner's answer is
the verdict; FERB's own detector (Space changes and app activations from NSWorkspace notifications, the frontmost
app polled every 50 ms, the mouse position) is recorded next to it in `~/.ferb/watch/<stamp>.json`. The owner's
three runs on 2026-10-02: 25 steps, none disturbed them (`docs/PLATFORM_MACOS.md`, "Owner-verified runs").

**Owner acceptance** (`docs/OWNER_ACCEPTANCE.md`): the list of tasks the owner gives FERB through `ferb chat`,
grouped as reading the Mac, acting in apps, browser tasks, files and shell, multi-app workflows, asking and
remembering, and safety, each with what FERB should do and what the owner checks. A task passes only when the
result is right, nothing on the owner's screen moved, and FERB reported the right status. **The rule: the owner's
eyes are the verifier.** Stale tests can't fool a person watching the screen; a step that moves the desktop,
raises a window over theirs or touches their mouse or keyboard fails even if the task completed. Results go in
`docs/OWNER_ACCEPTANCE_LOG.md`, tracked next to `ferb stats`. Sections B–H have not been run yet at 0.19.0
(`PROGRESS.md`).

---

The 0.19.0 recovery contracts, task control, hidden login-code flow, regression evidence and remaining limits are
summarized in `docs/RELIABILITY_019.md`.

## 15. Known gaps and what's next

Not built at 0.19.0:

- **Procedure memory** (`docs/REWORK_PLAN.md` phase 5): "how I did X in app Y" stored after a done task and
  recalled into the worker prompt. Nothing in `ferb/` implements it; the per-app profile learns mechanisms, not
  procedures. The `OWNER_ACCEPTANCE.md` row "repeat the haiku the next day with fewer tool calls" will not pass yet.
- **Safari acting**: read-only (section 7). A Safari extension needs a containing app (FERB.app) and has no trusted
  input anyway (`docs/PLATFORM_BROWSER.md` 8).
- **Firefox**: not supported.
- **The floor word list's false positives**: "Remove from playlist" and "Sort order" hit `remove` / `order` and
  ask every time as destroy / pay; to be refined from the acceptance runs, never with a permission list (`PROGRESS.md`).

Platform mechanisms the driver relies on that are not verified on this Mac (the driver treats each as a rung that
may do nothing and moves on; `docs/PLATFORM_MACOS.md` 13.6, `docs/PLATFORM_BROWSER.md` 11):

- `CGEventPostToPid` mouse clicks and scroll-wheel events to a window on the **current Space but occluded**:
  untested (every probe ran with the window on another Space, where they do nothing). The driver posts a click only
  for an on-screen window and keeps it only if the window changed.
- `AXPress` on Electron app controls through the rebuilt driver: seen in spikes 18 and 20 (WhatsApp), not re-probed
  since; `has_actions_on_groups` and `AXManualAccessibility` on a real Electron app remain to be probed.
- `AXUIElementPostKeyboardEvent` (the Accessibility keyboard): app-dependent (WhatsApp took it, TextEdit and
  Calculator ignore it); it is learned per app, never the default.
- `AXShowMenu` plus pressing items of the resulting `AXMenu`, `AXPopUpButton` select through `popup_menu`,
  `AXScrollToVisible` on table rows, modifier combos posted to a process: unverified on this Mac.
- `IsSecureEventInputEnabled` is not consulted (unverified: no call found under `ferb/act/mac`); a missing
  keystroke in a secure-input app is read as the app ignoring background keys.
- `AXVisibleCharacterRange` is read for scroll positions but not set as a scroll mechanism.
- Chromium: whether `chrome.windows.create` activates Chrome in the owner's own running Chrome (conflicting
  evidence; the frontmost put-back stays as the tripwire); the infobar's linger after detach and one-versus-two bars
  under concurrent attaches; dialog delivery in a background tab of an owner profile; `DOM.setFileInputFiles` on a
  hidden tab in an owner profile; whether an attached debugger prevents freeze or discard after 5 minutes hidden
  (the extension opts task tabs out of discarding).
- The screenshot of a window on another Space through ScreenCaptureKit is documented, not probed.

Other limits:

- Prompt injection is reduced (untrusted marking, exposure, data flow, floors), not eliminated: a single worker both
  reads and acts. Data-flow labels are per worker (anything it types after reading owner data carries the label),
  so a paraphrase is caught but so is unrelated typing to an unnamed site.
- A network fetch exposes the worker, so clone-then-test and browser-read-then-file-write cannot finish through
  shell / file tools in the same worker. The corresponding owner acceptance rows are targets, not implemented
  exceptions to exposure.
- Shell output is untrusted only for network-fetching commands matched by name; a program that fetches some other
  way isn't caught.
- Trusted browser input in child frames depends on the frame offset being composable from iframe rects; otherwise
  page-mode events. Uploads work in the top frame only.
- Focus emulation makes a page believe it is visible while FERB is attached.
- Chrome and Brave expose no web content to Accessibility unless an assistive technology enabled it; a web
  password field in an unpaired profile reads as "can't tell" and the window is captured.
- Learned trust ("FERB offers to stop asking about an action type") is not built; `[policy] trusted_kinds` is
  edited by hand.
- `[gates] owner_profiles` does not gate pairing in code (section 11).
- `plan.extended` is a bus topic nothing publishes; `browser_profiles` is a table nothing uses.
- `ExposureTracker.trusted_sources` (owner-marked sources that don't expose) is never populated.
- The Codex worker runtime is a stopgap: its lockdown depends on Codex's catalog fields and carries Codex's own
  instruction overhead per run; a direct-API runtime is the planned replacement.
- Captchas and human checks stop the task; FERB never hides automation signals (it sets none).
- Crash mid-task: the ledger marks in-flight actions unknown and reconciles; there is no checkpoint / resume.
- Plaintext SQLite (FileVault assumed); the Keychain items are not restricted to a signed FERB binary.

What's next (`PROGRESS.md`): the owner runs `docs/OWNER_ACCEPTANCE.md` B–H on the installed branch and reports;
then procedure memory; then the open probes above; then the floor-word refinements.

---

## 16. Future blocks

Short, so they don't get built by accident:

- **Voice beyond push-to-talk**: an always-on mode (`[voice] always_on`, off by default) with a local wake-word
  detector that records nothing before it fires, pause detection, echo cancellation so FERB can be interrupted, an
  optional speaker check, mic selection for away use; and system-audio listening (`audio.listen`, Core Audio
  process taps in FERB.app, song identification, transcription) with its own `audio` scope and call apps excluded.
  Spikes 9 and 17 proved the capture and on-device speech; nothing of this is in `ferb/` today.
- **Other operating systems and devices**: the brain (supervisor, policy, gateway, memory, traces, timers) and the
  nodes (a device's drivers and watchers) split over an authenticated connection; nodes announce capabilities per
  `device_id`. Rules kept today so this stays possible: every record carries `device_id`; OS-specific code lives
  in drivers, watchers, device setup, voice and platform adapters; the orchestrator and policy import interfaces
  (`ComputerDriver`, `BrowserDriver`, `Watcher`), never the Mac modules.
- **Safari and FERB.app as its extension container**; the stop hotkey; Keychain access anchored to FERB.app's
  signature; the unlisted Web Store listing of the extension (today it is loaded unpacked).
- **Jev when enabled**: every question is wired in shadow mode (section 5); enabling one is an owner decision taken
  per question from `ferb decide stats`. Jev never approves; it only adds blocks, asks, exposure, redaction or drops.
- **A direct-API worker runtime** replacing `codex exec`, where the model sees only FERB's tools.
- **Hand-editable markdown memory**, embeddings behind `recall()`, per-resource process isolation for arbitrary
  code, trail encryption.
