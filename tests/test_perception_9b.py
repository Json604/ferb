"""Step 9b perception and activity fixes (kb area memory) and the bot-check fact (HAND-208):
accessibility rows stay in the trail (7 days), never in today's activity or its search; browser titles, typed text and
copied text are checked against incognito and the excluded domains; activity search indexes values, prefers recent
rows and stays within its limit; each row's data is stored once; typed text survives a quick focus change; the software
watcher reads TCC and launch jobs only when they change; a bot-check widget's frame is stated with the ability."""
import json
from datetime import date, timedelta
from pathlib import Path
from typing import Any

from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from ferb.core.types import PageSnapshot, TabRef
from ferb.sense.capture import Field, TypedFields
from ferb.sense.exclusions import Exclusions
from ferb.sense.trail import Observation, Trail
from ferb.sense.watchers.accessibility import accessibility_observation
from ferb.store.db import Database


def trail(tmp_path: Path, domains: tuple[str, ...] = ()) -> tuple[Trail, list[Any]]:
    db = Database(tmp_path / "t.db")
    db.migrate()
    published: list[Any] = []
    excl = Exclusions.build(domains=domains)
    t = Trail(db, FakeClock(), SequentialIds("tr"), lambda v: v, lambda: excl,
              lambda topic, data: published.append(data), lambda: "dev", lambda: False)
    return t, published


# ---------- accessibility (SENSE-1) ----------

# decision: SENSE-1
def test_accessibility_rows_stay_out_of_todays_activity_and_its_search(daemon: Any) -> None:
    d = daemon.daemon
    daemon.call_soon(d._publish, "trail.event", {"id": "ax1", "source": "accessibility", "actor": "unknown",
                                                 "data": {"bundle_id": "com.apple.Notes", "role": "AXButton",
                                                          "notification": "AXValueChanged"}})
    daemon.call_soon(d._publish, "trail.event", {"id": "w1", "source": "window", "actor": "owner",
                                                 "data": {"title": "AXButton notes window"}})
    rows = d.daily_activity.search("AXButton notes")
    assert [r["source_id"] for r in rows] == ["w1"]
    assert "ax1" not in d.daily_activity.path.read_text()


# decision: SENSE-1, SENSE-4
def test_accessibility_rows_are_purged_after_7_days_summarized_or_not(tmp_path: Path) -> None:
    t, _ = trail(tmp_path)
    for n, (source, day) in enumerate([("accessibility", "2026-10-01"), ("accessibility", "2026-10-29"),
                                       ("window", "2026-10-01")] + [("accessibility", "2026-10-02")] * 5):
        t.db.conn.execute("INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) VALUES "
                          "(?,?,?,?,?,?,?,?)", (f"r{n}", day + "T10:00:00+00:00", day, "dev", "unknown", None, source,
                                                "{}"))
    assert t.purge_source("accessibility", 7, today=date(2026, 10, 31), chunk=2) == 6      # chunked
    left = {(r[0], r[1]) for r in t.db.conn.execute("SELECT source, day FROM trail")}
    assert left == {("accessibility", "2026-10-29"), ("window", "2026-10-01")}


# ---------- privacy ----------

# decision: SENSE-5
def test_a_browsers_accessibility_events_keep_no_title() -> None:
    facts = {"window_id": "1", "field_id": "f", "role": "AXWindow", "title": "Private bank statement"}
    chrome = accessibility_observation("com.google.Chrome", "Chrome", 1, "AXTitleChanged", facts, lambda v: v)
    notes = accessibility_observation("com.apple.Notes", "Notes", 2, "AXTitleChanged", facts, lambda v: v)
    assert "title" not in chrome.data and notes.data["title"] == "Private bank statement"


# decision: SENSE-5
def test_text_typed_or_copied_in_a_browser_follows_its_pages_privacy(tmp_path: Path) -> None:
    t, _ = trail(tmp_path, domains=("bank.example",))

    def typed(bundle: str = "com.google.Chrome", source: str = "typed_text") -> Any:
        return t.record(Observation(source, {"bundle_id": bundle, "app": "x", "text": "hello"}, {"actor": "owner"}))
    assert typed() is None                                        # no page known for that browser: can't be checked
    assert t.record(Observation("browser_tab", {"browser": "chrome", "host": "bank.example", "url": "https://bank.example/",
                                                "incognito": False}, {})) is None   # an excluded page: not recorded
    assert typed() is None and typed(source="clipboard") is None  # nor what was typed or copied on it
    t.record(Observation("browser_tab", {"browser": "chrome", "host": "docs.example", "url": "https://docs.example/",
                                         "incognito": False}, {}))
    row = typed()
    assert row is not None and row.data["host"] == "docs.example"
    t.record(Observation("browser_tab", {"browser": "chrome", "host": "docs.example", "incognito": True,
                                         "url": "https://docs.example/"}, {}))
    assert typed() is None                                        # a private window's typing never is
    assert typed("com.apple.Notes") is not None                   # not a browser: as before


