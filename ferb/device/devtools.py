"""Expose whether this Mac has Apple's developer tools. Must not install anything or start a tool that may be a stub.

On a Mac without the Command Line Tools (or Xcode), /usr/bin/python3, git, make, cc and clang are Apple's xcode-select
stubs: running one fails and opens macOS's "install the command line developer tools" dialog in front of the owner.
`xcode-select -p` only prints the selected developer folder (non-zero when there is none) and opens no dialog."""

import os
import subprocess
from pathlib import Path
from typing import Any, Callable

# The stubs a worker's terminal command is most likely to reach (DMN-511)
STUB_TOOLS = ("python3", "pip3", "git", "make", "cc", "clang")
STUB_FOLDER = Path("/usr/bin")
# Where an install of Apple's developer tools lands (the installer `xcode-select --install` opens, or Xcode)
INSTALL_FOLDERS = (Path("/Library/Developer/CommandLineTools/usr/bin"),
                   Path("/Applications/Xcode.app/Contents/Developer"))


def developer_folder(run: Callable[..., Any] = subprocess.run) -> Path | None:
    """The developer folder `xcode-select -p` names, when it exists; None when none is installed or selected."""
    try:
        done = run(["/usr/bin/xcode-select", "-p"], capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.SubprocessError):
        return None
    folder = Path((done.stdout or "").strip())
    return folder if done.returncode == 0 and folder.is_absolute() and folder.is_dir() else None


def missing_tools(path: str, run: Callable[..., Any] = subprocess.run) -> list[str]:
    """The STUB_TOOLS a command with this PATH would reach only as Apple's stub (or not at all): none when a developer
    folder is installed; otherwise each name whose first match on PATH is in /usr/bin or that has no match (a
    Homebrew python3 or git ahead of the stub counts as installed)."""
    if developer_folder(run) is not None:
        return []
    missing = []
    for name in STUB_TOOLS:
        found = next((Path(d) / name for d in path.split(":")
                      if d and os.access(Path(d) / name, os.X_OK) and (Path(d) / name).is_file()), None)
        if found is None or found.parent == STUB_FOLDER:
            missing.append(name)
    return missing


def installed_now(folders: tuple[Path, ...] = INSTALL_FOLDERS) -> bool:
    """Whether Apple's developer tools have been installed since the start read (a folder check only, cheap enough for
    every prompt build): the Command Line Tools' folder or Xcode."""
    return any(folder.is_dir() for folder in folders)


def missing_fact(missing: list[str]) -> str:
    """The worker's fact for `missing_tools` (empty when nothing is missing). DMN-512 (owner): the worker asks the
    owner once to install the tools when a task needs them; it never runs a stub, which opens Apple's dialog itself."""
    if not missing:
        return ""
    return (f"- This Mac has no developer tools (Apple's Command Line Tools aren't installed): in a terminal command "
            f"{', '.join(missing)} are only Apple's stubs; running one fails and opens macOS's \"install developer "
            "tools\" dialog in front of the owner, so never run them. When a task really needs one, ask the owner "
            "once (owner_ask) to install the developer tools; after their Yes, `xcode-select --install` opens "
            "Apple's installer for them. FERB doesn't install them itself.")
