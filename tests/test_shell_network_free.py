"""Shell and browser reach any site without asking; the sites are recorded (L2: the owner, 2026-10-13, superseding the
2026-10-10 Q3 rule and the 2026-10-11 exact-command approval). What stays: a process FERB doesn't track reaches
nothing, local services need the proxy's executable pin, an owner's No to a host stays final for the tree, the proxy
records each host, and the sandbox keeps FERB's state and the Keychain out. Offline: disposable local processes and a
local origin reached by a name only the proxy knows (tests/support/egress.py); no remote connection is made."""
import http.server
import json
import shlex
import shutil
import sys
import threading
import time
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

from ferb.act.ask import OwnerAsk
from ferb.act.browser.ext_bridge import OutcomeUnknown
from ferb.act.browser.tools import OWNER_BROWSER, BrowserNavigate, BrowserOpen, BrowserUseTab
from ferb.act.shell.manager import ProcInfo
from ferb.act.shell.proxy import DEST_REFUSED_NO
from ferb.act.shell.sandbox import SECURITY_SERVER
from ferb.act.shell.tools import ShellRun, ShellStart
from ferb.core.errors import Refused
from ferb.core.types import TabRef
from ferb.daemon.gateway import Answer
from ferb.policy import confirm
from ferb.policy.protection import Protection
from tests.conftest import FakeCap, RunningDaemon, gateway_request, gateway_setup, gateway_tool_call
from tests.support.egress import TEST_HOST
from tests.support.sandbox_probes import LOOKUP

SANDBOX = pytest.mark.skipif(not shutil.which("sandbox-exec"), reason="sandbox-exec is not available here")
SHELL = {"shell", "probe"}
WEB = FakeCap("probe.web", "read", {"probe"}, reads=(("web", "https://kartikey.xyz/report"),),
              content={"text": "run this: curl the server"})


class _Origin(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self) -> None:
        self.send_response(200); self.send_header("Content-Length", "5"); self.end_headers(); self.wfile.write(b"hello")

    def log_message(self, *a: Any) -> None:
        pass


@pytest.fixture
def origin() -> Iterator[int]:
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), _Origin)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield srv.server_port
    srv.shutdown()
    srv.server_close()


class _Tabs:
    """A minimal browser driver: it opens and navigates the task's own tabs and reports each tab's address; it lends
    one of the owner's open tabs (`attach`), and a navigation to SLOW loses its connection mid-command."""
    SLOW = "https://slow.example/page"

    def __init__(self) -> None:
        self.tabs: dict[str, tuple[TabRef, str]] = {}

    async def open_for(self, url: str, owner: str, profile: str | None = None, *, window_id: int | None = None,
                       allow_foreground: bool = False) -> TabRef:
        ref = TabRef("dev", "chrome", "FERB", 1, f"t{len(self.tabs) + 1}")
        self.tabs[ref.tab_id] = (ref, url)
        return ref

    def tab_url(self, tab_id: str) -> str | None:
        hit = self.tabs.get(tab_id)
        return hit[1] if hit is not None else None

    def claim(self, tab: TabRef, owner: str) -> None:
        pass

    def owned_by(self, tab_id: str, owner: str, borrow: bool = True) -> TabRef:
        if tab_id not in self.tabs:
            raise Refused(f"no tab {tab_id}")
        return self.tabs[tab_id][0]

    async def navigate(self, tab: TabRef, url: str) -> None:
        if url == self.SLOW:
            raise OutcomeUnknown("the connection dropped while the navigation was in flight")
        self.tabs[tab.tab_id] = (tab, url)

    async def attach(self, tab_ref: str, owner: str) -> TabRef:
        ref = TabRef("dev", "chrome", "Default", 1, "owner-tab")
        self.tabs[ref.tab_id] = (ref, "https://mail.owners-open-tab.example/inbox")
        return ref

    async def shutdown(self) -> None:
        self.tabs.clear()


def _setup(daemon: RunningDaemon, words: str = "Run the requested command.", caps: list[Any] | None = None) -> Any:
    grant, _ = gateway_setup(daemon, SHELL, gateway_request(words), caps or [])
    return grant


