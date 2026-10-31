"""A made-up store shaped like the owner's on 2026-10-24 (memory audit: 102 past tasks, 45 day narratives, 25 task
reflections, 21 facts, 14 entity summaries), for ranking tests (L36 §3). Invented text only (Ruling: the owner's chat
lines never enter the repository); every text holds the words nearly all of them share."""
from typing import Any

from ferb.memory.learning import Learning

TOPICS = ("gmail inbox", "kaggle leaderboard", "zomato order", "github pull request", "whatsapp group",
          "spotify playlist", "instagram reel", "scaler assignment", "resume pdf", "discord server", "brave profile",
          "chrome tab", "calendar invite", "flight booking", "notes folder", "desktop files", "terminal command",
          "youtube video", "linkedin post", "rsna competition")
USERNAME = "the username is Asha Rao"


def topic(n: int) -> str:
    return TOPICS[n % len(TOPICS)]


def fact(store: Learning, key: str, content: str, *, owner: bool = True, subject: str = "owner") -> dict[str, Any]:
    """A fact node as builds before step 9b indexed a memory item (`sync_facts`, removed)."""
    body = {"id": "item-" + key, "kind": "fact", "subject": subject, "key": key, "content": content,
            "origin": "owner_stated" if owner else "inferred", "status": "active", "unconfirmed_hint": False,
            "observed_at": "2026-10-21T00:00:00+00:00", "valid_from": "2026-10-21T00:00:00+00:00", "valid_to": None,
            "supersedes": None, "confidence": 1.0, "source_ids": []}
    return store.put("fact", f"{subject}/{key}", body, [], origin_label="owner" if owner else "inferred",
                     made_by="code:fact-index/1", state="active", owner=owner)


def owner_like_store(store: Learning) -> None:
    for n in range(102):
        store.put("episode", f"task{n:03d}", {
            "task_id": f"task{n:03d}", "intent": f"Check the {topic(n)} for the owner and report what is new",
            "request": f"check my {topic(n)}", "status": "done", "verified": n % 3 == 0,
            "narrative": f"The {topic(n)} is up to date and there is nothing new for the owner.",
            "ended_at": f"2026-10-{1 + n % 24:02d}T10:00:00+00:00", "actions": [], "apps": {}, "slots": {}},
            [], origin_label="ferb", made_by="code:evidence-card/4", state="active")
    for n in range(45):
        store.put("activity_summary", f"trail.day_narrative:2026-09-{1 + n % 30:02d}:{n}", {
            "day": f"2026-09-{1 + n % 30:02d}",
            "text": f"The owner spent the day on the {topic(n)} and the {topic(n + 7)}."},
            [], origin_label="inferred", made_by="model:test", state="active")
    for n in range(25):
        store.put("episode_annotation", f"task{n:03d}", {
            "reflection": f"The {topic(n)} is easier for the owner in the morning.",
            "narrative": "It went fine and the owner is happy."},
            [], origin_label="inferred", made_by="model:test", state="active")
    for n in range(21):
        fact(store, f"note_{n}", f"the {topic(n)} is kept in the {topic(n + 3)} for the owner")
    for n in range(14):
        store.put("entity", f"project:{topic(n).replace(' ', '-')}", {
            "text": f"The {topic(n)} is a project of the owner and it is active."},
            [], origin_label="inferred", made_by="code:entity-summary/1", state="active")


# Ordinary words of a long research-and-booking goal (none names a site): the owner's store has many past tasks whose
# long texts share such words with any long goal ("account" was in 275 of 699 nodes, "login" in 52; 2026-10-24).
RESEARCH_WORDS = (
    "search official information account report booking documents details available requires decisions security login "
    "monitoring frequency limits confirmation locations interview date first step final strategy update work personal "
    "choice agent paid flow system valid exact research chance getting slot blocks extensions optimize securing earliest "
    "corresponding permits acceptable selections applicable immediately waiting genuinely missing determine implement "
    "materially improves respecting avoiding bots unsafe guidance availability scheduling rules rescheduling access "
    "automation abuse restrictions government sources current collect propose good slots online anytime")


def goal_like_episodes(store: Learning, n: int = 80, width: int = 30) -> None:
    """Past tasks with long texts, each sharing `width` of `RESEARCH_WORDS` with a long goal (a rotating window, so each
    word stays in fewer than half the nodes and carries weight). Beside `owner_like_store` they bury a short fact that
    shares one rare word with the query (L36 §4 fix round: measured on the owner's store, the username ranked 94th for
    the research step's query)."""
    words = RESEARCH_WORDS.split()
    for i in range(n):
        start = (i * 7) % len(words)
        part = [words[(start + j) % len(words)] for j in range(width)]
        store.put("episode", f"research{i:03d}", {
            "task_id": f"research{i:03d}", "intent": f"Research the {topic(i)} and report: " + " ".join(part[:width // 2]),
            "request": f"look into my {topic(i)}", "status": "done", "verified": i % 2 == 0,
            "narrative": "FERB went through " + " ".join(part[width // 2:]) + f" for the {topic(i + 5)}.",
            "ended_at": f"2026-10-{1 + i % 24:02d}T11:00:00+00:00", "actions": [], "apps": {}, "slots": {}},
            [], origin_label="ferb", made_by="code:evidence-card/4", state="active")
