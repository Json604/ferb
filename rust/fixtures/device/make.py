"""Fixtures for the Rust device and internal-command port (rust/crates/ferb-device), made by the Python code (ENG-3).

    cd <repo> && uv run python rust/fixtures/device/make.py

Runs ferb/device/install.py's Installer and ferb/cli's commands with the Python test suite's fakes (tests/conftest.py
FakeRunner, SteppedTime; tests/support/app_bundle.py make_ferb_app, BundleRunner) in a fixed temporary folder
(ROOT, removed and made again; never ~/.ferb, ~/Library or a browser's folder), and writes next to this file:

    installer.json  one scenario per tests/test_install.py case: the starting files, then per step the operation, the
                    files the test changed before it, every command the installer ran (argv, exit code, output and the
                    files the fake command made), the answer or the error, the notes, and every file the step changed
    cli.json        the same for ferb/cli/setup.py's commands (setup --service, --cli, uninstall, the sign-outs, the
                    clean-up preview), with every daemon call (method, params, answer) and everything printed
    pure.json       the pure helpers: codesign parsing, requirement checks, launchctl wording, plists as plistlib
                    writes them, device profiles, scans of invented apps, persona facts, devtools, the argument parser,
                    the toolbox's printed lines from invented daemon answers

The root is fixed so that every path, and every hash of a file holding a path (service.json's `definition`, the
Claude Keychain service), is the same on every run and in the Rust replay. `gui/<uid>` is written `gui/$UID`; the
checkout's secret-helper source is written under `$REPO`. Everything else is invented.
"""

import contextlib
import io
import json
import math
import os
import plistlib
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Callable

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
ROOT = Path("/private/tmp/ferb-device-fx")
UID = os.getuid()

shutil.rmtree(ROOT, ignore_errors=True)
ROOT.mkdir(parents=True)
os.environ["HOME"] = str(ROOT / "user")
os.environ["FERB_HOME"] = str(ROOT / "user" / ".ferb")
sys.path.insert(0, str(REPO))

import ferb.device.install as install_mod  # noqa: E402
import ferb.device.extensions as extensions_mod  # noqa: E402
from ferb.device.install import Installer  # noqa: E402
from ferb.config import Config  # noqa: E402
from ferb.runtimes.codex_exec import CODEX_VERSION, _NATIVE_BIN  # noqa: E402
from tests.conftest import FakeRunner, SteppedTime  # noqa: E402
from tests.support.app_bundle import AGENT, PINNED, BundleRunner, make_ferb_app  # noqa: E402


def write(name: str, value: Any) -> None:
    (HERE / name).write_text(json.dumps(value, indent=1, ensure_ascii=False) + "\n")


def sub(text: Any) -> Any:
    """Recorded text: this Mac's uid and the checkout's path as placeholders."""
    if isinstance(text, str):
        return text.replace(f"gui/{UID}", "gui/$UID").replace(str(REPO), "$REPO")
    if isinstance(text, list):
        return [sub(t) for t in text]
    if isinstance(text, dict):
        return {k: sub(v) for k, v in text.items()}
    return text


# ---------- files ----------

def content(data: bytes) -> dict[str, str]:
    try:
        return {"text": data.decode("utf-8")}
    except UnicodeDecodeError:
        return {"hex": data.hex()}


def snap(root: Path) -> dict[str, dict[str, Any]]:
    """Every entry under `root`: a folder (mode), a file (mode, bytes) or a link (target)."""
    out: dict[str, dict[str, Any]] = {}
    for dirpath, dirnames, filenames in os.walk(root):
        for name in sorted(dirnames + filenames):
            p = Path(dirpath) / name
            rel = str(p.relative_to(root))
            st = p.lstat()
            if stat.S_ISLNK(st.st_mode):
                out[rel] = {"kind": "link", "target": os.readlink(p)}
            elif stat.S_ISDIR(st.st_mode):
                out[rel] = {"kind": "dir", "mode": st.st_mode & 0o7777}
            else:
                out[rel] = {"kind": "file", "mode": st.st_mode & 0o7777, **content(p.read_bytes())}
    return dict(sorted(out.items()))


def changes(before: dict[str, Any], after: dict[str, Any]) -> list[dict[str, Any]]:
    """What changed between two snapshots: entries added or changed, then entries gone."""
    out = [{"path": k, **v} for k, v in after.items() if before.get(k) != v]
    out += [{"path": k, "kind": "gone"} for k in before if k not in after]
    return out


def tree(root: Path) -> list[dict[str, Any]]:
    return [{"path": k, **v} for k, v in snap(root).items()]


# ---------- a scenario: steps with every command and file ----------

class Names:
    """tempfile's random names, made deterministic (the Rust replay reads `.ferb-lock-` names as any eight)."""

    def __init__(self) -> None:
        self.n = 0

    def __iter__(self) -> "Names":
        return self

    def __next__(self) -> str:
        self.n += 1
        return f"{self.n:08d}"


class Scenario:
    def __init__(self, name: str) -> None:
        tempfile._name_sequence = Names()      # type: ignore[attr-defined]
        self.name = name
        self.root = ROOT / name
        self.root.mkdir(parents=True)
        os.environ["HOME"] = str(self.root / "user")
        (self.root / "user").mkdir()
        self.events: list[dict[str, Any]] = []
        self.steps: list[dict[str, Any]] = []
        self.clock = SteppedTime()
        install_mod.time = self.clock      # type: ignore[assignment]
        self._last: dict[str, Any] | None = None
        self.initial: list[dict[str, Any]] = []
        self.started = False
        # the invented extension and checkout (install_files copies the extension; npm ci copies package files)
        ext = self.root / "src" / "extension"
        (ext / "sub").mkdir(parents=True)
        (ext / "manifest.json").write_text('{"manifest_version": 3, "name": "FERB", "version": "1.2.3"}\n')
        (ext / "background.js").write_text("// invented background\n")
        (ext / "sub" / "page.js").write_text("// invented page\n")
        repo = self.root / "src" / "repo"
        repo.mkdir(parents=True)
        (repo / "package.json").write_text('{"name": "ferb-invented"}\n')
        (repo / "package-lock.json").write_text('{"lockfileVersion": 3}\n')
        extensions_mod.extension_source = lambda: ext            # type: ignore[assignment]
        install_mod.repo_root = lambda: repo                     # type: ignore[assignment]
        if "ferb.cli.setup" in sys.modules:                       # it imported repo_root by name
            sys.modules["ferb.cli.setup"].repo_root = install_mod.repo_root   # type: ignore[attr-defined]

    def start(self) -> None:
        if not self.started:
            self.initial = tree(self.root)
            self._last = snap(self.root)
            self.started = True

    def record_cmd(self, kind: str, argv: list[str], r: Any, effects: list[dict[str, Any]],
                   extra: dict[str, Any] | None = None) -> None:
        self.events.append({"kind": kind, "argv": sub([str(a) for a in argv]), "rc": r.returncode,
                            "stdout": sub(r.stdout or ""), "stderr": sub(r.stderr or ""), "effects": effects,
                            **(extra or {})})

    def runner(self, inner: Callable[[list[str]], Any]) -> Callable[[list[str]], Any]:
        def run(cmd: list[str]) -> Any:
            before = snap(self.root)
            r = inner(cmd)
            self.record_cmd("cmd", cmd, r, changes(before, snap(self.root)))
            return r
        return run

    def step(self, op: str, fn: Callable[[], Any], inst: Installer | None = None, **args: Any) -> Any:
        self.start()
        now = snap(self.root)
        edits = changes(self._last or {}, now)
        self.events = []
        entry: dict[str, Any] = {"op": op, "args": sub(args), "edits": edits}
        result = None
        try:
            result = fn()
            entry["result"] = sub(jsonable(result))
        except BaseException as e:   # noqa: BLE001 — recorded, as the Rust replay must raise the same
            entry["error"] = {"type": type(e).__name__, "message": sub(str(e))}
        entry["events"] = self.events
        if inst is not None:
            entry["notes"] = sub(list(inst.notes))
        entry["clock"] = self.clock.now
        self._last = snap(self.root)
        entry["changes"] = changes(now, self._last)
        self.steps.append(entry)
        return result

    def dump(self, **extra: Any) -> dict[str, Any]:
        return {"name": self.name, "root": str(self.root), "user_home": str(self.root / "user"),
                "initial": self.initial, "steps": self.steps, **extra}


def jsonable(v: Any) -> Any:
    if isinstance(v, Path):
        return str(v)
    if isinstance(v, (list, tuple)):
        return [jsonable(x) for x in v]
    if isinstance(v, dict):
        return {str(k): jsonable(x) for k, x in v.items()}
    if isinstance(v, bytes):
        return {"hex": v.hex()}
    return v


# ---------- installer scenarios (tests/test_install.py) ----------

def config(s: Scenario, **kw: Any) -> dict[str, Any]:
    """The installer's settings as the Rust replay builds it."""
    out = {"home": str(s.root / ".ferb"), "label": "com.ferb.test", "launch_agents": str(s.root / "LaunchAgents"),
           "native_hosts_dirs": [], "extension_ids": [], "bundle": None, "cli_dir": None, "app_service": None,
           "fetch_model": None}
    out.update({k: (str(v) if isinstance(v, Path) else v) for k, v in kw.items()})
    out["native_hosts_dirs"] = [str(d) for d in out["native_hosts_dirs"]]
    return out


def checkout(s: Scenario, runner: Any = None, **kw: Any) -> tuple[Installer, Any, dict[str, Any]]:
    home = s.root / ".ferb"
    fake = runner or FakeRunner(home)
    cfg = config(s, **kw)
    inst = Installer(home, label="com.ferb.test", run=s.runner(fake), launch_agents=s.root / "LaunchAgents",
                     native_hosts_dirs=tuple(Path(d) for d in cfg["native_hosts_dirs"]),
                     extension_ids=tuple(cfg["extension_ids"]))
    return inst, fake, cfg


def bundle_inst(s: Scenario, sub_dir: str = "", **runner: Any) -> tuple[Installer, Any, dict[str, Any]]:
    base = s.root / sub_dir if sub_dir else s.root
    app = make_ferb_app(base / "Applications")
    home = s.root / ".ferb"
    fake = BundleRunner(home, **runner)
    cfg = config(s, bundle=app, cli_dir=s.root / "bin",
                 native_hosts_dirs=[s.root / "Chrome" / "NativeMessagingHosts"], extension_ids=["abc"])
    inst = Installer(home, label="com.ferb.test", run=s.runner(fake), launch_agents=s.root / "LaunchAgents",
                     bundle=app, cli_dir=s.root / "bin",
                     native_hosts_dirs=(s.root / "Chrome" / "NativeMessagingHosts",), extension_ids=("abc",))
    return inst, fake, cfg


def install_step(s: Scenario, inst: Installer, update_only: bool = False, work: str | None = None) -> None:
    def callback() -> None:
        if work == "record":
            s.events.append({"kind": "work"})
        elif work == "fail":
            s.events.append({"kind": "work"})
            raise RuntimeError("disk I/O error")
    s.step("install", lambda: inst.install(update_only=update_only, while_stopped=callback if work else None),
           inst, update_only=update_only, work=work)


def while_stopped_step(s: Scenario, inst: Installer, work: str) -> None:
    def callback() -> None:
        s.events.append({"kind": "work"})
        if work == "fail":
            raise RuntimeError("disk I/O error")
    s.step("while_stopped", lambda: inst.while_stopped(callback), inst, work=work)