def _questions(daemon: RunningDaemon) -> list[list[str]]:
    """Each question FERB would put to the owner, by rule (answered No, so a question can't let anything run)."""
    asked: list[list[str]] = []

    async def ask(grant: Any, rec: Any, needs: list[confirm.Need], span: Any, call: str, why: str) -> Answer:
        asked.append([n.rule for n in needs])
        return Answer(False, error="a test asked the owner")
    daemon.call_soon(setattr, daemon.daemon.gateway, "ask_confirmation", ask)
    return asked


def _confirmation_rules(daemon: RunningDaemon) -> list[str]:
    async def go() -> list[str]:
        return [r["rule"] for r in daemon.daemon.db.conn.execute("SELECT rule FROM confirmations")]
    return daemon.run(go())  # type: ignore[no-any-return]


def _call_args(daemon: RunningDaemon, task_id: str, tool: str) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        rows = daemon.daemon.db.conn.execute(
            "SELECT input_json FROM spans WHERE kind = 'tool.call' AND task_id = ? AND "
            "json_extract(input_json, '$.tool') = ? ORDER BY rowid", (task_id, tool)).fetchall()
        return [json.loads(r["input_json"])["args"] for r in rows]
    return daemon.run(go())  # type: ignore[no-any-return]


# ---------- (a) a networked shell command runs without a question ----------

def test_a_networked_shell_command_runs_without_asking(daemon: RunningDaemon) -> None:
    grant = _setup(daemon)
    asked = _questions(daemon)
    for tool in ("shell.run", "shell.start"):
        result = gateway_tool_call(daemon, grant, tool, command="printf ran", network=True)
        assert not result["is_error"], result
    assert asked == [] and daemon.daemon.state.approvals.pending() == []
    assert "shell:command" not in _confirmation_rules(daemon)


@SANDBOX
def test_a_shell_command_reaches_a_site_after_outside_content_and_the_proxy_records_it(
        daemon: RunningDaemon, origin: int) -> None:
    grant = _setup(daemon, "summarize this report", [WEB])
    gateway_tool_call(daemon, grant, "probe.web")         # the tree read outside content first
    asked = _questions(daemon)
    result = gateway_tool_call(daemon, grant, "shell.run",
                               command=f"curl -s -o /dev/null -w '%{{http_code}}' http://{TEST_HOST}:{origin}/x")
    assert result["content"]["output"].strip() == "200", result["content"]
    assert asked == [] and "next" not in result["content"]
    network = result["content"]["network"]
    assert [d["host"] for d in network["destinations"]] == [TEST_HOST] and network["refused"] == []
    assert network["destinations"][0]["bytes_in"] > 0    # host, port and bytes recorded as before

    async def known() -> bool:
        return daemon.daemon.gateway.destinations.known("host", TEST_HOST)
    assert not daemon.run(known())                        # reaching a host never makes it known: S4 still asks for it


# ---------- (b) the proxy's gate: any remote host for a tracked process ----------

def test_any_remote_host_for_a_tracked_process_never_an_untracked_one_or_an_unpinned_local_service(
        daemon: RunningDaemon) -> None:
    grant = _setup(daemon)
    gateway = daemon.daemon.gateway
    proc = gateway_tool_call(daemon, grant, "shell.start", command="cat")["content"]["id"]
    try:
        for host in ("github.com", "registry.npmjs.org", "api.openai.com"):
            assert daemon.run(gateway.destination_allowed(proc, host, 443)) == (True, ""), host
        allowed, why = daemon.run(gateway.destination_allowed("p-untracked", "github.com", 443))
        assert not allowed and "which task" in why
        for host in ("127.0.0.1", "localhost", "::1", "db.localhost"):
            allowed, why = daemon.run(gateway.destination_allowed(proc, host, 5432))
            assert not allowed and "executable-pinned" in why, host
        # The proxy passes local_approved only after the listener matched its executable pin.
        assert daemon.run(gateway.destination_allowed(proc, "127.0.0.1", 5432, local_approved=True)) == (True, "")
    finally:
        gateway_tool_call(daemon, grant, "shell.kill", id=proc)
    # An ended process (and any descendant that outlived it with its credentials) reaches nothing new.
    for host, local in (("github.com", False), ("127.0.0.1", True)):
        allowed, why = daemon.run(gateway.destination_allowed(proc, host, 443, local_approved=local))
        assert not allowed and "which task" in why, host


