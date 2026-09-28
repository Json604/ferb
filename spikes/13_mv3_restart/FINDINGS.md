# Spike 13 — MV3 service worker killed mid-command: reconnect, generation bump, no replay

Status: done (2026-09-28). 5/5 pass, 3 runs in a row. Chrome 154.

Run: `uv run --python 3.12 --no-project --with pytest pytest -q test_spike.py` (~45 s; one test waits for the
30-second alarm).

**Lab setup, not product design:** a spare Chrome (temporary `--user-data-dir`) driven over the CDP pipe with
`--enable-unsafe-extension-debugging`, used only to load the extension (`Extensions.loadUnpacked` works in
branded Chrome, keeps the pinned id) and to kill its worker (`Target.closeTarget` on the `service_worker` target).
Native-host manifest in the spare profile's own folder: nothing written to the owner's browsers, no manual steps.

## Measured

- Killing the worker drops the native port immediately (~0.01 s): the daemon sees the disconnect at once.
- With no other event, the worker comes back on the next `chrome.alarms` tick: 29.3 s (period 0.5 min, the
  minimum). Any tab finishing load in that profile brings it back in ~0.1 s (`tabs.onUpdated` listener).
  So an owner browser can be unreachable for up to ~30 s after a worker death; the daemon must treat that as
  "extension temporarily unavailable", not as unpaired or failed.

## Proven

1. **Killed after the effect** (`slow_open`: tab opened, reply delayed): ledger entry → `unknown`; new `hello` has
   generation +1, a new boot id, and re-announces the tabs FERB owns (with the command id that opened each);
   reconciliation from that fresh read → `done`; the tab exists exactly once (no replay).
2. **Killed before the effect** (`delayed_open`): `unknown` → reconciled `failed`; no tab ever appears.
3. **Old-generation command** after reconnect → extension answers `stale` and does nothing.
4. **Duplicate command id** → `duplicate`, executed once — also **across a worker restart**, because seen ids and
   owned tabs live in `chrome.storage.session` (survives worker restarts within the browser session).
5. Reconnect with no stimulus via the alarm within 40 s.

## Requirements for build step 6

- The extension records the command id as seen *before* executing, and tags what it creates with the command id
  so reconciliation can find it. `chrome.storage.session` for seen ids / owned tabs (lost on browser restart —
  fine, generations also restart then).
- `hello` re-announces owned resources; the daemon reconciles `unknown` from that (or a fresh `list`), never resends.
- Keep a `chrome.alarms` reconnect (30 s) plus event listeners; the daemon shouldn't expect to reach an extension
  whose worker died until one of those fires.
