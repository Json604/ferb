"""scripts/symbolicate-hang.py: a hang report's ferbd frames get their functions from the build's symbol map, found by
the UUID the report prints. Invented report and map; stdlib only."""

from __future__ import annotations

import gzip
import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "symbolicate-hang.py"
spec = importlib.util.spec_from_file_location("symbolicate_hang", SCRIPT)
hang = importlib.util.module_from_spec(spec)
spec.loader.exec_module(hang)

UUID = "97B43419-52AC-3281-B18C-4F1154723194"
MAP = """# Path: ferbd
# Sections:
# Address	Size    	Segment	Section
0x100001280	0x021D6FE4	__TEXT	__text
# Symbols:
# Address	Size    	File  Name
0x100000000	0x00000000	[  0] __mh_execute_header
0x100001280	0x00000068	[  1] __RNvCs5WqGp7zLRnA_7___rustc12___rust_alloc
0x100002000	0x00000100	[  2] __RNvMs_NtNtCs7rhly4KsPjI_5ferbd7computeNtB4_6Daemon16sample_resources
"""
REPORT = f"""Process:         FERB [6582]
Call graph:
    2108 Thread_2792673: ferbd
    +   2108 ???  (in FERB)  load address 0x104748000 + 0x2010  [0x10674a010]
    +     2108 ???  (in FERB)  load address 0x104748000 + 0x9000  [0x106751000]
    +       2108 __psynch_mutexwait  (in libsystem_kernel.dylib) + 8  [0x194]
Binary Images:
       0x104748000 -        0x1075e3e03 +com.ferb.daemon (0.28.0 - 202611271702) <{UUID}> /Applications/FERB.app/x
"""


class SymbolicateHang(unittest.TestCase):
    # decision: REL-455
    def test_a_frame_gets_the_function_it_is_in_and_an_unknown_one_stays(self):
        out = hang.symbolicate(REPORT, MAP)
        self.assertIn("ferbd::compute::Daemon::sample_resources  (in FERB) + 0x2010", out)
        self.assertIn("load address 0x104748000 + 0x9000", out)
        self.assertIn("__psynch_mutexwait", out)

    def test_the_map_is_found_by_the_report_s_uuid(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            (tmp / "report.txt").write_text(REPORT)
            (tmp / f"ferbd-{UUID}.map.gz").write_bytes(gzip.compress(MAP.encode()))
            run = subprocess.run(
                [sys.executable, "-I", str(SCRIPT), str(tmp / "report.txt"), str(tmp)],
                capture_output=True,
                text=True,
            )
            self.assertEqual(run.returncode, 0, run.stderr)
            self.assertIn("Daemon::sample_resources", run.stdout)
            (tmp / f"ferbd-{UUID}.map.gz").unlink()
            run = subprocess.run(
                [sys.executable, "-I", str(SCRIPT), str(tmp / "report.txt"), str(tmp)],
                capture_output=True,
                text=True,
            )
            self.assertEqual(run.returncode, 1)
            self.assertIn(UUID, run.stderr)


if __name__ == "__main__":
    unittest.main()
