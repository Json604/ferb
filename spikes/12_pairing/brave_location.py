"""Re-measure which NativeMessagingHosts folder Brave (default data folder) reads, in reverse order to rule out a
retry-timing coincidence: Chrome's folder alone for 150 s, then Brave's folder alone for 150 s. Counts hellos
per location (the extension retries with backoff; each successful connect sends one hello)."""

import time

from brave_check import log, write_manifest
from daemon_stub import Daemon, stop_stale_hosts


def main() -> None:
    d = Daemon()
    try:
        for where in ("chrome", "brave"):
            m = write_manifest(where)
            end, hellos = time.monotonic() + 150, 0
            while time.monotonic() < end:
                try:
                    d.wait(lambda e: e.msg.get("type") == "hello", max(0.5, end - time.monotonic()))
                    hellos += 1
                except TimeoutError:
                    break
            m.unlink(missing_ok=True)
            stop_stale_hosts()                     # drop the live relay so the next location must reconnect
            log(f"MEASURE: {hellos} hello(s) in 150 s with the manifest only in the {where} folder")
    finally:
        d.close()
        stop_stale_hosts()
        log("cleanup: manifest removed, relay stopped")


if __name__ == "__main__":
    main()
