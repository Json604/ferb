"""`python -m ferb.cli`: the same command line as `ferb` (FERB.app's launcher runs it this way)."""
import sys

from ferb.cli import main

sys.exit(main())