def cfg_step(s: Scenario, cfg: dict[str, Any]) -> None:
    """A new installer (the Rust replay builds it from these settings)."""
    s.step("configure", lambda: None, None, **cfg)


SCENARIOS: list[dict[str, Any]] = []


def scenario(fn: Callable[[Scenario], dict[str, Any] | None]) -> Callable[[Scenario], Any]:
    s = Scenario(fn.__name__)
    extra = fn(s) or {}
    SCENARIOS.append(s.dump(**extra))
    return fn


@scenario
def checkout_install_twice(s: Scenario) -> None:
    inst, _, cfg = checkout(s)
    cfg_step(s, cfg)
    install_step(s, inst)
    s.step("granted_binary", lambda: inst.granted_binary(), inst)
    install_step(s, inst)
    s.step("plist_bytes", lambda: inst.plist_bytes(), inst)
    s.step("daemon_command", lambda: inst.daemon_command(), inst)
    s.step("python_command", lambda: inst.python_command(), inst)
    s.step("grant_target", lambda: inst.grant_target(), inst)


@scenario
def checkout_update_before_install(s: Scenario) -> None:
    inst, _, cfg = checkout(s)
    cfg_step(s, cfg)
    install_step(s, inst, update_only=True)
    install_step(s, inst)
    install_step(s, inst, update_only=True)


for _step in (("uv", "pip", "install"), ("npm", "ci"), ("xcrun", "swiftc")):
    def _failed(s: Scenario, failing: tuple[str, ...] = _step) -> None:
        inst, fake, cfg = checkout(s)
        cfg_step(s, cfg)
        install_step(s, inst)
        s.start()
        (inst.venv / "old-code").write_text("the running daemon's")
        (inst.home / "node").rename(inst.home / "node.gone")
        fake.fail = failing
        install_step(s, inst, update_only=True)
        fake.fail = None
        install_step(s, inst, update_only=True)
    _failed.__name__ = "checkout_failed_" + "_".join(_step).replace("-", "_")
    scenario(_failed)


@scenario
def checkout_codex_roles(s: Scenario) -> None:
    inst, _, cfg = checkout(s)
    cfg_step(s, cfg)
    s.start()
    for role in ("orchestrator", "worker", "checker"):
        Config(inst.home).set("runtimes", role, "claude")
    install_step(s, inst)
    Config(inst.home).set("runtimes", "checker", "codex")
    install_step(s, inst, update_only=True)


@scenario
def checkout_fetch_model(s: Scenario) -> None:
    from ferb.memory.embeddings import default_root
    inst, _, cfg = checkout(s)
    cfg_step(s, {**cfg, "fetch_model": "ok"})

    def ok(root: Path) -> None:
        s.events.append({"kind": "fetch", "root": str(root)})
    inst.fetch_model = ok
    install_step(s, inst)

    def offline(root: Path) -> None:
        s.events.append({"kind": "fetch", "root": str(root)})
        raise OSError("network unreachable")
    s.step("set_fetch_model", lambda: setattr(inst, "fetch_model", offline), inst, fetch_model="fail")
    install_step(s, inst, update_only=True)
    return {"model_root": str(default_root(inst.home))}


@scenario
def checkout_secret_helper(s: Scenario) -> None:
    inst, fake, cfg = checkout(s)
    cfg_step(s, cfg)
    s.step("install_secret_helper", inst.install_secret_helper, inst)

    def unsigned(cmd: list[str]) -> Any:
        result = fake(cmd)
        if cmd[:3] == ['/usr/bin/codesign', '-d', '-r-']:
            result.stdout = 'designated => identifier "com.ferb.secrets"\n'
        return result
    inst.run = s.runner(unsigned)
    s.step("install_secret_helper", inst.install_secret_helper, inst)


@scenario
def checkout_swap_cut_off(s: Scenario) -> None:
    inst, _, cfg = checkout(s)
    cfg_step(s, cfg)
    install_step(s, inst)
    s.start()
    inst.venv.rename(inst.retired_venv)
    install_step(s, inst, update_only=True)


@scenario
def checkout_while_stopped(s: Scenario) -> None:
    inst, _, cfg = checkout(s)
    cfg_step(s, cfg)
    install_step(s, inst)
    install_step(s, inst, update_only=True, work="record")
    install_step(s, inst, update_only=True, work="fail")


@scenario
def checkout_unload_tail(s: Scenario) -> None:
    inst, fake, cfg = checkout(s)
    cfg_step(s, cfg)
    install_step(s, inst)
    fake.tail = 3
    s.step("unload", inst.unload, inst)
    s.step("exiting", inst.exiting, inst)
    install_step(s, inst, update_only=True)
    fake.tail = math.inf
    s.step("unload", inst.unload, inst)
    s.step("exiting", inst.exiting, inst)
    s.step("unload", inst.unload, inst)
    s.step("exiting", inst.exiting, inst)


@scenario
def checkout_update_while_exiting(s: Scenario) -> None:
    inst, fake, cfg = checkout(s)
    cfg_step(s, cfg)
    install_step(s, inst)
    s.start()
    (inst.venv.resolve() / "marker").write_text("the venv the old daemon runs")
    fake.tail = math.inf
    install_step(s, inst, update_only=True, work="record")


@scenario
def checkout_refuses_the_apps_service(s: Scenario) -> None:
    inst, fake, cfg = checkout(s)
    cfg_step(s, cfg)
    install_step(s, inst)

    def app_service(cmd: list[str]) -> Any:
        out = fake(cmd)
        if cmd[:2] == ["launchctl", "print"] and out.returncode == 0:
            out.stdout = ("path = /Applications/FERB.app/Contents/Library/LaunchAgents/com.ferb.daemon.plist\n" +
                          out.stdout.replace(f"path = {inst.plist_path}\n", ""))
        return out
    inst.run = s.runner(app_service)
    s.step("uninstall", lambda: inst.uninstall(), inst, purge=False)
    install_step(s, inst, update_only=True)


@scenario
def checkout_uninstall(s: Scenario) -> None:
    inst, _, cfg = checkout(s)
    cfg_step(s, cfg)
    install_step(s, inst)
    s.step("uninstall", lambda: inst.uninstall(), inst, purge=False)
    s.step("uninstall", lambda: inst.uninstall(), inst, purge=False)


@scenario
def checkout_purge(s: Scenario) -> None:
    from ferb.runtimes.claude_login import keychain_service
    inst, fake, cfg = checkout(s)
    cfg_step(s, cfg)
    install_step(s, inst)
    fake.keychain = {"ferb": {"api/anthropic": "k1", "jev/api_key": "k2", "ferb/email/password": "p"},
                     "ferb.owner-secrets": {"card/permit": "4111", "site/bank": "s"},
                     keychain_service(inst.home): {"Claude Code-credentials": "token"},
                     "Claude Code-credentials": {"sam": "the owner's own Claude login"},
                     "com.apple.account": {"x": "y"}}
    s.step("uninstall", lambda: inst.uninstall(purge=True), inst, purge=True)
    return {"claude_keychain_service": keychain_service(inst.home)}


@scenario
def purge_refusals(s: Scenario) -> None:
    home = s.root / "somefolder"
    home.mkdir()
    (home / "important.txt").write_text("keep me")
    fake = FakeRunner(home)
    i = Installer(home, label="com.ferb.test", run=s.runner(fake), launch_agents=s.root / "LA")
    cfg_step(s, config(s, home=home, launch_agents=s.root / "LA"))
    s.step("uninstall", lambda: i.uninstall(purge=True), i, purge=True)
    for danger in (Path.home(), Path("/"), Path.home() / "Desktop", Path("/private")):
        d = Installer(danger, label="com.ferb.test", run=s.runner(FakeRunner(danger)), launch_agents=s.root / "LA")
        cfg_step(s, config(s, home=danger, launch_agents=s.root / "LA"))
        s.step("check_purge_target", d._check_purge_target, d)


@scenario
def native_hosts(s: Scenario) -> None:
    dirs = (s.root / "Chrome" / "NativeMessagingHosts", s.root / "Brave" / "NativeMessagingHosts")
    home = s.root / ".ferb"
    inst = Installer(home, label="com.ferb.test", run=s.runner(FakeRunner(home)), launch_agents=s.root / "LA",
                     native_hosts_dirs=dirs, extension_ids=("abc", "def"))
    cfg_step(s, config(s, launch_agents=s.root / "LA", native_hosts_dirs=list(dirs), extension_ids=["abc", "def"]))
    s.step("write_native_host", inst.write_native_host, inst)
    s.start()
    # a folder that can't be written: a file where the folder should be
    (s.root / "Blocked").write_text("not a folder")
    blocked = Installer(home, label="com.ferb.test", run=s.runner(FakeRunner(home)), launch_agents=s.root / "LA",
                        native_hosts_dirs=(s.root / "Blocked" / "NativeMessagingHosts", dirs[0]),
                        extension_ids=("abc",))
    cfg_step(s, config(s, launch_agents=s.root / "LA",
                       native_hosts_dirs=[s.root / "Blocked" / "NativeMessagingHosts", dirs[0]],
                       extension_ids=["abc"]))
    s.step("write_native_host", blocked.write_native_host, blocked)
    cfg_step(s, config(s, launch_agents=s.root / "LA", native_hosts_dirs=list(dirs), extension_ids=["abc", "def"]))
    s.step("uninstall", lambda: inst.uninstall(), inst, purge=False)


@scenario
def checkout_registered_refuses(s: Scenario) -> None:
    home = s.root / ".ferb"
    app = make_ferb_app(s.root / "Applications")
    from ferb.device.install import BUNDLE_SERVICE

    class Registered(FakeRunner):
        def __call__(self, cmd: list[str]) -> Any:
            if cmd[:2] == ["launchctl", "print"]:
                self.cmds.append(cmd)
                return subprocess.CompletedProcess(cmd, 0, f"path = {app / BUNDLE_SERVICE}\npid = 4242\n", "")
            return super().__call__(cmd)
    inst, _, cfg = checkout(s, runner=Registered(home))
    cfg_step(s, cfg)
    install_step(s, inst)


@scenario
def checkout_codex_link_replaced(s: Scenario) -> None:
    inst, fake, cfg = checkout(s)
    app = make_ferb_app(s.root / "Applications")
    (inst.home / "node").parent.mkdir(parents=True, exist_ok=True)
    (inst.home / "node").symlink_to(app / "Contents" / "Resources" / "node")
    cfg_step(s, cfg)

    def pinned(cmd: list[str]) -> Any:
        result = fake(cmd)
        if cmd[1:] == ["--version"]:
            result.stdout = f"codex-cli {CODEX_VERSION}\n"
        return result
    inst.run = s.runner(pinned)
    s.step("ensure_codex", inst.ensure_codex, inst)
    s.step("ensure_codex", inst.ensure_codex, inst)


@scenario
def bundle_install(s: Scenario) -> None:
    inst, _, cfg = bundle_inst(s)
    cfg_step(s, cfg)
    install_step(s, inst)
    for op in ("plist_bytes", "daemon_command", "python_command", "grant_target", "granted_binary", "recorded",
               "active_program", "installed", "in_app", "cli_status", "definition_current", "signer",
               "own_requirement"):
        s.step(op, getattr(inst, op) if callable(getattr(type(inst), op, None)) else (lambda op=op: getattr(inst, op)),
               inst)


