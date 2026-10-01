"""`ferb` command line (section 15): logs, trace, daemon run, stop, resume-tasks, undo, autonomy, setup, doctor,
uninstall."""

import argparse
import sys

from ferb.cli import admin, logs
from ferb.cli.client import Client, RpcError
from ferb.core.errors import NotRunning
from ferb.paths import ferb_home, socket_path

LOG_FLAGS = ("tasks", "trail", "tools", "browsers", "policy", "actions", "timers")


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="ferb")
    sub = p.add_subparsers(dest="cmd", required=True)

    lp = sub.add_parser("logs", help="live stream from the daemon's bus")
    for flag in LOG_FLAGS:
        lp.add_argument(f"--{flag}", action="store_true")

    tp = sub.add_parser("trace", help="span tree for one task")
    tp.add_argument("task_id")

    dp = sub.add_parser("daemon", help="run the daemon")
    dp.add_argument("action", choices=["run"])

    sub.add_parser("stop", help="emergency stop: revoke tokens, block all actions, cancel held sends")
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
    dcp = sub.add_parser("decide", help="Jev's answers next to FERB's own decisions (shadow mode)")
    dsub = dcp.add_subparsers(dest="dcmd", required=True)
    dsub.add_parser("stats", help="agreement per question (flag rate + examples where FERB has no logic of its own)")
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
    dvp = sub.add_parser("device", help="this Mac's device profile (apps, scriptable apps, browsers); scan rescans")
    dvp.add_argument("action", nargs="?", choices=["show", "scan"], default="show")
    tl = sub.add_parser("timeline", help="the journal: chats, task outcomes, daily summaries")
    tl.add_argument("--since", help="YYYY-MM-DD")
    sub.add_parser("pause", help="stop every watcher: nothing is recorded until `ferb resume`")
    sub.add_parser("resume", help="start the watchers again after `ferb pause`")
    up = sub.add_parser("undo", help="cancel a pending send/post during its undo window")
    up.add_argument("hold_id", nargs="?")
    ap = sub.add_parser("autonomy", help="show or set the autonomy level")
    ap.add_argument("level", nargs="?", choices=["careful", "balanced", "hands-off"])

    bp = sub.add_parser("browsers", help="browser profiles, pairing and connection status")
    bsub = bp.add_subparsers(dest="bcmd")
    for name, help_ in (("pair", "connect a profile (the FERB extension must be loaded there)"),
                        ("unpair", "forget a profile's pairing")):
        x = bsub.add_parser(name, help=help_)
        x.add_argument("profile", help="the profile's name as shown in the browser (or its email)")
        x.add_argument("--browser", choices=["chrome", "brave"])

    sub.add_parser("ps", help="background processes FERB started")
    kp = sub.add_parser("kill", help="stop one of FERB's background processes")
    kp.add_argument("id")

    cp = sub.add_parser("chat", help="talk to FERB")
    cp.add_argument("--verbose", action="store_true", help="also show plans, steps, tools and checks")
    cp.add_argument("--speak", action="store_true", help="FERB also says its replies out loud (the Mac's voice)")
    sub.add_parser("mcp", help="(used by runtimes) the stdio MCP shim; needs FERB_RUN_TOKEN")
    sp = sub.add_parser("setup", help="install FERB's interpreter, venv and LaunchAgent; permissions; self-test")
    sp.add_argument("--update", action="store_true", help="only reinstall FERB into its venv and restart the agent")
    sp.add_argument("--identity", action="store_true",
                    help="choose FERB's identity profile in your Chrome (it must hold only FERB's account)")
    sp.add_argument("--profile", help="with --identity: the Chrome profile's name, if several hold FERB's account")
    sp.add_argument("--jev", action="store_true", help="store the Jev API key in the Keychain and check it answers")
    sp.add_argument("--codex-login", action="store_true",
                    help="sign in FERB's own Codex (its own home: your personal Codex settings and skills stay out)")
    sub.add_parser("doctor", help="check the installation, permissions (from inside the daemon) and self-test")
    xp = sub.add_parser("uninstall", help="remove the LaunchAgent; --purge also deletes ~/.ferb")
    xp.add_argument("--purge", action="store_true")

    args = p.parse_args(argv)
    if args.cmd == "daemon":
        from ferb.daemon.main import main as daemon_main
        daemon_main()
        return 0

    if args.cmd == "mcp":
        from ferb.cli.mcp import main as mcp_main
        mcp_main()
        return 0
    if args.cmd == "chat":
        from ferb.cli.chat import main_chat
        try:
            return main_chat(lambda: Client(socket_path(), timeout=1800), verbose=args.verbose,
                             speak=args.speak)
        except NotRunning as e:
            print(f"ferb: {e}", file=sys.stderr)
            return 2
        except KeyboardInterrupt:
            return 130
    if args.cmd in ("setup", "doctor", "uninstall"):
        from ferb.cli import setup as setup_cmd
        from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID
        from ferb.device.install import NATIVE_HOSTS_DIRS, InstallError, Installer
        inst = Installer(ferb_home(), native_hosts_dirs=NATIVE_HOSTS_DIRS, extension_ids=(FERB_EXTENSION_ID,))
        env = setup_cmd.Env(inst, lambda: Client(socket_path()), input, sys.stdout)
        try:
            if args.cmd == "setup" and args.jev:
                return setup_cmd.jev_setup(env)
            if args.cmd == "setup" and args.codex_login:
                return setup_cmd.codex_login(env)
            if args.cmd == "setup" and args.identity:
                return setup_cmd.identity_setup(env, args.profile)
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

    try:
        client = Client(socket_path())
        try:
            if args.cmd == "logs":
                filters = [f for f in LOG_FLAGS if getattr(args, f)]
                return logs.run(client, filters, sys.stdout)
            if args.cmd == "trace":
                return admin.trace(client, args.task_id, sys.stdout)
            if args.cmd == "stop":
                return admin.stop(client, sys.stdout)
            if args.cmd == "resume-tasks":
                return admin.resume(client, sys.stdout)
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
            if args.cmd == "pause":
                return admin.pause(client, sys.stdout)
            if args.cmd == "resume":
                return admin.sense_resume(client, sys.stdout)
            if args.cmd == "undo":
                return admin.undo(client, args.hold_id, sys.stdout)
            if args.cmd == "ps":
                return admin.ps(client, sys.stdout)
            if args.cmd == "kill":
                return admin.kill(client, args.id, sys.stdout)
            if args.cmd == "browsers":
                return admin.browsers(client, args.bcmd, getattr(args, "profile", None), getattr(args, "browser", None),
                                      sys.stdout)
            return admin.autonomy(client, args.level, sys.stdout)
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
