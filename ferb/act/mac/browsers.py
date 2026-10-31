"""Owns the one list of web browsers (notes/hands.md fix list, gateway decision fixes): the browsers FERB knows by
bundle id, plus every installed app that registers with Launch Services as opening http or https links. Must not
judge what a browser shows or choose a browser for a task.

Used wherever code needs "is this app a web browser": a Mac read of a browser window is outside web content, not the
owner's own app data (mac/tools.py `_sources`, sense's front window); a Mac-tool input in a browser asks the owner
first (policy/confirm.py `foreground_need`, rule 17); a browser's Accessibility tree is re-read while its web area is
built (accessibility.read_tree); an owner secret is never bound to a whole browser (mac.secret, OwnerSecrets, D12).
Bundle ids are data for a where-boundary (rule 5), never an intent list.

`known` and `is_browser` never block: they use the bundle-id list and the Launch Services list as last enumerated.
`installed` enumerates (at most once per HANDLERS_TTL_S) and blocks, so async code calls it off the event loop.
App names are compared without case and without invisible marks (U+200E/U+200F and the like: WhatsApp's name starts
with U+200E)."""

import re
import threading
import time

from ferb.act.browser.profiles import BROWSERS

# Browsers FERB knows by bundle id beyond the Chromium family FERB pairs with (profiles.BROWSERS).
OTHER_BROWSERS = frozenset({
    "com.apple.Safari", "com.apple.SafariTechnologyPreview",
    "org.mozilla.firefox", "org.mozilla.firefoxdeveloperedition", "org.mozilla.nightly",
    "com.kagi.kagimacOS", "com.duckduckgo.macos.browser", "io.gitlab.librewolf-community", "ai.perplexity.comet",
    "ru.yandex.desktop.yandex-browser", "one.ablaze.floorp", "net.mullvad.mullvadbrowser",
})
# Makers whose every app is a web browser, by bundle-id prefix (Opera GX, Arc and Dia, Thorium and other Chromium
# builds, Tor, Zen, Waterfox, SigmaOS).
BROWSER_VENDORS = frozenset({"com.operasoftware", "com.vivaldi", "company.thebrowser", "org.chromium", "org.torproject",
                             "app.zen-browser", "net.waterfox", "com.sigmaos"})
KNOWN = frozenset({b.bundle_id.casefold() for b in BROWSERS.values()} | {b.casefold() for b in OTHER_BROWSERS})
_FAMILIES = KNOWN | {v.casefold() for v in BROWSER_VENDORS}
SUPPORT = "~/Library/Application Support/"
# Where browsers keep their profiles, cookies and saved passwords, beyond each Chromium browser's user-data folder
# (profiles.BROWSERS), by bundle id or maker (a folder shared by a maker's channels). ~ is the owner's home. Data for
# the never-read list (ferb/policy/protection.py `BROWSER_STORES`): a browser added here or to profiles.BROWSERS is
# closed by that one edit (a config.toml written earlier gets it once, ferb/config.py `add_defaults`).
BROWSER_DATA: dict[str, tuple[str, ...]] = {
    "com.apple.Safari": ("~/Library/Safari", "~/Library/Containers/com.apple.Safari"),      # its cookies: the container
    "com.apple.SafariTechnologyPreview": ("~/Library/SafariTechnologyPreview",
                                          "~/Library/Containers/com.apple.SafariTechnologyPreview"),
    "com.brave.Browser": (SUPPORT + "BraveSoftware",),                 # Brave and its Beta and Nightly
    "company.thebrowser": (SUPPORT + "Arc", SUPPORT + "Dia"),
    "com.operasoftware": (SUPPORT + "com.operasoftware.OperaGX",),
    "org.mozilla.firefox": (SUPPORT + "Firefox",),                     # every Firefox channel's profiles
    "com.kagi.kagimacOS": (SUPPORT + "Orion",),
    "com.duckduckgo.macos.browser": ("~/Library/Containers/com.duckduckgo.macos.browser",),
    "io.gitlab.librewolf-community": (SUPPORT + "librewolf",),
    "ai.perplexity.comet": (SUPPORT + "Comet",),
    "ru.yandex.desktop.yandex-browser": (SUPPORT + "Yandex",),
    "one.ablaze.floorp": (SUPPORT + "Floorp",),
    "net.mullvad.mullvadbrowser": (SUPPORT + "MullvadBrowser",),
    "org.torproject": (SUPPORT + "TorBrowser-Data",),
    "app.zen-browser": (SUPPORT + "zen",),
    "net.waterfox": (SUPPORT + "Waterfox",),
    "org.chromium": (SUPPORT + "Thorium",),
}


