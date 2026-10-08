"""A success condition that names a target is met only by evidence about that target, never a substitute (D34).
The 'Code Astra' live case: a worker that used VS Code Copilot must not verify 'started Codex with the astra model'.
The checker judges which evidence is about the target; code only hands it the owner's named targets and keeps a
met=False verdict from ever becoming done."""
import json

from tests.conftest import chat, cond, done, install, outcome, sup, task

NAMED = "The owner named these; evidence must be about them, not a substitute."


def test_a_substitute_does_not_satisfy_a_named_target_condition(daemon):
    conds = (cond("a Codex session is running with the astra model"),)
    verdict = json.dumps({"results": [{"index": 0, "met": False,
                                       "evidence": "the running agent is VS Code Copilot, not Codex with the astra model"}]})
    finish = json.dumps({"action": "finish", "worker_type": "default", "instructions": "I started it in VS Code",
                         "question": ""})
    install(daemon, [sup("task", "On it.", task("start a codex astra session", apps=("Codex",), conds=conds))],
            {"worker": done("I started it in VS Code"), "worker.verify": verdict, "worker.dispatch": finish})
    out = outcome(chat(daemon, "start a codex astra session"))
    assert out["status"] == "failed" and not out["verified"]
    assert "VS Code Copilot" in out["summary"] and "VS Code Copilot" in out["answer"]


def test_the_named_targets_reach_the_checker_as_a_named_target_not_a_substitute(daemon):
    conds = (cond("the message reached Asmit"),)
    verdict = json.dumps({"results": [{"index": 0, "met": True, "evidence": "Read the chat with Asmit."}]})
    script = install(daemon, [sup("task", "On it.", task("message Asmit", recipients=("Asmit",), apps=("WhatsApp",),
                                                         conds=conds))],
                     {"worker": done("sent"), "worker.verify": verdict})
    chat(daemon, "message Asmit")
    context = next(ctx for _, key, ctx, _ in script.contexts if key == "worker.verify")
    assert "people: Asmit" in context and "apps: WhatsApp" in context and NAMED in context
