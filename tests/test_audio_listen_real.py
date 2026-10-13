"""L26, owner-observed: FERB hears a disposable page's speech in a lab Chrome, through the real gateway, extension,
ScreenCaptureKit capture and FERB.app's on-device speech recognition; then again with the Mac's output muted.

Run from a normal terminal (it needs Screen Recording for that terminal, since the lab daemon runs in pytest):
    uv run --no-sync pytest -q -s -m real tests/test_audio_listen_real.py
What happens:
- `say -o` writes a spoken sentence to a file (nothing plays yet);
- FERB.app is built and signed into the lab's home. It shares the installed app's bundle id and certificate, so the
  Speech Recognition you allowed in `ferb setup` covers it; the test stops if it doesn't;
- a lab Chrome opens a local page with the sentence as a paused, muted <audio>. FERB plays and unmutes it
  (browser.media), and you hear the sentence for a few seconds (owner accepted, L26). FERB listens with audio.listen,
  then mutes the Mac's output and listens again (your mute setting is restored afterwards), then records the tab
  with sound;
- before each supervised action the test pauses (CLAUDE.md rule 17): press Enter to let it run, then type what you
  saw or heard (cursor, focus, Space, sound), or just Enter for nothing. Each answer is recorded with its step, so a
  disturbance is tied to one action;
- results and transcripts go to result.json in a temporary folder that the test prints. Your verdict is separate
  from the automated result. The captured sound is deleted once transcribed (owner decision, 2026-10-14).
No installed daemon, personal browser profile, microphone or global input is used."""

import http.server
import json
import shutil
import subprocess
import threading
import time
from pathlib import Path
from typing import Any

import pytest

from tests.browserkit import OWNER, call, task
from tests.chromelab import browser_lab

pytestmark = pytest.mark.real

ROOT = Path(__file__).resolve().parents[1]
SENTENCE = "The startup he is talking about is called Ferb Labs. They build agents that work on your Mac."


def _muted() -> bool:
    out = subprocess.run(["osascript", "-e", "output muted of (get volume settings)"], capture_output=True, text=True)
    return out.stdout.strip() == "true"


def _set_muted(on: bool) -> None:
    subprocess.run(["osascript", "-e", f"set volume output muted {'true' if on else 'false'}"], check=True)


