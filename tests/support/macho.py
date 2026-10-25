"""Tiny real Mach-O executables for packaging tests: one per architecture, or one universal file holding several.
Built with the Xcode clang; nothing here signs or installs anything."""

import shutil
import subprocess
from pathlib import Path

import pytest

SOURCE = "int main(void) { return 0; }\n"


def clang() -> list[str]:
    if shutil.which("clang"):
        return ["clang"]
    if shutil.which("xcrun"):
        return ["xcrun", "clang"]
    pytest.skip("needs clang (Xcode or the Command Line Tools)")


def compile_macho(dest: Path, *archs: str) -> Path:
    """`dest`, an executable built for `archs` (two or more: a universal file); replaces whatever is there."""
    dest.parent.mkdir(parents=True, exist_ok=True)
    src = dest.with_name(dest.name + ".c")
    src.write_text(SOURCE)
    flags = [flag for arch in archs for flag in ("-arch", arch)]
    subprocess.run([*clang(), *flags, "-o", str(dest), str(src)], check=True, capture_output=True)
    src.unlink()
    return dest
