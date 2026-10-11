"""Call the installed, locally signed secret helper. Raw values travel only on private subprocess pipes."""
import base64
import json
import os
from pathlib import Path
import subprocess
from ferb.core.errors import Refused


def request(body: dict, home: Path | None = None) -> dict:
    root = home or Path(os.environ.get('FERB_HOME', str(Path.home()/'.ferb')))
    binary, requirement = root/'bin'/'ferb-secrets', root/'bin'/'ferb-secrets.requirement'
    if not binary.is_file() or not requirement.is_file():
        raise Refused('the signed secret helper is unavailable; owner setup is required')
    checked = subprocess.run(['/usr/bin/codesign', '--verify', '--strict', '-R=' + requirement.read_text().strip(), str(binary)],
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if checked.returncode:
        raise Refused('the secret helper signature does not match the installed identity')
    result = subprocess.run([str(binary)], input=json.dumps(body).encode(), stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL, close_fds=True)
    if result.returncode:
        raise Refused('the secret operation failed')
    try:
        out = json.loads(result.stdout)
    except (ValueError, UnicodeError):
        raise Refused('the secret helper returned an invalid result') from None
    if not isinstance(out, dict) or out.get('error'):
        raise Refused('the secret operation failed')
    return out


def seal(home: Path, key: bytes, value: str, command: str, origin: str) -> dict[str, str]:
    out = request({'operation': 'seal', 'key': base64.b64encode(key).decode(), 'value': value,
                   'command': command, 'origin': origin}, home)
    return {'nonce': out['nonce'], 'ct': out['ct']}
