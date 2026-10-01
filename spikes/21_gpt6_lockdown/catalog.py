"""Spike 21: a FERB-owned Codex model catalog where GPT-6 models run without code mode, multi-agent v2 and the
experimental tools. Usage: python catalog.py <codex models_cache.json> <out.json>"""
import json, sys
src, out = sys.argv[1], sys.argv[2]
d = json.load(open(src))
models = d.get("models", d)
for m in models:
    if str(m.get("slug", "")).startswith("gpt-6"):
        m["tool_mode"] = None
        m["multi_agent_version"] = None
        m["multi_agent_reasoning_effort"] = None
        m["experimental_supported_tools"] = []
json.dump(d, open(out, "w"))
print("wrote", out, "| gpt-6 entries:", [m["slug"] for m in models if str(m.get("slug","")).startswith("gpt-6")])