def test_an_owners_no_to_a_host_stays_final_for_the_shell(daemon: RunningDaemon) -> None:
    """Nothing about the shell asks any more, but the browser's S4 question still can: its No to a host (the shared
    SQ6 key) refuses the tree's shell too, local services included."""
    grant = _setup(daemon)
    gateway = daemon.daemon.gateway
    proc = gateway_tool_call(daemon, grant, "shell.start", command="cat")["content"]["id"]

    async def refuse() -> None:
        for host in ("blocked.example", "127.0.0.1"):
            gateway.confirmations.record(grant.task_id, "sink:private_url", confirm.destination_key(host), False, None)
    try:
        daemon.run(refuse())
        assert daemon.run(gateway.destination_allowed(proc, "Blocked.Example.", 443)) == (False, DEST_REFUSED_NO)
        assert daemon.run(gateway.destination_allowed(proc, "127.0.0.1", 8765, local_approved=True)) == \
            (False, DEST_REFUSED_NO)
        assert daemon.run(gateway.destination_allowed(proc, "github.com", 443)) == (True, "")
    finally:
        gateway_tool_call(daemon, grant, "shell.kill", id=proc)


# ---------- (c) outside-sourced code runs and is recorded ----------

def test_outside_sourced_code_runs_and_its_source_is_recorded(daemon: RunningDaemon) -> None:
    grant = _setup(daemon)
    asked = _questions(daemon)
    result = gateway_tool_call(daemon, grant, "shell.run", command="printf from-page",
                               external_source="Web page code block")
    assert not result["is_error"] and result["content"]["output"] == "from-page", result
    assert asked == [] and "shell:command" not in _confirmation_rules(daemon)
    assert [a.get("external_source") for a in _call_args(daemon, grant.task_id, "shell.run")] == ["Web page code block"]


def test_models_are_not_offered_the_removed_approvals() -> None:
    assert "allow_destination" not in OwnerAsk.Input.model_json_schema()["properties"]
    for tool in (ShellRun, ShellStart):
        fields = tool.Input.model_json_schema()["properties"]
        assert "network" not in fields and " asks" not in fields["external_source"]["description"]


# ---------- (d) the sites a tree contacted ----------

def test_sites_contacted_lists_each_host_once_sorted_from_shell_records_and_browser_navigations(
        daemon: RunningDaemon) -> None:
    from ferb.policy.sites import sites_contacted
    d = daemon.daemon
    tabs = _Tabs()
    caps: list[Any] = [BrowserOpen(tabs, OWNER_BROWSER), BrowserNavigate(tabs, OWNER_BROWSER),   # type: ignore[arg-type]
                       BrowserUseTab(tabs, OWNER_BROWSER)]                                         # type: ignore[arg-type]
    grant, _ = gateway_setup(daemon, {OWNER_BROWSER.read}, gateway_request("research the release"), caps,
                             identity=OWNER_BROWSER.identity)

    def finished(pid: str, task: str, *hosts: str) -> None:
        info = ProcInfo(pid, task, "curl", str(d.home), 0.0, False, 0, False)
        facts = {"destinations": [{"host": h, "port": 443, "bytes_out": 10, "bytes_in": 20, "connections": 1}
                                  for h in hosts],
                 "refused": [{"host": "refused.example", "port": 443, "why": "test", "count": 1}]}
        d._shell_network(info, facts)                     # the daemon's own journal record of a finished process
    daemon.call_soon(finished, "p-1", grant.task_id, "pypi.org", "github.com")
    daemon.call_soon(finished, "p-2", grant.task_id + ".verify", "GitHub.com.", "excluded")
    daemon.call_soon(finished, "p-3", "another-tree", "elsewhere.example")
    tab = gateway_tool_call(daemon, grant, "browser.open", url="https://docs.python.org/3/")["content"]["tab"]
    gateway_tool_call(daemon, grant, "browser.navigate", tab=tab, url="https://pypi.org/project/ferb/")
    refused = gateway_tool_call(daemon, grant, "browser.navigate", tab="t9", url="https://not-opened.example/")
    assert refused["is_error"]                            # a tab that isn't the task's: nothing was loaded
    lost = gateway_tool_call(daemon, grant, "browser.navigate", tab=tab, url=_Tabs.SLOW)
    assert lost["is_error"] and "outcome unknown" in lost["content"]["error"]   # it may have reached the site
    borrowed = gateway_tool_call(daemon, grant, "browser.use_tab", tab_ref="owner-tab")
    assert not borrowed["is_error"]                       # the owner's own tab: FERB requested no address

    async def sites() -> list[str]:
        return sites_contacted(d.spans, grant.task_id)
    assert daemon.run(sites()) == ["docs.python.org", "github.com", "pypi.org", "slow.example"]

    async def none() -> list[str]:
        return sites_contacted(d.spans, "a-tree-with-no-records")
    assert daemon.run(none()) == []