@scenario
def bundle_newer_build(s: Scenario) -> None:
    inst, _, cfg = bundle_inst(s)
    cfg_step(s, cfg)
    install_step(s, inst, update_only=True)
    s.start()
    info = inst.bundle / "Contents" / "Info.plist"
    info.write_bytes(plistlib.dumps({**plistlib.loads(info.read_bytes()), "CFBundleVersion": "202610150900"}))
    install_step(s, inst, update_only=True)
    install_step(s, inst, update_only=True)


for _older in ("venv", "agent"):
    def _older_fn(s: Scenario, older: str = _older) -> None:
        inst, fake, cfg = bundle_inst(s)
        cfg_step(s, cfg)
        s.start()
        inst.launch_agents.mkdir(parents=True)
        program = (str(inst.home / "venv" / "bin" / "python") if older == "venv" else
                   str(s.root / "Downloads" / "FERB.app" / AGENT))
        inst.plist_path.write_bytes(plistlib.dumps({"Label": "com.ferb.test", "ProgramArguments": [
            program, "-m", "ferb.daemon"], "WorkingDirectory": str(s.root / "Desktop" / "FERB")}))
        (inst.launch_agents / "com.owner.thing.plist").write_bytes(plistlib.dumps({"Label": "com.owner.thing"}))
        fake.loaded, fake.loaded_from = True, str(inst.plist_path)
        s.step("installed", inst.installed, inst)
        s.step("active_program", inst.active_program, inst)
        install_step(s, inst, update_only=True)
        s.step("active_program", inst.active_program, inst)
    _older_fn.__name__ = f"bundle_older_{_older}"
    scenario(_older_fn)


@scenario
def bundle_registered_job(s: Scenario) -> None:
    from ferb.device.install import BUNDLE_SERVICE
    inst, fake, cfg = bundle_inst(s)
    cfg_step(s, cfg)
    s.start()
    (inst.home).mkdir(parents=True, exist_ok=True)
    (inst.home / "service.json").write_text('{"build": "202611010900", "app": "%s", "label": "com.ferb.test"}'
                                            % inst.bundle)
    fake.loaded, fake.loaded_from = True, str(inst.bundle / BUNDLE_SERVICE)
    s.step("active_program", inst.active_program, inst)
    install_step(s, inst, update_only=True)


@scenario
def bundle_resubmits(s: Scenario) -> None:
    from ferb.device.install import BUNDLE_SERVICE
    inst, fake, cfg = bundle_inst(s)
    cfg_step(s, cfg)
    registered = str(inst.bundle / BUNDLE_SERVICE)
    fake.loaded, fake.loaded_from, fake.resubmits = True, registered, 2
    install_step(s, inst, update_only=True)
    fake.loaded, fake.loaded_from, fake.resubmits = True, registered, 10_000
    s.step("load", inst.load, inst)


@scenario
def bundle_restart(s: Scenario) -> None:
    inst, fake, cfg = bundle_inst(s)
    cfg_step(s, cfg)
    install_step(s, inst)
    s.step("restart", inst.restart, inst)
    fake.loaded = False
    s.step("restart", inst.restart, inst)
    fake.kickstart_refuses = 37
    install_step(s, inst, update_only=True)
    fake.kickstart_refuses, fake.fail = 37, ("launchctl", "bootstrap")
    s.step("restart", inst.restart, inst)


@scenario
def bundle_restore_definition(s: Scenario) -> None:
    inst, _, cfg = bundle_inst(s)
    cfg_step(s, cfg)
    install_step(s, inst)
    s.step("restore_definition", inst.restore_definition, inst)
    exact = inst.plist_path.read_bytes()
    inst.plist_path.write_bytes(plistlib.dumps({**plistlib.loads(exact), "ProgramArguments": ["/bin/sh", "-c", "x"]}))
    s.step("restore_definition", inst.restore_definition, inst)
    inst.plist_path.unlink()
    s.step("restore_definition", inst.restore_definition, inst)


@scenario
def bundle_switched_off(s: Scenario) -> None:
    inst, fake, cfg = bundle_inst(s)
    cfg_step(s, cfg)
    fake.disabled.add("com.ferb.test")
    s.step("disabled", inst.disabled, inst)
    install_step(s, inst)
    fake.disabled.clear()
    install_step(s, inst, update_only=True)


@scenario
def bundle_uninstall(s: Scenario) -> None:
    inst, _, cfg = bundle_inst(s)
    cfg_step(s, cfg)
    install_step(s, inst)
    s.start()
    login = inst.launch_agents / "com.ferb.FERB.login.plist"
    login.write_bytes(plistlib.dumps({"Label": "com.ferb.FERB.login"}))
    s.step("uninstall", lambda: inst.uninstall(), inst, purge=False)


@scenario
def bundle_while_stopped(s: Scenario) -> None:
    inst, _, cfg = bundle_inst(s)
    cfg_step(s, cfg)
    install_step(s, inst)
    while_stopped_step(s, inst, "fail")
    s.step("running", inst.running, inst)


@scenario
def bundle_helper_sealed(s: Scenario) -> None:
    inst, _, cfg = bundle_inst(s)
    cfg_step(s, cfg)
    s.step("install_secret_helper", inst.install_secret_helper, inst)
    swapped = make_ferb_app(s.root / "Swapped", sealed="0" * 40)
    inst.bundle = swapped
    cfg_step(s, {**cfg, "bundle": str(swapped)})
    s.step("install_secret_helper", inst.install_secret_helper, inst)


def helper_case(s: Scenario, sub_dir: str, **runner: Any) -> None:
    inst, _, cfg = bundle_inst(s, sub_dir, **runner)
    cfg_step(s, cfg)
    s.step("install_secret_helper", inst.install_secret_helper, inst)


DEV_ID = ('identifier "{}" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */ and '
          'certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */ and certificate leaf[subject.OU] = {}')
LEAF = "0123456789abcdef0123456789abcdef01234567"


def release(identifier: str, leaf: str = LEAF) -> str:
    return f'identifier "{identifier}" and certificate leaf = H"{leaf}"'


@scenario
def bundle_helper_requirements(s: Scenario) -> None:
    helper_case(s, "idonly", requirement='identifier "com.ferb.secrets"')
    helper_case(s, "devid", requirement=DEV_ID.format("com.ferb.secrets", "TEAM123456"), team="TEAM123456")
    helper_case(s, "devid_quoted", requirement=DEV_ID.format("com.ferb.secrets", '"TEAM123456"'), team="TEAM123456")
    helper_case(s, "other_team", requirement=DEV_ID.format("com.ferb.secrets", "OTHERTEAM1"), team="TEAM123456")
    helper_case(s, "release", requirement=release("com.ferb.secrets"),
                app_requirement=release("com.ferb.FERB", LEAF.upper()))
    for i, requirement in enumerate((release("com.ferb.secrets", "f" * 40),
                                     release("com.ferb.secrets") + " or anchor apple",
                                     f'certificate leaf = H"{LEAF}"')):
        helper_case(s, f"refused{i}", requirement=requirement, app_requirement=release("com.ferb.FERB"))
    helper_case(s, "adhoc_app", requirement=release("com.ferb.secrets"), app_requirement=PINNED)


@scenario
def bundle_same_certificate_update(s: Scenario) -> None:
    inst, fake, cfg = bundle_inst(s, requirement=release("com.ferb.secrets"),
                                  app_requirement=release("com.ferb.FERB"))
    cfg_step(s, cfg)
    install_step(s, inst)
    s.start()
    shutil.rmtree(inst.bundle)
    newer = "9" * 40
    make_ferb_app(inst.bundle.parent, build="202610260900", sealed=newer)
    fake.cdhash = newer
    install_step(s, inst, update_only=True)


@scenario
def bundle_signers(s: Scenario) -> None:
    for sub_dir, runner in (("adhoc", {}),
                            ("release", {"requirement": release("com.ferb.secrets"),
                                         "app_requirement": release("com.ferb.FERB", LEAF.upper())}),
                            ("devid", {"team": "TEAM123456",
                                       "requirement": DEV_ID.format("com.ferb.secrets", "TEAM123456"),
                                       "app_requirement": DEV_ID.format("com.ferb.FERB", "TEAM123456")})):
        inst, _, cfg = bundle_inst(s, sub_dir, **runner)
        cfg_step(s, cfg)
        install_step(s, inst)
        s.step("recorded", inst.recorded, inst)


@scenario
def bundle_app_service(s: Scenario) -> None:
    from ferb.device.install import BUNDLE_SERVICE
    inst, fake, cfg = bundle_inst(s, team="TEAM123456", requirement=DEV_ID.format("com.ferb.secrets", "TEAM123456"),
                                  app_requirement=DEV_ID.format("com.ferb.FERB", "TEAM123456"))
    cfg_step(s, cfg)
    install_step(s, inst)
    inst.app_service = True
    s.step("set_app_service", lambda: None, inst, app_service=True)
    install_step(s, inst, update_only=True)
    s.step("restore_definition", inst.restore_definition, inst)
    fake.loaded, fake.loaded_from = True, str(inst.bundle / BUNDLE_SERVICE)
    inst.app_service = None
    s.step("set_app_service", lambda: None, inst, app_service=None)
    s.step("in_app", lambda: inst.in_app, inst)
    install_step(s, inst, update_only=True)
    while_stopped_step(s, inst, "record")
    s.step("running", inst.running, inst)
    inst.app_service = True
    s.step("set_app_service", lambda: None, inst, app_service=True)
    fake.loaded = False
    install_step(s, inst, update_only=True)
    s.step("uninstall", lambda: inst.uninstall(), inst, purge=False)


@scenario
def bundle_codex_pinned(s: Scenario) -> None:
    inst, fake, cfg = bundle_inst(s, codex_version=f"codex-cli {CODEX_VERSION}\n")
    cfg_step(s, cfg)
    s.start()
    pinned = inst.home / "node" / _NATIVE_BIN
    pinned.parent.mkdir(parents=True)
    pinned.write_text("owner's npm install")
    s.step("ensure_codex", inst.ensure_codex, inst)
    fake.codex_version = "codex-cli 0.1.0\n"
    s.step("ensure_codex", inst.ensure_codex, inst)
    s.start()
    shutil.rmtree(inst.bundle / "Contents" / "Resources" / "node")
    (inst.home / "node").unlink()
    s.step("ensure_codex", inst.ensure_codex, inst)


@scenario
def checkout_after_app(s: Scenario) -> None:
    app_inst, fake, cfg = bundle_inst(s)
    cfg_step(s, cfg)
    install_step(s, app_inst)
    source = Installer(app_inst.home, label="com.ferb.test", run=s.runner(fake), launch_agents=s.root / "LaunchAgents")
    cfg_step(s, config(s))
    fake.requirement = 'identifier "com.ferb.secrets" and certificate leaf = H"local"'
    fake.codex_version = f"codex-cli {CODEX_VERSION}\n"
    s.start()
    (source.venv / "bin").mkdir(parents=True)
    (source.venv / "bin" / "python").write_text("")
    s.step("active_program", source.active_program, source)
    install_step(s, source, update_only=True)
    s.step("active_program", source.active_program, source)


