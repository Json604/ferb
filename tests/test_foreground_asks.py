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
    need = confirm.foreground_need("mac.menu", {"app": "Google Chrome", "path": "Profiles > Sam (Home_Personal)"})
    assert need is not None and need.rule == confirm.FOREGROUND
    assert confirm.foreground_need("mac.menu", {"app": "Google Chrome", "path": ""}) is None
    # any input into a browser through Mac tools asks, even in the background (the extension is the route)
    assert confirm.foreground_need("mac.act", {"app": "Google Chrome", "action": "set_value", "mechanism": "ax_value"})
    assert confirm.foreground_need("mac.click", {"app": "com.brave.Browser", "mechanism": "pointer"})
    # elsewhere a menu choice or background input asks nothing (VS Code's own terminal question still applies)
    assert confirm.foreground_need("mac.menu", {"app": "com.microsoft.VSCode", "path": "Terminal > New Terminal"}) is None
    assert confirm.foreground_need("mac.act", {"app": "Pages", "action": "press", "mechanism": "ax"}) is None


def test_a_browser_addressed_by_pid_still_asks(monkeypatch) -> None:
    from ferb.act.mac import browsers
    monkeypatch.setattr(browsers, "_running_bundles",
                        lambda pid=None, name=None: ["com.google.Chrome"] if pid == 1302 else [])
    assert confirm.foreground_need("mac.click", {"app": "pid:1302", "mechanism": "pointer"}) is not None
    assert confirm.foreground_need("mac.click", {"app": "pid:999", "mechanism": "pointer"}) is None


def test_the_short_question_says_the_tasks_reason() -> None:
    """Live 2026-10-26 (task 7c1eca3a): the owner said Yes to "read Chrome's files", "bring Chrome to the front" and
    "type chrome://extensions/…" without seeing that they served turning off their permit-slots extension. Owner
    2026-10-27: "just ask simply what you want with a small reason below": the reason is its own line, the task's
    words, a FERB lookalike marked as the task's (D33)."""
    need = confirm.foreground_need("mac.click", {"app": "Google Chrome", "allow_foreground": True})
    assert need is not None
    why = "Disable only `Check Slot Alerts - BookSlots` in Chrome Home_Personal"
    assert "Disable" not in confirm.summary([need])
    assert confirm.reason_line(why) == why
    assert confirm.reason_line("FERB: it is fine") == "(from the task) FERB: it is fine"
    assert confirm.reason_line("  ") == ""


def test_one_question_covers_an_app_by_name_pid_or_bundle_id(monkeypatch) -> None:
    """Step 12e, fix 2 (live 2026-10-31: the clean-up task asked about "Terminal", then about "pid:9040"): the answer is
    keyed by the running app, so a retry that spells the same app another way is covered by the same Yes."""
    from ferb.act.mac import apps
    from ferb.act.mac.apps import RunningApp
    monkeypatch.setattr(apps, "running", lambda: [RunningApp("com.apple.finder", "Finder", 96185, False),
                                                  RunningApp("com.apple.Terminal", "Terminal", 9040, True)])
    keys = {confirm.foreground_need("mac.act", {"app": app, "mechanism": "foreground", "action": "press"}).key
            for app in ("Finder", "pid:96185", "com.apple.finder", "‎finder")}
    assert len(keys) == 1
    terminal = confirm.foreground_need("mac.act", {"app": "pid:9040", "mechanism": "foreground", "action": "press"})
    assert terminal is not None and terminal.key not in keys
    assert terminal.key == confirm.foreground_need("mac.click", {"app": "Terminal", "allow_foreground": True}).key


def test_a_script_for_a_browser_asks_like_mac_input_there() -> None:
    """Review 2026-11-01: mac.applescript addressed to a web browser skipped the browser checks (it can act in any of
    the owner's tabs). It now takes the same question as Mac input in that browser, under the same key, and a script
    naming one of the browser's own pages asks every time like input there."""
    import asyncio
    from types import SimpleNamespace
    from ferb.act.mac.tools import MacAppleScript
    script = confirm.foreground_need("mac.applescript", {"app": "Google Chrome", "script": "get URL of active tab"})
    click = confirm.foreground_need("mac.click", {"app": "Google Chrome", "mechanism": "pointer"})
    assert script is not None and click is not None and script.key == click.key
    assert "script" in confirm.summary([script])
    assert confirm.foreground_need("mac.applescript", {"app": "Notes", "script": "get name of every note"}) is None
    tool = MacAppleScript.__new__(MacAppleScript)
    page = 'tell window 1 to make new tab with properties {URL:"chrome://extensions/?id=abc"}'
    assert asyncio.run(tool.browser_page(SimpleNamespace(script=page))) == "chrome://extensions/?id=abc"
    assert asyncio.run(tool.browser_page(SimpleNamespace(script='open location "https://mysite.example"'))) is None
