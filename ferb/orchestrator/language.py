"""P27 audit hints only: do not rewrite answers or use vocabulary as a runtime gate."""
import re

INTERNAL = re.compile(r'\b(?:harness|orchestrator|context envelope|scope grant|tool schema|gateway|worker runtime)\b', re.I)


def internal_terms(text: str) -> list[str]:
    return sorted({match.group(0).lower() for match in INTERNAL.finditer(text)})