@scenario
def bundle_cli_link(s: Scenario) -> None:
    inst, _, cfg = bundle_inst(s)
    cfg_step(s, cfg)
    s.start()
    (s.root / "bin").mkdir()
    (s.root / "bin" / "ferb").write_text("#!/bin/sh\necho someone else's ferb\n")
    s.step("link_cli", inst.link_cli, inst)
    s.step("cli_status", inst.cli_status, inst)
    s.start()
    (s.root / "bin" / "ferb").unlink()
    (s.root / "bin" / "ferb").symlink_to(s.root / "Old" / "FERB.app" / "Contents" / "Resources" / "bin" / "ferb")
    s.step("link_cli", inst.link_cli, inst)
    s.step("cli_status", inst.cli_status, inst)
    s.step("uninstall", lambda: inst.uninstall(), inst, purge=False)


@scenario
def bundle_check_refusals(s: Scenario) -> None:
    inst, _, cfg = bundle_inst(s, bad_signature=True)
    cfg_step(s, cfg)
    install_step(s, inst)
    s.start()
    (inst.bundle / AGENT).unlink()
    install_step(s, inst)
    trans = make_ferb_app(s.root / "AppTranslocation" / "X")
    cfg_step(s, {**cfg, "bundle": str(trans)})
    inst.bundle = trans
    install_step(s, inst)


write("installer.json", {"constants": {"codex_version": CODEX_VERSION, "native_bin": str(_NATIVE_BIN),
                                       "repo": "$REPO"},
                         "scenarios": SCENARIOS})
print(f"installer.json: {len(SCENARIOS)} scenarios, "
      f"{sum(len(s['steps']) for s in SCENARIOS)} steps, "
      f"{sum(len(st['events']) for s in SCENARIOS for st in s['steps'])} events")


# ---------- cli scenarios (ferb/cli/setup.py, tests/test_setup_flow.py) ----------

from datetime import datetime, timedelta, timezone  # noqa: E402

import ferb.cli.setup as S  # noqa: E402
import ferb.device.scan as scan_mod  # noqa: E402
from ferb.core.errors import FerbError, NotRunning  # noqa: E402
from ferb.device import permissions as perms_mod  # noqa: E402
from tests.conftest import make_app  # noqa: E402
from tests.test_setup_flow import FakeDaemon, FakeRun  # noqa: E402

SCANNED = datetime(2026, 11, 13, 9, 30, 5, tzinfo=timezone(timedelta(hours=5, minutes=30)))
OS_VERSION = "26.1"


class FixedNow(datetime):
    @classmethod
    def now(cls, tz: Any = None) -> "FixedNow":       # type: ignore[override]
        return cls.fromtimestamp(SCANNED.timestamp(), SCANNED.tzinfo)

    def astimezone(self, tz: Any = None) -> "FixedNow":  # type: ignore[override]
        return self


scan_mod.datetime = FixedNow                       # type: ignore[misc]
scan_mod.platform.mac_ver = lambda: (OS_VERSION, ("", "", ""), "arm64")   # type: ignore[assignment]
_real_missing, _real_fda_ready = perms_mod.missing, perms_mod.fda_ready
CURRENT: dict[str, Any] = {}


def _missing(rows: Any) -> Any:
    out = _real_missing(rows)
    CURRENT["s"].events.append({"kind": "perm_missing", "result": [list(x) for x in out]})
    return out


def _fda_ready(containment: Any) -> bool:
    out = _real_fda_ready(containment)
    if not CURRENT.get("inside_rows"):             # report_rows' own use is the permissions port's business
        CURRENT["s"].events.append({"kind": "fda_ready", "containment": containment, "result": out})
    return out


_real_report_rows = perms_mod.report_rows


def _report_rows(report: Any) -> Any:
    CURRENT["inside_rows"] = True
    try:
        return _real_report_rows(report)
    finally:
        CURRENT["inside_rows"] = False


perms_mod.report_rows = _report_rows               # type: ignore[assignment]
perms_mod.missing = _missing                       # type: ignore[assignment]
perms_mod.fda_ready = _fda_ready                   # type: ignore[assignment]


import ferb.policy.protection as protection_mod  # noqa: E402
import ferb.runtimes.catalog as catalog_mod  # noqa: E402
import ferb.store.cleanup as cleanup_mod  # noqa: E402
import ferb.store.db as db_mod  # noqa: E402


class Lists:
    reads, writes = ("~/.ssh",), ("~/.ssh", "~/Library/Keychains")


def _protection(get: Any, home: Path, ferb_home: Path) -> Any:
    fail = bool(CURRENT.get("protection_fails"))
    CURRENT["s"].events.append({"kind": "protection", "fail": fail})
    if fail:
        raise FerbError("[protection] never_read entry 3 isn't a path")
    return Lists()


def _pending(path: Path) -> bool:
    CURRENT["s"].events.append({"kind": "store", "op": "pending", "path": str(path), "result": True})
    return True


def _store_size(path: Path) -> int:
    CURRENT["s"].events.append({"kind": "store", "op": "size", "path": str(path), "result": 2_345_678})
    return 2_345_678


class Entry:
    def __init__(self, name: str) -> None:
        self.name = name

    def available(self, home: Path, model: str | None = None) -> tuple[bool, str]:
        ok = self.name == "codex"
        fix = "" if ok else "sign in to Claude"
        CURRENT["s"].events.append({"kind": "runtime_available", "name": self.name, "result": [ok, fix]})
        return ok, fix


protection_mod.load = _protection                                  # type: ignore[assignment]
cleanup_mod.pending, db_mod.store_size = _pending, _store_size     # type: ignore[assignment]
catalog_mod.CATALOG = {"codex": Entry("codex"), "claude": Entry("claude")}   # type: ignore[assignment]


class RecClient:
    def __init__(self, s: Scenario, inner: Any) -> None:
        self.s, self.inner = s, inner

    def call(self, method: str, params: Any = None) -> Any:
        ev: dict[str, Any] = {"kind": "rpc", "op": "call", "method": method, "params": sub(params)}
        try:
            result = self.inner.call(method, params)
        except Exception as e:
            ev["error"] = {"type": type(e).__name__, "message": sub(str(e)), "code": getattr(e, "code", None)}
            self.s.events.append(ev)
            raise
        ev["result"] = sub(result)
        self.s.events.append(ev)
        return result

    def close(self) -> None:
        self.s.events.append({"kind": "rpc", "op": "close"})


class Run:
    """FakeRun, plus the sign-in programs (codex logout, Claude Code's auth logout/status) and recording."""

    def __init__(self, s: Scenario) -> None:
        self.s, self.fake = s, FakeRun()
        self.codex_logout_rc = 0
        self.claude_logged_in = False

    def __call__(self, argv: list[str], **kw: Any) -> Any:
        before = snap(self.s.root)
        argv = [str(a) for a in argv]
        env = kw.get("env") or {}
        if argv[-1:] == ["logout"] and argv[0].endswith("/codex"):
            if self.codex_logout_rc == 0:
                (Path(env["CODEX_HOME"]) / "auth.json").unlink()
            r = subprocess.CompletedProcess(argv, self.codex_logout_rc, None, None)
        elif argv[1:3] == ["auth", "logout"]:
            r = subprocess.CompletedProcess(argv, 0, None, None)
        elif argv[1:3] == ["auth", "status"]:
            r = subprocess.CompletedProcess(argv, 0, json.dumps({"loggedIn": self.claude_logged_in,
                                                                 "authMethod": "claude.ai"}), "")
        else:
            r = self.fake(argv, **kw)
        extra = {"env": {k: v for k, v in env.items() if os.environ.get(k) != v},
                 "flags": sorted(k for k in kw if k != "env")}
        self.s.record_cmd("run", argv, r, changes(before, snap(self.s.root)), sub(extra))
        return r


def cli_world(s: Scenario, kind: str = "bundle", pid: int | None = None, **runner: Any) -> dict[str, Any]:
    """An installer (FERB.app's or a checkout's), a fake daemon that answers once launchd loaded the service (or
    always, with `pid`: a daemon outside launchd), the scan's apps, FERB's Codex signed in."""
    CURRENT.clear()
    CURRENT["s"] = s
    apps = s.root / "Applications"
    if kind == "bundle":
        inst, fake, cfg = bundle_inst(s, **runner)
    else:
        inst, fake, cfg = checkout(s, **runner)
    make_app(apps, "Notes", "com.apple.Notes", sdef=True)
    make_app(apps, "Google Chrome", "com.google.Chrome", NSAppleScriptEnabled=True)
    make_app(apps, "Brave Browser", "com.brave.Browser", NSAppleScriptEnabled=True)
    S.scan = lambda did: scan_mod.scan(did, [apps])     # type: ignore[assignment]
    home = inst.home
    Config(home).set("device", "id", "0123456789abcdef0123456789abcdef")
    (home / "codex-home").mkdir(parents=True)
    (home / "codex-home" / "auth.json").write_text("{}")
    daemon = FakeDaemon(pid or 4242)
    run = Run(s)

    def connect() -> RecClient:
        if pid is None and not fake.loaded:
            s.events.append({"kind": "rpc", "op": "connect", "error": "FERB daemon is not running (start it with "
                                                                        "`ferb daemon run`)"})
            raise NotRunning("FERB daemon is not running (start it with `ferb daemon run`)")
        s.events.append({"kind": "rpc", "op": "connect"})
        return RecClient(s, daemon.client())
    answers: list[str] = []

    def ask(prompt: str) -> str:
        a = answers.pop(0)
        s.events.append({"kind": "ask", "prompt": sub(prompt), "answer": a})
        return a

    def tokenizer(h: Path) -> str:
        s.events.append({"kind": "tokenizer", "home": str(h)})
        return "Tokenizer: FERB's pinned copy is in place."

    def sleep(seconds: float) -> None:
        s.events.append({"kind": "sleep", "seconds": seconds})
    out = io.StringIO()
    env = S.Env(inst, connect, ask, out, open_pane=lambda pane: None, sleep=sleep, running_apps=lambda: set(),
                persona_usage=lambda apps: [], run=run, tokenizer=tokenizer)
    return {"inst": inst, "fake": fake, "cfg": cfg, "env": env, "out": out, "daemon": daemon, "run": run,
            "answers": answers, "apps": apps}


def command(s: Scenario, w: dict[str, Any], name: str, fn: Callable[[], int], **args: Any) -> None:
    """A command's step: its exit code (or the error main() turns into one line), and everything it printed."""
    w["out"].seek(0)
    w["out"].truncate()

    def call() -> Any:
        try:
            return {"rc": fn()}
        except KeyboardInterrupt:
            return {"rc": 130, "interrupted": True}
    s.step(name, call, w["inst"], **args)
    s.steps[-1]["out"] = sub(w["out"].getvalue())


CLI: list[dict[str, Any]] = []


def cli(fn: Callable[[Scenario], Any]) -> Callable[[Scenario], Any]:
    s = Scenario(fn.__name__)
    extra = fn(s) or {}
    CLI.append(s.dump(**extra))
    return fn


@cli
def service_bundle_install_then_update(s: Scenario) -> dict[str, Any]:
    w = cli_world(s)
    cfg_step(s, w["cfg"])
    command(s, w, "service_setup", lambda: S.service_setup(w["env"]), update=False)
    command(s, w, "service_setup", lambda: S.service_setup(w["env"], update=True), update=True)
    command(s, w, "service_setup", lambda: S.service_setup(w["env"]), update=False)
    return {"world": "bundle"}


