"""Writes tab_link.json: one scenario of the real Python `Gateway` rebuilt from a live run on the Rust engine
(2026-11-14, step `cameras`), with invented data, for the Rust test `tests/scenarios.rs`
(`the_shop_link_case_replays_as_python_ran_it`).

The live run: a task opened a search in the owner's paired browser profile, read the results (`browser.snapshot`,
`browser.find`, `browser.html`), listed the profile's tabs (`browser.tabs`, which also lists the owner's own tabs on
other sites), then opened `https://<shop>/dp/<id>` with a product id it had read in a result link's address. The Rust
gateway asked the sink question (sink:private_url). This records what the Python gateway does on the same sequence.

Run from the repo root:  uv run python rust/fixtures/gateway/make_tab_link.py

It reuses make.py's harness (imported: its stubs of every launch path, its temp FERB home, FakeClock, ids, SpecCap).
The tools are SpecCaps shaped as the real browser tools are (ferb/act/browser/tools.py): their kinds, `browser.tabs`'s
`origin = "private"`, and each result's read sources as the real tool gives them (`_page`: ("web", tab url); the
owner's profile adds ("owner_account", "host|") for a snapshot and an html read; `browser.tabs` one ("web", url) per
listed tab). Two runs give the same bytes.
"""

import asyncio
import json
import os
import shutil
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import make  # noqa: E402  (its module setup: temp home, stubs; scenarios.json is not written)

SHOP = "www.amazon.in"
SEARCH = f"https://{SHOP}/s?k=digital+camera+under+3000"
PRODUCT = "B0ZQ7XK2PW"                                   # invented
LINK = (f"https://{SHOP}/Snapcam-Digital-Compact-Vlogging-Camera/dp/{PRODUCT}/ref=sr_1_3?dib_tag=se"
        "&amp;keywords=digital+camera+under+3000&amp;qid=1700000000&amp;sr=8-3")
OTHER_TABS = ["https://notes.example.org/board/today", "https://jobs.example.net/openings/intern"]  # invented
OWNER_PAGE = [["web", SEARCH], ["owner_account", f"{SHOP}|"]]

TABS = [{"tab_ref": f"brave:Default:{100 + i}", "profile": "Brave “Work”", "profile_id": "brave/Default",
         "url": u, "title": t, "site": u.split("/")[2], "active": i == 0, "audible": False, "muted": False}
        for i, (u, t) in enumerate([(OTHER_TABS[0], "Today | Notes"), (OTHER_TABS[1], "Intern openings | Jobs"),
                                    (SEARCH, f"{SHOP} : digital camera under 3000")])]

TOOLS = [
    {"name": "browser.open", "kind": "open_tab", "site_from_url": True,
     "extra_fields": {"url": "str", "profile": "str"},
     "results": [{"reads": [["web", SEARCH]], "content": {"tab": "t1", "url": SEARCH, "profile": "brave/Default"}},
                 {"reads": [["web", f"https://{SHOP}/dp/{PRODUCT}"]],
                  "content": {"tab": "t2", "url": f"https://{SHOP}/dp/{PRODUCT}", "profile": "brave/Default"}}]},
    {"name": "browser.snapshot", "kind": "read", "reversible": True, "observation": True, "site": SHOP,
     "extra_fields": {"tab": "str"},
     "results": [{"reads": OWNER_PAGE,
                  "content": {"tab": "t1", "url": SEARCH, "title": f"{SHOP} : digital camera under 3000",
                              "text": "listitem \"Snapcam 48MP Digital Camera with Flip Screen\" ₹2,499"}}]},
    {"name": "browser.find", "kind": "read", "reversible": True, "observation": True, "site": SHOP,
     "extra_fields": {"tab": "str"},
     "results": [{"reads": [["web", SEARCH]],
                  "content": {"matches": 1, "text": json.dumps([{"element": "s1e64", "role": "listitem",
                                                                 "label": "Snapcam 48MP Digital Camera with Flip Screen"}])}}]},
    {"name": "browser.tabs", "kind": "read", "reversible": True, "observation": True, "origin": "private",
     "extra_fields": {"profile": "str"},
     "results": [{"reads": [["web", t["url"]] for t in TABS], "content": {"tabs": TABS}}]},
    {"name": "browser.html", "kind": "read", "reversible": True, "observation": True, "site": SHOP,
     "extra_fields": {"tab": "str", "element": "str"},
     "results": [{"reads": OWNER_PAGE,
                  "content": {"tab": "t1", "snapshot": 1,
                              "text": f"<!-- frame 0: {SEARCH} -->\n<a class=\"a-link-normal\" target=\"_blank\" "
                                      f"href=\"{LINK}\" ferb-element=\"s1e241\"><h2><span>Snapcam 48MP Digital Camera "
                                      "with Flip Screen</span></h2></a>\n"}}]},
]

SCENARIO = {
    "name": "shop_link_read_in_the_owner_profile",
    "from": ["live 2026-11-14 step cameras (Rust asked sink:private_url)", "GATE-506", "GATE-528", "GATE-541",
             "GATE-568"],
    "tools": TOOLS,
    "requests": [{"id": "req-1", "words": "research digital cameras under 3000 rupees"}],
    "steps": [make.call("browser.open", profile="brave/Work", url=SEARCH),
              make.call("browser.snapshot", tab="t1"),
              make.call("browser.find", tab="t1", text="Snapcam 48MP Digital Camera"),
              make.call("browser.tabs", profile="brave/Work"),
              make.call("browser.html", tab="t1", element="s1e241"),
              make.call("browser.open", [make.YES], profile="brave/Work", url=f"https://{SHOP}/dp/{PRODUCT}")],
}


async def main() -> int:
    out = await make.scenario(SCENARIO)
    data = {"about": "See make_tab_link.py and README.md. One scenario of the real Python Gateway with SpecCap tools.",
            "clock": "2026-01-01T09:00:00+00:00", "scenarios": [out]}
    text = json.dumps(data, ensure_ascii=False, indent=1) + "\n"
    for real in sorted({str(make.ROOT), str(make.ROOT.resolve()), os.path.realpath(make.ROOT)}, key=len, reverse=True):
        text = text.replace(real, "/FX")
    (HERE / "tab_link.json").write_text(text, encoding="utf-8")
    return len(text.encode())


if __name__ == "__main__":
    try:
        size = asyncio.run(main())
    finally:
        shutil.rmtree(make.ROOT, ignore_errors=True)
    print(f"tab_link.json: {size} bytes")
