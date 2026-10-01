"""The browser the owner names is the one FERB uses (owner's report 2026-10-01: "my scaler gmail in my chrome" was
opened in their Brave instead). If it isn't connected, FERB says so and how to connect it — never another browser —
and `ferb browsers pair` walks them through installing the extension where it's missing."""

import io
import json
from pathlib import Path
from typing import Any

from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID
from ferb.act.browser.profiles import BrowserProfile, Named, has_extension, named_in
from ferb.cli.admin import browsers
from tests.conftest import RunningDaemon
from tests.test_ext_bridge import OWNER, bd, call, pair, task  # noqa: F401  (bd is a fixture)

KNOWN = [BrowserProfile("chrome", "Default", "Kartikey", "kartikey@gmail.com"),
         BrowserProfile("chrome", "Profile 2", "scaler", "kartikey.24bcs@sst.scaler.com"),
         BrowserProfile("brave", "Default", "Work", "")]


def test_the_profile_and_browser_the_owner_names() -> None:
    n = named_in("open my scaler gmail in chrome and find the FM I assignment", KNOWN)
    assert n is not None and n.browser == "chrome" and [p.folder for p in n.profiles] == ["Profile 2"]
    assert named_in("check the sst.scaler.com mail", KNOWN).profiles[0].folder == "Profile 2"   # by its account
    assert named_in("check gmail in my chrome", KNOWN) == Named("chrome", ())                   # just the browser
    assert named_in("finish the work report", KNOWN) is None                  # "work" alone isn't the Work profile
    assert named_in("use my work profile", KNOWN).profiles[0].browser == "brave"
    assert named_in("search for budgeting tips", KNOWN) is None
    n = named_in("open my scaler gmail in chrome", KNOWN)
    assert n.allows("chrome", "Profile 2") and not n.allows("brave", "Default")
    assert Named("chrome", ()).allows("chrome", "Default") and not Named("chrome", ()).allows("brave", "Default")


def test_whether_the_extension_is_installed_is_read_from_the_profile(tmp_path: Path) -> None:
    (tmp_path / "Profile 2").mkdir()
    p = KNOWN[1]
    assert not has_extension(p, frozenset({FERB_EXTENSION_ID}), tmp_path)
    (tmp_path / "Profile 2" / "Secure Preferences").write_text(json.dumps(
        {"extensions": {"settings": {FERB_EXTENSION_ID: {"location": 4}}}}))
    assert has_extension(p, frozenset({FERB_EXTENSION_ID}), tmp_path)


def test_a_task_never_swaps_in_another_browser(bd: Any) -> None:     # noqa: F811
    d, launched = bd
    pair(d, launched, "Work")
    personal = next(p for p in d.daemon._profiles() if p.display_name == "Personal")
    d.daemon.router.named_for = lambda task_id: Named("chrome", (personal,))    # type: ignore[union-attr]
    g = task(d, "w", OWNER, {"research"})
    r = call(d, g, "browser.open", url="https://mail.google.com/", profile="Work")
    assert r["is_error"] and "the owner asked for chrome “Personal”" in r["content"]["error"]


def test_the_planner_is_told_to_use_it_or_say_how_to_connect_it(bd: Any) -> None:     # noqa: F811
    d, launched = bd
    pair(d, launched, "Work")
    daemon = d.daemon

    async def note_for(words: str) -> str:                                      # on the daemon's own thread
        return str(daemon._named_note(daemon._named_browser(words)))
    note = d.run(note_for("open my Work profile in chrome"))
    assert "use only that" in note and "“Work”" in note
    data = daemon.browser_data_dirs["chrome"] / "Default" / "Secure Preferences"
    data.unlink()                                                               # no extension in Personal
    note = d.run(note_for("open my Personal profile in chrome"))
    assert "can't use it yet: FERB's extension isn't installed there" in note
    assert "Don't use another browser or profile" in note and "`ferb browsers pair Personal`" in note


class FakeClient:
    def __init__(self) -> None:
        self.installed_after, self.calls = 2, []

    def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        self.calls.append(method)
        if method == "browsers.pair" and self.calls.count("browsers.pair") == 1:
            return {"paired": False, "needs_install": True, "browser": "chrome", "folder": "Profile 2",
                    "name": "scaler", "extension_dir": "/repo/extensions/chromium"}
        if method == "browsers.installed":
            self.installed_after -= 1
            return {"installed": self.installed_after <= 0}
        return {"paired": True, "browser": "chrome", "folder": "Profile 2", "name": "scaler"}


