from pathlib import Path
import re

base = Path(__file__).resolve().parent
fragment = (base / "ferb-notch.html").read_text()
preview = fragment.replace('class="viz-carousel"', 'class="concept-gallery"')
preview = re.sub(r'(<section\b[^>]*)( hidden)(>)', r'\1\3', preview)
for name in ("Veil", "Relay", "Thread"):
    preview = preview.replace(f'data-variant="{name}"', f'id="{name.lower()}"')
shell = (base / "preview-shell.html").read_text()
(base / "index.html").write_text(shell.replace("<!-- STUDY -->", preview))
print("Built local review page with all three directions.")
