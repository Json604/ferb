"""Does the seeded ferb-identity profile's Google session work under CDP? Read-only: open the inbox, verify account.

Usage: python gmail_session_check.py
"""

import re
import time

from cdp_pipe import ChromePipe, Page
from ferb_config import FERB_HOME, load_secret_specs

PROFILE = FERB_HOME / "chrome" / "identity"
INBOX = "https://mail.google.com/mail/u/0/#inbox"


def classify(title: str, url: str, has_inbox_list: bool, email: str) -> str:
    if url.startswith("https://mail.google.com/") and email in title and has_inbox_list:
        return f"inbox loaded as {email}"
    if "accounts.google.com" in url:
        return "redirected to sign-in (session not accepted)"
    return "unrecognised page"


def check_pipe(email: str) -> str:
    chrome = ChromePipe(PROFILE, headless=False)
    try:
        page = Page(chrome)
        page.goto(INBOX)
        result = "timeout"
        for _ in range(60):
            title, url = page.eval("document.title") or "", page.eval("location.href") or ""
            rows = bool(page.eval("!!document.querySelector('[role=main] table, [role=main] [role=grid]')"))
            result = classify(title, url, rows, email)
            if result != "unrecognised page":
                break
            time.sleep(0.5)
        return f"{result}; webdriver={page.eval('navigator.webdriver')}; final url host={url.split('/')[2]}"
    finally:
        chrome.close()


if __name__ == "__main__":
    specs = {s.ref.key: s for s in load_secret_specs()}
    email = specs["ferb/email/password"].account_hint
    print(f"[pipe] {re.sub(r'[^ -~]', '', check_pipe(email))}")
