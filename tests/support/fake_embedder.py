"""A deterministic stand-in for the local embedding model (step 9b): no download, no model. Each word maps to a concept
(a test-only table of words that mean alike), each concept to one of 256 dimensions; a text's vector counts its
concepts. Texts sharing a concept are near; texts sharing none are orthogonal. Tests never download anything."""
import hashlib

from ferb.memory.embeddings import quantize
from ferb.memory.items import query_terms

# Words that mean alike, for tests that find by meaning (the real model learned these from text)
CONCEPTS = {
    "piano": "music", "keyboard": "music", "melody": "music", "chords": "music", "sonata": "music",
    "beginner": "novice", "learning": "novice", "newbie": "novice",
    "visa": "visa", "consulate": "visa", "appointment": "visa", "ofc": "visa", "biometrics": "visa",
    "email": "mail", "mail": "mail", "outreach": "mail", "mailing": "mail", "inbox": "mail",
    "groceries": "food", "blinkit": "food", "zomato": "food", "dinner": "food",
    "tone": "style", "linkedinish": "style", "buzzwords": "style", "jargon": "style",
}


class FakeEmbedder:
    name = "fake-embedder:256"
    dimensions = 256

    def __init__(self) -> None:
        self.calls = 0

    def encode(self, text: str, *, query: bool = False) -> list[int] | None:
        self.calls += 1
        vec = [0.0] * self.dimensions
        for word in query_terms(text):
            concept = CONCEPTS.get(word)
            if concept is None:
                continue                                    # only the table's words carry meaning here
            vec[int(hashlib.sha256(concept.encode()).hexdigest(), 16) % self.dimensions] += 1.0
        return quantize(vec) if any(vec) else None
