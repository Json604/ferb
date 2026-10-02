"""The service catalogue (section 9.3): facts about common services — their Mac apps and websites.

Used for the environment line the supervisor model sees ("WhatsApp: app WhatsApp; web web.whatsapp.com"), for
naming checks ("Gmail" names mail.google.com, `policy/targets.py`), and for the messaging / calling apps where a
screenshot click must say what it's for (`communication_bundles`). It decides nothing about how a request is
carried out: the model picks the app or the site, the floors protect.
"""

import re
from dataclasses import dataclass, field


@dataclass(frozen=True)
class Service:
    name: str
    aliases: tuple[str, ...] = ()
    bundles: tuple[str, ...] = ()
    domains: tuple[str, ...] = ()


CATALOGUE: tuple[Service, ...] = (
    Service("WhatsApp", ("whatsapp", "whats app", "wa"), ("net.whatsapp.WhatsApp", "desktop.WhatsApp"),
            ("web.whatsapp.com", "whatsapp.com")),
    Service("Slack", ("slack",), ("com.tinyspeck.slackmacgap",), ("slack.com",)),
    Service("Gmail", ("gmail", "google mail"), (), ("mail.google.com",)),
    Service("Mail", ("apple mail", "mail app"), ("com.apple.mail",), ()),
    Service("YouTube", ("youtube", "yt"), (), ("youtube.com", "youtu.be")),
    Service("Spotify", ("spotify",), ("com.spotify.client",), ("open.spotify.com",)),
    Service("Zoom", ("zoom",), ("us.zoom.xos",), ("zoom.us",)),
    Service("Microsoft Teams", ("teams", "ms teams"), ("com.microsoft.teams2", "com.microsoft.teams"),
            ("teams.microsoft.com",)),
    Service("Google Meet", ("meet", "google meet"), (), ("meet.google.com",)),
    Service("FaceTime", ("facetime",), ("com.apple.FaceTime",), ()),
    Service("Messages", ("imessage", "messages"), ("com.apple.MobileSMS",), ()),
    Service("Telegram", ("telegram",), ("ru.keepcoder.Telegram", "org.telegram.desktop"), ("web.telegram.org",)),
    Service("Discord", ("discord",), ("com.hnc.Discord",), ("discord.com",)),
    Service("Notion", ("notion",), ("notion.id",), ("notion.so",)),
    Service("Notes", ("notes", "apple notes"), ("com.apple.Notes",), ()),
    Service("Reminders", ("reminders",), ("com.apple.reminders",), ()),
    Service("Calendar", ("calendar", "ical"), ("com.apple.iCal",), ("calendar.google.com",)),
    Service("Google Calendar", ("google calendar", "gcal"), (), ("calendar.google.com",)),
    Service("Google Docs", ("google docs", "gdocs"), (), ("docs.google.com",)),
    Service("Google Drive", ("google drive", "gdrive"), (), ("drive.google.com",)),
    Service("Instagram", ("instagram", "insta", "ig"), (), ("instagram.com",)),
    Service("X", ("twitter", "x.com"), (), ("x.com", "twitter.com")),
    Service("LinkedIn", ("linkedin",), (), ("linkedin.com",)),
    Service("GitHub", ("github",), ("com.github.GitHubClient",), ("github.com",)),
    Service("Figma", ("figma",), ("com.figma.Desktop",), ("figma.com",)),
    Service("Signal", ("signal",), ("org.whispersystems.signal-desktop",), ()),
    Service("Skype", ("skype",), ("com.skype.skype",), ("web.skype.com",)),
    Service("Outlook", ("outlook",), ("com.microsoft.Outlook",), ("outlook.live.com", "outlook.office.com")),
    Service("Music", ("apple music", "music app"), ("com.apple.Music",), ("music.apple.com",)),
    Service("Netflix", ("netflix",), (), ("netflix.com",)),
    Service("Amazon", ("amazon",), (), ("amazon.in", "amazon.com")),
    Service("ChatGPT", ("chatgpt",), ("com.openai.chat",), ("chatgpt.com",)),
)


# Messaging and calling services: in their apps a screenshot click must say what it's for (build step 16a).
COMMUNICATION = frozenset({"WhatsApp", "Slack", "Zoom", "Microsoft Teams", "FaceTime", "Messages", "Telegram",
                           "Discord", "Signal", "Skype"})

BROWSER_BUNDLE_IDS = frozenset({"com.brave.browser", "com.google.chrome", "com.apple.safari", "org.mozilla.firefox",
                                "company.thebrowser.browser", "com.microsoft.edgemac", "com.operasoftware.opera"})


def communication_bundles() -> frozenset[str]:
    return frozenset(b.lower() for s in CATALOGUE if s.name in COMMUNICATION for b in s.bundles)


@dataclass
class Environment:
    """What the environment line needs to know about the Mac right now."""
    apps: list[tuple[str, str]]                                   # (bundle id, name) installed
    paired: list[tuple[str, str, str]] = field(default_factory=list)   # (browser, folder, display name) owner profiles


def _words(text: str) -> set[str]:
    return set(re.findall(r"[a-z0-9.]+", text.lower()))


def services_named(own_words: str, env: Environment) -> list[Service]:
    """Services the owner's words mention, for the environment line: the catalogue by name or alias, then any
    installed app by its name. Informational only."""
    lw, words = own_words.lower(), _words(own_words)
    found: list[Service] = []

    def said(alias: str) -> bool:
        return (alias in words) if " " not in alias else (alias in lw)
    for s in CATALOGUE:
        if said(s.name.lower()) or any(said(a) for a in s.aliases):
            found.append(s)
    known = {b.lower() for s in CATALOGUE for b in s.bundles} | BROWSER_BUNDLE_IDS
    for bundle, name in env.apps:
        first = re.findall(r"[a-z0-9]+", name.lower())
        if bundle.lower() not in known and len(name) >= 3 and (name.lower() in lw or (
                first and len(first[0]) >= 4 and first[0] in words)):
            found.append(Service(name, (), (bundle,), ()))
    return found


def summary(env: Environment, macos: str, relevant: list[Service], running: set[str]) -> tuple[str, str]:
    """The environment summary for the supervisor: (stable part, the part that changes — running apps — for the
    end)."""
    installed = {b.lower(): n for b, n in env.apps}
    rel = []
    for s in relevant:
        apps = [installed[b.lower()] for b in s.bundles if b.lower() in installed]
        rel.append(f"{s.name}: {'app ' + ', '.join(apps) if apps else 'no app'}"
                   f"{'; web ' + s.domains[0] if s.domains else ''}")
    profiles = ", ".join(f"{b.capitalize()} “{n}”" for b, _f, n in env.paired) or "none paired"
    stable = f"This Mac: macOS {macos}. Browser profiles FERB can use: {profiles}."
    if rel:
        stable += " Relevant here — " + "; ".join(rel) + "."
    live = [installed[b.lower()] for s in relevant for b in s.bundles if b.lower() in installed and b in running]
    return stable, (f"Running now: {', '.join(sorted(set(live)))}." if live else "")