def data_folders() -> tuple[str, ...]:
    """Every browser data folder above and each Chromium browser's user-data folder, in that order; a folder inside
    another listed one is left out (that one already closes it)."""
    folders = [f for found in BROWSER_DATA.values() for f in found] + [SUPPORT + b.data for b in BROWSERS.values()]
    out: list[str] = []
    for folder in dict.fromkeys(folders):
        if not any(folder.startswith(other + "/") for other in folders):
            out.append(folder)
    return tuple(out)
# The names those browsers' apps go by (an agent may name an app instead of its bundle id).
KNOWN_NAMES = frozenset({n.casefold() for b in BROWSERS.values() for n in (b.label, b.app)} | {
    "safari", "safari technology preview", "firefox", "firefox developer edition", "firefox nightly", "orion",
    "duckduckgo", "librewolf", "comet", "yandex", "floorp", "mullvad browser", "tor browser", "zen", "zen browser",
    "waterfox", "sigmaos", "opera gx", "dia", "thorium"})

# Invisible format marks an app's name can carry (bidi marks and embeddings, zero-width characters, BOM).
INVISIBLE = re.compile("[\u200b-\u200f\u202a-\u202e\u2060-\u2064\u2066-\u2069\ufeff]")


def clean(name: str) -> str:
    """An app's name or id as compared: invisible marks removed, outer spaces trimmed, casefolded."""
    return INVISIBLE.sub("", name).strip().casefold()


def pair_key(bundle_id: str) -> str:
    """The name FERB's browser connection knows this browser by (profiles.BROWSERS' key: "chrome", "brave"…;
    "safari"), else its bundle id (no connection pairs with it)."""
    b = clean(bundle_id)
    if b == "com.apple.safari":
        return "safari"
    return next((key for key, v in BROWSERS.items() if v.bundle_id.casefold() == b), bundle_id)


def known(bundle_id: str) -> bool:
    """A bundle id of a browser FERB knows: one above, any app under one (a channel, or a web app's shim:
    `com.google.Chrome.app.<id>`, `com.apple.Safari.WebApp.<id>`), or any app of a browser maker. No I/O."""
    b = clean(bundle_id)
    return any(b == f or b.startswith(f + ".") for f in _FAMILIES)


HANDLERS_TTL_S = 60.0     # how long Launch Services' list is reused before it is asked again (speed only)
_clock = time.monotonic
_handlers: tuple[float, frozenset[str], frozenset[str]] | None = None    # (when, bundle ids, app names), casefolded
_handlers_lock = threading.Lock()


def _enumerate() -> tuple[frozenset[str], frozenset[str]]:
    """Every app Launch Services lists as opening http or https links: (bundle ids, app names), casefolded. Read-only;
    nothing opens."""
    from AppKit import NSFileManager, NSWorkspace
    from Foundation import NSURL, NSBundle
    ids: set[str] = set()
    names: set[str] = set()
    for link in ("http://example.com", "https://example.com"):          # only the scheme is looked up
        for url in NSWorkspace.sharedWorkspace().URLsForApplicationsToOpenURL_(NSURL.URLWithString_(link)) or ():
            bundle = NSBundle.bundleWithURL_(url)
            ident = bundle.bundleIdentifier() if bundle is not None else None
            if not ident:
                continue
            ids.add(clean(str(ident)))
            name = NSFileManager.defaultManager().displayNameAtPath_(url.path())
            if name:
                names.add(clean(re.sub(r"\.app$", "", str(name))))
    return frozenset(ids), frozenset(names)


