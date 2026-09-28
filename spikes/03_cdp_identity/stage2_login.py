"""Spike 3 stage 2: sign FERB's own profile into FERB's Gmail with the Keychain password via fill_secret.

Headed Chrome, user-data-dir ~/.ferb/chrome/identity. One password attempt, no retries.
The password is never typed: if Google rejects a directly set value, stop and report (owner's rule).
Prints an outcome line; never prints the password.
"""

import sys
import time

from cdp_pipe import ChromePipe, Page
from ferb_config import FERB_HOME, load_secret_specs
from fill import FillTarget, fill_secret
from secrets_kc import KeychainSecretStore, SecretRef

REF = SecretRef("ferb/email/password")
SIGNIN = "https://accounts.google.com/ServiceLogin?continue=https%3A%2F%2Fmail.google.com%2Fmail%2F"
HANDOVER_SECONDS = 300


def visible(page: Page, sel: str) -> bool:
    return bool(page.eval(f"(e => !!e && e.getClientRects().length > 0)(document.querySelector('{sel}'))"))


def text(page: Page) -> str:
    return page.eval("document.body ? document.body.innerText : ''") or ""


def url(page: Page) -> str:
    return page.eval("location.href") or ""


def click(page: Page, sel: str) -> None:
    """Trusted click at the element's centre (real mouse events via CDP)."""
    r = page.eval(f"(r => [r.x + r.width / 2, r.y + r.height / 2])"
                  f"(document.querySelector('{sel}').getBoundingClientRect())")
    for kind in ("mousePressed", "mouseReleased"):
        page.send("Input.dispatchMouseEvent", {"type": kind, "x": r[0], "y": r[1], "button": "left",
                                               "clickCount": 1})


def wait_for(page: Page, conditions: dict, timeout: float = 30) -> str:
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        for name, check in conditions.items():
            try:
                if check():
                    return name
            except Exception:
                pass  # page mid-navigation
        time.sleep(0.5)
    return "timeout"


def signed_in_as(page: Page, email: str) -> bool:
    return url(page).startswith("https://mail.google.com/") and email in (page.eval("document.title") or "")


def main() -> str:
    specs = load_secret_specs()
    store = KeychainSecretStore(specs)  # default search list -> login keychain
    email = store.spec(REF).account_hint
    if not store.has(REF):
        return "refused: keychain item or spec missing"

    chrome = ChromePipe(FERB_HOME / "chrome" / "identity", headless=False)
    try:
        page = Page(chrome)
        page.goto(SIGNIN)
        state = wait_for(page, {"inbox": lambda: signed_in_as(page, email),
                                "identifier": lambda: visible(page, "input#identifierId")})
        if state == "inbox":
            return f"already signed in as {email}"
        if state != "identifier":
            return f"stopped: sign-in page not recognised ({url(page)[:80]})"

        page.eval("document.querySelector('input#identifierId').focus()")
        page.send("Input.insertText", {"text": email})  # not a secret
        click(page, "#identifierNext button")

        pw = 'input[name="Passwd"]'
        state = wait_for(page, {"password": lambda: visible(page, pw),
                                "blocked": lambda: "may not be secure" in text(page)
                                or "Couldn't sign you in" in text(page),
                                "captcha": lambda: visible(page, "#captchaimg")
                                or "unusual traffic" in text(page)})
        if state != "password":
            return f"stopped before password: {state} ({url(page)[:80]})"

        frame = page.send("Page.getFrameTree")["frameTree"]["frame"]["id"]
        r = fill_secret(page, FillTarget(frame, pw), REF, store)
        if not r.ok:
            return f"refused by fill_secret: {r.reason}"
        click(page, "#passwordNext button")

        state = wait_for(page, {
            "inbox": lambda: signed_in_as(page, email),
            "rejected_value": lambda: "Enter a password" in text(page),
            "wrong_password": lambda: "Wrong password" in text(page),
            "challenge": lambda: "/challenge/" in url(page) or "2-Step" in text(page),
            "other_account_page": lambda: url(page).startswith(("https://myaccount.google.com",
                                                                "https://gds.google.com")),
        }, timeout=45)
        if state == "rejected_value":
            return "refused: Google did not accept the directly set password value (no typing fallback)"
        if state == "challenge":
            print(f"verification step shown; complete it in the Chrome window ({HANDOVER_SECONDS}s)",
                  flush=True)
            state = wait_for(page, {"inbox": lambda: signed_in_as(page, email)}, timeout=HANDOVER_SECONDS)
        if state == "other_account_page":  # e.g. recovery-info interstitial; go on to Gmail and verify
            page.goto("https://mail.google.com/mail/")
            state = wait_for(page, {"inbox": lambda: signed_in_as(page, email)})
        if state == "inbox":
            return f"signed in; verified account {email}"
        return f"stopped after password: {state} ({url(page)[:80]})"
    finally:
        chrome.close()


if __name__ == "__main__":
    outcome = main()
    print(outcome)
    sys.exit(0 if outcome.startswith(("signed in", "already signed in")) else 1)
