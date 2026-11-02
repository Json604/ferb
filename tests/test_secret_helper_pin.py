"""The secret helper ships as pinned bytes (REL-404, T-502): the Keychain admits FERB's team-less helper to each saved
secret by its exact code hash, so a release that compiled it anew (another compiler, SDK or code) made macOS ask again
for every saved secret, with the login password. Reads only the repository's own files."""

import hashlib
import importlib.util
import json
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
_spec = importlib.util.spec_from_file_location("secret_helper", ROOT / "scripts/secret_helper.py")
assert _spec is not None and _spec.loader is not None
helper = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(helper)

CPU = {0x0100000C: "arm64", 0x01000007: "x86_64"}       # mach/machine.h CPU_TYPE_ARM64, CPU_TYPE_X86_64


def _slices(data: bytes) -> dict[str, bytes]:
    magic, count = struct.unpack(">II", data[:8])
    assert magic == 0xCAFEBABE                              # FAT_MAGIC: a universal binary
    out = {}
    for i in range(count):
        cpu, _, offset, size, _ = struct.unpack(">iiIII", data[8 + 20 * i:28 + 20 * i])
        out[CPU.get(cpu, hex(cpu))] = data[offset:offset + size]
    return out


# decision: REL-404
def test_the_pinned_helper_is_the_current_code_for_both_architectures() -> None:
    assert helper.problems() == []
    pin = json.loads(helper.PIN.read_text())
    slices = _slices(helper.BINARY.read_bytes())
    assert sorted(slices) == ["arm64", "x86_64"]
    assert {arch: hashlib.sha256(b).hexdigest() for arch, b in slices.items()} == pin["slices"]
    assert all(b[:4] == b"\xcf\xfa\xed\xfe" for b in slices.values())     # MH_MAGIC_64: each a 64-bit Mach-O


# decision: REL-404
def test_only_a_change_to_the_helpers_code_needs_a_new_pin(tmp_path: Path) -> None:
    source = helper.SOURCE.read_text()
    same = {"a comment": source.replace("import Foundation", "import Foundation  // a comment", 1),
            "a block comment": "/* about it */\n" + source,
            "blank lines": source.replace("\n", "\n\n", 3)}
    changed = {"code": source.replace('"ferb.owner-secrets"', '"ferb.other-secrets"', 1),
               "a string holding //": source.replace('"input"', '"in//put"', 1)}
    for what, text in same.items():
        assert helper.code_sha256(text) == helper.code_sha256(source), what
    for what, text in changed.items():
        assert text != source and helper.code_sha256(text) != helper.code_sha256(source), what
    edited = tmp_path / "secret_helper.swift"
    edited.write_text(changed["code"])
    assert helper.problems(source=edited)                   # build-app.sh refuses to ship a stale helper
    other = tmp_path / "ferb-secrets"
    other.write_bytes(helper.BINARY.read_bytes() + b"\0")
    assert helper.problems(binary=other)
