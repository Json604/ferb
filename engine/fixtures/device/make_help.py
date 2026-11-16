"""`ferb --help` as the Python build printed it (T-1128): argparse's help for the program and each of its commands, from
ferb/cli/__init__.py's `build_parser` at commit P (kb/config.toml `historical`), read out of git and run with Python
3.12's argparse at 80 columns. Only `ferb.__version__` and the browsers' keys (ferb/act/browser/profiles.py BROWSERS)
are stood in; nothing else of FERB runs.

Run from the repository: python3.12 -I engine/fixtures/device/make_help.py
Writes engine/fixtures/device/help.json: {"<command path>": "<help text>"}, "" being the program itself."""

import argparse
import json
import os
import subprocess
import sys
import types

P = "9d3f84bc1258a824ddcc43e50e8653fdc5f68c96"
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "help.json")


def source(path: str) -> str:
    return subprocess.run(["git", "show", f"{P}:{path}"], check=True, capture_output=True, text=True).stdout


def build_parser() -> argparse.ArgumentParser:
    text = source("ferb/cli/__init__.py")
    start = text.index("def build_parser()")
    end = text.index("\ndef main(", start)
    browsers = types.ModuleType("ferb.act.browser.profiles")
    profiles = source("ferb/act/browser/profiles.py")
    keys = []
    block = profiles[profiles.index("BROWSERS"):]
    for line in block.splitlines()[1:]:
        line = line.strip()
        if line.startswith("}"):
            break
        if line.startswith('"'):
            keys.append(line.split('"')[1])
    browsers.BROWSERS = {k: None for k in keys}
    ferb = types.ModuleType("ferb")
    ferb.__version__ = "0.28.0"
    for name, module in (("ferb", ferb), ("ferb.act", types.ModuleType("ferb.act")),
                         ("ferb.act.browser", types.ModuleType("ferb.act.browser")),
                         ("ferb.act.browser.profiles", browsers)):
        sys.modules[name] = module
    scope = {"argparse": argparse, "LOG_FLAGS": ("tasks", "trail", "tools", "browsers", "policy", "actions",
                                                  "timers")}
    exec(text[start:end], scope)
    return scope["build_parser"]()


def walk(parser: argparse.ArgumentParser, path: list[str], out: dict[str, str]) -> None:
    out[" ".join(path)] = parser.format_help()
    for action in parser._actions:
        if isinstance(action, argparse._SubParsersAction):
            for name, sub in action.choices.items():
                walk(sub, path + [name], out)


def main() -> None:
    os.environ["COLUMNS"] = "80"
    assert sys.version_info[:2] == (3, 12), "Python 3.12's argparse, as FERB's build used"
    out: dict[str, str] = {}
    walk(build_parser(), [], out)
    with open(OUT, "w", encoding="utf-8") as f:
        json.dump(out, f, indent=1, ensure_ascii=False, sort_keys=True)
        f.write("\n")


if __name__ == "__main__":
    main()
