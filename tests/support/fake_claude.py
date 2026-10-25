"""A stand-in for the pinned Claude Code's `auth login` and `auth status` (L34 C). It prints the sign-in page's address
the way Claude Code 2.1.283 does, takes a pasted code on stdin, "saves" a credential in its config dir as Claude Code
saves one in the Keychain, and even echoes it, so tests can check that FERB keeps none of it."""
import sys
from pathlib import Path

CREDENTIAL = "sk-ant-oat01-FAKE-CREDENTIAL-0123456789"
GOOD_CODE = "fake-auth-code-0123456789"
PAGE = "https://claude.ai/oauth/authorize?code=true&client_id=fake&state=fake-state"

SCRIPT = '''#!{python}
import json, os, pathlib, sys
home = pathlib.Path(os.environ["CLAUDE_CONFIG_DIR"])
saved = home / "fake-keychain-item"
args = sys.argv[1:]
if args[:2] == ["auth", "status"]:
    print(json.dumps({{"loggedIn": saved.exists(), "authMethod": "claude.ai" if saved.exists() else "none",
                      "apiProvider": "firstParty"}}))
    sys.exit(0)
if args == ["auth", "login", "--claudeai"]:
    if os.environ.get("ANTHROPIC_API_KEY") != "" or os.environ.get("CLAUDE_CODE_OAUTH_TOKEN") != "":
        print("a key or token from the environment would take over the login", flush=True)
        sys.exit(3)
    print("Opening browser to sign in... If the browser didn't open, visit: {page}", flush=True)
    sys.stdout.write("Paste code here if prompted > ")
    sys.stdout.flush()
    code = sys.stdin.readline().strip()
    if code != "{code}":
        print("Invalid code. Please make sure the full code was copied", flush=True)
        sys.exit(1)
    saved.write_text("{credential}")
    print("Login successful. Credential {credential}", flush=True)
    sys.exit(0)
print("unknown command", args, flush=True)
sys.exit(2)
'''


def fake_claude(folder: Path) -> Path:
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / "claude"
    path.write_text(SCRIPT.format(python=sys.executable, page=PAGE, code=GOOD_CODE, credential=CREDENTIAL))
    path.chmod(0o755)
    return path
