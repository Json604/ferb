"""Commands from FERB's own live traces that offline tests replay (L39)."""

# Task 39e35d57 (2026-10-25), span 5265f54b: PyMuPDF installed with certificate checks off for both pip hosts, unasked
LIVE_PIP = ("set -eu\npkgdir=$(mktemp -d)\npython3 -m pip install --quiet --trusted-host pypi.org --trusted-host "
            "files.pythonhosted.org --target \"$pkgdir\" PyMuPDF\nprintf '%s\\n' \"$pkgdir\"")
