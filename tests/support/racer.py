"""Swaps a path between a plain file and a symlink to a closed file, atomically, until killed: what a `shell.start`
loop can do inside FERB's sandbox (`while :; do ln -sf T n.l; mv -f n.l n; echo x > n.r; mv -f n.r n; done`), since
Seatbelt checks a read, never a link's target (review of L39 fix round 1, R1; the reviewer's probe racer.py)."""
import os
import sys

path, target = sys.argv[1], sys.argv[2]
link, plain = path + ".l", path + ".r"
while True:
    try:
        os.symlink(target, link)
    except FileExistsError:
        pass
    os.replace(link, path)
    with open(plain, "w") as f:
        f.write("plain\n")
    os.replace(plain, path)