def test_pairing_walks_the_owner_through_installing_the_extension_then_pairs() -> None:
    c, out = FakeClient(), io.StringIO()
    assert browsers(c, "pair", "scaler", None, out, sleep=lambda s: None) == 0   # type: ignore[arg-type]
    text = out.getvalue()
    assert "isn't installed in Chrome “scaler”" in text and "Load unpacked" in text
    assert "/repo/extensions/chromium" in text and "Paired chrome profile “scaler”" in text
    assert c.calls == ["browsers.pair", "browsers.installed", "browsers.installed", "browsers.pair"]


def test_a_folder_named_in_the_owners_own_way_counts() -> None:
    """Owner's report (2026-10-01): "clg/uni folder in desktop" was asked about three times because the plan wrote
    Desktop/clg/uni. A path counts when its folder names are all in the owner's words (home folders may be implied)."""
    from ferb.sanitize.untrusted import targets_in_own_words
    said = "do that assignment in clg/uni fodler in desktop"
    assert targets_in_own_words(["Desktop/clg/uni", "~/Desktop/clg/uni", "clg/uni"], said) == []
    assert targets_in_own_words(["Documents/clg/uni"], "the uni folder inside clg") == []
    assert targets_in_own_words(["Desktop/clg/secret"], said) == ["Desktop/clg/secret"]     # never named: refused
    assert targets_in_own_words(["~/.ssh/id_rsa"], said) == ["~/.ssh/id_rsa"]
    assert targets_in_own_words(["Desktop/../clg"], "the clg folder") == []                  # ".." isn't a name
    import os
    home = os.path.expanduser("~")                                         # their home folder is never "a name"
    said2 = "make it complete the assignment in clg:uni folder in desktop"
    assert targets_in_own_words([f"{home}/Desktop/clg/uni", f"{home}/Desktop/clg:uni", "/Users/someone/Desktop/clg/uni"],
                                said2) == []


def test_naming_a_profile_counts_as_asking_for_their_own_browser(bd: Any) -> None:     # noqa: F811
    d, launched = bd
    pair(d, launched, "Work")

    async def owner_browser(words: str) -> bool:
        return bool(d.daemon._owner_browser(words))
    assert d.run(owner_browser("open my Personal gmail account in chrome and search it"))
    assert not d.run(owner_browser("search the web for budgeting tips"))


def test_finder_names_and_home_folders(tmp_path: Path, monkeypatch: Any) -> None:
    """macOS stores a Finder name "clg/uni" as clg:uni on disk; "Desktop/…" is the owner's Desktop, not FERB's
    workspace (owner's report 2026-10-01: FERB made a new empty clg/uni instead)."""
    from ferb.act.fs.files import normalize_path, owner_path
    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    monkeypatch.setenv("HOME", str(tmp_path))                               # ~ too
    (tmp_path / "Desktop" / "clg:uni").mkdir(parents=True)
    (tmp_path / "Desktop" / "clg").mkdir()                                  # a plain clg folder doesn't hide it
    assert owner_path("Desktop/clg/uni", "/ws") == tmp_path / "Desktop" / "clg:uni"
    assert normalize_path("~/Desktop/clg/uni", "/ws") == str(tmp_path / "Desktop" / "clg:uni")
    assert owner_path("notes.md", "/ws") == Path("/ws/notes.md")            # other relative paths: the workspace
    assert owner_path("Desktop/new/x", "/ws") == tmp_path / "Desktop" / "new" / "x"   # nothing there: as written


def test_the_folder_question_comes_for_a_folder_named_in_the_owners_words() -> None:
    """Owner's run (2026-10-01): the plan left ~/Desktop/clg:uni out of its intent, so FERB never asked for access and
    every use was refused. A folder named in the owner's own words now gets the question; all of Desktop never does."""
    import os

    from ferb.daemon.gateway import _named_folder
    home = os.path.expanduser("~")
    said = "make a prompt for a claude session you will manage in clg:uni folder in desktop"
    assert _named_folder(f"{home}/Desktop/clg:uni/prompt.md", said) == f"{home}/Desktop/clg:uni"
    assert _named_folder(f"{home}/Desktop", said) is None                  # never the whole Desktop
    assert _named_folder(f"{home}/Desktop/other/x.md", said) is None       # not a folder they named
    assert _named_folder(f"{home}/Desktop/clg:uni", "") is None            # no words, no question


