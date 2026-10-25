"""Stub codesign and security for signing-step tests (L34 Phase G), the shapes tests/test_packaging.py uses: every
codesign call is logged to $STUB_LOG, the entitlements a call gives are copied beside it, and `-dv` reports ad-hoc."""

import shutil
from pathlib import Path

STUB_CODESIGN = r"""#!/bin/zsh
print -r -- "codesign|${(j:|:)@}" >> "$STUB_LOG"
if [[ " $* " == *" -dv "* ]]; then print -u2 "Signature=adhoc"; exit 0; fi
for (( i = 1; i < $#; i++ )); do
  if [[ "${@[i]}" == --entitlements ]]; then cp "${@[i+1]}" "$STUB_DIR/ent${${@[-1]}//\//_}"; fi
done
exit 0
"""
STUB_SECURITY = r"""#!/bin/zsh
print -r -- "${STUB_IDENTITIES:-}"
print "     0 valid identities found"
"""


def stub_env(tmp_path: Path) -> dict[str, str]:
    stubs = tmp_path / "stubs"
    stubs.mkdir()
    for name, body in (("codesign", STUB_CODESIGN), ("security", STUB_SECURITY)):
        (stubs / name).write_text(body)
        (stubs / name).chmod(0o755)
    return {"PATH": f"{stubs}:/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(tmp_path / "home"),
            "STUB_LOG": str(tmp_path / "calls.log"), "STUB_DIR": str(tmp_path)}


def signed_calls(tmp_path: Path) -> list[str]:
    """The codesign calls that wrote a signature, in order."""
    log = tmp_path / "calls.log"
    calls = log.read_text().splitlines() if log.exists() else []
    return [c for c in calls if c.startswith("codesign") and "|--sign|" in c and "--verify" not in c]


def entitlements(tmp_path: Path, signed: Path) -> str:
    return (tmp_path / ("ent" + str(signed).replace("/", "_"))).read_text()


def macho(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy("/usr/bin/true", path)