@cli
def service_app_service(s: Scenario) -> dict[str, Any]:
    w = cli_world(s, team="TEAM123456", requirement=DEV_ID.format("com.ferb.secrets", "TEAM123456"),
                  app_requirement=DEV_ID.format("com.ferb.FERB", "TEAM123456"))
    cfg_step(s, w["cfg"])
    command(s, w, "service_setup", lambda: S.service_setup(w["env"]), update=False)
    w["inst"].app_service = True
    s.step("set_app_service", lambda: None, w["inst"], app_service=True)
    command(s, w, "service_setup", lambda: S.service_setup(w["env"], update=True), update=True)
    return {"world": "bundle"}


@cli
def service_switched_off(s: Scenario) -> dict[str, Any]:
    w = cli_world(s)
    cfg_step(s, w["cfg"])
    w["fake"].disabled.add("com.ferb.test")
    command(s, w, "service_setup", lambda: S.service_setup(w["env"]), update=False)
    return {"world": "bundle"}


@cli
def service_foreign_daemon(s: Scenario) -> dict[str, Any]:
    w = cli_world(s, pid=999)
    cfg_step(s, w["cfg"])
    command(s, w, "service_setup", lambda: S.service_setup(w["env"]), update=False)
    command(s, w, "setup", lambda: S.setup(w["env"]), update=False)
    return {"world": "bundle", "pid": 999}


@cli
def service_damaged_app(s: Scenario) -> dict[str, Any]:
    w = cli_world(s, bad_signature=True)
    cfg_step(s, w["cfg"])
    command(s, w, "service_setup", lambda: S.service_setup(w["env"]), update=False)
    return {"world": "bundle"}


@cli
def checkout_setup_then_update(s: Scenario) -> dict[str, Any]:
    w = cli_world(s, kind="checkout")
    cfg_step(s, w["cfg"])
    command(s, w, "setup", lambda: S.setup(w["env"]), update=False)
    w["daemon"].perms = {name: "granted" for name in S.SETUP_PERMISSIONS}
    w["daemon"].tap_ok = False
    command(s, w, "setup", lambda: S.setup(w["env"], update=True), update=True)
    (w["inst"].home / "codex-home" / "auth.json").unlink()
    w["answers"].append("n")
    command(s, w, "setup", lambda: S.setup(w["env"]), update=False)
    command(s, w, "setup", lambda: S.setup(w["env"], update=True), update=True)
    return {"world": "checkout"}


@cli
def bundle_setup_from_link(s: Scenario) -> dict[str, Any]:
    w = cli_world(s)
    cfg_step(s, w["cfg"])
    command(s, w, "setup", lambda: S.setup(w["env"]), update=False)
    w["daemon"].hearing = {"installed": True, "signed": False}
    command(s, w, "doctor", lambda: S.doctor(w["env"]))
    return {"world": "bundle"}


@cli
def cli_link_commands(s: Scenario) -> dict[str, Any]:
    w = cli_world(s)
    cfg_step(s, w["cfg"])
    command(s, w, "cli_setup", lambda: S.cli_setup(w["env"]))
    s.start()
    (s.root / "bin" / "ferb").unlink()
    (s.root / "bin" / "ferb").write_text("someone else's\n")
    command(s, w, "cli_setup", lambda: S.cli_setup(w["env"]))
    return {"world": "bundle"}


@cli
def cli_link_checkout(s: Scenario) -> dict[str, Any]:
    w = cli_world(s, kind="checkout")
    cfg_step(s, w["cfg"])
    command(s, w, "cli_setup", lambda: S.cli_setup(w["env"]))
    return {"world": "checkout"}


@cli
def uninstall_commands(s: Scenario) -> dict[str, Any]:
    w = cli_world(s)
    cfg_step(s, w["cfg"])
    command(s, w, "service_setup", lambda: S.service_setup(w["env"]), update=False)
    w["answers"].append("no")
    command(s, w, "uninstall", lambda: S.uninstall(w["env"], purge=True), purge=True, confirmed=False)
    command(s, w, "uninstall", lambda: S.uninstall(w["env"], purge=False), purge=False, confirmed=False)
    command(s, w, "service_setup", lambda: S.service_setup(w["env"]), update=False)
    w["answers"].append("delete")
    command(s, w, "uninstall", lambda: S.uninstall(w["env"], purge=True), purge=True, confirmed=False)
    command(s, w, "uninstall", lambda: S.uninstall(w["env"], purge=True, confirmed=True), purge=True, confirmed=True)
    command(s, w, "uninstall", lambda: S.uninstall(w["env"], purge=False), purge=False, confirmed=False)
    return {"world": "bundle"}


@cli
def uninstall_purge_confirmed(s: Scenario) -> dict[str, Any]:
    w = cli_world(s)
    cfg_step(s, w["cfg"])
    command(s, w, "service_setup", lambda: S.service_setup(w["env"]), update=False)
    w["fake"].keychain = {"ferb": {"api/anthropic": "k1"}, "ferb.owner-secrets": {"card/x": "c"}}
    command(s, w, "uninstall", lambda: S.uninstall(w["env"], purge=True, confirmed=True), purge=True, confirmed=True)
    return {"world": "bundle"}


def codex_binary_file(home: Path) -> None:
    b = home / "node" / _NATIVE_BIN
    b.parent.mkdir(parents=True, exist_ok=True)
    b.write_text("#!/bin/sh\n")


@cli
def sign_ins(s: Scenario) -> dict[str, Any]:
    import ferb.runtimes.codex_exec as codex_exec
    import ferb.runtimes.claude_login as claude_login
    w = cli_world(s)
    cfg_step(s, w["cfg"])
    home = w["inst"].home
    catalog = {"value": True}

    def bundled_catalog(h: Path) -> Any:
        s.events.append({"kind": "catalog", "home": str(h), "result": catalog["value"]})
        return h / "run" / "catalog.json" if catalog["value"] else None
    codex_exec.bundled_catalog = bundled_catalog          # type: ignore[assignment]
    cli_path = s.root / "claude-sdk" / "_bundled" / "claude"
    claude_state = {"installed": True}

    def cli_binary() -> Path:
        s.events.append({"kind": "claude_cli", "installed": claude_state["installed"]})
        if not claude_state["installed"]:
            raise FerbError("the Claude Agent SDK's bundled Claude Code is missing")
        return cli_path
    claude_login.cli_binary = cli_binary                  # type: ignore[assignment]
    run = w["run"]
    command(s, w, "codex_logout", lambda: S.codex_logout(w["env"], run=run))      # no binary yet
    s.start()
    codex_binary_file(home)
    command(s, w, "codex_logout", lambda: S.codex_logout(w["env"], run=run))
    command(s, w, "codex_logout", lambda: S.codex_logout(w["env"], run=run))      # signed out already
    command(s, w, "codex_login", lambda: S.codex_login(w["env"], run=run))
    catalog["value"] = False
    run.codex_logout_rc = 1
    command(s, w, "codex_logout", lambda: S.codex_logout(w["env"], run=run))
    command(s, w, "codex_login", lambda: S.codex_login(w["env"], run=run))
    command(s, w, "claude_logout", lambda: S.claude_logout(w["env"], run=run))     # not signed in
    s.start()
    (home / "claude-home").mkdir()
    (home / "claude-home" / ".ferb-signed-in").write_text("signed in\n")
    run.claude_logged_in = True
    command(s, w, "claude_logout", lambda: S.claude_logout(w["env"], run=run))
    run.claude_logged_in = False
    command(s, w, "claude_logout", lambda: S.claude_logout(w["env"], run=run))
    s.start()
    (home / "claude-home" / ".ferb-signed-in").write_text("signed in\n")
    claude_state["installed"] = False
    command(s, w, "claude_logout", lambda: S.claude_logout(w["env"], run=run))
    return {"world": "bundle", "claude_cli": str(cli_path)}


@cli
def codex_login_installs_codex(s: Scenario) -> dict[str, Any]:
    import ferb.runtimes.codex_exec as codex_exec
    w = cli_world(s, codex_version=f"codex-cli {CODEX_VERSION}\n")
    cfg_step(s, w["cfg"])

    def bundled_catalog(h: Path) -> Any:
        s.events.append({"kind": "catalog", "home": str(h), "result": True})
        return h
    codex_exec.bundled_catalog = bundled_catalog          # type: ignore[assignment]
    (w["inst"].home / "codex-home" / "auth.json").unlink()
    command(s, w, "codex_login", lambda: S.codex_login(w["env"], run=w["run"]))
    return {"world": "bundle"}


@cli
def cleanup_commands(s: Scenario) -> dict[str, Any]:
    import ferb.store.cleanup as cleanup_mod
    import ferb.store.db as db_mod
    import ferb.memory.journal as journal_mod
    w = cli_world(s)
    cfg_step(s, w["cfg"])
    home = w["inst"].home
    plan = {"steps": [{"table": "old_rows", "rows": 12, "what": "invented"}], "rows": 12, "compact": True,
            "bytes": 4096, "free": 10 ** 9, "ok": "d1g3st"}
    outcome: dict[str, Any] = {"value": {}}

    def preview(path: Path) -> Any:
        s.events.append({"kind": "store", "op": "preview", "path": str(path), "result": plan})
        return plan

    def apply(db: Any, journal: Any, ok: str, stop: Any, say: Any) -> Any:
        result = outcome["value"]
        if isinstance(result, Exception):
            s.events.append({"kind": "store", "op": "apply", "ok": ok, "error": [type(result).__name__, str(result)]})
            raise result
        s.events.append({"kind": "store", "op": "apply", "ok": ok, "stopped_asked": bool(stop()), "result": result})
        return result

    class DB:
        def __init__(self, path: Path) -> None:
            s.events.append({"kind": "store", "op": "open", "path": str(path)})

        def migrate(self) -> None:
            pass

        def close(self) -> None:
            s.events.append({"kind": "store", "op": "close"})
    cleanup_mod.preview, cleanup_mod.apply = preview, apply          # type: ignore[assignment]
    db_mod.Database = DB                                             # type: ignore[assignment,misc]
    journal_mod.Journal = lambda *a, **k: None                       # type: ignore[assignment]
    command(s, w, "cleanup_preview", lambda: S.cleanup_preview(w["env"]))
    s.start()
    (home).mkdir(parents=True, exist_ok=True)
    (home / "ferb.db").write_bytes(b"invented")
    command(s, w, "cleanup_preview", lambda: S.cleanup_preview(w["env"]))
    outcome["value"] = {"removed": {"old_rows": 12}, "stopped": False, "compacted": True, "bytes_before": 5_300_000,
                        "bytes_after": 1_200_000}
    command(s, w, "cleanup_apply", lambda: S.cleanup_apply(w["env"], "d1g3st"), ok="d1g3st")   # no service yet
    command(s, w, "service_setup", lambda: S.service_setup(w["env"]), update=False)
    for value in ({"refused": True, "plan": plan},
                  {"removed": {"old_rows": 1234, "more": 5}, "stopped": True},
                  {"removed": {"old_rows": 12}, "stopped": False, "compacted": False, "needed": 2_500_000_000,
                   "free": 900_000},
                  {"removed": {"old_rows": 12}, "stopped": False, "compacted": True, "bytes_before": 2_000_000_000,
                   "bytes_after": 1_500},
                  {"removed": {"old_rows": 0}, "stopped": False},
                  OSError("disk I/O error")):
        outcome["value"] = value
        command(s, w, "cleanup_apply", lambda: S.cleanup_apply(w["env"], "d1g3st"), ok="d1g3st")
    w["fake"].tail = math.inf
    outcome["value"] = {"removed": {}, "stopped": False}
    command(s, w, "cleanup_apply", lambda: S.cleanup_apply(w["env"], "d1g3st"), ok="d1g3st")
    return {"world": "bundle"}