def _fresh() -> tuple[frozenset[str], frozenset[str]] | None:
    with _handlers_lock:
        if _handlers is not None and _clock() - _handlers[0] < HANDLERS_TTL_S:
            return _handlers[1], _handlers[2]
    return None


def _cached() -> tuple[frozenset[str], frozenset[str]]:
    """The last list Launch Services gave, whatever its age (empty before the first): never enumerates."""
    with _handlers_lock:
        return (_handlers[1], _handlers[2]) if _handlers is not None else (frozenset(), frozenset())


def installed() -> tuple[frozenset[str], frozenset[str]]:
    """Blocking: the apps that open http/https links (bundle ids, names), enumerated at most once per HANDLERS_TTL_S.
    When Launch Services can't be asked, the last list (nothing new is kept): FERB's bundle-id list still applies."""
    global _handlers
    fresh = _fresh()
    if fresh is not None:
        return fresh
    try:
        ids, names = _enumerate()
    except Exception:
        return _cached()
    with _handlers_lock:
        _handlers = (_clock(), ids, names)
    return ids, names


def opens_web_links(bundle_id: str) -> bool:
    """Blocking: Launch Services lists this app among those that open http/https links (a fresh list)."""
    return clean(bundle_id) in installed()[0]


def known_opens_web_links(bundle_id: str) -> bool:
    """`opens_web_links` from a list enumerated within HANDLERS_TTL_S, never enumerating; False without one."""
    fresh = _fresh()
    return fresh is not None and clean(bundle_id) in fresh[0]


def is_browser(bundle_id: str) -> bool:
    """A web browser: known by bundle id, or in the Launch Services list as last enumerated. Never blocks."""
    return known(bundle_id) or clean(bundle_id) in _cached()[0]


def is_browser_now(bundle_id: str) -> bool:
    """Blocking: `is_browser` with Launch Services asked afresh when its list is stale."""
    return known(bundle_id) or opens_web_links(bundle_id)


def _bundle_like(text: str) -> bool:
    return bool(re.fullmatch(r"[a-z0-9-]+(?:\.[a-z0-9_-]+){2,}", text))


def is_browser_app(app_id: str) -> bool:
    """Whether a Mac tool's `app` argument (a name, a bundle id or `pid:N`) names a web browser. A bundle id or name
    by the lists above; a pid, or a name not on them, by the running app it names (its bundle id). Names compare
    without case or invisible marks. Fast reads only (the running-app list), no Launch Services enumeration."""
    q = clean(app_id)
    if not q:
        return False
    if q.startswith("pid:"):
        try:
            pid = int(q[4:])
        except ValueError:
            return False
        return any(is_browser(b) for b in _running_bundles(pid=pid))
    if _bundle_like(q):
        return is_browser(q)
    if q in KNOWN_NAMES or q in _cached()[1]:
        return True
    return any(is_browser(b) for b in _running_bundles(name=q))


def _running_bundles(pid: int | None = None, name: str | None = None) -> list[str]:
    """The bundle ids of running apps with this pid or (cleaned) name; none when the list can't be read."""
    try:
        from AppKit import NSWorkspace
        apps = NSWorkspace.sharedWorkspace().runningApplications()
    except Exception:
        return []
    out = []
    for a in apps:
        bundle = a.bundleIdentifier()
        if not bundle:
            continue
        if (pid is not None and int(a.processIdentifier()) == pid) or \
                (name is not None and clean(str(a.localizedName() or "")) == name):
            out.append(str(bundle))
    return out
