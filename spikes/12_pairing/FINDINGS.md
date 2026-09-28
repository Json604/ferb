# Spike 12 — Pairing end to end (Chrome only)

Status: done (2026-09-28). 12/12 pass, 3 runs in a row (8 pairing-rule unit tests + 4 end-to-end).
Owner's rule: **Chrome only**, test profiles "FERB Test 1" (`Profile 23`) and "FERB Test 2" (`Profile 24`) in the
owner's normal Chrome; Brave untouched (a short Brave check in a temporary profile is scheduled before build
step 6, see PROGRESS.md).

Run (Chrome must already be running; "FERB spike 12" loaded unpacked in both test profiles):
`uv run --python 3.12 --no-project --with pytest pytest -q test_pairing.py test_e2e.py`
The native-host manifest exists in Chrome's default `NativeMessagingHosts` folder only during the run.

## Design proven

- The extension generates `install_id` once per profile (`chrome.storage.local`).
- FERB issues a one-time token for a (browser, folder) and hands the running Chrome
  `--profile-directory=<folder> chrome-extension://<id>/pair.html#token=…`. The pairing page is
  the **extension's own page** (blank, closes itself — CLAUDE.md minimum), so only the extension in *that* profile
  can read the token; no web page or other extension sees it. Command-line navigation to it works.
- Daemon binds `install_id → chrome/<folder>` and returns a per-install key; every later message carries
  `seq` (strictly increasing) + HMAC-SHA256 over `install_id|seq|type|body_json`.

## Tested

- Unit (`pairing.py`, fake clock): unpaired installs may only send `hello`/`pair` (no events, results, acks);
  tokens single-use (a second install can't reuse one) and expiring; binding = the folder the token was issued for;
  tampered body, another install's key, unsigned message, replayed `seq` — all refused.
- End to end: each test profile pairs to its own folder with a distinct `install_id`; the key works (signed ack);
  a tab opened by a daemon action in FERB Test 1 comes back as a signed `tab_event` attributed to `Profile 23`;
  reusing a token is refused; after FERB Test 2 forgets its pairing, its events arrive from a new `install_id`
  and are refused (`unpaired_install`) and no action can be routed to it (safety test 21.16).
- Native-host manifest in the owner's **default** Chrome folder is found by default-folder profiles (spike 4/10
  open item, Chrome half).

## Found (requirements for build step 6)

1. **The native host must exit when the daemon connection drops.** Otherwise the extension keeps writing into a
   dead relay and the next message (here: the pairing token) is lost. Reruns failed until the host exited on EOF.
2. The extension should also queue outgoing messages until a fresh connection is up (not done here; spike 13).
3. Read `chrome.runtime.lastError` in `onDisconnect`, or Chrome lists "Unchecked runtime.lastError: Specified native
   messaging host not found" on the extension's error page whenever the daemon isn't installed/running.
4. **Never start or kill the owner's browser implicitly.** Running the browser binary with `--profile-directory`
   when the browser isn't running makes that process the browser itself; a harness timeout then force-quit it
   (happened once in this spike with a Chrome started by the harness, profile "FERB Test 1" only). Fixed: refuse
   unless the browser is running; hand off without waiting on/killing the launcher.
5. Loading the wrong unpacked folder is easy (spike 4's was loaded first): the daemon should report the extension
   id and version from `hello` so `doctor` can say which extension a profile actually has.

## Brave pairing check (before build step 6, owner-scheduled, 2026-09-29)

Brave 151.1.93.136, the owner's running Brave with its **default** data folder; one temporary profile
"FERB Brave Check" (created by handing `--profile-directory` to the running Brave; deleted by the owner in Brave
afterwards). The owner's own Brave profile untouched. `brave_check.py`, `brave_location.py`, `brave_check.log`.

- **PASS:** spike 12's extension (loaded unpacked in the temp profile) paired via its own pairing page; the signed
  ack was accepted and a daemon-opened background tab came back as a signed `tab_event` attributed to
  `('brave', 'FERB Brave Check')`.
- **Native-host manifest location:** Brave reads **Google Chrome's** `NativeMessagingHosts` folder, not
  `BraveSoftware/Brave-Browser/NativeMessagingHosts`. Measured twice, in both orders, one location at a time:
  Brave folder only → 0 hellos in 150 s (twice); Chrome folder only → hello within the retry window (1, then 3).
  Same as spike 4's custom-data-folder result. So one manifest in Chrome's folder serves Chrome and Brave.
- Both manifests and the relay were removed after each run.
