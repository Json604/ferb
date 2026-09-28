# Spike 10 — Browser profiles: discovery, opening a named profile, External Extensions

Status: done (2026-09-28). 13/13 pass, twice (incl. the real-Chrome check). Chrome 154, Brave 151.

Run: `uv run --python 3.12 --no-project --with pyobjc-framework-ApplicationServices --with pyobjc-framework-Cocoa
--with pytest pytest -q test_resolver.py test_spike.py` (spare profiles only; ~2 min).
Add `FERB_REAL=1` to also open/close one window in the owner's Chrome **FERB** profile (FERB-only).

## Discovery (`profiles.py`)

- `Local State` → `profile.info_cache`: folder → display name (`name`), signed-in email (`user_name`). Read-only;
  profile folders, cookies and other data are never touched. This Mac: Chrome 6 profiles (incl. `Profile 22`
  "FERB"), Brave 1 (`Default` "Work").
- **`Local State` is written lazily**: a profile created while the browser runs isn't in the file until the
  browser writes it (seen reliably on quit). The section 9.5 plan (re-read on FSEvents change) covers it; a
  resolver miss right after the owner creates a profile is expected, not an error.

## Resolver (section 9.5), 8 unit tests

Exact display name (case-insensitive) → email or its local part → saved alias; any step with several matches is
**ambiguous** (same name in two browsers, or differing only in case); naming the browser restricts the pool; no
match returns "no profile called X; found: …" and never a near-match. On real data: "FERB" and "ferb.kartikey" →
`chrome/Profile 22`; "work" → `brave/Default`.

## Opening a named profile while the browser runs

- `<browser binary> --profile-directory=<folder> <url>` hands off to the running instance in ~0.1–0.2 s (the
  running pid is unchanged); the window opens in that profile. Proven in spare Chrome and Brave, and in the
  owner's real Chrome for the FERB profile (window found by a one-time title token, closed via its AX close button;
  nothing else touched).
- **A folder that doesn't exist silently creates a new profile.** Guard: `open_known_profile` only accepts folders
  returned by discovery (tested).
- Window-title profile suffix is unreliable for identity (`"… - Google Chrome - ferb"` for display name "FERB");
  identify by folder / paired `install_id`, never by title.

## External Extensions (section 9.6)

- Stand-in: Google Translate (`aapbdbdomjkkjkaonfhkkikfgjllcleb`, Web Store); ours isn't published.
- A JSON file in `<user-data-dir>/External Extensions/` (`external_update_url` = Web Store) → **every profile
  opened afterwards** gets the extension downloaded and installed **disabled with reason 8192 (external install)**
  until the owner accepts Chrome's prompt. Chrome and Brave both.
- **Brave honours the same mechanism and reads its own user-data-dir** (unlike native-messaging manifests, where
  a spare Brave read Chrome's folder — spike 4).
- Preferences flush lazily too: read too early, a profile can look like it has no disable reason.

## Not tested here

- The owner's *default* data folders (`~/Library/Application Support/Google/Chrome/External Extensions`, and
  Brave's): writing there would prompt in every real profile. Same for native-messaging manifest locations of
  default profiles — both are exercised by spike 12's end-to-end pairing in real profiles.
- `--profile-directory` when the browser is *not* running (spare first launches worked; real browsers weren't
  started from cold).
