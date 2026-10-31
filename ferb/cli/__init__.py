"""Register CLI commands and common connection helpers. Must not implement task orchestration.

`ferb` is plumbing and a developer toolbox, never a user interface (the FERB app is; notes/daemon.md decisions 3, 4):
setup --service/--update/--cleanup/--codex-login/--codex-logout/--claude-logout/--cli, uninstall, mcp and daemon run
for the app and the models (`APP_COMMANDS`); a checkout's `setup [--update]`; trace, logs, stats, doctor, memory,
restore, ps, verdict, runtimes and browsers for debugging. Errors are one plain line (`plain_error`), never a
traceback."""

import argparse
import sys

from ferb.cli import admin, logs
from ferb.cli.client import Client, RpcError
from ferb.core.errors import FerbError, NotRunning
from ferb.paths import ferb_home, socket_path

LOG_FLAGS = ("tasks", "trail", "tools", "browsers", "policy", "actions", "timers")
# What FERB.app runs (ui/mac/FERBNotch: AppSetup.swift, SetupModel.swift, SettingsPages.swift; `--cleanup` is for its
# clean-up screen, notes/storage.md decision 4); tests keep them working
APP_COMMANDS = (("setup", "--service"), ("setup", "--service", "--update"), ("setup", "--codex-login"),
                ("setup", "--codex-logout"), ("setup", "--claude-logout"), ("setup", "--cli"), ("setup", "--cleanup"),
                ("uninstall",), ("uninstall", "--purge", "--yes"), ("mcp",))


def plain_error(e: BaseException) -> str:
    """One line for an error, plain cause first: a daemon failure that names only its exception type says it failed
    inside FERB (`ferb stats` lists it)."""
    if isinstance(e, RpcError) and e.code == -32603:              # server.INTERNAL: the message is the type only
        return f"ferb: that failed inside FERB ({e}); `ferb stats` lists it"
    text = str(e).strip() or type(e).__name__
    return f"ferb: {text}"


