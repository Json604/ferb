"""`ferb` command line (section 15). Steps 1-2: logs, trace, daemon run, stop, resume-tasks, undo, autonomy."""

import argparse
import sys

from ferb.cli import admin, logs
from ferb.cli.client import Client, RpcError
from ferb.core.errors import NotRunning
from ferb.paths import socket_path

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
    up = sub.add_parser("undo", help="cancel a pending send/post during its undo window")
    up.add_argument("hold_id", nargs="?")
    ap = sub.add_parser("autonomy", help="show or set the autonomy level")
    ap.add_argument("level", nargs="?", choices=["careful", "balanced", "hands-off"])

    args = p.parse_args(argv)
    if args.cmd == "daemon":
        from ferb.daemon.main import main as daemon_main
        daemon_main()
        return 0

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
            if args.cmd == "undo":
                return admin.undo(client, args.hold_id, sys.stdout)
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
