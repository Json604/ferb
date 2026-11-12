"""Owns which coding agents' command-line programs a shell command starts, and whether the task's owner words name
that agent. Must not judge what the command is for: which program runs is a fact about the command, as HAND-429's app
programs are; the owner's words are checked with the one quote check (memory.items.quoted).

Owner, 2026-11-12 (ORCH-634, ORCH-635): "another agent" means another FERB worker, never a coding agent's CLI; "for
them specifially i can either say them by hteir naem or hteir models, can be anythgin". So a command that starts a
known agent CLI (AGENT_CLIS: its program, its npm package through npx / bunx / pnpm dlx / yarn dlx, or its package's
script run by node) is refused unless the task's own owner words (the request, its corrections, the owner's answers)
name that agent or one of its models (Codex's catalog for Codex, the Claude models FERB documents for Claude Code:
runtimes.models.known_models). The word "agent" alone never does.

Read: each simple command after `;`, `&&`, `||`, `|`, `&`, a newline, after its environment assignments and wrappers
(env, nohup, time…), and the command text of `sh -c` / `bash -c`. Residual (accepted, ORCH-635): a program reached
through a name built at run time (`$(echo codex)`), a script file that starts it, or a copy under another name."""

import re
import shlex
from dataclasses import dataclass
from typing import Callable

from ferb.memory.items import quoted


@dataclass(frozen=True)
class AgentCli:
    label: str                     # how FERB names it to the worker
    programs: tuple[str, ...]      # the program's file name
    packages: tuple[str, ...]      # its npm package (npx, bunx, pnpm dlx, a node_modules path)
    names: tuple[str, ...]         # owner words that name it (whole words, case aside)
    models: str = ""               # whose model list also names it (runtimes.models.known_models: "codex" | "claude")


# Agent ruling (ORCH-635): the coding agents' CLIs FERB knows, 2026-11
AGENT_CLIS: tuple[AgentCli, ...] = (
    AgentCli("Codex", ("codex",), ("@openai/codex",), ("codex",), "codex"),
    AgentCli("Claude Code", ("claude",), ("@anthropic-ai/claude-code",), ("claude code", "claude"), "claude"),
    AgentCli("Gemini CLI", ("gemini",), ("@google/gemini-cli",), ("gemini",)),
    AgentCli("Aider", ("aider",), (), ("aider",)),
    AgentCli("Cursor Agent", ("cursor-agent",), (), ("cursor",)),
    AgentCli("OpenCode", ("opencode",), ("opencode-ai",), ("opencode",)),
    AgentCli("Amp", ("amp",), ("@sourcegraph/amp",), ("amp",)),
    AgentCli("Goose", ("goose",), (), ("goose",)),
    AgentCli("Qwen Code", ("qwen",), ("@qwen-code/qwen-code",), ("qwen",)),
    AgentCli("GitHub Copilot CLI", ("copilot",), ("@github/copilot",), ("copilot",)),
    AgentCli("Crush", ("crush",), ("@charmland/crush",), ("crush",)),
    AgentCli("Factory Droid", ("droid",), (), ("droid", "factory")),
    AgentCli("Cline", ("cline",), ("cline",), ("cline",)),
)

REFUSED = ("{agent} is a separate coding agent, and the owner's words in this task don't name it or one of its models, "
           "so FERB didn't start it. \"Another agent\" means another FERB worker, not a coding agent's program: do the "
           "work yourself with FERB's tools, or ask the owner (owner_ask) whether they want {agent} to do it.")

_OPERATORS = {";", "&&", "||", "|", "&", "\n", "(", ")", ";;", "|&"}
_WRAPPERS = {"env", "sudo", "command", "exec", "time", "nohup", "nice", "caffeinate", "builtin", "doas", "timeout",
             "gtimeout", "arch", "stdbuf", "chronic", "unbuffer", "script"}
_ASSIGNMENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*=.*", re.S)
_SHELLS = re.compile(r"(ba|z|da|k|fi)?sh")
_VALUE = re.compile(r"[\d.]+[smhd]?")          # a wrapper's number (nice -n 10, timeout 60s), never a program
_RUNNERS = {"npx": 0, "bunx": 0, "pnpx": 0, "pnpm": 1, "yarn": 1, "bun": 1, "npm": 1}   # words before the package
_RUNNER_VERBS = {"pnpm": ("dlx", "exec"), "yarn": ("dlx",), "bun": ("x",), "npm": ("exec", "x")}


def _words(text: str) -> list[str]:
    try:
        lex = shlex.shlex(text, posix=True, punctuation_chars=";&|<>()\n")
        lex.whitespace, lex.whitespace_split, lex.commenters = " \t\r", True, ""
        return list(lex)
    except ValueError:
        return [w.strip("'\"") for w in re.findall(r"&&|\|\||[;&|()\n]|[^\s;&|()]+", text)]