@cli
def cleanup_outside_launchd(s: Scenario) -> dict[str, Any]:
    w = cli_world(s, pid=999)
    cfg_step(s, w["cfg"])
    command(s, w, "cleanup_apply", lambda: S.cleanup_apply(w["env"], "d1g3st"), ok="d1g3st")
    return {"world": "bundle", "pid": 999}


@cli
def doctor_checkout(s: Scenario) -> dict[str, Any]:
    w = cli_world(s, kind="checkout")
    cfg_step(s, w["cfg"])
    command(s, w, "doctor", lambda: S.doctor(w["env"]))                # nothing installed, no daemon
    command(s, w, "setup", lambda: S.setup(w["env"]), update=False)
    s.start()
    (w["inst"].home / "ferb.db").write_bytes(b"invented")
    Config(w["inst"].home).set("runtimes", "worker", "claude")
    d = w["daemon"]
    d.clipboard, d.tokenizer = "default", "bytes"
    d.hearing = {"installed": True, "signed": True, "speech": "denied", "on_device": True, "locale": "en-US"}
    command(s, w, "doctor", lambda: S.doctor(w["env"]))
    CURRENT["protection_fails"] = True
    d.perms = {name: "granted" for name in S.SETUP_PERMISSIONS}
    make_app(w["apps"], "Maps", "com.apple.Maps")
    command(s, w, "doctor", lambda: S.doctor(w["env"]))
    return {"world": "checkout"}


write("cli.json", {"constants": {"codex_version": CODEX_VERSION, "native_bin": str(_NATIVE_BIN),
                                 "scanned_at": SCANNED.isoformat(timespec="seconds"), "os_version": OS_VERSION},
                   "scenarios": CLI})
print(f"cli.json: {len(CLI)} scenarios, {sum(len(s['steps']) for s in CLI)} steps, "
      f"{sum(len(st['events']) for s in CLI for st in s['steps'])} events")


# ---------- pure helpers, the device profile, scan, persona, devtools, parser, toolbox ----------

import time as _time  # noqa: E402

os.environ["TZ"] = "UTC"
_time.tzset()

from ferb.act.browser.profiles import BROWSERS as PAIRED  # noqa: E402
from ferb.act.mac import browsers as browser_list  # noqa: E402
from ferb.cli import admin, logs as logs_mod, tokenizer as tok_mod  # noqa: E402
from ferb.cli import build_parser, plain_error  # noqa: E402
from ferb.cli.client import RpcError  # noqa: E402
from ferb.device import devtools, persona, profile as profile_mod  # noqa: E402
from ferb.device.install import (certificate_leaf, codesign_field, designated_requirement,  # noqa: E402
                                 launchctl_words, trusted_requirement)

PURE: dict[str, Any] = {}
proot = ROOT / "pure"
proot.mkdir()

REQS = ['cdhash H"' + "a" * 40 + '"', 'cdhash H"' + "A" * 40 + '"', 'cdhash H"' + "a" * 39 + '"',
        'identifier "com.ferb.secrets" and certificate leaf = H"' + "0123456789abcdef" * 2 + "01234567" + '"',
        '  identifier "x" and certificate leaf = H"' + "AbCdEf0123" * 4 + '"  ', 'identifier "" and certificate '
        'leaf = H"' + "a" * 40 + '"', 'identifier "a"b" and certificate leaf = H"' + "a" * 40 + '"',
        'identifier "com.ferb.secrets"', "", "certificate leaf[subject.OU] = TEAM"]
CODESIGN = ["Executable=/x\n# designated => cdhash H\"abc\"\n", "designated => identifier \"a\" and anchor apple\n",
            "Executable=/x\r\nCDHash=  abc  \r\nTeamIdentifier=not set\n", "TeamIdentifier=T1=2\n", "nothing\n",
            "x\x0bdesignated => after a vertical tab\n"]
PURE["requirements"] = [{"req": r, "trusted": trusted_requirement(r, pinned_ok=False),
                         "trusted_pinned": trusted_requirement(r, pinned_ok=True), "leaf": certificate_leaf(r)}
                        for r in REQS]
PURE["codesign"] = [{"out": o, "designated": designated_requirement(o), "cdhash": codesign_field(o, "CDHash"),
                     "team": codesign_field(o, "TeamIdentifier")} for o in CODESIGN]
PURE["launchctl_words"] = [{"rc": rc, "stdout": out, "stderr": err,
                            "words": launchctl_words(subprocess.CompletedProcess([], rc, out, err))}
                           for rc, out, err in ((5, "", "Bootstrap failed: 5: Input/output error\n"), (37, "", ""),
                                                (1, "  said on stdout \n", "  \n"), (3, "", "é" * 450),
                                                (2, None, None))]
PLISTS = [{"Label": "a&b<c>", "Args": ["x", "y\r\nz"], "Empty": {}, "None": [], "N": -3, "T": True,
           "Ünïcode": "café ☕", "Nested": {"b": {"c": []}, "a": 1}}, {}]
PURE["plists"] = [{"value": v, "hex": plistlib.dumps(v).hex()} for v in PLISTS]


def plist_err(v: Any) -> str | None:
    try:
        plistlib.dumps(v)
    except ValueError as e:
        return str(e)
    return None


PURE["plist_errors"] = [{"value": "bad\x01char", "error": plist_err({"k": "bad\x01char"})}]

# device profile: diff and the store's history
apps_a = [profile_mod.AppInfo("com.a", "A", "1", "/A.app", True), profile_mod.AppInfo("com.b", "B", "2", "/B.app", False)]
apps_b = [profile_mod.AppInfo("com.b", "B", "3", "/B.app", False), profile_mod.AppInfo("com.c", "C", "1", "/C.app", True)]
br = [profile_mod.BrowserInfo("chrome", "com.google.Chrome", "/C.app", {"ax:chrome": "x", "ext:chrome": "y"})]
pa = profile_mod.DeviceProfile("dev1", "macos", "26.1", "2026-11-13T09:30:05+05:30", apps_a, br)
pb = profile_mod.DeviceProfile("dev1", "macos", "26.1", "2026-11-14T09:30:05+05:30", apps_b, br)
pc = profile_mod.DeviceProfile("dev1", "macos", "26.1", "2026-11-15T09:30:05+05:30", apps_b, br)
store_home = proot / "home"
ps = profile_mod.ProfileStore(store_home)
saves = [ps.save(pa, "first"), ps.save(pb), ps.save(pc, "only the time")]
(ps.history_path).open("a").write("not json\n{\"no\": 1}\n")
PURE["profile"] = {"a": json.loads(pa.to_json()), "b": json.loads(pb.to_json()), "c": json.loads(pc.to_json()),
                   "a_json": pa.to_json(), "saves": saves, "history": ps.history(),
                   "files": tree(store_home), "diff": vars(profile_mod.diff(pa, pb)),
                   "summary": profile_mod.summarize(pb)}

# scan: invented bundles
sroot = proot / "Apps"
make_app(sroot, "Notes", "com.apple.Notes", sdef=True)
make_app(sroot, "Chrome", "com.google.Chrome", NSAppleScriptEnabled=True, CFBundleDisplayName="Google Chrome")
make_app(sroot, "Binary", "com.example.binary", binary=True, OSAScriptingDefinition="x.sdef")
make_app(sroot, "NoId", None)
make_app(sroot, "Web", "com.example.web", CFBundleURLTypes=[{"CFBundleURLSchemes": ["HTTPS"]}])
make_app(sroot, "Dup", "com.apple.Notes")
make_app(sroot, "Safari", "com.apple.Safari")
make_app(sroot, "Brave", "com.brave.Browser")
(sroot / "Empty.app" / "Contents").mkdir(parents=True)
(sroot / "Empty.app" / "Contents" / "Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": "",
                                                                                "CFBundleName": ""}))
(sroot / "Listed.app" / "Contents").mkdir(parents=True)
(sroot / "Listed.app" / "Contents" / "Info.plist").write_bytes(plistlib.dumps(["not", "a", "dict"]))
reads = []
for b in sorted(sroot.glob("*.app")):
    try:
        app = scan_mod.read_app(b)
        reads.append({"bundle": b.name, "app": vars(app) if app else None})
    except Exception as e:
        reads.append({"bundle": b.name, "error": type(e).__name__})
(sroot / "Listed.app" / "Contents" / "Info.plist").unlink()
scanned = scan_mod.scan("dev2", [sroot, proot / "Missing"])
ids = sorted({a.bundle_id for a in scanned.apps} | {"com.google.Chrome.app.x", "org.example", "com.apple.Maps", "com.ferb.FERB"})
PURE["scan"] = {"files": tree(sroot), "reads": reads, "profile": json.loads(scanned.to_json()),
                "web": {b.name: scan_mod.registers_web(b) for b in sorted(sroot.glob("*.app"))},
                "catalog": {i: {"known": browser_list.known(i), "pair_key": browser_list.pair_key(i),
                                "paired": any(p.bundle_id.casefold() == i.casefold() for p in PAIRED.values())}
                            for i in ids}}

# persona
usage_apps = [profile_mod.AppInfo("com.a", "A", "1", str(sroot / "Notes.app"), True),
              profile_mod.AppInfo("com.b", "B", "1", str(sroot / "Web.app"), False),
              profile_mod.AppInfo("com.c", "C", "1", "/nowhere/C.app", False)]
mdls = plistlib.dumps([{"kMDItemPath": str(sroot / "Notes.app"), "kMDItemUseCount": 7,
                        "kMDItemLastUsedDate": datetime(2026, 11, 1, 8, 0, 0)},
                       {"kMDItemPath": str(sroot / "Web.app"), "kMDItemUseCount": True},
                       {"kMDItemPath": 5}, "junk"])
rows = persona._usage_rows(usage_apps, mdls)
activity = [{"bundle_id": "com.b", "focus_events": 4, "last_used": "2026-11-12"},
            {"bundle_id": "com.c", "focus_events": 9, "last_used": "2026-11-10"}]
rows2 = persona.with_activity(rows, activity)
facts = persona.mac_facts(rows2, "2026-11-13T09:30:05+05:30", ["chrome"],
                          [{"browser": "Google Chrome", "name": "Work", "email": "w@example.invalid", "last_used": True}])
PURE["persona"] = {"apps": [vars(a) for a in usage_apps], "mdls_hex": mdls.hex(), "rows": rows,
                   "bad_rows": persona._usage_rows(usage_apps, b"not a plist"),
                   "command": persona._usage_command(usage_apps), "activity": activity, "with_activity": rows2,
                   "facts": facts, "fingerprint": persona.facts_fingerprint(facts)}

# devtools
bins = proot / "bins"
for d, names in (("brew", ["python3", "git"]), ("usr_bin", ["python3", "git", "make", "cc"])):
    (bins / d).mkdir(parents=True)
    for n in names:
        (bins / d / n).write_text("#!/bin/sh\n")
        (bins / d / n).chmod(0o755)
(bins / "brew" / "pip3").write_text("not executable")
devtools.STUB_FOLDER = bins / "usr_bin"


def xcode(rc: int, out: str) -> Any:
    return lambda argv, **kw: subprocess.CompletedProcess(argv, rc, out, "")