def test_a_reader_may_click_to_read() -> None:
    """Opening an email or its "View here" link is a click; a reader keeps browser:act / mac:act (the gateway only
    lets it open and look), and only fs:write is turned into fs:read."""
    from ferb.orchestrator import dag
    from ferb.orchestrator.schemas import Plan
    from tests.test_supervisor import plan, sub
    p = Plan.model_validate(plan([sub("r", role="reader", scopes=("browser:act", "browser:read", "fs:write"),
                                      fields=(("x", "string"),))], verbs=("summarize",)))
    dag.tidy(p)
    assert p.subtasks[0].scopes == ["browser:act", "browser:read", "fs:read"]
    assert not [x for x in dag.validate(p, "summarize it", {"browser:act", "browser:read", "fs:read"})
                if "reader only reads" in x]


def test_taking_over_a_tab_also_keeps_to_the_named_profile(bd: Any) -> None:     # noqa: F811
    """Owner's run (2026-10-01): a check step got into Brave by taking over an open tab, though the owner named
    Chrome "scaler". Taking over and listing tabs keep to the named profile too."""
    import asyncio
    d, launched = bd
    pair(d, launched, "Work")
    personal = next(p for p in d.daemon._profiles() if p.display_name == "Personal")
    d.daemon.router.named_for = lambda task_id: Named("chrome", (personal,))    # type: ignore[union-attr]
    work = next(p for p in d.daemon._profiles() if p.display_name == "Work")

    async def attach() -> str:
        try:
            await d.daemon.router.attach(f"{work.browser}:{work.folder}:7", "task-x")
        except Exception as e:                                                      # noqa: BLE001
            return str(e)
        return ""
    assert "the owner asked for chrome “Personal”" in d.run(attach())


def test_typing_a_search_is_a_search_and_a_missing_input_skips_the_step(daemon: RunningDaemon) -> None:
    """Owner's run: searching WhatsApp for "syllabus" was blocked for a reader, and the next step ran with
    "{find_syllabus_link.syllabus_url}" because nothing was found."""
    import asyncio
    from ferb.act.mac.accessibility import AxNode
    from ferb.act.mac.tools import MacAct
    from ferb.policy.rules import Kind

    class Driver:
        def node(self, bundle: str, el: str) -> AxNode:
            return {"s1e1": AxNode("r", "textbox", "Search", None, (0, 0, 1, 1), False, True),
                    "s1e2": AxNode("r", "textbox", "Type a message", None, (0, 0, 1, 1), False, True)}[el]
    tool = MacAct.__new__(MacAct)
    tool.driver = Driver()                                                       # type: ignore[assignment]
    tool._bundle = lambda args: "net.whatsapp.WhatsApp"                          # type: ignore[method-assign]
    search = tool.Input(app="WhatsApp", action="set_value", element="s1e1", text="syllabus")
    message = tool.Input(app="WhatsApp", action="set_value", element="s1e2", text="hi")
    assert tool._kind(search) == Kind.SEARCH and tool._kind(message) == Kind.MAC_INPUT

    from tests.test_supervisor import chat, install, outcome, plan, sub, sup
    p = plan([sub("find", role="reader", scopes=("fs:read",), fields=(("url", "string"),)),
              sub("open", deps=("find",), inputs=(("find", "url"),))], verbs=("summarize",))
    p["subtasks"][1]["goal"] = "open {find.url} in chrome"
    empty = json.dumps({"status": "done", "summary": "nothing found", "fields": {"url": ""}, "artifacts": [],
                        "evidence": []})
    script = install(daemon, [sup("plan", "x", p)], {"find": empty})
    out = outcome(chat(daemon, "summarize it"))
    assert "skipped: an earlier step found no find.url" in out["subtasks"]["open"]["summary"]
    assert not [k for m, k, *_ in script.contexts if k == "open"]                # never ran on an empty value


