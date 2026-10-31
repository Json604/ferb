"""The deterministic profile resolver (ARCH 9.5): the promises are 'never guess' and 'ambiguity is reported'."""

import json

from profiles import BrowserProfile, discover, resolve

P = [
    BrowserProfile("chrome", "Default", "Alex", "k.personal@example.com"),
    BrowserProfile("chrome", "Profile 3", "Work", "k@work.example"),
    BrowserProfile("chrome", "Profile 7", "alex", "k.other@example.com"),   # same name, different case
    BrowserProfile("brave", "Default", "Work", ""),
    BrowserProfile("brave", "Profile 1", "Shopping", "shop@example.com"),
]


def test_exact_display_name_case_insensitive_is_ambiguous_when_two_match():
    r = resolve("ALEX", P)
    assert (r.kind, r.step) == ("ambiguous", "display_name")
    assert {c.folder for c in r.candidates} == {"Default", "Profile 7"}


def test_same_display_name_in_two_browsers_is_ambiguous():
    assert resolve("work", P).kind == "ambiguous"


def test_naming_the_browser_restricts_the_search():
    r = resolve("work", P, browser="brave")
    assert (r.kind, r.candidates[0].folder) == ("one", "Default")


def test_email_and_email_local_part():
    assert resolve("k@work.example", P).candidates[0].folder == "Profile 3"
    r = resolve("shop", P)
    assert (r.kind, r.step, r.candidates[0].browser) == ("one", "email", "brave")


def test_display_name_wins_before_email():
    extra = P + [BrowserProfile("chrome", "Profile 9", "shop", "")]
    r = resolve("shop", extra)
    assert (r.step, r.candidates[0].folder) == ("display_name", "Profile 9")


def test_alias_after_name_and_email():
    r = resolve("my work profile", P, aliases={"my work profile": ("brave", "Default")})
    assert (r.kind, r.step, r.candidates[0].browser) == ("one", "alias", "brave")


def test_no_match_lists_what_exists_and_never_guesses():
    r = resolve("ale", P)  # close to "Alex" — still no match
    assert r.kind == "none" and r.candidates == ()
    assert "no profile called 'ale'; found:" in r.message("ale", P)


def test_discover_reads_only_local_state(tmp_path):
    (tmp_path / "Local State").write_text(json.dumps({"profile": {"info_cache": {
        "Profile 2": {"name": "Two", "user_name": "two@example.com"}, "Default": {"name": "One"}}}}))
    assert discover("chrome", tmp_path) == [BrowserProfile("chrome", "Default", "One", ""),
                                            BrowserProfile("chrome", "Profile 2", "Two", "two@example.com")]
    assert discover("brave", tmp_path / "missing") == []