PURE["devtools"] = {"stub_folder": str(bins / "usr_bin"), "files": tree(bins), "cases": [
    {"path": p, "rc": rc, "out": out, "missing": devtools.missing_tools(p, xcode(rc, out))}
    for p, rc, out in ((f"{bins}/brew:{bins}/usr_bin", 2, ""), (f"{bins}/usr_bin::{bins}/brew", 2, ""),
                       (f"{bins}/brew", 0, str(bins) + "\n"), (f"{bins}/brew", 0, "relative\n"))],
    "facts": [devtools.missing_fact(m) for m in ([], ["python3", "git"])]}

# the argument parser
ARGV = [["setup", "--service"], ["setup", "--service", "--update"], ["setup", "--service", "--app-service"],
        ["setup", "--service", "--update", "--app-service"], ["setup", "--codex-login"], ["setup", "--codex-logout"],
        ["setup", "--claude-logout"], ["setup", "--cli"], ["setup", "--cleanup"], ["setup", "--cleanup", "--ok", "d1"],
        ["setup", "--cleanup", "--ok=d2"], ["uninstall"], ["uninstall", "--purge", "--yes"], ["mcp"], ["setup"],
        ["setup", "--update"], ["doctor"], ["stats"], ["stats", "--days", "3"], ["stats", "--days=x"],
        ["logs", "--tasks", "--tools"], ["trace"], ["trace", "abc", "--full"], ["trace", "--json", "--full"],
        ["verdict", "t1", "pass", "--note", "fine"], ["verdict", "t1", "maybe"], ["daemon", "run"], ["daemon"],
        ["memory", "list", "--all"], ["memory", "show"], ["memory", "add", "likes tea", "--kind", "preference"],
        ["memory", "forget", "m-1", "--yes", "--keep-sources"], ["memory", "search", "tea", "--app", "Notes"],
        ["memory"], ["runtimes"], ["runtimes", "use", "worker", "claude", "--model", "m"], ["runtimes", "pins"],
        ["restore"], ["restore", "ab12"], ["browsers"], ["browsers", "pair", "Work", "--browser", "chrome"],
        ["browsers", "unpair", "Work", "--browser", "netscape"], ["browsers", "extension"],
        ["browsers", "extension", "package", "--out", "/tmp/x"], ["ps"], [], ["nope"], ["setup", "--bogus"],
        ["--version"], ["setup", "--ser"], ["uninstall", "--purge", "extra"], ["memory", "edit", "m-2", "new text"]]


def parse(argv: list[str]) -> dict[str, Any]:
    err, out = io.StringIO(), io.StringIO()
    try:
        with contextlib.redirect_stderr(err), contextlib.redirect_stdout(out):
            ns = build_parser().parse_args(argv)
        return {"argv": argv, "args": vars(ns)}
    except SystemExit as e:
        return {"argv": argv, "exit": e.code, "stdout": out.getvalue(), "stderr_last": err.getvalue().strip()
                .splitlines()[-1] if err.getvalue().strip() else ""}


PURE["argv"] = [parse(a) for a in ARGV]
PURE["version"] = __import__("ferb").__version__


# the toolbox's printed lines from invented daemon answers
class Canned:
    def __init__(self, answers: dict[str, Any]) -> None:
        self.answers, self.calls = answers, []

    def call(self, method: str, params: Any = None) -> Any:
        self.calls.append([method, json.loads(json.dumps(params))])   # a copy: callers change theirs later
        a = self.answers[method]
        if isinstance(a, list) and a and isinstance(a[0], dict) and "__seq__" in a[0]:
            return a.pop(0)["value"]
        return a

    def set_timeout(self, s: Any) -> None:
        self.calls.append(["set_timeout", s])


TOOLBOX: list[dict[str, Any]] = []


def toolbox(name: str, answers: dict[str, Any], fn: Callable[[Any, Any], int], **args: Any) -> None:
    c = Canned(json.loads(json.dumps(answers)))
    out = io.StringIO()
    rc = fn(c, out)
    TOOLBOX.append({"name": name, "args": args, "answers": answers, "calls": c.calls, "rc": rc,
                    "out": out.getvalue()})


PROFILES = {"profiles": [
    {"browser": "chrome", "name": "Work", "folder": "Profile 1", "email": "w@example.invalid", "ferb_identity": False,
     "paired": True, "connected": True, "version": "0.19.4", "refused": None},
    {"browser": "chrome", "name": "FERB", "folder": "Profile 7", "email": "", "ferb_identity": True, "paired": True,
     "connected": False, "version": "0.19.1", "refused": None, "last_version": "0.19.0"},
    {"browser": "brave", "name": "Home", "folder": "Default", "email": "", "ferb_identity": False, "paired": True,
     "connected": False, "version": None, "refused": "the extension's key changed"},
    {"browser": "edge", "name": "Old", "folder": "Default", "email": "", "ferb_identity": False, "paired": False,
     "connected": False, "version": None, "refused": None},
    {"browser": "chrome", "name": "New", "folder": "Profile 9", "email": "", "ferb_identity": True, "paired": False,
     "connected": False, "version": None, "refused": None},
    {"browser": "chrome", "name": "Live", "folder": "Profile 3", "email": "", "ferb_identity": True, "paired": True,
     "connected": True, "version": "", "refused": None}], "unpaired_connections": {"brave": 2}}
toolbox("verdict", {"verdict.set": {"verdict": "pass", "task_id": "t-1"}},
        lambda c, o: admin.verdict(c, "t-1", "pass", "fine", o))
toolbox("ps_empty", {"ps.list": {"processes": []}}, lambda c, o: admin.ps(c, o))
toolbox("ps", {"ps.list": {"processes": [
    {"id": "p1", "running": True, "exit_code": None, "long_lived": True, "owner_task": "t-1",
     "command": "python3 -m http.server " + "x" * 90},
    {"id": "p2", "running": False, "exit_code": 2, "long_lived": False, "owner_task": "t-2", "command": "ls"}]}},
    lambda c, o: admin.ps(c, o))
toolbox("browsers_list", {"browsers.list": PROFILES}, lambda c, o: admin.browsers(c, None, None, None, o))
toolbox("browsers_unpair", {"browsers.unpair": {"unpaired": True}},
        lambda c, o: admin.browsers(c, "unpair", "Work", "chrome", o))
toolbox("browsers_unpair_none", {"browsers.unpair": {"unpaired": False}},
        lambda c, o: admin.browsers(c, "unpair", "Work", None, o))
toolbox("browsers_pair", {"browsers.pair": {"paired": True, "browser": "chrome", "name": "Work",
                                            "folder": "Profile 1"}},
        lambda c, o: admin.browsers(c, "pair", "Work", "chrome", o))
toolbox("browsers_pair_refused", {"browsers.pair": {"paired": False, "hint": "open the profile first"}},
        lambda c, o: admin.browsers(c, "pair", "Work", None, o))
toolbox("browsers_pair_no_hint", {"browsers.pair": {"paired": False}},
        lambda c, o: admin.browsers(c, "pair", "Work", None, o))
admin.subprocess = type("P", (), {"run": staticmethod(lambda *a, **k: None)})       # pbcopy: nothing really runs
toolbox("browsers_pair_install", {"browsers.pair": [
    {"__seq__": 1, "value": {"needs_install": True, "extension_dir": "/x/ext", "browser": "chrome", "name": "Work"}},
    {"__seq__": 1, "value": {"paired": True, "browser": "chrome", "name": "Work", "folder": "Profile 1"}}],
    "browsers.installed": [{"__seq__": 1, "value": {"installed": False}}, {"__seq__": 1, "value": {"installed": True}}]},
    lambda c, o: admin.browsers(c, "pair", "Work", "chrome", o, sleep=lambda s: None))
toolbox("browsers_pair_install_gives_up", {"browsers.pair": {"needs_install": True, "extension_dir": "/x/ext",
                                                             "browser": "brave", "name": "Home"},
                                           "browsers.installed": {"installed": False}},
        lambda c, o: admin.browsers(c, "pair", "Home", None, o, sleep=lambda s: None))
RECORDS = {"records": [
    {"id": "0123456789abcdef", "at": "2026-11-12T08:05:00+00:00", "kind": "file", "reason": "edit",
     "original": "/u/notes.txt", "available": True},
    {"id": "short", "at": "2026-11-12T09:05:00+05:30", "kind": "trash", "reason": "trash-unverified",
     "original": "/u/a\x1b[31mred.txt", "available": False},
    {"id": "abcdefabcdefabcd", "at": "2026-11-11T23:59:00-08:00", "kind": "file", "reason": None,
     "original": "/u/gone.txt", "available": False},
    {"id": "trashcopy1", "at": "2026-11-10T00:00:00+00:00", "kind": "trash", "reason": "trash copy",
     "original": "/u/x", "available": True, "needs_owner": True},
    {"id": "snap-0001", "at": "2026-11-09T10:00:00+00:00", "kind": "snapshot", "reason": None, "original": "",
     "available": True, "saved_to": "com.apple.TimeMachine.2026-11-09-100000.local"},
    {"id": "odd", "at": "2026-11-08T10:00:00+00:00", "kind": "file", "reason": "rename", "original": "/u/y",
     "available": True}], "days": 7, "total": 9}
toolbox("restore_list", {"recovery.list": RECORDS}, lambda c, o: admin.restore(c, None, o))
toolbox("restore_none", {"recovery.list": {"records": [], "days": 7}}, lambda c, o: admin.restore(c, None, o))
for kind, in_place in (("trash", True), ("trash", False), ("file", True), ("file", False)):
    toolbox(f"restore_{kind}_{in_place}", {"recovery.restore": {"kind": kind, "in_place": in_place,
                                                                "original": "/u/n\x07.txt",
                                                                "restored_to": "/u/n 2.txt"}},
            lambda c, o: admin.restore(c, "ab12", o), id="ab12")
ITEM = {"id": "i-1", "kind": "preference", "subject": "owner", "key": "drink", "content": "likes tea",
        "origin": "observed", "status": "proposed", "evidence_days": 2, "in_profile": True,
        "valid_to": "2026-11-20T00:00:00+05:30", "sources": ["trail:1", "trail:2"]}
ITEM2 = {"id": "i-2", "kind": "fact", "subject": "owner", "key": None, "content": "lives in Pune",
         "origin": "owner_stated", "status": "active", "sources": []}


def mem(args: dict[str, Any], answers: list[str] | None = None) -> Callable[[Any, Any], int]:
    import argparse
    ns = argparse.Namespace(**{"query": None, "kind": None, "app": None, "person": None, "id": None,
                               "content": None, "yes": False, "keep_sources": False, "all": False,
                               "proposed": False, "subject": "owner", "key": None, "batch": None, **args})
    replies = list(answers or [])
    return lambda c, o: admin.memory(c, ns, lambda prompt: replies.pop(0), o)


toolbox("memory_list", {"memory.list": {"items": [ITEM, ITEM2]}}, mem({"mcmd": "list"}), mcmd="list")
toolbox("memory_list_all_empty", {"memory.list": {"items": []}}, mem({"mcmd": "list", "all": True}), mcmd="list",
        all=True)
toolbox("memory_list_proposed", {"memory.list": {"items": []}}, mem({"mcmd": "list", "proposed": True}), mcmd="list",
        proposed=True)
