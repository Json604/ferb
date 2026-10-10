"""P22 watch an existing FERB PTY, with an explicit exclusive owner input mode."""
import os
import select
import sys
import termios
import threading
import tty
import codecs
from typing import Callable
from ferb.cli.client import Client
from ferb.sanitize.terminal import printable


def attach(connect: Callable[[], Client], process: str, take: bool = False) -> int:
    if take and not sys.stdin.isatty():
        print('Input takeover needs a terminal.', file=sys.stderr)
        return 2
    watching = connect()
    if not take:
        print('Watching. Ctrl-C detaches; use --take to take over input.')
        try:
            for event in watching.stream('ps.attach', {'id': process}):
                print(printable(event.get('text', '')), end='', flush=True)
        except KeyboardInterrupt:
            pass
        finally:
            watching.close()
        return 0
    print('You own input while attached. Ctrl-] detaches and returns input to FERB.')
    finished = threading.Event()
    ready = threading.Event()
    lease = ['']
    def output() -> None:
        try:
            for event in watching.stream('ps.attach', {'id': process, 'take': True}):
                if 'lease' in event:
                    lease[0] = event['lease']
                    ready.set()
                print(printable(event.get('text', '')), end='', flush=True)
        except Exception as exc:
            if not finished.is_set():
                print(printable(str(exc)), file=sys.stderr)
        finally:
            finished.set()
            ready.set()
    thread = threading.Thread(target=output, daemon=True)
    thread.start()
    ready.wait()
    if not lease[0]:
        watching.close()
        return 2
    sending = None
    fd = sys.stdin.fileno()
    saved = None
    decoder = codecs.getincrementaldecoder('utf-8')(errors='replace')
    try:
        sending = connect()
        saved = termios.tcgetattr(fd)
        tty.setraw(fd)
        while not finished.is_set():
            if not select.select([fd], [], [], 0.1)[0]:
                continue
            data = os.read(fd, 4096)
            if not data or b'\x1d' in data:
                break
            text = decoder.decode(data)
            if text:
                sending.call('ps.send', {'id': process, 'lease': lease[0], 'text': text})
    finally:
        if saved is not None:
            termios.tcsetattr(fd, termios.TCSADRAIN, saved)
        finished.set()
        if sending is not None:
            sending.close()
        # Interrupt the socket reader before closing its buffered file from this thread.
        watching.interrupt()
        thread.join()
        watching.close()
        print()
    return 0
