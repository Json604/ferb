"""Accessibility fallback for browsers (sections 9.2, 9.3): reads a browser window's web content through AX when
its profile has no paired extension. Acting is refused: through AX, FERB can't establish which profile and account
it is looking at (window titles are unreliable, spike 10), and the fallback stops rather than guess (9.3)."""

BROWSER_BUNDLES = {"com.google.Chrome": "chrome", "com.brave.Browser": "brave", "com.apple.Safari": "safari"}
ACT_REFUSED = ("FERB acts in browsers only through its extension, where the profile and account are known; "
               "reading this window through Accessibility is fine, acting isn't")


def is_browser(bundle_id: str) -> bool:
    return bundle_id in BROWSER_BUNDLES
