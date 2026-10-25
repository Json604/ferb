"""Owns one fact about a sealed value (L36 §7): whether it is a single ordinary word, from the macOS system word list
(/usr/share/dict/words), never a hand-made list. It only tells the owner why FERB hides that word everywhere; it
decides nothing about an action or an intent (rule 6)."""
from pathlib import Path

SYSTEM_WORDS = Path("/usr/share/dict/words")


def ordinary_word(value: str, words: Path | None = None) -> bool:
    """Whether `value` is one word of the system word list, case aside; False when the list can't be read. The list
    (about 236,000 words) is read line by line each time and never kept (scan ruling m7): this runs only when the owner
    saves a sealed answer, off the event loop."""
    word = value.strip()
    if not word or not word.isalpha():
        return False
    wanted = word.casefold()
    try:
        with (words or SYSTEM_WORDS).open(encoding="utf-8", errors="ignore") as lines:
            return any(line.strip().casefold() == wanted for line in lines)
    except OSError:
        return False
