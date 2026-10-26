"""Owner 2026-10-26: "didnt i build ferb to do tasks in the background... then why is this happening?" The worker could
mark its own Mac input "foreground, allowed" and take over the owner's screen. Now any input that brings an app to the
front asks the owner first, once per task for that app; the worker's flag alone never does it."""
from ferb.policy import confirm


def test_foreground_input_asks_once_per_task_for_that_app() -> None:
    need = confirm.foreground_need("mac.click", {"app": "Google Chrome", "mechanism": "foreground",
                                                 "allow_foreground": True})
    assert need is not None and need.rule == confirm.FOREGROUND and not need.always
    same = confirm.foreground_need("mac.act", {"app": "google chrome", "allow_foreground": True, "action": "type"})
    assert same is not None and same.key == need.key            # one Yes covers that app for the rest of the task
    other = confirm.foreground_need("mac.act", {"app": "Pages", "mechanism": "foreground"})
    assert other is not None and other.key != need.key


def test_background_input_asks_nothing_outside_browsers() -> None:
    for args in ({"app": "Pages", "mechanism": "ax"}, {"app": "Pages", "mechanism": "pid"},
                 {"app": "Pages", "mechanism": "pointer"}, {"app": "Pages"}):
        assert confirm.foreground_need("mac.click", args) is None
    assert confirm.foreground_need("fs.read", {"allow_foreground": True}) is None


def test_the_question_says_the_app_comes_to_the_front_for_this_task() -> None:
    need = confirm.foreground_need("mac.click", {"app": "Google Chrome", "allow_foreground": True})
    assert need is not None
    text = confirm.summary([need])
    assert "Google Chrome" in text and "front" in text and "this task" in text
    assert "Google Chrome" in confirm.line([need], "mac.click", "")


def test_a_menu_choice_asks_and_listing_menus_does_not() -> None:
    """Live 2026-10-26: the worker switched Chrome profiles through its Profiles menu, moving the owner's Space."""
    need = confirm.foreground_need("mac.menu", {"app": "Google Chrome", "path": "Profiles > Kartikey (New_Personal)"})
    assert need is not None and need.rule == confirm.FOREGROUND
    assert confirm.foreground_need("mac.menu", {"app": "Google Chrome", "path": ""}) is None
    # any input into a browser through Mac tools asks, even in the background (the extension is the route)
    assert confirm.foreground_need("mac.act", {"app": "Google Chrome", "action": "set_value", "mechanism": "ax_value"})
    assert confirm.foreground_need("mac.click", {"app": "com.brave.Browser", "mechanism": "pointer"})
    # elsewhere a menu choice or background input asks nothing (VS Code's own terminal question still applies)
    assert confirm.foreground_need("mac.menu", {"app": "com.microsoft.VSCode", "path": "Terminal > New Terminal"}) is None
    assert confirm.foreground_need("mac.act", {"app": "Pages", "action": "press", "mechanism": "ax"}) is None


def test_a_browser_addressed_by_pid_still_asks(monkeypatch) -> None:
    from ferb.act.mac import apps
    monkeypatch.setattr(apps, "pids_of", lambda ids: {1302} if "com.google.chrome" in ids else set())
    assert confirm.foreground_need("mac.click", {"app": "pid:1302", "mechanism": "pointer"}) is not None
    assert confirm.foreground_need("mac.click", {"app": "pid:999", "mechanism": "pointer"}) is None


def test_the_short_question_says_the_tasks_reason() -> None:
    """Live 2026-10-26 (task 7c1eca3a): the owner said Yes to "read Chrome's files", "bring Chrome to the front" and
    "type chrome://extensions/…" without seeing that they served turning off their visa-slots extension; the reason
    was only under Details. The short line now carries it, quoted as the task's words (D33), never FERB's."""
    need = confirm.foreground_need("mac.click", {"app": "Google Chrome", "allow_foreground": True})
    assert need is not None
    why = "Disable only `Check US Visa Slots - USVisaScheduling` in Chrome New_Personal"
    text = confirm.summary([need], why)
    assert text.endswith(f' The task\'s reason: "{why}".')
    assert confirm.summary([need]) == confirm.summary([need], "  ")      # no reason given: nothing added
