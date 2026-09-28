# Spike 3 — CDP over a pipe to `ferb-identity`; origin-bound `fill_secret`

Status: done (2026-09-28). Stage 1: 8/8 (practice-login test skips with a reason when the external site is down).
Stage 2: automated Google sign-in refused; seeded session works over the pipe. ARCHITECTURE.md edits proposed.

Environment: Google Chrome 154.0.8037.57 (headless=new for tests), Python 3.12, macOS `security` CLI.

Run (real Chrome; reaches https://the-internet.herokuapp.com):

    uv run --python 3.12 --no-project --with pytest pytest -q test_spike.py

## Stage 1 result (practice site, published test credentials)

- **CDP over pipe works** with the stdlib only: `--remote-debugging-pipe`, commands on fd 3, replies on fd 4,
  NUL-delimited JSON. No TCP listener in any Chrome process (checked with `lsof`).
  Gotcha: `subprocess` runs `preexec_fn` before closing inherited fds, so the `dup2` targets must be listed in
  `pass_fds=(3, 4)` or Chrome starts with them closed and never answers.
- **Login on https://the-internet.herokuapp.com/login** with the password read from Keychain by `fill_secret`.
- **`fill_secret` design** (`fill.py`):
  1. pre-check: target frame's `securityOrigin` (CDP `Page.getFrameTree`, across auto-attached OOPIF sessions)
     must be in `allowed_origins` (normalized scheme://host[:non-default port]) before the secret is read;
  2. in the same JS call that writes the value, in an isolated world of that frame, re-check `location.origin`;
  3. set the value on that exact element (native setter + `input`/`change` events), not by typing into whatever
     has focus.
  Results carry only `ref.key`; the CDP client logs method names only.
- **Refusals proven**: lookalike host (127.0.0.1 vs allowed localhost), scheme mismatch (http vs allowed https),
  cross-origin iframe (frame refused; top-frame selector doesn't pierce into the iframe), unknown secret.
  **Focus stealing**: allowed page moves focus into an attacker frame (asserted); secret still lands only in the
  target field. **Control**: filling the iframe succeeds when its own origin is allowed, so the refusal is the
  origin check, not failure to reach an out-of-process frame.
- Temporary keychain (`security create-keychain`, deleted after) keeps tests off the login keychain; the test
  asserts the keychain search list is unchanged afterwards.

## Known limits / decisions for build step 5

- Setting `value` + synthetic events is untrusted input. Plain forms and typical React forms accept it; a site
  that insists on trusted keystrokes would need `Input.insertText`, which types into *focused* element and so
  reopens the focus-stealing hole. If that ever happens: pause page JS around the typing, or refuse.
- The race "frame navigates between pre-check and write" is closed by the in-frame `location.origin` check in the
  same JS task as the write; not separately tested (can't be triggered deterministically).
- Keychain ACL: an item created with the `security` tool trusts `security`, so any process of this user that runs
  `security find-generic-password -w` reads it without a prompt. Harden when FERB has a signed identity
  (FERB.app): restrict the ACL to FERB's binary.

## Stage 2 (owner-created account)

Keychain item FERB expects (login keychain, generic password):

| Field | Value |
|-------|-------|
| Service (`-s`) | `ferb` |
| Account (`-a`) | `ferb/email/password` (this is `SecretRef.key`) |
| Label (`-l`) | `FERB email password` (cosmetic) |
| Password | the account password, entered at the hidden prompt |

Create it in your own terminal (not through this chat):

    security add-generic-password -s ferb -a ferb/email/password -l "FERB email password" -w

With `-w` last and no value, `security` prompts for the password without echo and it stays out of shell history.
The email address and provider are not secret; they go in the `SecretSpec` (`account_hint`, `allowed_origins`).

## Stage 2 result (2026-09-28): Google refuses automated sign-in

- Config: `~/.ferb/config.toml` (0600, dir 0700) holds the spec: `allowed_origins = ["https://accounts.google.com"]`,
  `account_hint = "ferb.kartikey@gmail.com"`. `ferb_config.py` refuses a config or directory readable/writable by
  others (`test_config.py`, 5 tests). Keychain item `-s ferb -a ferb/email/password` present (checked without
  reading the value).
- `stage2_login.py` (headed, `~/.ferb/chrome/identity`): after the email step Google redirected to
  `accounts.google.com/v3/signin/rejected` ("This browser or app may not be secure"). **The password page was
  never reached, so no secret was revealed or sent.** One attempt only.
- Cause: a CDP-controlled Chrome (pipe included) exposes `navigator.webdriver === true`; Google blocks sign-in
  from automated browsers. Hiding it would be detection evasion (against Google's terms, an arms race) — not done.
- Consequence: `fill_secret` can't be used for Google itself. It still applies to sites FERB signs into with
  email + password. Options are in the report to the owner.

## Stage 2, continued: seeding the identity profile (owner-approved)

- Owner's Chrome quit; `Local State` → `Profile 22` (renamed "FERB" by the owner) is the only profile with
  `ferb.kartikey@gmail.com`, and its only Chrome account (`Preferences` → `account_info`). Cookies and saved
  passwords were not read. Owner confirmed the profile holds no other site logins.
- Seeded with `rsync -a "Profile 22/" ~/.ferb/chrome/identity/Default/` (owner-only dirs). The earlier empty
  profile from the failed sign-in was moved aside, not deleted.
- **`navigator.webdriver` experiment** (`webdriver_probe.py`, value read by a local page, not via CDP):

  | Launch | webdriver |
  |--------|-----------|
  | no debugging flags | false |
  | `--enable-automation` (control) | true |
  | `--remote-debugging-port=0 --remote-debugging-address=127.0.0.1` | true |
  | `--remote-debugging-pipe`, no CDP commands | true |
  | `--remote-debugging-pipe`, CDP attached + reload | true |

  On Chrome 154, remote debugging itself (pipe or port) sets it; no opt-in automation flag was in use, so there
  was nothing to drop. Pipe and port expose the same signal.
- **Gmail over the pipe** (`gmail_session_check.py`, read-only): inbox loads, verified as
  `ferb.kartikey@gmail.com`, with `webdriver=true` (twice). Google refuses new automated sign-ins but accepts an
  existing session. The port variant was not needed and not tested.
- Cookie decryption works in the copy because it's the same macOS user and Chrome's Keychain "Safe Storage" key.