# ---------- today's activity ----------


# decision: MEM-27
def test_activity_stays_within_its_limit_and_says_what_it_left_out(daemon: Any) -> None:
    from ferb.memory.search import PLANNER
    d = daemon.daemon
    for n in range(8):
        d.daily_activity.record({"id": f"big{n}", "source": "window", "what": "window",
                                 "data": {"title": f"quarterly report draft {n} " + "z" * 3000}})
    text = d._relevant_context("quarterly report draft")[0]
    part = text.split("Relevant activity today (data): ")[1]
    assert len(part) <= PLANNER.activity_tokens * 4 + 400
    assert "more matching activity row(s) today left out" in part and "trail.query finds them" in part


# ---------- typed text ----------

def field(fid: str, safe: bool = True) -> Field:
    return Field("com.apple.Notes", "Notes", 1, "w", fid, "AXTextField" if safe else "AXStaticText")


class Redact:
    def __call__(self, value: Any) -> Any:
        return value

    def field(self, text: str, **_: Any) -> str:
        return text


# decision: SENSE-401
def test_typed_text_is_kept_when_focus_moves_on_within_the_hold() -> None:
    """Fix list: a burst was dropped when focus moved within 0.5 s of the last key."""
    out: list[Any] = []
    fields = TypedFields(out.append, Redact())
    fields.feed(field("a"), "first note", actor="owner", now=0.0)
    fields.feed(field("b"), "second", actor="owner", now=0.1)        # straight on to another field
    assert [o.data["text"] for o in out] == ["first note"]
    fields.flush(None, secure=False, now=0.2)                         # focus left text fields altogether
    assert [o.data["text"] for o in out] == ["first note", "second"]
    fields.feed(field("c"), "typed then hidden", actor="owner", now=1.0)
    fields.leave(field("c", safe=False))                              # the same field turned secret: dropped
    fields.feed(field("d"), "secret soon", actor="owner", now=2.0)
    fields.flush(field("e"), secure=True, now=2.1)                    # Secure Input on: nothing held is kept
    assert [o.data["text"] for o in out] == ["first note", "second"]


# ---------- the software watcher ----------


# ---------- the bot-check fact (HAND-208) ----------

# decision: HAND-1, HAND-208
def test_a_bot_check_frame_is_stated_with_the_ability_that_covers_it() -> None:
    from ferb.act.browser.ext_bridge import frame_urls
    from ferb.act.browser.snapshot import BOT_CHECK_ABILITY, for_model
    frames = [{"frame_id": 0, "url": "https://shop.example/checkout", "iframes": [
                  {"src": "https://challenges.cloudflare.com/cdn-cgi/challenge-platform/h/b/turnstile/if/ov2/x"},
                  {"src": "https://www.youtube.com/embed/abc"}]},
              {"frame_id": 3, "url": "https://www.google.com/recaptcha/api2/anchor?k=x"}]
    urls = frame_urls(frames)
    assert urls[0].startswith("https://challenges.cloudflare.com/") and len(urls) == 3
    tab = TabRef("dev", "chrome", "Default", 1, "t1")
    shown = for_model(PageSnapshot(tab, 1, "https://shop.example/checkout", "Checkout", frame_urls=urls))
    check = shown["bot_check"]
    assert check["widgets"] == ["Cloudflare Turnstile", "reCAPTCHA"] and check["ability"] == BOT_CHECK_ABILITY
    assert check["read"] == "abilities.read name=browser_verification"
    assert list(shown)[:4] == ["tab", "url", "title", "bot_check"]          # stated first
    plain = for_model(PageSnapshot(tab, 1, "https://shop.example/", "Shop",
                                   frame_urls=["https://www.youtube.com/embed/abc",
                                               "https://www.google.com/maps/embed"]))
    assert "bot_check" not in plain                                        # a frame is not a check by itself
