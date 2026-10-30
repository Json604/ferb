"""Staging one upload source the way the driver does (`Uploads.stage_all` on a handle opened without following a link
at its last step), for tests about the staging itself."""

import os

from ferb.act.browser.uploads import StagingFailed, Uploads

OPEN_FILE = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC


def stage(uploads: Uploads, tree: str, source: str, sha256: str, tab: str | None = None) -> str:
    try:
        fd = os.open(source, OPEN_FILE)
    except OSError as e:
        raise StagingFailed(f"FERB can't read {source} ({e.strerror or e})") from None
    try:
        return uploads.stage_all(tree, [(fd, source, sha256)], tab)[0]
    finally:
        os.close(fd)
