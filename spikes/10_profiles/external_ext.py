"""External Extensions file (ARCH 9.6): does each profile get the extension offered, and from which folder is the
file read? Measured in spare --user-data-dir folders with a Web Store extension as stand-in (ours isn't published).

Detection without screenshots: an externally offered extension is recorded in the profile's (Secure) Preferences,
installed but disabled until the owner accepts the prompt.
"""

import json
from pathlib import Path

STANDIN_ID = "aapbdbdomjkkjkaonfhkkikfgjllcleb"  # Google Translate (Web Store, published by Google)
UPDATE_URL = "https://clients2.google.com/service/update2/crx"


def write_external(folder: Path, ext_id: str = STANDIN_ID) -> Path:
    folder.mkdir(parents=True, exist_ok=True)
    f = folder / f"{ext_id}.json"
    f.write_text(json.dumps({"external_update_url": UPDATE_URL}))
    return f


def profile_state(user_data_dir: Path, profile: str, ext_id: str = STANDIN_ID) -> dict | None:
    """The extension's settings entry in this profile, or None if the profile never saw it."""
    for name in ("Secure Preferences", "Preferences"):
        p = user_data_dir / profile / name
        if p.exists():
            entry = json.loads(p.read_text()).get("extensions", {}).get("settings", {}).get(ext_id)
            if entry:
                return {k: entry.get(k) for k in ("location", "state", "disable_reasons", "from_webstore",
                                                  "creation_flags", "path")}
    return None