def build_parser() -> argparse.ArgumentParser:
    import ferb
    p = argparse.ArgumentParser(prog="ferb")
    p.add_argument("--version", action="version", version=f"ferb {ferb.__version__}")
    sub = p.add_subparsers(dest="cmd", required=True)

    lp = sub.add_parser("logs", help="live stream from the daemon's bus")
    for flag in LOG_FLAGS:
        lp.add_argument(f"--{flag}", action="store_true")

    tp = sub.add_parser("trace", help="what a task did, step by step (default: the latest task)")
    tp.add_argument("task_id", nargs="?", help="a task id or its first characters (default: the latest task)")
    trace_format = tp.add_mutually_exclusive_group()
    trace_format.add_argument("--full", action="store_true", help="show all recorded span fields")
    trace_format.add_argument("--json", action="store_true", help="export the complete recorded trace as JSON")

    vd = sub.add_parser("verdict", help="record your pass/fail verdict on a task (acceptance is your word)")
    vd.add_argument("task_id")
    vd.add_argument("verdict", choices=["pass", "fail"])
    vd.add_argument("--note", default="")

    dp = sub.add_parser("daemon", help="run the daemon")
    dp.add_argument("action", choices=["run"])

    mp = sub.add_parser("memory", help="what FERB remembers: list, show, profile, add, edit, retract, forget")
    msub = mp.add_subparsers(dest="mcmd", required=True)
    ml = msub.add_parser("list", help="active items (--proposed: observed patterns still gathering days; --all: everything)")
    ml.add_argument("--proposed", action="store_true")
    ml.add_argument("--all", action="store_true")
    ms = msub.add_parser("show", help="every item about a subject, with its history and sources")
    ms.add_argument("subject", nargs="?", default="owner")
    ma = msub.add_parser("add", help="remember something you state")
    ma.add_argument("content")
    ma.add_argument("--kind", default="fact", choices=["fact", "preference", "person", "project", "env"])
    ma.add_argument("--subject", default="owner")
    ma.add_argument("--key")
    msub.add_parser("profile", help="the core profile (Tier 1): what is always in FERB's prompts")
    x = msub.add_parser("retract", help="stop using an item (kept as history)")
    x.add_argument("id")
    me = msub.add_parser("edit", help="correct an item (a new version; the old one stays as history)")
    me.add_argument("id")
    me.add_argument("content")
    mf = msub.add_parser("forget", help="delete an item, its versions and the activity rows it came from (asks first)")
    mf.add_argument("id")
    mf.add_argument("--keep-sources", action="store_true")
    mf.add_argument("--yes", action="store_true")
    msub.add_parser("dashboard", help="open the token-protected local memory dashboard link")
    msub.add_parser("eval-export", help="export actual owner-history queries for relevance labels")
    msub.add_parser("embeddings", help="the local embedding model: loaded or not, where, and how many vectors")
    msub.add_parser("digest", help="pending identity suggestions, at most once a day")
    msr = msub.add_parser("search", help="find episodes, facts and activity summaries")
    msr.add_argument("query")
    msr.add_argument("--kind")
    msr.add_argument("--app")
    msr.add_argument("--person")
    mrb = msub.add_parser("rollback", help="invalidate a sleep-learning batch while preserving history")
    mrb.add_argument("batch")
    rp = sub.add_parser("runtimes", help="model runtimes: the runtime table, what each role uses; `use` switches a role")
    rsub = rp.add_subparsers(dest="rcmd")
    rsub.add_parser("pins", help="compare FERB's Codex and Claude Agent SDK pins with npm and PyPI (reads the network; "
                                 "changes nothing)")
    ru = rsub.add_parser("use", help="make a role (orchestrator, worker, checker) use a runtime, from its next call")
    ru.add_argument("role")
    ru.add_argument("runtime")
    ru.add_argument("--model", default=None)

    rsp = sub.add_parser("restore", help="files FERB changed or moved to the Trash lately; restore <id> puts one back")
    rsp.add_argument("id", nargs="?", help="the record to put back (its first characters are enough)")
    bp = sub.add_parser("browsers", help="browser profiles, pairing and connection status")
    bsub = bp.add_subparsers(dest="bcmd")
    extension = bsub.add_parser("extension", help="prepare, locate or package FERB's browser extension")
    extension.add_argument("action", choices=["install", "path", "package"], nargs="?", default="path",
                           help="install: native host, Web Store request and each profile's state; path: the managed "
                                "folder; package: the Chrome Web Store zip")
    extension.add_argument("--out", help="with package: the folder for the zip (default: FERB's extensions folder)")
    from ferb.act.browser.profiles import BROWSERS
    for name, help_ in (("pair", "connect a profile (the FERB extension must be loaded there)"),
                        ("unpair", "forget a profile's pairing")):
        x = bsub.add_parser(name, help=help_)
        x.add_argument("profile", help="the profile's name as shown in the browser (or its email)")
        x.add_argument("--browser", choices=list(BROWSERS))

    sub.add_parser("ps", help="background processes FERB started")
    sub.add_parser("mcp", help="(used by runtimes) the stdio MCP shim; needs FERB_RUN_TOKEN")
    sp = sub.add_parser("setup", help="a checkout: install FERB's interpreter, venv and development LaunchAgent; "
                                      "FERB.app: its setup steps")
    sp.add_argument("--update", action="store_true", help="only reinstall FERB into a new venv and restart the daemon")
    sp.add_argument("--codex-login", action="store_true",
                    help="sign in FERB's own Codex (its own home: your personal Codex settings and skills stay out)")
    sp.add_argument("--codex-logout", action="store_true",
                    help="sign out FERB's own Codex (your personal Codex login is never touched)")
    sp.add_argument("--cli", action="store_true",
                    help="link the `ferb` command at ~/.local/bin to this FERB.app (Settings → Advanced)")
    sp.add_argument("--claude-logout", action="store_true",
                    help="sign out FERB's own Claude (your personal Claude Code login is never touched)")
    sp.add_argument("--cleanup", action="store_true",
                    help="FERB.app's clean-up screen: what the one-time clean-up of unused data removes, as JSON; with "
                         "--ok, run it (the daemon is stopped meanwhile)")
    sp.add_argument("--ok", metavar="DIGEST", help="with --cleanup: the `ok` of the preview the owner agreed to")
    sp.add_argument("--service", action="store_true",
                    help="FERB.app's setup window: install (with --update, update) the background service with no "
                         "questions and wait until it answers")
    sub.add_parser("doctor", help="check the installation, permissions (from inside the daemon) and self-test")
    stp = sub.add_parser("stats", help="how FERB is doing: task outcomes, tool errors, model runs, and its own CPU and "
                                       "memory per day (from the database)")
    stp.add_argument("--days", type=int, default=7)
    xp = sub.add_parser("uninstall", help="stop the background service and remove its files; --purge also deletes "
                                          "~/.ferb and every FERB Keychain item")
    xp.add_argument("--purge", action="store_true")
    xp.add_argument("--yes", action="store_true",
                    help="with --purge: the owner already confirmed in FERB.app (Settings → Advanced), so no question")
    return p


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.cmd == "daemon":
        from ferb.daemon.main import main as daemon_main
        daemon_main()
        return 0

    if args.cmd == "stats":
        return admin.stats(ferb_home(), sys.stdout, args.days)
    if args.cmd == "mcp":
        from ferb.cli.mcp import main as mcp_main
        mcp_main()
        return 0
    if args.cmd == "runtimes":
        from ferb.cli import runtimes as runtimes_cli
        if args.rcmd == "use":
            return runtimes_cli.use(ferb_home(), args.role, args.runtime, args.model, sys.stdout)
        if args.rcmd == "pins":
            return runtimes_cli.pins(sys.stdout)
        return runtimes_cli.show(ferb_home(), sys.stdout)
    if args.cmd == "browsers" and args.bcmd == "extension":
        from ferb.act.browser.profiles import SUPPORT
        from ferb.device.extensions import extension_path, extension_source, install_extension
        if args.action == "package":
            from pathlib import Path
            from ferb.act.browser.autoinstall import package
            try:
                out_dir = Path(args.out) if args.out else extension_path(ferb_home()).parent
                target, notes = package(extension_source(), out_dir)
            except (OSError, ValueError) as e:
                print(f"ferb: cannot package the extension: {e}", file=sys.stderr)
                return 1
            print(f"Chrome Web Store package: {target}")
            for note in notes:
                print(f"  Before uploading: {note}")
            return 0
        if args.action == "install":
            from ferb.cli import setup as setup_cmd
            target = install_extension(ferb_home())
            inst = setup_cmd.installer(ferb_home(), SUPPORT)
            inst.write_native_host()
            print(f"Extension files and native host prepared: {target}")
            print("Existing managed installations reload when idle.")
            setup_cmd.extension_setup(setup_cmd.Env(inst, lambda: Client(socket_path()), input, sys.stdout,
                                                    browser_support=SUPPORT))
            return 0
        print(extension_path(ferb_home()))
        return 0
    if args.cmd in ("setup", "doctor", "uninstall"):
        from ferb.cli import setup as setup_cmd
        from ferb.act.browser.profiles import SUPPORT
        from ferb.device.install import InstallError
        from pathlib import Path
        if args.cmd == "setup":                   # before anything reads config.toml or installs (owner, 2026-11-01)
            from ferb.store import salvage
            try:
                salvage.refuse_newer(ferb_home())  # a newer FERB's database: nothing is changed
                for line in salvage.describe(salvage.check_files(ferb_home())):
                    print(line)
            except (FerbError, OSError) as e:
                print(plain_error(e), file=sys.stderr)
                return 1
        inst = setup_cmd.installer(ferb_home(), SUPPORT, every_browser=args.cmd == "uninstall",
                                   cli_dir=Path.home() / ".local" / "bin")       # where FERB.app links `ferb`
        env = setup_cmd.Env(inst, lambda: Client(socket_path()), input, sys.stdout, browser_support=SUPPORT)
        try:
            if args.cmd == "setup" and args.cleanup:
                return setup_cmd.cleanup_apply(env, args.ok) if args.ok else setup_cmd.cleanup_preview(env)
            if args.cmd == "setup" and args.service:
                return setup_cmd.service_setup(env, update=args.update)
            if args.cmd == "setup" and args.codex_logout:
                return setup_cmd.codex_logout(env)
            if args.cmd == "setup" and args.cli:
                return setup_cmd.cli_setup(env)
            if args.cmd == "setup" and args.codex_login:
                return setup_cmd.codex_login(env)
            if args.cmd == "setup" and args.claude_logout:
                return setup_cmd.claude_logout(env)
            if args.cmd == "setup":
                return setup_cmd.setup(env, update=args.update)
            if args.cmd == "doctor":
                return setup_cmd.doctor(env)
            return setup_cmd.uninstall(env, purge=args.purge, confirmed=args.yes)
        except (InstallError, FerbError, OSError) as e:
            print(plain_error(e), file=sys.stderr)
            return 1
        except KeyboardInterrupt:
            return 130

    if args.cmd == "memory" and args.mcmd == "dashboard":
        from ferb.memory.dashboard import Dashboard
        def memory_call(params):
            c = Client(socket_path())
            try:
                return c.call("memory.learning", params)
            finally:
                c.close()
        dashboard = Dashboard(memory_call)
        print("Memory dashboard (keep this terminal open): " + dashboard.url)
        try:
            dashboard.serve()
        except KeyboardInterrupt:
            return 0
        return 0

    try:
        client = Client(socket_path())
        try:
            if args.cmd == "logs":
                filters = [f for f in LOG_FLAGS if getattr(args, f)]
                return logs.run(client, filters, sys.stdout)
            if args.cmd == "trace":
                return admin.trace(client, args.task_id, sys.stdout, args.full, args.json)
            if args.cmd == "verdict":
                return admin.verdict(client, args.task_id, args.verdict, args.note, sys.stdout)
            if args.cmd == "memory":
                return admin.memory(client, args, input, sys.stdout)
            if args.cmd == "restore":
                return admin.restore(client, args.id, sys.stdout)
            if args.cmd == "ps":
                return admin.ps(client, sys.stdout)
            if args.cmd == "browsers":
                return admin.browsers(client, args.bcmd, getattr(args, "profile", None), getattr(args, "browser", None),
                                      sys.stdout)
            return 2
        finally:
            client.close()
    except NotRunning as e:
        print(plain_error(e), file=sys.stderr)
        return 2
    except (FerbError, OSError) as e:
        print(plain_error(e), file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    sys.exit(main())
