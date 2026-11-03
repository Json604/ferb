"""Own the owner's line when one of code's safety checks stops something a chat turn asked for (owner 2026-11-04, ORCH-519):
that a safety check stopped it, which check, why in this case and what to do, in plain words with no ids. Must not
judge intent or content: the caller names the check that refused and the structural reason it found."""

# The checks, as the owner reads them (one clause each)
REMINDERS = "reminders and schedules are set only from words you wrote in this chat"
CHANGES = "a reminder or schedule changes only when your own words in this message ask for it"
END_DATE = "an end date comes only from words you wrote"
MEMORY = "I save a memory only from words you wrote in this chat"
FORGET = "I delete a memory only when a whole sentence or clause of yours asks for it"
CONVERSATION = "a conversation I'm having for you changes only on your own instruction"

# Why, in this case (since ORCH-518 everything in the owner's message is their words, pasted text too)
NOT_IN_WORDS = "the words I quoted from you aren't in what you wrote"
NOT_IN_RECENT = "the words I quoted from you aren't in this message or your recent messages in this chat"
DOESNT_SAY = "the words I quoted from you don't say it"


def blocked(check: str, why: str, outcome: str, todo: str) -> str:
    """The one line: "Blocked by a safety check: <check>. <Why>, so <outcome>. <Todo>"."""
    return f"Blocked by a safety check: {_plain(check)}. {_upper(_plain(why))}, so {_plain(outcome)}. " + \
        _upper(_plain(todo)) + "."


def _plain(text: str) -> str:
    return " ".join(text.split()).rstrip(".")


def _upper(text: str) -> str:
    return text[:1].upper() + text[1:]
