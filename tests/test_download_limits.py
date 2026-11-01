"""HAND-210 (owner, 2026-10-31): one download budget per task tree (300 files, 32 GB) shared by the browser and
FERB's shell, and the 10 GB free-disk floor for both. A shell command receiving bytes through FERB's egress proxy is
stopped at the budget or the floor, and its result says why. Offline: a disposable local origin reached by a name only
the proxy knows (tests/support/egress.py), streaming slowly enough for the watch to see it arrive."""
import http.server
import shutil
import threading
import time
from collections.abc import Iterator
from typing import Any

import pytest

from ferb.act.shell import manager
from ferb.core.origin import PRIVATE
from tests.conftest import RunningDaemon, gateway_request, gateway_setup, gateway_tool_call
from tests.support.egress import TEST_HOST

SANDBOX = pytest.mark.skipif(not shutil.which("sandbox-exec"), reason="sandbox-exec is not available here")
CHUNK, CHUNKS = 64 * 1024, 80                       # 5.2 MB, a chunk every 30 ms: about 2.4 s in all


class _Stream(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self) -> None:
        self.send_response(200)
        self.send_header("Content-Length", str(CHUNK * CHUNKS))
        self.end_headers()
        try:
            for _ in range(CHUNKS):
                self.wfile.write(b"x" * CHUNK)
                self.wfile.flush()
                time.sleep(0.03)
        except (BrokenPipeError, ConnectionResetError):
            pass                                    # the command was stopped mid-download

    def log_message(self, *a: Any) -> None:
        pass


@pytest.fixture
def origin() -> Iterator[int]:
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), _Stream)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield srv.server_port
    srv.shutdown()
    srv.server_close()


@pytest.fixture
def quick_watch(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(manager, "WATCH_S", 0.1)


def _grant(daemon: RunningDaemon) -> Any:
    grant, _ = gateway_setup(daemon, {"shell"}, gateway_request("Download the big file."), [])
    return grant


def _fetch(daemon: RunningDaemon, grant: Any, port: int) -> dict[str, Any]:
    return gateway_tool_call(daemon, grant, "shell.run", command=f"curl -s -o /dev/null http://{TEST_HOST}:{port}/big",
                             timeout_s=30)


def _taken(daemon: RunningDaemon, task: str, size: int) -> None:
    """A file the task's browser tab already took (its downloads row)."""
    async def go() -> None:
        daemon.daemon.db.conn.execute(
            "INSERT INTO downloads (id, task_id, profile, identity, state, path, name, bytes, origin, at) "
            "VALUES ('dl-000000000001', ?, 'chrome/Work', 'owner', 'taken', NULL, 'a.bin', ?, ?, ?)",
            (task, size, PRIVATE, daemon.daemon.clock.now().isoformat()))
    daemon.run(go())


@SANDBOX
def test_a_shell_download_is_stopped_at_the_tasks_budget_and_says_why(daemon: RunningDaemon, origin: int,
                                                                      quick_watch: None) -> None:
    daemon.daemon.config.set("browser", "download_task_max_gb", 0.001)        # 1,000,000 bytes
    grant = _grant(daemon)
    result = _fetch(daemon, grant, origin)
    content = result["content"]
    assert result["is_error"] and "browser.download_task_max_gb" in content["stopped_by_ferb"], content
    assert content["running"] is False                                       # stopped, its whole group
    received = sum(d["bytes_in"] for d in content["network"]["destinations"])
    assert 1_000_000 < received < CHUNK * CHUNKS                             # stopped well before the end


@SANDBOX
def test_a_shell_download_is_stopped_at_the_free_disk_floor(daemon: RunningDaemon, origin: int,
                                                            quick_watch: None) -> None:
    daemon.daemon.downloads.free_bytes = lambda: 9 * 1000 ** 3                # under the 10 GB floor
    grant = _grant(daemon)
    content = _fetch(daemon, grant, origin)["content"]
    assert "browser.download_min_free_gb" in content["stopped_by_ferb"] and content["running"] is False


@SANDBOX
def test_the_budget_is_one_counter_for_the_browser_and_the_shell(daemon: RunningDaemon, origin: int,
                                                                quick_watch: None) -> None:
    """A file the task's tab took counts against what its shell may still receive, and what the shell received
    counts against its next browser download (admission sees both). Shell downloads add no file count."""
    daemon.daemon.config.set("browser", "download_task_max_gb", 0.002)        # 2,000,000 bytes
    grant = _grant(daemon)
    _taken(daemon, grant.task_id, 1_500_000)
    content = _fetch(daemon, grant, origin)["content"]
    assert "browser.download_task_max_gb" in content["stopped_by_ferb"]
    received = sum(d["bytes_in"] for d in content["network"]["destinations"])
    assert 500_000 < received < 2_000_000                                    # the browser's 1.5 MB counted first

    async def after() -> tuple[int, int, str | None]:
        dl = daemon.daemon.downloads
        files, _ = dl._tree_usage(grant.task_id)
        row = daemon.daemon.db.conn.execute("SELECT bytes FROM shell_downloads WHERE tree_id = ?",
                                            (grant.task_id,)).fetchone()
        return files, int(row["bytes"]), dl.shell_stop(grant.task_id)
    files, shell_bytes, why = daemon.run(after())
    assert files == 1 and shell_bytes >= received - CHUNK                    # kept; no file counted for the shell
    assert why is not None and "browser.download_task_max_gb" in why


@SANDBOX
def test_a_command_that_isnt_downloading_is_never_stopped_even_under_the_floor(daemon: RunningDaemon,
                                                                               quick_watch: None) -> None:
    daemon.daemon.downloads.free_bytes = lambda: 1 * 1000 ** 3
    grant = _grant(daemon)
    result = gateway_tool_call(daemon, grant, "shell.run", command="sleep 0.5; printf done")
    assert not result["is_error"] and "stopped_by_ferb" not in result["content"]
    assert result["content"]["output"] == "done"


def test_a_trees_shell_count_is_kept_across_a_restart(daemon: RunningDaemon) -> None:
    grant = _grant(daemon)

    async def go() -> tuple[int, int]:
        dl = daemon.daemon.downloads
        dl.shell_received(grant.task_id, 700)
        dl.shell_received(grant.task_id + ".step2", 300)                      # a step of the same tree
        dl._shell.clear()                                                    # as after a restart: read from the row
        return dl._shell_bytes(grant.task_id), dl._used(grant.task_id)
    assert daemon.run(go()) == (1000, 1000)
