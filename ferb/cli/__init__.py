"""`ferb` command line (section 15): logs, trace, daemon run, stop, resume-tasks, undo, autonomy, setup, doctor,
uninstall."""

import argparse
import sys

from ferb.cli import admin, logs
from ferb.cli.client import Client, RpcError
from ferb.core.errors import NotRunning
from ferb.paths import ferb_home, socket_path

LOG_FLAGS = ("tasks", "trail", "tools", "browsers", "policy", "actions")


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

    sub.add_parser("chat", help="talk to FERB")
    sub.add_parser("mcp", help="(used by runtimes) the stdio MCP shim; needs FERB_RUN_TOKEN")
    sp = sub.add_parser("setup", help="install FERB's interpreter, venv and LaunchAgent; permissions; self-test")
    sp.add_argument("--update", action="store_true", help="only reinstall FERB into its venv and restart the agent")
    sp.add_argument("--identity", action="store_true",
                    help="choose FERB's identity profile in your Chrome (it must hold only FERB's account)")
    sp.add_argument("--profile", help="with --identity: the Chrome profile's name, if several hold FERB's account")
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
            return main_chat(lambda: Client(socket_path(), timeout=1800))
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
