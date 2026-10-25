"""Register CLI commands and common connection helpers. Must not implement task orchestration.

`ferb` command line: logs, trace, daemon run, stop, resume-tasks, restore, setup, doctor,
uninstall."""

import argparse
import json
import sys

from ferb.cli import admin, logs
from ferb.cli.client import Client, RpcError
from ferb.core.errors import NotRunning
from ferb.paths import ferb_home, socket_path

LOG_FLAGS = ("tasks", "trail", "tools", "browsers", "policy", "actions", "timers", "voice")


def main(argv: list[str] | None = None) -> int:
    import ferb
    p = argparse.ArgumentParser(prog="ferb")
    p.add_argument("--version", action="version", version=f"ferb {ferb.__version__}")
    sub = p.add_subparsers(dest="cmd", required=True)

    from ferb.cli.conversations import register
    register(sub)

    lp = sub.add_parser("logs", help="live stream from the daemon's bus")
    for flag in LOG_FLAGS:
        lp.add_argument(f"--{flag}", action="store_true")

    vp = sub.add_parser("voice", help="push-to-talk: watch it live (no argument), or turn it on / off")
    vp.add_argument("switch", nargs="?", choices=["on", "off"],
                    help="on: start push-to-talk (after `ferb setup --voice`); off: stop it and quit FERB.app")

    tp = sub.add_parser("trace", help="what a task did, step by step (default: the latest task)")
    tp.add_argument("task_id", nargs="?", help="a task id or its first characters (default: the latest task)")
    trace_format = tp.add_mutually_exclusive_group()
    trace_format.add_argument("--steps", action="store_true", help="one line per tool call")
    trace_format.add_argument("--full", action="store_true", help="show all recorded span fields")
    trace_format.add_argument("--json", action="store_true", help="export the complete recorded trace as JSON")

    vd = sub.add_parser("verdict", help="record your pass/fail verdict on a task (acceptance is your word)")
    vd.add_argument("task_id")
    vd.add_argument("verdict", choices=["pass", "fail"])
    vd.add_argument("--note", default="")

    dp = sub.add_parser("daemon", help="run the daemon")
    dp.add_argument("action", choices=["run"])

    sub.add_parser("stop", help="emergency stop: revoke tokens, cancel active tasks")
    sub.add_parser("resume-tasks", help="lift the emergency stop")
    mp = sub.add_parser("memory", help="what FERB remembers: list, show, add, confirm, edit, retract, forget")
    msub = mp.add_subparsers(dest="mcmd", required=True)
    ml = msub.add_parser("list", help="active items (--proposed: waiting for you; --all: everything)")
    ml.add_argument("--proposed", action="store_true")
    ml.add_argument("--all", action="store_true")
    ms = msub.add_parser("show", help="every item about a subject, with its history and sources")
    ms.add_argument("subject", nargs="?", default="owner")
    ma = msub.add_parser("add", help="remember something you state")
    ma.add_argument("content")
    ma.add_argument("--kind", default="fact", choices=["fact", "preference", "person", "project", "env"])
    ma.add_argument("--subject", default="owner")
    ma.add_argument("--key")
    for name, help_ in (("confirm", "activate an inferred item"), ("retract", "stop using an item (kept as history)")):
        x = msub.add_parser(name, help=help_)
        x.add_argument("id")
    me = msub.add_parser("edit", help="correct an item (a new version; the old one stays as history)")
    me.add_argument("id")
    me.add_argument("content")
    mf = msub.add_parser("forget", help="purge an item, its versions and its sources (asks first)")
    mf.add_argument("id")
    mf.add_argument("--keep-sources", action="store_true")
    mf.add_argument("--yes", action="store_true")
    msub.add_parser("dashboard", help="open the token-protected local memory dashboard link")
    msub.add_parser("eval-export", help="export actual owner-history queries for relevance labels")
    emb = msub.add_parser("embeddings", help="local model prepare/evaluate/adopt/status")
    emb.add_argument("action", choices=("status", "prepare", "evaluate", "adopt"))
    emb.add_argument("--manifest")
    emb.add_argument("--dataset")
    emb.add_argument("--evaluation")
    msub.add_parser("digest", help="pending identity suggestions, at most once a day")
    msub.add_parser("consolidate", help="run incremental learning while no task is running")
    msr = msub.add_parser("search", help="find episodes, procedures, lessons and facts")
    msr.add_argument("query")
    msr.add_argument("--kind")
    msr.add_argument("--app")
    msr.add_argument("--person")
    mid = msub.add_parser("identity", help="state an owner rule for always-loaded identity")
    mid.add_argument("text")
    mid.add_argument("--key", default="owner")
    mrb = msub.add_parser("rollback", help="invalidate a sleep-learning batch while preserving history")
    mrb.add_argument("batch")
    dcp = sub.add_parser("decide", help="Jev's answers next to FERB's own decisions (shadow mode)")
    dsub = dcp.add_subparsers(dest="dcmd", required=True)
    dsub.add_parser("stats", help="observation and memory checks: flag rate, examples and errors")
    for name, help_ in (("enable", "let Jev's stricter answers act for one question"),
                        ("disable", "back to shadow mode for one question")):
        x = dsub.add_parser(name, help=help_)
        x.add_argument("question")
    scp = sub.add_parser("schedules", help="scheduled and recurring tasks: list, cancel / pause / resume <id>")
    scp.add_argument("action", nargs="?", choices=["list", "cancel", "pause", "resume"], default="list")
    scp.add_argument("id", nargs="?")
    rp = sub.add_parser("reminders", help="pending reminders and timers (cancel <id> to cancel one)")
    rp.add_argument("action", nargs="?", choices=["list", "cancel"], default="list")
    rp.add_argument("id", nargs="?")
    rp = sub.add_parser("runtimes", help="model runtimes: capability table, what each role uses; `use` switches a role")
    rsub = rp.add_subparsers(dest="rcmd")
    ru = rsub.add_parser("use", help="make a role (orchestrator, worker, checker) use a runtime, from its next call")
    ru.add_argument("role")
    ru.add_argument("runtime")
    ru.add_argument("--model", default=None)

    dvp = sub.add_parser("device", help="this Mac's device profile (apps, scriptable apps, browsers); scan rescans")
    dvp.add_argument("action", nargs="?", choices=["show", "scan"], default="show")
    rsp = sub.add_parser("restore", help="files FERB changed or moved to the Trash lately; restore <id> puts one back")
    rsp.add_argument("id", nargs="?", help="the record to put back (its first characters are enough)")
    tl = sub.add_parser("timeline", help="the journal: chats, task outcomes, daily summaries")
    tl.add_argument("--since", help="YYYY-MM-DD")
    sub.add_parser("pause", help="stop every watcher: nothing is recorded until `ferb resume`")
    sub.add_parser("resume", help="start the watchers again after `ferb pause`")
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
    kp = sub.add_parser("kill", help="stop one of FERB's background processes")
    kp.add_argument("id")
    ap = sub.add_parser('attach', help='watch an existing FERB process; optionally take over its input')
    ap.add_argument('id')
    ap.add_argument('--take', action='store_true', help='pause worker input until you detach with Ctrl-]')

    cp = sub.add_parser("chat", help="talk to FERB")
    cp.add_argument("--verbose", action="store_true", help="also show plans, steps, tools and checks")
    cp.add_argument("--speak", action="store_true", help="FERB also says its replies out loud (the Mac's voice)")
    conversation = cp.add_mutually_exclusive_group()
    conversation.add_argument("--new", action="store_true", help="start a separate conversation")
    conversation.add_argument("--resume", metavar="ID", help="resume a conversation printed by --new")
    sub.add_parser("mcp", help="(used by runtimes) the stdio MCP shim; needs FERB_RUN_TOKEN")
    sp = sub.add_parser("setup", help="install FERB's interpreter, venv and LaunchAgent; permissions; self-test")
    sp.add_argument("--update", action="store_true", help="only reinstall FERB into its venv and restart the agent")
    sp.add_argument("--persona", action="store_true", help="review local app-use evidence without reinstalling or permission dialogs")
    sp.add_argument("--identity", action="store_true",
                    help="choose FERB's identity profile in your Chrome (it must hold only FERB's account)")
    sp.add_argument("--profile", help="with --identity: the Chrome profile's name, if several hold FERB's account")
    sp.add_argument("--key", choices=["anthropic", "openai", "google"],
                    help="store a provider's API key in the Keychain and check it answers (direct-API runtimes)")
    sp.add_argument("--jev", action="store_true", help="store the Jev API key in the Keychain and check it answers")
    sp.add_argument("--codex-login", action="store_true",
                    help="sign in FERB's own Codex (its own home: your personal Codex settings and skills stay out)")
    sp.add_argument("--codex-logout", action="store_true",
                    help="sign out FERB's own Codex (your personal Codex login is never touched)")
    sp.add_argument("--cli", action="store_true",
                    help="link the `ferb` command at ~/.local/bin to this FERB.app (Settings → Advanced)")
    sp.add_argument("--claude-login", action="store_true",
                    help="sign in FERB's own Claude (its own home: your personal Claude Code settings stay out)")
    sp.add_argument("--voice", action="store_true",
                    help="push-to-talk: build and sign FERB.app, ask for Microphone + Speech Recognition, turn it on")
    sp.add_argument("--service", action="store_true",
                    help="FERB.app's setup window: install (with --update, update) the background service with no "
                         "questions and wait until it answers")
    sub.add_parser("doctor", help="check the installation, permissions (from inside the daemon) and self-test")
    wp = sub.add_parser("watch", help="the owner-verified background test: each interaction runs in the background "
                                      "and you say what you saw")
    wp.add_argument("--mac", action="store_true")
    wp.add_argument("--browser", action="store_true")
    wp.add_argument("--only", help="comma-separated step ids, e.g. M4,M9")
    wp.add_argument("--dry-run", action="store_true", help="list the steps and exit")
    stp = sub.add_parser("stats", help="how FERB is doing: task outcomes, tool errors, model runs, and its own CPU and "
                                       "memory per day (from the database)")
    stp.add_argument("--days", type=int, default=7)
    bp = sub.add_parser("baseline", help="capture metrics or compare two saved baseline JSON files")
    bp.add_argument('--compare', nargs=2, metavar=('BEFORE', 'AFTER'), help='read-only saved-snapshot comparison')
    sub.add_parser("release-checklist", help="print owner-supervised release checks as JSON; never runs them")
    tm = sub.add_parser("test-mode", help="inspect or set the exact send allowlist for owner-run tests")
    tm.add_argument("action", nargs="?", default="status", choices=("status", "on", "off"))
    tm.add_argument("--allow-send", action="append", default=[],
                    help='exact recipient, or "SITE @ACCOUNT" (or "SITE") for a post on that page; '
                         'repeat per destination')
    rg = sub.add_parser("regress", help="run a regression task set through the daemon; you give the verdicts")
    rg.add_argument("--allow-send", action="append", default=[], help="exact recipient allowed in test-mode send actions")
    rg.add_argument("set_file", nargs="?", default=None, help="task set TOML (default: the built-in smoke set)")
    xp = sub.add_parser("uninstall", help="remove the LaunchAgent; --purge also deletes ~/.ferb")
    xp.add_argument("--purge", action="store_true")

    args = p.parse_args(argv)
    if args.cmd == "daemon":
        from ferb.daemon.main import main as daemon_main
        daemon_main()
        return 0

    if args.cmd == "stats":
        return admin.stats(ferb_home(), sys.stdout, args.days)
    if args.cmd == "release-checklist":
        from ferb.eval.release import checklist
        print(json.dumps(checklist(), ensure_ascii=False, indent=2))
        return 0
    if args.cmd == "baseline":
        if args.compare:
            from pathlib import Path
            from ferb.eval.comparison import compare_baselines
            try:
                before, after = (json.loads(Path(p).read_text()) for p in args.compare)
                print(json.dumps(compare_baselines(before, after), indent=2))
                return 0
            except (OSError, ValueError, TypeError, KeyError) as exc:
                print(f"ferb: cannot compare baselines: {exc}", file=sys.stderr)
                return 1
        return admin.baseline(ferb_home(), sys.stdout)
    if args.cmd == "watch":
        from ferb.device.watch import main as watch_main
        flags = [f for f, on in (("--mac", args.mac), ("--browser", args.browser), ("--dry-run", args.dry_run)) if on]
        return watch_main(flags + (["--only", args.only] if args.only else []))
    if args.cmd == "mcp":
        from ferb.cli.mcp import main as mcp_main
        mcp_main()
        return 0
    if args.cmd == "runtimes":
        from ferb.cli import runtimes as runtimes_cli
        if args.rcmd == "use":
            return runtimes_cli.use(ferb_home(), args.role, args.runtime, args.model, sys.stdout)
        return runtimes_cli.show(ferb_home(), sys.stdout)
    if args.cmd == "chat":
        from ferb.cli.chat import main_chat
        try:
            return main_chat(lambda: Client(socket_path(), timeout=1800), verbose=args.verbose,
                             speak=args.speak, new=args.new, resume=args.resume)
        except NotRunning as e:
            print(f"ferb: {e}", file=sys.stderr)
            return 2
        except KeyboardInterrupt:
            return 130
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
        inst = setup_cmd.installer(ferb_home(), SUPPORT, every_browser=args.cmd == "uninstall",
                                   cli_dir=Path.home() / ".local" / "bin")       # where FERB.app links `ferb`
        env = setup_cmd.Env(inst, lambda: Client(socket_path()), input, sys.stdout, browser_support=SUPPORT)
        try:
            if args.cmd == "setup" and args.service:
                return setup_cmd.service_setup(env, update=args.update)
            if args.cmd == "setup" and args.key:
                return setup_cmd.key_setup(env, args.key)
            if args.cmd == "setup" and args.jev:
                return setup_cmd.jev_setup(env)
            if args.cmd == "setup" and args.voice:
                return setup_cmd.voice_setup(env)
            if args.cmd == "setup" and args.codex_logout:
                return setup_cmd.codex_logout(env)
            if args.cmd == "setup" and args.cli:
                return setup_cmd.cli_setup(env)
            if args.cmd == "setup" and args.codex_login:
                return setup_cmd.codex_login(env)
            if args.cmd == "setup" and args.claude_login:
                return setup_cmd.claude_login(env)
            if args.cmd == "setup" and args.identity:
                return setup_cmd.identity_setup(env, args.profile)
            if args.cmd == "setup" and args.persona:
                return setup_cmd.persona_setup(env)
            if args.cmd == "setup":
                return setup_cmd.setup(env, update=args.update)
            if args.cmd == "doctor":
                return setup_cmd.doctor(env)
            return setup_cmd.uninstall(env, purge=args.purge)
        except (InstallError, NotRunning) as e:
            print(f"ferb: {e}", file=sys.stderr)
            return 1
        except KeyboardInterrupt:
            return 130

    if args.cmd == "device":
        from ferb.cli.admin import device
        return device(ferb_home(), args.action, lambda: Client(socket_path()), sys.stdout)

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
            if args.cmd == "voice" and args.switch:
                return logs.voice_switch(client, args.switch == "on", sys.stdout)
            if args.cmd == "voice":
                return logs.voice(client, sys.stdout)
            if args.cmd == "trace":
                return admin.trace(client, args.task_id, sys.stdout, args.full, args.json, args.steps)
            if args.cmd == "verdict":
                return admin.verdict(client, args.task_id, args.verdict, args.note, sys.stdout)
            if args.cmd == "test-mode":
                if args.allow_send and args.action != "on":
                    raise ValueError("--allow-send requires test-mode on")
                params = ({"status": True} if args.action == "status" else
                          {"off": True} if args.action == "off" else {"allow_send": args.allow_send})
                print(json.dumps(client.call("test.mode", params), indent=2))
                return 0
            if args.cmd == "regress":
                from pathlib import Path

                from ferb.eval.regress import DEFAULT_SET
                set_path = Path(args.set_file) if args.set_file else DEFAULT_SET
                if not set_path.is_file():
                    print(f"ferb: no such regression set: {set_path}", file=sys.stderr)
                    return 2
                return admin.regress(client, lambda: Client(socket_path()), set_path, ferb_home(), sys.stdout,
                                     allow_send=args.allow_send)
            if args.cmd == "stop":
                return admin.stop(client, sys.stdout)
            if args.cmd == "resume-tasks":
                return admin.resume(client, sys.stdout)
            if args.cmd in ('conversations', 'notifications'):
                from ferb.cli.conversations import run
                return run(client, args, sys.stdout)
            if args.cmd == "memory":
                return admin.memory(client, args, input, sys.stdout)
            if args.cmd == "schedules":
                return admin.schedules(client, args.action, args.id, sys.stdout)
            if args.cmd == "reminders":
                return admin.reminders(client, args.action, args.id, sys.stdout)
            if args.cmd == "decide":
                return admin.decide(client, args, input, sys.stdout)
            if args.cmd == "timeline":
                return admin.timeline(client, args.since, sys.stdout)
            if args.cmd == "restore":
                return admin.restore(client, args.id, sys.stdout)
            if args.cmd == "pause":
                return admin.pause(client, sys.stdout)
            if args.cmd == "resume":
                return admin.sense_resume(client, sys.stdout)
            if args.cmd == "ps":
                return admin.ps(client, sys.stdout)
            if args.cmd == "kill":
                return admin.kill(client, args.id, sys.stdout)
            if args.cmd == 'attach':
                from ferb.cli.attach import attach
                return attach(lambda: Client(socket_path()), args.id, args.take)
            if args.cmd == "browsers":
                return admin.browsers(client, args.bcmd, getattr(args, "profile", None), getattr(args, "browser", None),
                                      sys.stdout)
            return 2
        finally:
            client.close()
    except NotRunning as e:
        print(f"ferb: {e}", file=sys.stderr)
        return 2
    except RpcError as e:
        print(f"ferb: {e}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    sys.exit(main())