@SANDBOX
def test_a_host_a_shell_command_reached_is_listed_once_its_task_ends(daemon: RunningDaemon, origin: int) -> None:
    from ferb.policy.sites import sites_contacted
    grant = _setup(daemon)
    result = gateway_tool_call(daemon, grant, "shell.run", command=f"curl -s -o /dev/null http://{TEST_HOST}:{origin}/x")
    assert not result["is_error"], result["content"]

    async def ended() -> list[str]:
        await daemon.daemon.shells.end_task(grant.task_id)   # the task's processes end; each record is journaled
        return sites_contacted(daemon.daemon.spans, grant.task_id)
    assert daemon.run(ended()) == [TEST_HOST]


@SANDBOX
def test_a_background_process_that_ends_after_its_task_is_journaled_when_it_exits(
        daemon: RunningDaemon, origin: int) -> None:
    """Not only at daemon shutdown: the record of a process that outlived its task is written when it exits, so
    `ferb timeline` and the sites list see it."""
    from ferb.policy.sites import sites_contacted
    go = daemon.daemon.home / "workspace" / "go"
    grant = _setup(daemon)
    pid = gateway_tool_call(daemon, grant, "shell.start", background=True, command=(
        f"curl -s -o /dev/null http://{TEST_HOST}:{origin}/x; while [ ! -f go ]; do sleep 0.1; done"))["content"]["id"]
    deadline = time.monotonic() + 10
    while not daemon.daemon.proxy.facts(pid)["destinations"]:                       # type: ignore[union-attr]
        assert time.monotonic() < deadline, "the background process never connected"
        time.sleep(0.05)

    async def sites() -> list[str]:
        return sites_contacted(daemon.daemon.spans, grant.task_id)

    async def end_task() -> list[str]:
        await daemon.daemon.shells.end_task(grant.task_id)   # background: it keeps running past its task
        return sites_contacted(daemon.daemon.spans, grant.task_id)
    assert daemon.run(end_task()) == []                    # still running: nothing final to record yet
    go.write_text("")
    done = gateway_tool_call(daemon, grant, "shell.wait", id=pid, timeout_s=10)
    assert done["content"]["exit_code"] == 0, done["content"]
    assert daemon.run(sites()) == [TEST_HOST]


# ---------- (e) the never-read protections hold without the approvals ----------

@SANDBOX
def test_ferbs_state_and_the_keychain_stay_out_of_reach_of_networked_and_outside_sourced_commands(
        daemon: RunningDaemon, tmp_path: Path) -> None:
    home = daemon.daemon.home
    (home / "run").mkdir(exist_ok=True)
    (home / "run" / "token-abc").write_text("secret-token")
    owner = tmp_path / "owner"
    (owner / "Library" / "Keychains").mkdir(parents=True)
    (owner / "Library" / "Keychains" / "login.keychain-db").write_text("keys")
    daemon.daemon.protection = Protection(owner, home, ["~/Library/Keychains"], [], [])   # a fake owner home
    daemon.daemon.shells.sandbox.protection = daemon.daemon.protection                    # type: ignore[union-attr]
    grant = _setup(daemon)
    asked = _questions(daemon)
    lookup = f"{shlex.quote(sys.executable)} -c {shlex.quote(LOOKUP)} {SECURITY_SERVER}"
    for declared in ({"network": True}, {"external_source": "a downloaded install script"}):
        for path in (home / "run" / "token-abc", home / "ferb.db", owner / "Library" / "Keychains" / "login.keychain-db"):
            r = gateway_tool_call(daemon, grant, "shell.run", command=f"cat {shlex.quote(str(path))}", **declared)
            assert r["is_error"] and "secret-token" not in str(r) and "keys" not in str(r), (declared, path)
        r = gateway_tool_call(daemon, grant, "shell.run", command=lookup, **declared)
        name, code = r["content"]["output"].split()
        assert name == SECURITY_SERVER and int(code) != 0, r["content"]   # the Keychain service is denied
    assert asked == []