def test_a_reader_may_open_a_chat_but_never_press_anything_consequential() -> None:
    """Owner's run: the WhatsApp reader couldn't open a chat to find the syllabus link. A reader may press a
    labeled button whose label isn't risky; send / delete / call / unlabeled stay refused (actors unchanged)."""
    from ferb.act.mac.accessibility import AxNode
    from ferb.act.mac.tools import MacAct
    nodes = {"s1e1": AxNode("r", "button", "SST Students Community", None, (0, 0, 1, 1), False, True),
             "s1e2": AxNode("r", "button", "Delete chat", None, (0, 0, 1, 1), False, True),
             "s1e3": AxNode("r", "button", "Send", None, (0, 0, 1, 1), False, True),
             "s1e4": AxNode("r", "button", "", None, (0, 0, 1, 1), False, True),
             "s1e5": AxNode("r", "button", "Voice call", None, (0, 0, 1, 1), False, True),
             "s1e6": AxNode("r", "link", "https://docs.google.com/document/d/x", None, (0, 0, 1, 1), False, True)}

    class Driver:
        def node(self, bundle: str, el: str) -> AxNode:
            return nodes[el]
    tool = MacAct.__new__(MacAct)
    tool.driver = Driver()                                                       # type: ignore[assignment]
    tool._bundle = lambda args: "net.whatsapp.WhatsApp"                          # type: ignore[method-assign]
    ok = lambda el: tool.reader_ok(tool.Input(app="WhatsApp", action="press", element=el))   # noqa: E731
    assert ok("s1e1") and ok("s1e6")
    assert not ok("s1e2") and not ok("s1e3") and not ok("s1e4") and not ok("s1e5")
    assert not tool.reader_ok(tool.Input(app="WhatsApp", action="set_value", element="s1e1", text="x"))


def test_mac_menu_reads_freely_and_presses_only_harmless_items_for_readers() -> None:
    """Apps keep commands in their menus (WhatsApp: File > Search). Listing is reading; a reader may press a harmless
    item, never a risky one; an actor needs mac:act for anything else."""
    from ferb.act.mac.tools import MacMenu
    from ferb.policy.rules import Kind
    tool = MacMenu.__new__(MacMenu)
    tool._bundle = lambda args: "net.whatsapp.WhatsApp"                          # type: ignore[method-assign]
    inp = lambda path: tool.Input(app="WhatsApp", path=path)                      # noqa: E731
    assert tool.describe(inp(None)).kind == Kind.READ
    assert tool.reader_ok(inp("File > Search")) and tool.reader_ok(inp("View > Chats"))
    assert not tool.reader_ok(inp("Chat > Delete Chat")) and not tool.reader_ok(inp("Chat > Voice Call"))
    assert tool.describe(inp("Chat > Voice Call")).kind == Kind.CALL


def test_a_refused_reader_call_is_in_the_trail(daemon: RunningDaemon) -> None:
    """Owner's run: the worker said it was blocked, but `ferb trace` showed nothing. Refusals are recorded."""
    from tests.test_gateway import FakeCap, setup
    send = FakeCap("fake.send", "send", {"browser:read"})
    grant, _ = setup(daemon, {"browser:read"}, None, [send])
    import dataclasses
    record = daemon.daemon.tasks.get("task-1")
    assert record is not None
    record.task = dataclasses.replace(record.task, role="reader")                # this step is a reader
    r = daemon.run(daemon.daemon.gateway.call(grant, "fake.send", {}))
    assert r["is_error"] and "a reader subtask can't send" in r["content"]["error"]

    async def spans() -> list[Any]:
        return daemon.daemon.spans.tree(grant.task_id)                          # type: ignore[union-attr]
    def walk(nodes: list[Any]) -> list[Any]:
        return [x for n in nodes for x in [n.span.error, *walk(n.children)]]
    errs = walk(daemon.run(spans()))
    assert any("a reader subtask can't send" in (e or "") for e in errs)


def test_a_reader_may_type_only_the_owners_own_words(daemon: RunningDaemon) -> None:
    """Owner's audit (2026-10-01): WhatsApp's search box is an unlabeled button, so "is it a search box?" refused the
    reader's background set_value that had worked on Sept 30. A reader may type text from the owner's own words this
    request, into any box; anything else stays refused."""
    gw = daemon.daemon.gateway
    gw.request_words = lambda rid: "search in my WhatsApp for syllabus, then open the latest syllabus link"

    class A:
        def __init__(self, action: str, text: str) -> None:
            self.action, self.text = action, text
    assert gw._reader_types_own_words("mac.act", A("set_value", "syllabus"), "r")
    assert gw._reader_types_own_words("mac.act", A("type", "Syllabus"), "r")             # case doesn't matter
    assert not gw._reader_types_own_words("mac.act", A("set_value", "my bank pin is 1234"), "r")
    assert not gw._reader_types_own_words("mac.act", A("key", "return"), "r")
    assert not gw._reader_types_own_words("browser.act", A("type", "syllabus"), "r")
    gw.request_words = lambda rid: ""
    assert not gw._reader_types_own_words("mac.act", A("set_value", "syllabus"), "r")     # no words, no typing
