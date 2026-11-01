"""Every FERB module imports (kb area tests): a module that only runs on some path (a setup step, a
rarely used tool, an adapter the owner hasn't picked) still fails here, not in the owner's hands. Entry points
(`__main__`) are skipped: importing one runs it. A fresh interpreter with a temporary FERB home, so nothing an import
does reaches the owner's ~/.ferb."""
import json
import os
import subprocess
import sys
from pathlib import Path

SWEEP = """
import importlib, json, pkgutil, sys, traceback
import ferb
failed, seen = {}, []
for m in pkgutil.walk_packages(ferb.__path__, "ferb."):
    if m.name.endswith(".__main__"):
        continue
    seen.append(m.name)
    try:
        importlib.import_module(m.name)
    except BaseException:
        failed[m.name] = traceback.format_exc(limit=3)
print(json.dumps({"seen": len(seen), "failed": failed}))
"""


# decision: TEST-500
def test_every_ferb_module_imports(tmp_path: Path) -> None:
    env = {**os.environ, "FERB_HOME": str(tmp_path / "home"), "HOME": str(tmp_path)}
    out = subprocess.run([sys.executable, "-c", SWEEP], capture_output=True, text=True, timeout=300, env=env,
                         cwd=Path(__file__).resolve().parents[1])
    assert out.returncode == 0, out.stderr[-2000:]
    result = json.loads(out.stdout.strip().splitlines()[-1])
    assert result["seen"] > 200 and result["failed"] == {}, result["failed"]
