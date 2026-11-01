#!/usr/bin/env python3
"""gate.py: the repository's checks, run by git itself at fixed points (installed by the repo-kb skill).

One list of checks lives in kb/gate.toml, each at a level: commit (staged changes, seconds) < push (minutes) <
release. Git hooks in tools/kb/hooks run them, so nobody has to remember the list. A passing push writes a gate note
(git notes --ref=gate) tied to the exact tree; pushes refuse commits without one. Each commit gets a
`Gate-commit:` trailer naming its tree, so `gate audit` (and CI) can see commits that skipped the hooks.
Deterministic: same tree, same result; no model calls. Stdlib only, Python 3.11+ (run it through tools/kb/run).

    tools/kb/run gate list           # the checks and their levels
    tools/kb/run gate push           # run commit+push checks on HEAD and write the gate note
    tools/kb/run gate audit          # commits that skipped hooks, changed the gate, or said `Decisions: none`
    tools/kb/run gate install        # point git at tools/kb/hooks (after kb/setup.toml is answered)
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import platform
import re
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
import kb as kbmod  # noqa: E402
import tomllib  # noqa: E402

VERSION = kbmod.VERSION
LEVELS = ["commit", "push", "release"]
EMPTY_TREE = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
ZERO = "0" * 40
HOOKS_PATH = "tools/kb/hooks"

GATE_DEFAULTS = {
    "base": "main",
    "note_ref": "gate",
    "on_commit_failure": "block",          # block | warn
    "commit_budget_seconds": 30,
    "decisions_trailer": "present",        # off | present | all
    "ci_require_trailers": True,
    "protected": ["tools/kb/**", "kb/gate.toml", "kb/gate-allow.toml", "kb/config.toml", "kb/owner-words.lock",
                  ".github/workflows/**", ".claude/settings.json"],
}

SECRET_PATTERNS = {
    "private key": r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP |ENCRYPTED )?PRIVATE KEY-----",
    "AWS access key": r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
    "GitHub token": r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36,}\b|\bgithub_pat_[A-Za-z0-9_]{50,}\b",
    "Slack token": r"\bxox[abposr]-[A-Za-z0-9-]{10,}\b",
    "Anthropic API key": r"\bsk-ant-[A-Za-z0-9_-]{20,}",
    "OpenAI API key": r"\bsk-(?:proj-|svcacct-)[A-Za-z0-9_-]{20,}|\bsk-[A-Za-z0-9]{40,}\b",
    "Google API key": r"\bAIza[0-9A-Za-z_-]{35}\b",
    "Stripe live key": r"\b[sr]k_live_[0-9A-Za-z]{20,}\b",
    "JSON web token": r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
}
FORBIDDEN_DEFAULT = [".env", ".env.*", "**/.env", "**/.env.*", "**/*.pem", "**/*.p12", "**/*.pfx", "**/id_rsa*",
                     "**/id_ed25519*", "**/*.keychain", "**/*.keychain-db", "kb/local/**"]


# ---------------------------------------------------------------- git

def git(root: Path, *args: str, check: bool = True, input_: str | None = None) -> str:
    r = subprocess.run(["git", "-c", "core.quotepath=off", *args], cwd=root, capture_output=True, text=True,
                       input=input_)
    if check and r.returncode:
        raise SystemExit(f"gate.py: git {' '.join(args)} failed: {r.stderr.strip()}")
    return r.stdout


def git_ok(root: Path, *args: str) -> bool:
    return subprocess.run(["git", *args], cwd=root, capture_output=True).returncode == 0


def rev(root: Path, r: str) -> str | None:
    out = subprocess.run(["git", "rev-parse", "--verify", "--quiet", r], cwd=root, capture_output=True, text=True)
    return out.stdout.strip() or None


def tree_of(root: Path, commit: str) -> str:
    return git(root, "rev-parse", f"{commit}^{{tree}}").strip()


def clean(root: Path) -> bool:
    return not git(root, "status", "--porcelain", "--untracked-files=no").strip()


# ---------------------------------------------------------------- config

def load_gate(root: Path) -> tuple[dict, list[dict], dict]:
    p = root / "kb" / "gate.toml"
    doc = tomllib.loads(p.read_text()) if p.exists() else {}
    cfg = dict(GATE_DEFAULTS)
    cfg.update(doc.get("gate", {}))
    checks = doc.get("check", [])
    for c in checks:
        if c.get("level") not in LEVELS:
            raise SystemExit(f"gate.py: kb/gate.toml: check {c.get('name')!r}: level must be one of {LEVELS}")
        if bool(c.get("builtin")) == bool(c.get("run")):
            raise SystemExit(f"gate.py: kb/gate.toml: check {c.get('name')!r}: give exactly one of builtin / run")
        if c.get("builtin") and c["builtin"] not in BUILTINS:
            raise SystemExit(f"gate.py: kb/gate.toml: check {c.get('name')!r}: unknown builtin {c['builtin']!r} "
                             f"(known: {', '.join(sorted(BUILTINS))})")
    ap = root / "kb" / "gate-allow.toml"
    allow = tomllib.loads(ap.read_text()) if ap.exists() else {}
    for kind in ("secret", "file", "audit", "personal"):
        for a in allow.get(kind, []):
            if not a.get("reason"):
                raise SystemExit(f"gate.py: kb/gate-allow.toml: every [[{kind}]] exception needs a reason")
    return cfg, checks, allow


# ---------------------------------------------------------------- what changed

@dataclass
class Ctx:
    root: Path
    level: str
    base: str | None                      # None: the index against HEAD (commit level)
    cfg: dict
    allow: dict
    changes: list[tuple[str, str, str]] = field(default_factory=list)   # (status, path, old path)
    added: dict[str, list[tuple[int, str]]] = field(default_factory=dict)

    def diff_args(self) -> list[str]:
        if self.base is None:
            return ["--cached"] + (["HEAD"] if rev(self.root, "HEAD") else [EMPTY_TREE])
        return [self.base, "HEAD"]

    def load(self):
        out = git(self.root, "diff", "--name-status", "-z", "-M", "--no-ext-diff", *self.diff_args())
        toks = [t for t in out.split("\0")]
        i = 0
        while i < len(toks) and toks[i]:
            st = toks[i]
            if st[0] in "RC":
                self.changes.append((st[0], toks[i + 2], toks[i + 1])); i += 3
            else:
                self.changes.append((st[0], toks[i + 1], toks[i + 1])); i += 2
        patch = git(self.root, "diff", "-U0", "--no-color", "--no-ext-diff", *self.diff_args())
        cur, n = None, 0
        for line in patch.splitlines():
            if line.startswith("+++ "):
                p = line[4:]
                cur = None if p == "/dev/null" else (p[2:] if p.startswith("b/") else p)
                if cur is not None:
                    self.added.setdefault(cur, [])
            elif line.startswith("@@"):
                m = re.search(r"\+(\d+)", line)
                n = int(m.group(1)) if m else 0
            elif line.startswith("+") and cur is not None:
                self.added[cur].append((n, line[1:])); n += 1

    def paths(self) -> list[str]:
        return [p for st, p, _ in self.changes if st != "D"]

    def size(self, path: str) -> int:
        spec = f":{path}" if self.base is None else f"HEAD:{path}"
        out = subprocess.run(["git", "cat-file", "-s", spec], cwd=self.root, capture_output=True, text=True)
        return int(out.stdout.strip() or 0)


# ---------------------------------------------------------------- built-in checks (each returns problems)

def chk_kb(ctx: Ctx, args: dict) -> list[str]:
    return kbmod.KB(ctx.root).check()


def chk_whitespace(ctx: Ctx, args: dict) -> list[str]:
    r = subprocess.run(["git", "diff", "--check", "--no-ext-diff", *ctx.diff_args()], cwd=ctx.root,
                       capture_output=True, text=True)
    return [l for l in r.stdout.splitlines() if l.strip()][:40]


def _allowed(ctx: Ctx, kind: str, path: str, name: str = "") -> bool:
    for a in ctx.allow.get(kind, []):
        if kbmod.path_matches(a.get("path", ""), path) and (not a.get("pattern") or a.get("pattern") == name):
            return True
    return False


def chk_secrets(ctx: Ctx, args: dict) -> list[str]:
    pats = {**SECRET_PATTERNS, **args.get("extra", {})}
    rx = {k: re.compile(v) for k, v in pats.items()}
    skip = args.get("skip", [])
    out = []
    for path, lines in ctx.added.items():
        if kbmod.any_match(skip, path):
            continue
        for n, text in lines:
            for name, r in rx.items():
                if r.search(text) and not _allowed(ctx, "secret", path, name):
                    out.append(f"{path}:{n}: looks like a {name}. Remove it (and rotate it if it was real), or add a "
                               f"[[secret]] exception with a reason to kb/gate-allow.toml")
    return out


def chk_personal(ctx: Ctx, args: dict) -> list[str]:
    rel = args.get("list", "kb/local/personal.txt")
    p = ctx.root / rel
    out = []
    if git_ok(ctx.root, "ls-files", "--error-unmatch", rel):
        return [f"{rel} is tracked: it must stay local (git rm --cached {rel})"]
    if not p.exists():
        return [f"{rel} is missing (required = true)"] if args.get("required") else []
    terms = [t.strip() for t in p.read_text().splitlines() if t.strip() and not t.startswith("#")]
    low = [(t, t.lower()) for t in terms]
    for path, lines in ctx.added.items():
        if path == rel or _allowed(ctx, "personal", path):
            continue
        for n, text in lines:
            lt = text.lower()
            for t, l in low:
                if l in lt:
                    out.append(f"{path}:{n}: contains a personal term from {rel} ({t[:2]}…). Keep personal data in "
                               f"kb/local/ or other untracked files")
        for t, l in low:
            if l in path.lower():
                out.append(f"{path}: the file name contains a personal term ({t[:2]}…)")
    return out


def chk_forbidden(ctx: Ctx, args: dict) -> list[str]:
    globs = FORBIDDEN_DEFAULT + list(args.get("globs", []))
    cap = args.get("max_file_kb")
    if cap in (None, ""):
        s = kbmod.setup_answer(kbmod.KB(ctx.root), "size_cap_kb")
        cap = int(s) if s.isdigit() else None
    out = []
    for st, path, _ in ctx.changes:
        if st == "D":
            continue
        if kbmod.any_match(globs, path) and not _allowed(ctx, "file", path):
            out.append(f"{path}: this kind of file never goes into the repository")
        if cap and st in "AMRC" and ctx.size(path) > int(cap) * 1024 and not _allowed(ctx, "file", path):
            out.append(f"{path}: {ctx.size(path) // 1024} KB, over the {cap} KB cap (kb/setup.toml size_cap_kb)")
    return out


def chk_append_only(ctx: Ctx, args: dict) -> list[str]:
    globs = args.get("globs", [])
    out = []
    for st, path, old in ctx.changes:
        target = old if st in "RC" else path
        if st in "MDR" and kbmod.any_match(globs, target):
            out.append(f"{target}: append-only (kb/gate.toml); add a new file instead of changing or removing this one")
    return out


def chk_local_ignored(ctx: Ctx, args: dict) -> list[str]:
    out = []
    probe = "kb/local/.probe"
    if not git_ok(ctx.root, "check-ignore", "-q", probe):
        out.append("kb/local/ is not ignored by git: add `kb/local/` to .gitignore")
    tracked = [f for f in git(ctx.root, "ls-files", "kb/local").splitlines() if f]
    out += [f"{f}: tracked, but kb/local/ is local only (git rm --cached {f})" for f in tracked]
    return out


def chk_live(ctx: Ctx, args: dict) -> list[str]:
    """Release: every live check tied to a decision whose files changed since the last tag has a pass or a waiver."""
    since = rev(ctx.root, args["since"]) if args.get("since") else None
    if since is None:
        tag = subprocess.run(["git", "describe", "--tags", "--abbrev=0"], cwd=ctx.root, capture_output=True,
                             text=True).stdout.strip()
        since = rev(ctx.root, tag) if tag else None
    kb = kbmod.KB(ctx.root)
    changed = [l for l in git(ctx.root, "diff", "--name-only", since or EMPTY_TREE, "HEAD").splitlines() if l]
    decs = set()
    for f in changed:
        decs |= set(kb.governing(f))
    out = []
    for k, v in sorted(kb.live.items(), key=lambda x: kbmod.id_key(x[0])):
        linked = set(v.get("decisions") or []) | {d for d in decs if k in (kb.decisions[d].get("live") or [])}
        if linked & decs and kbmod.is_pending(v):
            out.append(f"{k} ({', '.join(sorted(linked & decs))}): no passing result or waiver since "
                       f"{since[:10] if since else 'the start'}: {v.get('title')}")
    return out


BUILTINS = {"kb_check": chk_kb, "whitespace": chk_whitespace, "secrets": chk_secrets, "personal": chk_personal,
            "forbidden": chk_forbidden, "append_only": chk_append_only, "local_ignored": chk_local_ignored,
            "live_checks": chk_live}


# ---------------------------------------------------------------- running checks

@dataclass
class Result:
    name: str
    status: str            # ok | FAIL | skip
    seconds: float
    detail: list[str]


def run_checks(ctx: Ctx, checks: list[dict], upto: str, only_ci: bool = False) -> list[Result]:
    results = []
    files_fd, files_path = tempfile.mkstemp(prefix="gate-files-")
    with os.fdopen(files_fd, "w") as fh:
        fh.write("\n".join(ctx.paths()))
    try:
        for c in checks:
            if LEVELS.index(c["level"]) > LEVELS.index(upto):
                continue
            if only_ci and c.get("ci") is False:
                results.append(Result(c["name"], "skip", 0.0, ["not run in CI (ci = false)"]))
                continue
            when = c.get("when")
            if when and not any(kbmod.any_match(when, p) for p in ctx.paths()):
                results.append(Result(c["name"], "skip", 0.0, ["no changed file matches `when`"]))
                continue
            t0 = time.monotonic()
            if c.get("builtin"):
                try:
                    probs = BUILTINS[c["builtin"]](ctx, c.get("args", {}))
                except SystemExit as e:
                    probs = [str(e)]
                status = "FAIL" if probs else "ok"
                detail = probs
            else:
                # Hooks run with git's GIT_DIR / GIT_INDEX_FILE / GIT_WORK_TREE set to this repository; a check that
                # runs git elsewhere (a test making a temp repo) would otherwise change this repository instead.
                env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
                env.update(GATE_LEVEL=upto, GATE_BASE=ctx.base or "INDEX", GATE_FILES=files_path)
                try:
                    r = subprocess.run(c["run"], shell=True, cwd=ctx.root, env=env, capture_output=True, text=True,
                                       timeout=c.get("timeout", 3600))
                    status = "ok" if r.returncode == 0 else "FAIL"
                    detail = [] if status == "ok" else (r.stdout + r.stderr).strip().splitlines()[-40:]
                except subprocess.TimeoutExpired:
                    status, detail = "FAIL", [f"timed out after {c.get('timeout', 3600)} s"]
            results.append(Result(c["name"], status, round(time.monotonic() - t0, 2), detail))
    finally:
        os.unlink(files_path)
    return results


def report(results: list[Result], title: str) -> bool:
    print(f"gate {title}:", file=sys.stderr)
    for r in results:
        print(f"  {r.status:<4} {r.name} ({r.seconds:.1f}s)", file=sys.stderr)
        if r.status == "FAIL":
            for line in r.detail[:30]:
                print(f"         {line}", file=sys.stderr)
            if len(r.detail) > 30:
                print(f"         ... {len(r.detail) - 30} more", file=sys.stderr)
    return all(r.status != "FAIL" for r in results)


def resolve_base(root: Path, cfg: dict, explicit: str | None) -> str:
    if explicit:
        b = rev(root, explicit)
        if not b:
            raise SystemExit(f"gate.py: base {explicit!r} is not a known commit")
        return b
    head = rev(root, "HEAD")
    up = rev(root, "@{u}")
    if up and up != head and git_ok(root, "merge-base", "--is-ancestor", up, "HEAD"):
        return up
    for b in (f"origin/{cfg['base']}", cfg["base"]):
        r = rev(root, b)
        if r and head:
            mb = git(root, "merge-base", r, "HEAD", check=False).strip()
            if mb and mb != head:
                return mb
    return EMPTY_TREE


# ---------------------------------------------------------------- notes and trailers

def read_note(root: Path, cfg: dict, commit: str) -> dict | None:
    r = subprocess.run(["git", "notes", f"--ref={cfg['note_ref']}", "show", commit], cwd=root, capture_output=True,
                       text=True)
    if r.returncode:
        return None
    try:
        return json.loads(r.stdout.strip().splitlines()[-1])
    except (json.JSONDecodeError, IndexError):
        return None


def note_passes(root: Path, cfg: dict, commit: str) -> bool:
    n = read_note(root, cfg, commit)
    return bool(n and n.get("result") == "pass" and n.get("tree") == tree_of(root, commit)
                and n.get("level") in ("push", "release"))


def write_note(root: Path, cfg: dict, level: str, base: str, results: list[Result]):
    head = rev(root, "HEAD")
    untracked = [l for l in git(root, "status", "--porcelain").splitlines() if l.startswith("??")]
    note = {"tool": "repo-kb gate", "version": VERSION, "level": level, "result": "pass", "head": head,
            "tree": tree_of(root, "HEAD"), "base": base, "when": dt.datetime.now(dt.timezone.utc).isoformat(
                timespec="seconds"), "python": platform.python_version(), "untracked_files": len(untracked),
            "checks": [{"name": r.name, "status": r.status, "seconds": r.seconds} for r in results]}
    git(root, "notes", f"--ref={cfg['note_ref']}", "add", "-f", "-m", json.dumps(note), "HEAD")


def trailers(message: str) -> dict[str, list[str]]:
    lines = [l for l in message.splitlines() if not l.startswith("#")]
    while lines and not lines[-1].strip():
        lines.pop()
    block = []
    for l in reversed(lines):
        if not l.strip():
            break
        block.append(l)
    out: dict[str, list[str]] = {}
    for l in reversed(block):
        m = re.match(r"^([A-Za-z][A-Za-z0-9-]*):\s*(.*)$", l)
        if m:
            out.setdefault(m.group(1).lower(), []).append(m.group(2).strip())
    return out


def listed(values: list[str]) -> set[str]:
    return {x for v in values for x in re.split(r"[,\s]+", v) if x}


def toml_keys_at(root: Path, spec: str) -> set[str]:
    r = subprocess.run(["git", "show", spec], cwd=root, capture_output=True, text=True)
    if r.returncode:
        return set()
    try:
        return set(tomllib.loads(r.stdout))
    except tomllib.TOMLDecodeError:
        return set()


def msg_problems(root: Path, cfg: dict, message: str, staged: list[str], merging: bool) -> list[str]:
    tr = trailers(message)
    out = []
    kb = kbmod.KB(root)
    if cfg["decisions_trailer"] != "off" and not merging:
        gov = sorted({d for f in staged for d in kb.governing(f)}, key=kbmod.id_key)
        given = listed(tr.get("decisions", []))
        if gov and not given:
            out.append(f"these staged files are governed by {', '.join(gov)}: add a trailer `Decisions: <IDs>` "
                       f"(or `Decisions: none` when the change doesn't touch them)")
        if cfg["decisions_trailer"] == "all" and given and given != {"none"}:
            missing = [d for d in gov if d not in given]
            if missing:
                out.append(f"`Decisions:` must name every governing decision; missing {', '.join(missing)}")
        for d in sorted(given - {"none"}):
            if d not in kb.decisions:
                out.append(f"`Decisions:` names {d}, which does not exist")
    removed = toml_keys_at(root, "HEAD:kb/tasks.toml") - toml_keys_at(root, ":kb/tasks.toml")
    if removed and not merging:
        given_t = listed(tr.get("tasks", []))
        miss = sorted(removed - given_t, key=kbmod.id_key)
        if miss:
            out.append(f"tasks {', '.join(miss)} were removed from kb/tasks.toml: name them in a `Tasks:` trailer")
    prot = [f for f in staged if kbmod.any_match(cfg["protected"], f)]
    if prot and not any(v for v in tr.get("gate-change", [])):
        out.append(f"this commit changes the gate itself ({', '.join(prot[:5])}): add `Gate-change: <why>`")
    return out


def state_path(root: Path) -> Path:
    return root / git(root, "rev-parse", "--git-path", "kb-gate-commit.json").strip()


# ---------------------------------------------------------------- commands

def cmd_commit(root: Path) -> int:
    cfg, checks, allow = load_gate(root)
    ctx = Ctx(root, "commit", None, cfg, allow); ctx.load()
    t0 = time.monotonic()
    results = run_checks(ctx, checks, "commit")
    ok = report(results, "commit")
    took = time.monotonic() - t0
    if took > float(cfg["commit_budget_seconds"]):
        print(f"gate: the commit checks took {took:.0f}s (budget {cfg['commit_budget_seconds']}s); move slow ones "
              f"to level = \"push\"", file=sys.stderr)
    failed = [r.name for r in results if r.status == "FAIL"]
    tree = git(root, "write-tree").strip()
    state_path(root).write_text(json.dumps({"tree": tree, "failed": failed}))
    if failed and cfg["on_commit_failure"] == "block":
        print("gate: commit refused. Fix the problems above (the hooks are the repository's rules, not "
              "suggestions).", file=sys.stderr)
        return 1
    return 0


def cmd_msg(root: Path, msg_file: str) -> int:
    cfg, checks, allow = load_gate(root)
    tree = git(root, "write-tree").strip()
    sp = state_path(root)
    state = json.loads(sp.read_text()) if sp.exists() else {}
    failed = state.get("failed", []) if state.get("tree") == tree else None
    if failed is None:                          # pre-commit didn't run for this exact index (merge, -n on pre-commit)
        ctx = Ctx(root, "commit", None, cfg, allow); ctx.load()
        results = run_checks(ctx, checks, "commit")
        report(results, "commit")
        failed = [r.name for r in results if r.status == "FAIL"]
    staged = [l for l in git(root, "diff", "--cached", "--name-only", "-z").split("\0") if l]
    merging = bool(rev(root, "MERGE_HEAD"))
    message = Path(msg_file).read_text(encoding="utf-8", errors="replace")
    probs = msg_problems(root, cfg, message, staged, merging)
    for p in probs:
        print(f"gate: commit message: {p}", file=sys.stderr)
    if probs:
        failed = failed + ["message"]
    if failed and cfg["on_commit_failure"] == "block":
        print("gate: commit refused.", file=sys.stderr)
        return 1
    value = f"pass {tree[:12]}" if not failed else f"warn {tree[:12]} {','.join(failed)}"
    git(root, "interpret-trailers", "--in-place", "--if-exists", "replace", "--trailer", f"Gate-commit: {value}",
        msg_file)
    if sp.exists():
        sp.unlink()
    return 0


def cmd_push(root: Path, base: str | None, level: str = "push") -> int:
    cfg, checks, allow = load_gate(root)
    if not clean(root):
        print("gate: the working tree has uncommitted changes to tracked files. Commit them first: the gate note "
              "must describe exactly the committed tree.", file=sys.stderr)
        return 1
    b = EMPTY_TREE if level == "release" else resolve_base(root, cfg, base)
    ctx = Ctx(root, level, b, cfg, allow); ctx.load()
    results = run_checks(ctx, checks, level)
    if level == "release":
        audit = audit_problems(root, cfg, allow, None)
        results.append(Result("audit", "FAIL" if audit else "ok", 0.0, audit))
    if not report(results, f"{level} (base {b[:10]})"):
        return 1
    write_note(root, cfg, level, b, results)
    print(f"gate: {level} passed; note written on {rev(root, 'HEAD')[:10]}", file=sys.stderr)
    return 0


def cmd_pre_push(root: Path, stdin: str) -> int:
    cfg, _, _ = load_gate(root)
    head = rev(root, "HEAD")
    bad = 0
    for line in stdin.splitlines():
        parts = line.split()
        if len(parts) != 4:
            continue
        local_ref, local_sha, remote_ref, remote_sha = parts
        if local_sha == ZERO or local_ref.startswith("refs/notes/"):
            continue
        commit = rev(root, f"{local_sha}^{{commit}}") or local_sha
        if note_passes(root, cfg, commit):
            continue
        if commit == head and clean(root):
            base = remote_sha if remote_sha != ZERO and git_ok(root, "cat-file", "-e", remote_sha) else None
            if cmd_push(root, base) == 0:
                continue
        print(f"gate: refusing to push {local_ref} ({commit[:10]}): no passing gate note for that exact tree. "
              f"Check it out and run `tools/kb/run gate push`.", file=sys.stderr)
        bad += 1
    return 1 if bad else 0


def cmd_ci(root: Path, base: str | None) -> int:
    cfg, checks, allow = load_gate(root)
    b = resolve_base(root, cfg, base)
    ctx = Ctx(root, "push", b, cfg, allow); ctx.load()
    results = run_checks(ctx, checks, "push", only_ci=True)
    if cfg["ci_require_trailers"] and b != EMPTY_TREE:
        probs = audit_problems(root, cfg, allow, f"{b}..HEAD", strict_only=True)
        results.append(Result("hooks ran on every commit", "FAIL" if probs else "ok", 0.0, probs))
    return 0 if report(results, f"ci (base {b[:10]})") else 1


def audit_rows(root: Path, cfg: dict, allow: dict, rng: str | None) -> list[tuple[str, str, str]]:
    """(commit, severity, text); severity 'problem' fails a release/CI, 'note' is for the owner to read."""
    if rng is None:
        tag = subprocess.run(["git", "describe", "--tags", "--abbrev=0"], cwd=root, capture_output=True,
                             text=True).stdout.strip()
        rng = f"{tag}..HEAD" if tag else "HEAD"
    out = git(root, "log", "--no-merges", "--format=%H%x00%T%x00%B%x1e", rng, check=False)
    allowed = {a["commit"][:10] for a in allow.get("audit", []) if a.get("commit")}
    rows = []
    for chunk in out.split("\x1e"):
        chunk = chunk.strip("\n")
        if not chunk:
            continue
        sha, tree, body = chunk.split("\0", 2)
        if not git_ok(root, "cat-file", "-e", f"{sha}:tools/kb/gate.py"):
            continue                              # made before the gate existed
        if sha[:10] in allowed:
            continue
        tr = trailers(body)
        g = tr.get("gate-commit", [])
        s = sha[:10]
        if not g:
            rows.append((s, "problem", "no Gate-commit trailer: the commit hooks were skipped (--no-verify?)"))
        else:
            parts = g[-1].split()
            if len(parts) < 2 or not tree.startswith(parts[1]):
                rows.append((s, "problem", "its tree differs from the one the gate checked (amended or rebased "
                                           "without hooks)"))
            elif parts[0] == "warn":
                rows.append((s, "problem", f"committed with failing checks: {' '.join(parts[2:])}"))
        if listed(tr.get("decisions", [])) == {"none"}:
            rows.append((s, "note", "said `Decisions: none` while changing governed files"))
        files = [f for f in git(root, "diff-tree", "--no-commit-id", "--name-only", "-r", sha).splitlines() if f]
        prot = [f for f in files if kbmod.any_match(cfg["protected"], f)]
        if prot:
            why = "; ".join(tr.get("gate-change", [])) or "(no reason given)"
            rows.append((s, "note", f"changed the gate ({', '.join(prot[:4])}): {why}"))
    return rows


def audit_problems(root: Path, cfg: dict, allow: dict, rng: str | None, strict_only: bool = False) -> list[str]:
    return [f"{c}: {t}" for c, sev, t in audit_rows(root, cfg, allow, rng) if sev == "problem"]


def cmd_audit(root: Path, rng: str | None) -> int:
    cfg, _, allow = load_gate(root)
    rows = audit_rows(root, cfg, allow, rng)
    head = rev(root, "HEAD")
    print(f"gate audit ({rng or 'since the last tag'}):")
    for c, sev, t in rows:
        print(f"  {'PROBLEM' if sev == 'problem' else 'note   '} {c} {t}")
    if not rows:
        print("  nothing to report")
    if head:
        print(f"  HEAD {head[:10]}: {'passing gate note' if note_passes(root, cfg, head) else 'no passing gate note'}")
    print("Accept a listed commit with an [[audit]] entry (commit + reason) in kb/gate-allow.toml.")
    return 1 if any(sev == "problem" for _, sev, _ in rows) else 0


def cmd_install(root: Path) -> int:
    probs = kbmod.setup_problems(kbmod.KB(root))
    if probs:
        for p in probs:
            print(f"gate: {p}", file=sys.stderr)
        print("gate: answer the setup questions with the owner first (kb/setup.toml); the hooks change how every "
              "commit and push works.", file=sys.stderr)
        return 1
    hooks = root / HOOKS_PATH
    for h in ("pre-commit", "commit-msg", "pre-push"):
        p = hooks / h
        if not p.exists():
            print(f"gate: {HOOKS_PATH}/{h} is missing", file=sys.stderr)
            return 1
        p.chmod(0o755)
    (root / "tools/kb/run").chmod(0o755)
    git(root, "config", "core.hooksPath", HOOKS_PATH)
    print(f"gate: git now runs {HOOKS_PATH}/ on commit and push in this clone. Every other clone runs "
          f"`tools/kb/run gate install` once.")
    return 0


def cmd_hooks_check(root: Path) -> int:
    v = git(root, "config", "--get", "core.hooksPath", check=False).strip()
    if v.rstrip("/") != HOOKS_PATH:
        print(f"gate: this clone doesn't run the repository's hooks (core.hooksPath = {v or 'unset'}). Run "
              f"`tools/kb/run gate install`.", file=sys.stderr)
        return 1
    return 0


def cmd_list(root: Path) -> int:
    cfg, checks, _ = load_gate(root)
    for lvl in LEVELS:
        rows = [c for c in checks if c["level"] == lvl]
        print(f"{lvl}:")
        for c in rows:
            what = f"builtin {c['builtin']}" if c.get("builtin") else c["run"]
            extra = (f"  when {c['when']}" if c.get("when") else "") + ("  (not in CI)" if c.get("ci") is False else "")
            print(f"  {c['name']}: {what}{extra}")
        if not rows:
            print("  (none)")
    print(f"on commit failure: {cfg['on_commit_failure']}; Decisions trailer: {cfg['decisions_trailer']}")
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="gate", description="The repository's checks (repo-kb).")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("commit", help="pre-commit: the commit-level checks on staged changes")
    p = sub.add_parser("msg", help="commit-msg: trailer rules, then the Gate-commit trailer"); p.add_argument("file")
    p = sub.add_parser("push", help="commit+push checks on HEAD; writes the gate note"); p.add_argument("--base")
    p = sub.add_parser("pre-push", help="the pre-push hook"); p.add_argument("rest", nargs="*")
    sub.add_parser("release", help="every check on the whole tree, plus the audit; writes a release note")
    p = sub.add_parser("ci", help="the push-level checks in CI, plus: every commit ran the hooks")
    p.add_argument("--base")
    p = sub.add_parser("audit", help="commits that skipped hooks or changed the gate"); p.add_argument("--range")
    sub.add_parser("install", help="point git at tools/kb/hooks (needs kb/setup.toml answered)")
    sub.add_parser("hooks-check", help="exit 1 when this clone doesn't run the hooks")
    sub.add_parser("list", help="the configured checks")
    sub.add_parser("version")
    a = ap.parse_args(argv)
    root = kbmod.find_root()
    if a.cmd == "commit":
        return cmd_commit(root)
    if a.cmd == "msg":
        return cmd_msg(root, a.file)
    if a.cmd == "push":
        return cmd_push(root, a.base)
    if a.cmd == "pre-push":
        return cmd_pre_push(root, sys.stdin.read())
    if a.cmd == "release":
        return cmd_push(root, None, "release")
    if a.cmd == "ci":
        return cmd_ci(root, a.base)
    if a.cmd == "audit":
        return cmd_audit(root, a.range)
    if a.cmd == "install":
        return cmd_install(root)
    if a.cmd == "hooks-check":
        return cmd_hooks_check(root)
    if a.cmd == "list":
        return cmd_list(root)
    if a.cmd == "version":
        print(VERSION)
    return 0


if __name__ == "__main__":
    sys.exit(main())
