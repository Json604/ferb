"""Identify supported browser bundle protocols for native read checks. Must not choose a browser profile or account.

Browser bundle identifiers for native AX source annotations."""

BROWSER_BUNDLES = {"com.google.Chrome": "chrome", "com.brave.Browser": "brave", "com.apple.Safari": "safari"}


def is_browser(bundle_id: str) -> bool:
    return bundle_id in BROWSER_BUNDLES
