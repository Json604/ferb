"""Local app-use evidence for a setup persona. Installed software never establishes an owner's occupation."""
import plistlib
import subprocess
from typing import Any, Callable


def app_usage(apps: list[Any], run: Callable[..., Any] = subprocess.run) -> list[dict[str, Any]]:
    rows = []
    for app in apps:
        try:
            result = run(["/usr/bin/mdls", "-plist", "-", "-name", "kMDItemUseCount", "-name", "kMDItemLastUsedDate", app.path], capture_output=True)
            data = plistlib.loads(result.stdout) if result.returncode == 0 else {}
            count = data.get("kMDItemUseCount")
            rows.append({"bundle_id": app.bundle_id, "app": app.name, "uses": count if isinstance(count, int) else None,
                         "last_used": str(data.get("kMDItemLastUsedDate") or "unknown")})
        except (OSError, ValueError, plistlib.InvalidFileException):
            rows.append({"bundle_id": app.bundle_id, "app": app.name, "uses": None, "last_used": "unknown"})
    return sorted(rows, key=lambda r: -(r["uses"] or 0))


def usage_summary(rows: list[dict[str, Any]]) -> str:
    used = [r for r in rows if isinstance(r.get("uses"), int) and r["uses"] > 0]
    return "Apps you use: " + ", ".join(f"{r['app']} ({r['uses']} recorded uses)" for r in used) if used else "App-use history is unavailable; tell me how you use this Mac."


def confirm_persona(proposal: str, answer: str) -> str | None:
    if not answer.strip() or answer.strip().lower() in {"skip", "no"}:
        return None
    return proposal if answer.strip().lower() in {"yes", "y"} else answer.strip()


def setup_persona(env: Any, config: Any, profile: Any, client: Any,
                  usage: Callable[[list[Any]], list[dict[str, Any]]] = app_usage) -> None:
    if config.get("setup", "persona_reviewed", False):
        return
    from ferb.sanitize.redact import Redactor
    redact = Redactor()
    proposal = redact({"text": usage_summary(usage(profile.apps))})["text"]
    env.say("     " + proposal)
    answer = env.ask("     Does that describe you? yes, edit in your own words, or skip: ")
    confirmed = confirm_persona(proposal, answer)
    if confirmed is not None:
        confirmed = redact({"text": confirmed})["text"]
    if confirmed is not None:
        # Stored as an explicit owner preference through the same CLI memory path, not an inferred identity edit.
        client.call("memory.add", {"content": confirmed, "kind": "preference", "key": "setup-persona"})
        config.set("setup", "persona", confirmed)
    config.set("setup", "persona_reviewed", True)
