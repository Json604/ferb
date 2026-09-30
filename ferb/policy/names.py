"""Names the owner uses count as intent targets (section 10.2, build step 14a).

The owner says "Brave" or "Slack", not com.brave.Browser or slack.com. A target still has to come from the owner's own
words (never from pasted text), but it may appear there by its natural name:
- an app: its name (or a word of it, "Brave" for "Brave Browser") from the device profile -> its bundle id;
- a site: its name ("slack" for slack.com / app.slack.com), a known alias ("gmail" for mail.google.com), or the
  domain itself.
"""

import re

SITE_ALIASES: dict[str, tuple[str, ...]] = {
    "youtube.com": ("youtube", "yt"), "youtu.be": ("youtube", "yt"), "mail.google.com": ("gmail", "google mail"),
    "google.com": ("google",), "docs.google.com": ("google docs", "docs"), "drive.google.com": ("google drive", "drive"),
    "instagram.com": ("instagram", "insta", "ig"), "x.com": ("twitter", "x"), "twitter.com": ("twitter",),
    "linkedin.com": ("linkedin",), "github.com": ("github",), "slack.com": ("slack",), "notion.so": ("notion",),
    "reddit.com": ("reddit",), "amazon.in": ("amazon",), "amazon.com": ("amazon",), "whatsapp.com": ("whatsapp",),
    "web.whatsapp.com": ("whatsapp",), "facebook.com": ("facebook", "fb"), "outlook.com": ("outlook",),
}
GENERIC_LABELS = {"www", "app", "web", "mail", "m", "com", "co", "in", "org", "net", "io", "so", "be", "ai"}


def _words(text: str) -> set[str]:
    return set(re.findall(r"[a-z0-9]+", text.lower()))


def site_named(site: str, own_words: str) -> bool:
    """The domain itself, its name label, or a known alias appears in the owner's own words."""
    lw, words = own_words.lower(), _words(own_words)
    site = site.lower().strip().removeprefix("www.")
    if not site:
        return False
    if site in lw:
        return True
    for alias in SITE_ALIASES.get(site, ()):
        if (alias in words) if " " not in alias else (alias in lw):
            return True
    labels = [x for x in site.split(".") if x not in GENERIC_LABELS]
    return bool(labels) and labels[-1] in words and len(labels[-1]) >= 3


def app_named(app_name: str, own_words: str) -> bool:
    """The app's name, or its distinctive first word ("Brave" for "Brave Browser"), is in the owner's own words."""
    lw, words = own_words.lower(), _words(own_words)
    name = app_name.lower().strip()
    if not name:
        return False
    if name in lw:
        return True
    first = re.findall(r"[a-z0-9]+", name)
    return bool(first) and len(first[0]) >= 3 and first[0] in words


def resolve_apps(names: list[str], apps: list[tuple[str, str]], own_words: str) -> tuple[dict[str, str], list[str]]:
    """App names the plan uses -> bundle ids from the device profile ((bundle_id, name) pairs). Returns (found, missing):
    a name is found only when the owner used it and exactly one installed app matches."""
    found, missing = {}, []
    for n in names:
        if not app_named(n, own_words):
            missing.append(n)
            continue
        hits = {b for b, name in apps if name.lower() == n.lower().strip()} or \
               {b for b, name in apps if app_named(name, n) or n.lower().strip() in name.lower()}
        if len(hits) == 1:
            found[n] = hits.pop()
        else:
            missing.append(n)
    return found, missing
