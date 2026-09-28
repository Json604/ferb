# Spike 16 — FERB identity through the extension in its original Chrome profile

Status: done (2026-09-29). Owner-requested after build step 5's copied-profile session kept dying.

## Question

Does FERB's Google session survive 30+ minutes of periodic use when FERB's identity is driven through an extension
in the **original** "FERB" profile of the owner's normal Chrome (no copy, no CDP launch)?

## Background (build step 5)

- The copied profile (`~/.ferb/chrome/identity`, CDP over pipe) lost its Google session twice: once after a
  headless Gmail visit, and again 1-2 minutes after a successful check in a headed background window.
- Diagnostic A (owner-approved, one launch, read-only): in the copy, the Atom feed returned 401 and
  myaccount.google.com showed the signed-out page. The **original profile stayed signed in** throughout, so Google
  didn't end the account's session; only the copy's cookies stopped working. Most likely cause: Chrome binds
  Google's session cookies to the original profile (device-bound session credentials) and a copy can't renew them.
  Not proven; the fix below doesn't depend on the cause.

## Setup

- Minimal MV3 extension (`ext/`): open background tab / read / close. No debugger permission; host access only to
  mail.google.com, myaccount.google.com, www.google.com, accounts.google.com. Loaded unpacked by the owner in the
  "FERB" profile only.
- Relay (`host.py`, from spike 4 + spike 12's exit-on-EOF) and a native-host manifest in Chrome's default
  NativeMessagingHosts folder, installed to `~/.ferb/spike16` for the run and removed after.
- `runner.py` (stand-in daemon): a round every 5 min for 35 min. Each round opens background tabs in the FERB
  profile for the inbox Atom feed, myaccount.google.com and the Gmail inbox, reads them, closes them. Only
  booleans logged.

## Result

8/8 rounds pass over 35.2 min (`log.jsonl`): feed = FERB's inbox, myaccount signed in, Gmail inbox loads,
`navigator.webdriver` false every round. One connection, no drops.

`log-attempt1.jsonl`: first attempt, discarded — my feed check read the title (an XML feed has none) and the
extension reconnected several times while the owner was loading it. Fixed (source check; reconnect with backoff
and the disconnect reason in `hello`), then re-run.

## Implications for the design

- Drive FERB's identity through the FERB extension in a dedicated profile of the owner's normal Chrome, bound to
  identity FERB at pairing. No profile copy, no CDP launch, no `--remote-debugging-pipe`, no webdriver signal.
- The extension must reconnect by itself (with backoff) and say why it last disconnected (confirms spike 12/13).
- Needs the owner's Chrome running with the FERB profile loaded; FERB may open that profile (spike 12/13 rules:
  start detached, never quit or kill, wake page to reconnect).
