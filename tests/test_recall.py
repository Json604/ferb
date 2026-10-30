"""Recall: data-driven term filtering (no word list) and whole-word matching for Devanagari."""
from ferb.memory.items import query_terms


def test_query_terms_keep_devanagari_words_whole() -> None:
    assert query_terms("दिल्ली जाऊँगा, bijli ka bill!") == ["दिल्ली", "जाऊँगा", "bijli", "ka", "bill"]
    assert query_terms("The THE the") == ["the"]
