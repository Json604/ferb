"""Spike 13: MV3 worker killed mid-command -> reconnect with a new generation, no replay, unknown reconciled.

Lab: a spare Chrome driven over the CDP pipe only to load the extension and kill its worker (never the owner's
browser). Waits ~30 s once to measure the alarm-driven reconnect.
"""

import secrets
import time

import pytest

from harness import Lab, wait_until


@pytest.fixture(scope="module")
def lab():
    lab = Lab()
    lab.daemon.wait(lambda m: m["type"] == "hello", 30)
    yield lab
    lab.close()


def kill_and_revive(lab) -> dict:
    """Kill the worker, wake it with a plain tab load, return the new hello."""
    lab.kill_worker()
    lab.daemon.wait(lambda m: m["type"] == "_disconnected", 10)
    lab.open_page("about:blank")
    return lab.daemon.wait(lambda m: m["type"] == "hello", 20)


def marker() -> str:
    return f"data:text/html,<title>ferb13-{secrets.token_hex(4)}</title>"


def count(lab, url: str) -> int:
    return sum(1 for u in lab.page_urls() if u == url)


def test_kill_after_effect_no_replay_and_reconciled_done(lab):
    url = marker()
    e = lab.daemon.send("slow_open", {"url": url, "delay_ms": 4000})
    assert wait_until(lambda: count(lab, url) == 1, 10), "effect never happened"
    old_gen = e.gen
    hello = kill_and_revive(lab)
    assert e.state == "unknown"                              # connection died before the result
    assert hello["gen"] == old_gen + 1                        # generation bump
    assert any(o["id"] == e.id for o in hello["owned"])      # state re-announced after restart
    lab.daemon.reconcile_open(e, hello["owned"])
    assert (e.state, e.reconciled) == ("done", True)
    time.sleep(5)                                            # past the old command's delay
    assert count(lab, url) == 1                              # not executed again
    assert e.result and "tabId" in e.result


def test_kill_before_effect_reconciled_failed(lab):
    url = marker()
    e = lab.daemon.send("delayed_open", {"url": url, "delay_ms": 4000})
    time.sleep(0.5)
    hello = kill_and_revive(lab)
    assert e.state == "unknown"
    lab.daemon.reconcile_open(e, hello["owned"])
    assert (e.state, e.reconciled) == ("failed", True)
    time.sleep(5)
    assert count(lab, url) == 0                              # died before acting, and never replayed


def test_old_generation_command_is_refused(lab):
    hello = kill_and_revive(lab)
    url = marker()
    lab.daemon.send_raw({"type": "cmd", "id": "stale-" + secrets.token_hex(4), "gen": hello["gen"] - 1,
                         "name": "open", "args": {"url": url}})
    m = lab.daemon.wait(lambda m: m["type"] == "stale", 10)
    assert m["current"] == hello["gen"]
    time.sleep(1)
    assert count(lab, url) == 0


def test_duplicate_id_ignored_even_across_worker_restart(lab):
    url = marker()
    e = lab.daemon.send("open", {"url": url})
    lab.daemon.wait(lambda m: m["type"] == "result" and m["id"] == e.id, 10)
    lab.daemon.send_raw({"type": "cmd", "id": e.id, "gen": lab.daemon.gen, "name": "open", "args": {"url": url}})
    lab.daemon.wait(lambda m: m["type"] == "duplicate" and m["id"] == e.id, 10)
    hello = kill_and_revive(lab)                             # seen ids live in storage.session
    lab.daemon.send_raw({"type": "cmd", "id": e.id, "gen": hello["gen"], "name": "open", "args": {"url": url}})
    lab.daemon.wait(lambda m: m["type"] == "duplicate" and m["id"] == e.id, 10)
    time.sleep(1)
    assert count(lab, url) == 1


def test_reconnect_without_any_event_via_alarm(lab):
    lab.kill_worker()
    lab.daemon.wait(lambda m: m["type"] == "_disconnected", 10)
    t0 = time.monotonic()
    hello = lab.daemon.wait(lambda m: m["type"] == "hello", 45)
    assert time.monotonic() - t0 <= 40
    assert hello["gen"] == lab.daemon.gen


def test_daemon_wakes_extension_by_opening_a_blank_page_in_that_profile(lab):
    """Owner decision: don't wait for the alarm — hand the running browser a unique blank page for that profile
    (--profile-directory), wait for the reconnect, then close exactly that page."""
    import http.server
    import subprocess
    import threading

    class Blank(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.send_header("content-type", "text/html")
            self.end_headers()

        def log_message(self, *a):
            pass

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Blank)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    lab.kill_worker()
    lab.daemon.wait(lambda m: m["type"] == "_disconnected", 10)
    # A *unique* blank local page (about:blank#fragment arrives as plain about:blank and can't be told apart
    # from the owner's own blank tabs, so it couldn't be closed safely).
    wake = f"http://127.0.0.1:{srv.server_port}/wake/{secrets.token_hex(8)}"
    t0 = time.monotonic()
    p = subprocess.Popen(["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                          f"--user-data-dir={lab.run / 'profile'}", "--profile-directory=Default", wake],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
    lab.daemon.wait(lambda m: m["type"] == "hello", 10)
    took = time.monotonic() - t0
    assert took < 5, f"wake took {took:.1f}s"
    assert p.wait(timeout=10) == 0                            # handed off to the running browser
    assert wait_until(lambda: count(lab, wake) == 1, 5)
    e = lab.daemon.send("close_url", {"url": wake})
    lab.daemon.wait(lambda m: m["type"] == "result" and m["id"] == e.id, 10)
    assert e.result == {"closed": 1}
    assert wait_until(lambda: count(lab, wake) == 0, 5)
    srv.shutdown()