def test_hear_a_tab_then_with_the_mac_muted_then_record_it_with_sound(tmp_path: Path) -> None:
    speech = tmp_path / "speech.wav"
    subprocess.run(["say", "-o", str(tmp_path / "speech.aiff"), SENTENCE], check=True)
    subprocess.run(["afconvert", "-f", "WAVE", "-d", "LEI16", str(tmp_path / "speech.aiff"), str(speech)], check=True)
    page = ('<!doctype html><title>FERB hearing check</title><h1>Hearing check</h1>'
            '<audio id="a" controls loop muted src="/speech.wav"></audio>'
            '<button onclick="const a=document.getElementById(\'a\');a.muted=false;a.play()">Play</button>').encode()

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            body, kind = (speech.read_bytes(), "audio/wav") if self.path == "/speech.wav" else (page, "text/html")
            self.send_response(200)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args: Any) -> None:
            pass
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = f"http://127.0.0.1:{server.server_port}/"
    input("Keep using another app; you will hear a spoken sentence a few times. Press Enter when ready. ")
    was_muted = _muted()
    report: dict[str, Any] = {"owner_verdict": "pending", "owner_observations": []}

    def supervised(step: str) -> None:
        """Rule 17: one supervised action at a time; what the owner noticed is recorded against the previous step."""
        input(f"\nNext: {step}. Press Enter to run it. ")
        report["owner_observations"].append({"before": step})

    def observed(step: str) -> None:
        seen = input(f"Done: {step}. What did you see or hear (cursor, focus, Space, sound)? Enter for nothing: ")
        report["owner_observations"].append({"after": step, "owner_saw": seen.strip() or "nothing"})
    try:
        with browser_lab() as (d, chrome, front):
            app = d.daemon.home / "FERB.app"
            subprocess.run([str(ROOT / "macapp" / "FERB" / "build.sh"), str(app)], check=True, capture_output=True)
            shown = subprocess.run(["codesign", "-d", "-r-", str(app)], capture_output=True, text=True)
            req = next(ln.split("=>", 1)[1].strip() for ln in (shown.stdout + shown.stderr).splitlines()
                       if ln.startswith("designated =>"))
            d.daemon.config.set("hearing", "requirement", req)
            status = d.run(d.daemon.hearing.transcriber.status(d.daemon.hearing.folder), timeout=60)
            report["speech_status"] = status
            assert status.get("speech") == "granted", f"allow Speech Recognition for FERB (`ferb setup`): {status}"
            grant = task(d, "hearing-check", OWNER | {"audio", "fs:write"}, "Find the startup he is talking about.")

            def invoke(name: str, timeout: float = 60, **args: Any) -> dict[str, Any]:
                if name == "browser.open":
                    args.setdefault("profile", "Lab")
                result = d.run(d.daemon.gateway.call(grant, name, args), timeout=timeout)
                assert not result["is_error"], result
                return result["content"]
            try:
                supervised("open the disposable page in the lab Chrome (background)")
                tab = invoke("browser.open", url=url)["tab"]
                invoke("browser.wait", tab=tab, until="load")
                observed("open the page")
                report["before"] = invoke("browser.snapshot", tab=tab).get("media")
                supervised("play and unmute the page's audio with browser.media (you should hear the sentence)")
                media_play = d.run(d.daemon.gateway.call(grant, "browser.media", {"tab": tab, "action": "play"}))
                media_unmute = d.run(d.daemon.gateway.call(grant, "browser.media", {"tab": tab, "action": "unmute"}))
                report["media_calls"] = [media_play["content"], media_unmute["content"]]
                state = invoke("browser.snapshot", tab=tab)
                if not (state["media"][0]["playing"] and not state["media"][0]["muted"]):
                    button = next(e["id"] for e in state["elements"] if e["role"] == "button" and e["label"] == "Play")
                    invoke("browser.act", tab=tab, element=button, action="click")   # a real click: user activation
                    report["needed_page_play_button"] = True
                    state = invoke("browser.snapshot", tab=tab)
                report["playing"] = state["media"]
                observed("play and unmute")
                assert state["media"][0]["playing"] and not state["media"][0]["muted"], state["media"]
                supervised("listen to the tab for 8 seconds with audio.listen (output on)")
                heard = invoke("audio.listen", timeout=120, tab=tab, seconds=8)
                report["listen"] = heard
                observed("listen (output on)")
                supervised("mute the Mac's output, listen for 8 seconds, then restore your mute setting")
                _set_muted(True)
                try:
                    muted = d.run(d.daemon.gateway.call(grant, "audio.listen", {"tab": tab, "seconds": 8}), timeout=120)
                finally:
                    _set_muted(was_muted)
                report["listen_with_output_muted"] = muted["content"]           # recorded either way (L26 ruling)
                observed("listen with output muted")
                supervised("record the tab with sound for 6 seconds, then stop the recording")
                rec = invoke("browser.record", tab=tab, path=str(tmp_path / "with-sound.mp4"), fps=2, with_audio=True)
                time.sleep(6)
                stopped = invoke("recording.stop", timeout=180, id=rec["id"])
                report["recording"] = stopped
                if shutil.which("ffprobe"):
                    report["recording_streams"] = json.loads(subprocess.check_output(
                        ["ffprobe", "-v", "error", "-show_entries", "stream=codec_type", "-of", "json", stopped["path"]]))
                observed("record with sound")
                supervised("pause the page's audio")
                invoke("browser.media", tab=tab, action="pause")
                observed("pause")
                report["browser_took_focus"] = front.took_focus(chrome.pid)
                report["focus_changes"] = front.changes
            finally:
                d.run(d.daemon._task_end("hearing-check"), timeout=60)
                report["sound_files_left"] = [str(f) for f in d.daemon.hearing.folder.glob("*.wav")]   # expected: none
                (tmp_path / "result.json").write_text(json.dumps(report, indent=2, default=str))
                print(f"\nResults in {tmp_path / 'result.json'}.")
                print("Heard (output on):", (report.get("listen") or {}).get("transcript"))
                print("Heard (output muted):", (report.get("listen_with_output_muted") or {}).get("transcript"),
                      "| peak", (report.get("listen_with_output_muted") or {}).get("peak"))
            assert "startup" in heard["transcript"].lower(), heard
            assert heard["on_device"] is True and heard["segments"]
            assert stopped.get("transcript"), stopped
            assert not report["sound_files_left"], report["sound_files_left"]      # owner decision: no sound kept
            assert not report["browser_took_focus"], report
    finally:
        if _muted() != was_muted:
            _set_muted(was_muted)
        server.shutdown()
        server.server_close()
