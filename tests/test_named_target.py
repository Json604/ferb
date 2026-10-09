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


def test_the_checker_is_told_an_uninspectable_named_target_is_unknown_with_how() -> None:
    from pathlib import Path
    text = " ".join((Path(__file__).parents[1] / "ferb" / "orchestrator" / "prompts" / "verifier.md").read_text().split())
    assert "If the named target could not be inspected, that is `met=null`, and give `how`, in the owner's terms" in text


def test_an_unreachable_named_target_ends_not_confirmed_never_done(daemon):
    """The checker couldn't reach what the owner named (met=null) and the orchestrator finishes it as unconfirmed:
    done_unverified with the reason and how to check, never done (V5, D34)."""
    conds = (cond("a Codex session is running with the astra model"),)
    verdict = json.dumps({"results": [{"index": 0, "met": None, "evidence": "Codex's window couldn't be read",
                                       "how": "Open Codex and look for a session using the astra model."}]})
    finish = json.dumps({"action": "finish", "worker_type": "default", "instructions": "Stop and report.",
                         "question": "", "stop_reason": "", "result": "unconfirmed"})
    install(daemon, [sup("task", "On it.", task("start a codex astra session", apps=("Codex",), conds=conds))],
            {"worker": done("started it"), "worker.verify": verdict, "worker.dispatch": finish})
    out = outcome(chat(daemon, "start a codex astra session"))
    assert out["status"] == "done_unverified" and not out["verified"]
    assert out["answer"].startswith("Done, but not confirmed:")
    assert "To check: Open Codex and look for a session using the astra model." in out["answer"]
