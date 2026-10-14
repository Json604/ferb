"""M1: the front tab's URL and title for every Chromium browser whose AppleScript has tabs (Chrome and its channels,
Chromium, Brave, Edge and its channels, Vivaldi, Arc) and Safari, read as two lines rather than an AppleScript list
cut at ", " (a title holding ", " or quotes came back cut, with escaped quotes). No osascript runs: it is faked."""
import subprocess
from typing import Any

import pytest

from ferb.sense.watchers import browser_tabs


@pytest.mark.parametrize("bundle", ["com.google.Chrome", "com.google.Chrome.beta", "com.google.Chrome.canary",
                                    "org.chromium.Chromium", "com.brave.Browser", "com.microsoft.edgemac",
                                    "com.vivaldi.Vivaldi", "company.thebrowser.Browser", "com.apple.Safari"])
def test_the_front_tab_is_read_for_each_browser_with_tab_scripting(bundle: str) -> None:
    assert bundle in browser_tabs.SCRIPTS


def test_a_title_with_commas_and_quotes_comes_back_whole(monkeypatch: pytest.MonkeyPatch) -> None:
    seen: list[list[str]] = []

    def run(argv: list[str], **kw: Any) -> subprocess.CompletedProcess[str]:
        seen.append(argv)
        return subprocess.CompletedProcess(argv, 0, 'https://x.test/a?b=1, 2\nNews, "today", and more\n', "")
    monkeypatch.setattr(browser_tabs.subprocess, "run", run)
    assert browser_tabs.applescript_front_tab("com.microsoft.edgemac") == \
        ("https://x.test/a?b=1, 2", 'News, "today", and more')
    assert "-s" not in seen[0]                                    # plain text, not AppleScript's list notation
    assert 'tell application id "com.microsoft.edgemac"' in seen[0][2]


def test_no_tab_gives_nothing(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(browser_tabs.subprocess, "run",
                        lambda argv, **kw: subprocess.CompletedProcess(argv, 0, "\n", ""))
    assert browser_tabs.applescript_front_tab("com.google.Chrome") is None