toolbox("memory_show", {"memory.show": {"subject": "owner", "items": [ITEM, ITEM2]}},
        mem({"mcmd": "show"}), mcmd="show")
toolbox("memory_profile", {"memory.profile": {"markdown": "# Core\n- likes tea"}}, mem({"mcmd": "profile"}),
        mcmd="profile")
toolbox("memory_add", {"memory.add": ITEM2}, mem({"mcmd": "add", "content": "lives in Pune", "kind": "fact"}),
        mcmd="add", content="lives in Pune", kind="fact")
toolbox("memory_edit", {"memory.edit": ITEM2}, mem({"mcmd": "edit", "id": "i-2", "content": "lives in Pune"}),
        mcmd="edit", id="i-2", content="lives in Pune")
toolbox("memory_retract", {"memory.retract": {}}, mem({"mcmd": "retract", "id": "i-2"}), mcmd="retract", id="i-2")
toolbox("memory_forget_no", {"memory.forget": {"will_delete": ["item i-2", "3 trail rows"], "keeps": "keeps x"}},
        mem({"mcmd": "forget", "id": "i-2"}, ["n"]), mcmd="forget", id="i-2", replies=["n"])
toolbox("memory_forget_yes", {"memory.forget": {"will_delete": ["item i-2"]}},
        mem({"mcmd": "forget", "id": "i-2", "yes": True, "keep_sources": True}), mcmd="forget", id="i-2", yes=True,
        keep_sources=True)
toolbox("memory_learning_forget", {"memory.learning": {"plan": ["m-1"], "note": "café"}},
        mem({"mcmd": "forget", "id": "m-1"}, ["yes"]), mcmd="forget", id="m-1", replies=["yes"])
toolbox("memory_learning_forget_no", {"memory.learning": {"plan": ["m-1"]}},
        mem({"mcmd": "forget", "id": "m-1"}, [""]), mcmd="forget", id="m-1", replies=[""])
toolbox("memory_learning_edit", {"memory.learning": {"ok": True}}, mem({"mcmd": "edit", "id": "m-3", "content": "x"}),
        mcmd="edit", id="m-3", content="x")
toolbox("memory_search", {"memory.learning": {"hits": [{"t": "é"}]}},
        mem({"mcmd": "search", "query": "tea", "app": "Notes"}), mcmd="search", query="tea", app="Notes")
toolbox("memory_digest", {"memory.learning": {"pending": []}}, mem({"mcmd": "digest"}), mcmd="digest")
toolbox("memory_embeddings", {"memory.learning": {"loaded": True}}, mem({"mcmd": "embeddings"}), mcmd="embeddings")
toolbox("memory_eval_export", {"memory.learning": {"rows": 3}}, mem({"mcmd": "eval-export"}), mcmd="eval-export")
toolbox("memory_rollback", {"memory.learning": {"rolled_back": "b1"}}, mem({"mcmd": "rollback", "batch": "b1"}),
        mcmd="rollback", batch="b1")
PURE["toolbox"] = TOOLBOX

EVENTS = [{"topic": "task.created", "at": "2026-11-13T09:30:05.123456+05:30", "class": "critical",
           "data": {"id": "t-1", "goal": "café ☕"}},
          {"topic": "trail.app_focus", "at": "2026-11-13T04:00:00+00:00", "class": "notice", "data": {}},
          {"topic": "a.very.long.topic.name.beyond.24", "at": "2026-11-13T04:00:00", "class": "debug",
           "data": {"n": 1.5, "l": [1, None, True]}}]
PURE["log_events"] = [{"event": e, "line": logs_mod.format_event(e)} for e in EVENTS]
PURE["plain_error"] = [{"type": t, "code": c, "message": m, "line": plain_error(RpcError(c, m) if c else
                                                                                FerbError(m))}
                       for t, c, m in (("RpcError", -32603, "KeyError"), ("RpcError", -32001, "not allowed for cli"),
                                       ("FerbError", None, "  "), ("FerbError", None, "plain cause"))]
PURE["shown_bytes"] = [[n, admin.shown_bytes(n)] for n in (0, 999, 1500, 999_999, 1_000_000, 2_345_678,
                                                          999_999_999, 1e9, 2.5e9)]
PURE["status_label"] = [[s, admin.status_label(s)] for s in ("done", "done_unverified", "failed")]
PURE["checks"] = {
    "context_window": [vars(S.context_window_check(r, lim)) for r, lim in (
        ("worker", {"runtime": "codex", "model": "gpt-x", "context_window": 272000, "max_output": 128000}),
        ("planner", {"runtime": "codex", "model": "gpt-x", "context_window": 1, "max_output": 2,
                     "max_input_chars": 1048576}),
        ("checker", {"runtime": "claude", "model": "m", "problem": "no documented limits"}))],
    "clipboard": [None if (c := S.clipboard_check(st)) is None else vars(c)
                  for st in (None, "allow", "unrestricted", "default", "ask", "deny", "odd")],
    "containment": [vars(S.containment_check(c)) for c in (
        None, {}, {"profile_ok": True, "shell": True},
        {"profile_ok": True, "shell": True, "proxy": True, "proxy_alive": False, "proxy_last_error": "refused"},
        {"profile_ok": True, "shell": True, "proxy_restarted": True},
        {"profile_ok": False, "reason": "sandbox-exec missing"},
        {"profile_ok": True, "shell": False, "shell_reason": "denied"})],
    "local_reports": [None if (c := S.local_reports_check(x)) is None else vars(c) for x in (
        None, {}, {"local_reports": {"running": True}}, {"local_reports": {"running": False}},
        {"local_reports": {"running": False, "stopped": "log stream ended"}})],
    "snapshots": [vars(S.snapshot_check(x)) for x in (
        None, {"snapshots": False}, {"snapshot_failure": {"off": True}},
        {"snapshot_failure": {"at": "2026-11-12T08:05:00+05:30", "why": "tmutil failed"}},
        {"snapshot_failure": {"at": "not a time", "why": "x"}}, {"snapshot_failure": {"why": "y"}},
        {"snapshots": True})],
}
HEARING = [None, {}, {"installed": False}, {"installed": True}, {"installed": True, "signed": False},
           {"installed": True, "signed": True, "error": "timed out"},
           {"installed": True, "signed": True, "speech": "denied"},
           {"installed": True, "signed": True, "speech": "granted", "on_device": False, "locale": "hi-IN"},
           {"installed": True, "signed": True, "speech": "granted", "on_device": True, "locale": "en-US"}]
hearing_out = []
for h in HEARING:
    got: list[Any] = []
    S.add_hearing_check(got.append, h)
    hearing_out.append([vars(c) for c in got])
PURE["checks"]["hearing"] = [{"in": h, "out": o} for h, o in zip(HEARING, hearing_out)]


def status_run(rc: int, out: str, raises: bool = False) -> Any:
    def run(argv: list[str], **kw: Any) -> Any:
        if raises:
            raise OSError("no such program")
        return subprocess.CompletedProcess(argv, rc, out, "")
    return run


SIP = [(0, "System Integrity Protection status: enabled.\n", False), (0, "System Integrity Protection status: "
       "disabled.\n", False), (0, "Configuration:\n Apple Internal: enabled\n", False), (1, "", False), (0, "", True)]
FV = [(0, "FileVault is On.\n", False), (0, "FileVault is Off.\n", False), (2, "x", False), (0, "", True)]
PURE["checks"]["sip"] = [{"rc": rc, "out": o, "raises": r, "check": vars(S.sip_check(status_run(rc, o, r)))}
                         for rc, o, r in SIP]
PURE["checks"]["filevault"] = [{"rc": rc, "out": o, "raises": r, "check": vars(S.filevault_check(status_run(rc, o, r)))}
                               for rc, o, r in FV]

# tokenizer: FERB's pinned copy, with an invented pin
DATA = b"invented ranks\n"
import hashlib  # noqa: E402
tok_mod.TOKENIZER_SHA256 = hashlib.sha256(DATA).hexdigest()       # type: ignore[assignment]
tok_cases = []
thome = proot / "tok"


def tok_case(name: str, cache: bytes | None, fetched: Any) -> None:
    tok_mod.verified_tokenizer = lambda: cache                     # type: ignore[assignment]

    def fetch(url: str) -> bytes:
        if isinstance(fetched, Exception):
            raise fetched
        return fetched
    before = snap(proot)
    line = tok_mod.ensure(thome, fetch)
    tok_cases.append({"name": name, "cache": None if cache is None else cache.decode(),
                      "fetched": fetched.decode() if isinstance(fetched, bytes) else
                      [type(fetched).__name__, str(fetched)], "line": line,
                      "changes": changes(before, snap(proot))})


tok_case("downloaded", None, DATA)
tok_case("in_place", None, b"never fetched")
(thome / "cache" / "tokenizer" / "o200k_base.tiktoken").write_bytes(b"tampered")
tok_case("cached_copy", DATA, b"never fetched")
(thome / "cache" / "tokenizer" / "o200k_base.tiktoken").unlink()
tok_case("mismatch", None, b"something else")
tok_case("offline", None, OSError("network unreachable"))
PURE["tokenizer"] = {"pin": tok_mod.TOKENIZER_SHA256, "url": tok_mod.TOKENIZER_URL, "home": str(thome),
                     "cases": tok_cases}

# persona.json: moved out of config.toml once, then changed in place
phome = proot / "persona"
phome.mkdir(mode=0o700)
Config(phome).set("setup", "persona_pending", {"summary": "café notes", "facts": ["a"]})
Config(phome).set("setup", "persona_reviewed", True)
Config(phome).set("setup", "other", 1)
config_before = (phome / "config.toml").read_text()
state = persona.PersonaState(Config(phome))
steps = [{"op": "pending", "result": state.pending()}]
steps.append({"op": "config", "text": (phome / "config.toml").read_text()})
state.set_seen({"summary": "Notes ☕", "at": "2026-11-13"})
steps.append({"op": "reviewed", "result": state.reviewed()})
steps.append({"op": "seen", "result": state.seen()})
steps.append({"op": "file", "text": (phome / "persona.json").read_text(),
              "mode": (phome / "persona.json").stat().st_mode & 0o777})
PURE["persona_state"] = {"config_before": config_before, "steps": steps}

# code_hash: a package with caches, a .DS_Store and the checkout's extension folder beside it
cpkg = proot / "codehash" / "ferb"
(cpkg / "__pycache__").mkdir(parents=True)
(cpkg / "a.py").write_text("print(1)\n")
(cpkg / "sub").mkdir()
(cpkg / "sub" / "b.py").write_bytes(b"\x00binary")
(cpkg / "__pycache__" / "a.cpython-312.pyc").write_bytes(b"cache")
(cpkg / "x.pyc").write_bytes(b"cache")
(cpkg / ".DS_Store").write_bytes(b"finder")
(cpkg / "_extension").mkdir()
(cpkg / "_extension" / "old.js").write_text("old")
first = install_mod.code_hash(cpkg)
ext = proot / "codehash" / "extensions" / "chromium"
ext.mkdir(parents=True)
(ext / "manifest.json").write_text("{}")
(ext / "bg.js").write_text("bg")
PURE["code_hash"] = {"files": tree(proot / "codehash"), "with_extension": install_mod.code_hash(cpkg),
                     "without_extension": first}

write("pure.json", PURE)
print(f"pure.json: {len(PURE['argv'])} argv, {len(TOOLBOX)} toolbox runs")
shutil.rmtree(ROOT, ignore_errors=True)
