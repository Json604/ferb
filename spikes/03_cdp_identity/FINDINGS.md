# Spike 3 — CDP over a pipe to `ferb-identity`; origin-bound `fill_secret`

Status: stage 1 done (2026-09-28), 8/8 tests pass. Stage 2 (FERB's real email account) waiting on the owner.

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
