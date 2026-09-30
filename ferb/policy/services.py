"""Environment-aware resolution (section 9.3, build step 16): how to reach a service or app the owner names.

A service is found by name in: the catalogue below (~30 common services: their Mac apps and websites); else an
installed app (device profile); else a site the owner has visited (trail). Unknown: FERB asks once and remembers.

The route, decided in code, never by the model:
0. the owner's explicit correction ("use the web version"), stored as an owner-stated preference — always wins;
1. how the owner actually uses it: the most used of its app / website (per browser profile) over the last 14 days of
   the owner's own activity (FERB's never counts), ties broken by the most recent;
2. an installed native app;
3. the website in a paired browser profile;
4. nothing workable: ask.
A route FERB can't drive (a website only in an unpaired profile) is never chosen.
"""

import re
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any

from ferb.policy.names import app_named


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


@dataclass(frozen=True)
class Route:
    service: str
    kind: str                     # "app" | "web"
    target: str                   # bundle id (app) or domain (web)
    browser: str | None = None
    profile: str | None = None    # browser profile folder (web)
    profile_name: str | None = None
    reason: str = ""

    def say(self) -> str:
        """The route in plain words, shown when FERB starts."""
        if self.kind == "app":
            return f"Using the {self.service} app"
        where = f" in your {self.browser.capitalize()} ({self.profile_name or self.profile}) profile" \
            if self.browser else ""
        return f"Using {self.service} on the web ({self.target}){where}"


@dataclass
class Environment:
    """What resolution needs to know about the Mac right now."""
    apps: list[tuple[str, str]]                                   # (bundle id, name) installed
    paired: list[tuple[str, str, str]]                            # (browser, folder, display name) owner profiles
    usage_apps: dict[str, tuple[int, datetime]] = field(default_factory=dict)          # bundle -> (count, last)
    usage_web: dict[tuple[str, str, str], tuple[int, datetime]] = field(default_factory=dict)  # (host, b, f)
    preferences: dict[str, str] = field(default_factory=dict)     # "route:<service>" -> owner's words


def _words(text: str) -> set[str]:
    return set(re.findall(r"[a-z0-9.]+", text.lower()))


def services_named(own_words: str, env: Environment) -> list[Service]:
    """Services the owner named: the catalogue, then installed apps and visited sites by name."""
    lw, words = own_words.lower(), _words(own_words)
    found: list[Service] = []

    def named(alias: str) -> bool:
        return (alias in words) if " " not in alias else (alias in lw)
    for s in CATALOGUE:
        if named(s.name.lower()) or any(named(a) for a in s.aliases):
            found.append(s)
    known = {b.lower() for s in found for b in s.bundles}
    for bundle, name in env.apps:                                   # anything installed, by its name
        if bundle.lower() not in known and len(name) >= 3 and app_named(name, own_words) \
                and name.lower() not in ("mail", "music", "notes", "photos", "preview", "finder"):
            found.append(Service(name, (), (bundle,), ()))
    seen = {d for s in found for d in s.domains}
    for (host, _b, _f) in env.usage_web:                            # anything visited, by its name
        label = [x for x in host.split(".") if x not in ("www", "app", "web", "com", "co", "in", "org", "io")]
        if label and len(label[-1]) >= 4 and label[-1] in words and host not in seen:
            found.append(Service(label[-1].capitalize(), (label[-1],), (), (host,)))
            seen.add(host)
    return found


def _host_is(host: str, domains: tuple[str, ...]) -> bool:
    return any(host == d or host.endswith("." + d) for d in domains)


def route(s: Service, env: Environment) -> Route | None:
    """The owner's correction first; then the most used (14 days, ties: most recent); then app; then paired web."""
    installed = {b.lower() for b, _ in env.apps}
    my_apps = [b for b in s.bundles if b.lower() in installed]
    pref = next((env.preferences[k] for k in (f"route:{a}" for a in (s.name.lower(), *s.aliases))
                 if k in env.preferences), "").lower()
    web_options = [(b, f, n) for (b, f, n) in env.paired] if s.domains else []
    if pref:
        if ("web" in pref or "browser" in pref or "site" in pref) and web_options:
            pb, pf, pn = _best_profile(s, env, web_options)
            return Route(s.name, "web", s.domains[0], pb, pf, pn, "you asked for the web version")
        if "app" in pref and my_apps:
            return Route(s.name, "app", my_apps[0], reason="you asked for the app")
    candidates: list[tuple[int, datetime, Route]] = []
    for b in my_apps:
        if b in env.usage_apps:
            n, last = env.usage_apps[b]
            candidates.append((n, last, Route(s.name, "app", b, reason="how you use it")))
    paired = {(b, f): name for (b, f, name) in env.paired}
    for (host, b, f), (n, last) in env.usage_web.items():
        if _host_is(host, s.domains) and (b, f) in paired:          # only a profile FERB can drive
            candidates.append((n, last, Route(s.name, "web", host, b, f, paired[(b, f)], "how you use it")))
    if candidates:
        candidates.sort(key=lambda c: (c[0], c[1]), reverse=True)   # most used, then most recent
        return candidates[0][2]
    if my_apps:
        return Route(s.name, "app", my_apps[0], reason="installed")
    if web_options:
        wb, wf, wn = _best_profile(s, env, web_options)
        return Route(s.name, "web", s.domains[0], wb, wf, wn, "your browser")
    return None


def _best_profile(s: Service, env: Environment, options: list[tuple[str, str, str]]) -> tuple[str, str, str]:
    used = [((n, last), (b, f)) for (host, b, f), (n, last) in env.usage_web.items() if _host_is(host, s.domains)]
    for _, bf in sorted(used, reverse=True):
        hit = next((o for o in options if (o[0], o[1]) == bf), None)
        if hit:
            return hit
    return options[0]


def summary(env: Environment, macos: str, relevant: list[Service], running: set[str]) -> tuple[str, str]:
    """The environment summary for the planner: (stable part, the part that changes — running apps — for the end)."""
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


def usage_from_trail(rows: list[Any]) -> tuple[dict[str, tuple[int, datetime]], dict[tuple[str, str, str], tuple[int, datetime]]]:
    """The owner's own app_focus / browser_tab rows -> counts and last use (FERB's rows never passed in)."""
    import json
    apps: dict[str, tuple[int, datetime]] = {}
    web: dict[tuple[str, str, str], tuple[int, datetime]] = {}
    for r in rows:
        d, at = json.loads(r["data_json"]), datetime.fromisoformat(r["at"])
        if r["source"] == "app_focus" and d.get("bundle_id"):
            n, last = apps.get(d["bundle_id"], (0, at))
            apps[d["bundle_id"]] = (n + 1, max(last, at))
        elif r["source"] == "browser_tab" and d.get("host") and d.get("profile"):
            k = (str(d["host"]).lower(), str(d.get("browser")), str(d["profile"]))
            n, last = web.get(k, (0, at))
            web[k] = (n + 1, max(last, at))
    return apps, web
