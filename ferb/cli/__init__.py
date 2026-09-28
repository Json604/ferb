"""`ferb` command line (section 15). Build step 1: `ferb logs`, `ferb trace`, `ferb daemon run`."""

import argparse
import sys

from ferb.cli import admin, logs
from ferb.cli.client import Client, RpcError
from ferb.core.errors import NotRunning
from ferb.paths import socket_path


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="ferb")
    sub = p.add_subparsers(dest="cmd", required=True)

    lp = sub.add_parser("logs", help="live stream from the daemon's bus")
    for flag in ("tasks", "trail", "tools", "browsers"):
        lp.add_argument(f"--{flag}", action="store_true")

    tp = sub.add_parser("trace", help="span tree for one task")
    tp.add_argument("task_id")

    dp = sub.add_parser("daemon", help="run the daemon")
    dp.add_argument("action", choices=["run"])

    args = p.parse_args(argv)
    if args.cmd == "daemon":
        from ferb.daemon.main import main as daemon_main
        daemon_main()
        return 0

    try:
        client = Client(socket_path())
        try:
            if args.cmd == "logs":
                filters = [f for f in ("tasks", "trail", "tools", "browsers") if getattr(args, f)]
                return logs.run(client, filters, sys.stdout)
            return admin.trace(client, args.task_id, sys.stdout)
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