def _simple(text: str) -> list[list[str]]:
    out: list[list[str]] = [[]]
    for w in _words(text):
        if w in _OPERATORS:
            out.append([])
        elif not (w and set(w) <= set("<>")):
            out[-1].append(w)
    return [c for c in out if c]


def _package(word: str) -> str:
    """An npm package as written (@scope/name@1.2 -> @scope/name)."""
    at = word.find("@", 1)
    return word[:at] if at > 0 else word


def _programs(command: str, depth: int = 0) -> list[str]:
    """What each simple command starts: its program's file name, or `npm:<package>` for a package a runner fetches or a
    node_modules script node runs."""
    found: list[str] = []
    for cmd in _simple(command):
        i = 0
        while i < len(cmd) and (_ASSIGNMENT.fullmatch(cmd[i]) or cmd[i].rpartition("/")[2] in _WRAPPERS
                                or (i > 0 and (cmd[i].startswith("-") or _VALUE.fullmatch(cmd[i])))):
            i += 1
        if i >= len(cmd):
            continue
        prog, rest = cmd[i].rpartition("/")[2], cmd[i + 1:]
        found.append(prog)
        if "node_modules/" in cmd[i]:
            found.append("npm:" + cmd[i].split("node_modules/", 1)[1].rsplit("/bin/", 1)[0])
        if _SHELLS.fullmatch(prog) and "-c" in rest and depth < 3:
            at = rest.index("-c") + 1
            if at < len(rest):
                found += _programs(rest[at], depth + 1)
        if prog == "node":
            script = next((w for w in rest if not w.startswith("-")), "")
            if "node_modules/" in script:
                found.append("npm:" + script.split("node_modules/", 1)[1].rsplit("/bin/", 1)[0].rsplit("/dist/", 1)[0])
        if prog in _RUNNERS:
            args = list(rest)
            if _RUNNERS[prog]:
                if not args or args[0] not in _RUNNER_VERBS.get(prog, ()):
                    continue
                args = args[1:]
            pkg = next((w for w in args if not w.startswith("-")), "")
            if pkg:
                found.append("npm:" + _package(pkg))
    return found


def started(command: str) -> list[AgentCli]:
    """The known agent CLIs this command starts, in the order it names them (each once)."""
    progs = _programs(command)
    out = []
    for agent in AGENT_CLIS:
        if any(p in agent.programs for p in progs) or \
                any(p.startswith("npm:") and (p[4:] in agent.packages or any(p[4:].startswith(k + "/")
                                                                           for k in agent.packages)) for p in progs):
            out.append(agent)
    return out


def _norm(text: str) -> str:
    return re.sub(r"[-_]+", " ", text.casefold())


def model_phrases(model_id: str, vendor: str = "") -> set[str]:
    """How the owner may name a model, from its id alone: each leading run of its parts that ends in a version or is
    the whole id ("gpt 5.1", "gpt 5", "gpt 5.1 codex"), the same without the vendor's own first word ("opus 5.5"),
    and a Claude model's family word ("opus"). Never one generic word of an id ("max", "mini")."""
    parts = _norm(model_id).split()
    out: set[str] = set()
    starts = [0] + ([1] if parts and vendor and parts[0] == vendor else [])
    for s in starts:
        for k in range(s + 1, len(parts) + 1):
            run = parts[s:k]
            if k == len(parts) or any(ch.isdigit() for ch in parts[k - 1]):
                if len(run) > 1 or s == 1:
                    out.add(" ".join(run))
                if "." in run[-1]:
                    out.add(" ".join(run[:-1] + [run[-1].split(".", 1)[0]]))
    if vendor and len(parts) > 1 and parts[0] == vendor and not any(ch.isdigit() for ch in parts[1]):
        out.add(parts[1])
    return {p for p in out if p.strip()}


class AgentCheck:
    """`words(task)`: the task's owner words; `models(kind)`: the model ids FERB knows for "codex" or "claude"."""

    def __init__(self, words: Callable[[str], str], models: Callable[[str], list[str]]) -> None:
        self.words, self.models = words, models

    def named(self, agent: AgentCli, words: str) -> bool:
        if any(quoted(words, name) for name in agent.names):
            return True
        if not agent.models:
            return False
        said = _norm(words)
        vendor = "claude" if agent.models == "claude" else ""
        try:
            ids = self.models(agent.models)
        except Exception:
            ids = []
        return any(quoted(said, phrase) for mid in ids for phrase in model_phrases(mid, vendor))

    def refusal(self, command: str, task_id: str) -> str | None:
        """Why this command can't start (the first agent it starts that the owner's words don't name), else None."""
        agents = started(command)
        if not agents:
            return None
        words = self.words(task_id)
        for agent in agents:
            if not self.named(agent, words):
                return REFUSED.format(agent=agent.label)
        return None
