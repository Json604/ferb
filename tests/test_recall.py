"""Recall: data-driven term filtering (no word list) and whole-word matching for Devanagari."""
from ferb.memory.items import query_terms


def test_query_terms_keep_devanagari_words_whole() -> None:
    assert query_terms("दिल्ली जाऊँगा, bijli ka bill!") == ["दिल्ली", "जाऊँगा", "bijli", "ka", "bill"]
    assert query_terms("The THE the") == ["the"]


def _memory(daemon):
    return daemon.daemon.memory


def _add(daemon, key, content):
    async def go():
        return _memory(daemon).propose("fact", "owner", key, content, "owner_stated", [("journal", "j-" + key)])
    return daemon.run(go())


def _recall(daemon, query):
    async def go():
        return [r.item.key for r in _memory(daemon).recall(query)]
    return daemon.run(go())


def test_common_words_do_not_match_every_memory(daemon) -> None:
    _add(daemon, "diet", "The owner eats the vegetarian diet")
    _add(daemon, "editor", "The owner uses the VS Code editor")
    _add(daemon, "college", "The owner studies at the Scaler school")
    assert _recall(daemon, "delete the hf cache completely") == []
    assert _recall(daemon, "which editor do I use") == ["editor"]


def test_hindi_word_recalls_its_memory(daemon) -> None:
    _add(daemon, "travel", "मैं अगले हफ्ते दिल्ली जाऊँगा")
    _add(daemon, "bills", "The electricity bill (bijli ka bill) is due on the 5th")
    assert _recall(daemon, "दिल्ली कब") == ["travel"]
    assert "bills" in _recall(daemon, "bijli bill")
