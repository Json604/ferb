"""D31 exact-command identity. No command parsing or intent judgment."""
import hashlib
import json


def escape_key(rule: str, command: str, cwd: str) -> str:
    body = json.dumps([command, cwd], ensure_ascii=False, separators=(",", ":"))
    return "escape:" + rule + ":" + hashlib.sha256(body.encode()).hexdigest()
