"""Own the check that a socket peer is FERB.app (kb area app: the live preview is FERB.app's alone, not
every program running as the owner). Must not grant anything else: auth.py's APP_ONLY names what needs it.

The kernel's audit token of the connected process (LOCAL_PEERTOKEN: its pid and pid version, so a reused pid can't stand
in) is handed to the Security framework, which checks the running code's signature against a requirement: FERB.app's
own designated requirement (its exact code hash for an ad-hoc build, its release certificate or Developer ID team
otherwise), so only that app, signed as it is, passes. FERB.app is the bundle the daemon runs from, or, for a daemon run
from FERB's own venv, the installed FERB.app (`AppPeers`). Only a daemon run from a source checkout (the developer
toolbox) falls back to the notch's identifiers, which an owner-run program could also claim. Anything that can't be
checked fails closed."""

import ctypes
import plistlib
import socket
import sys
from pathlib import Path
from typing import Any

from ferb.paths import app_bundle, ferb_home, installed_app

SOL_LOCAL, LOCAL_PEERTOKEN = 0, 0x006            # <sys/un.h>: the peer's audit_token_t (32 bytes)
AUDIT_TOKEN_SIZE = 32
DEV_REQUIREMENT = 'identifier "com.ferb.FERB" or identifier "dev.ferb.notch"'
_UTF8 = 0x08000100                               # kCFStringEncodingUTF8


def peer_token(sock: socket.socket) -> bytes | None:
    """The connected process's audit token, or None when the socket can't say."""
    try:
        token = sock.getsockopt(SOL_LOCAL, LOCAL_PEERTOKEN, AUDIT_TOKEN_SIZE)
    except (OSError, AttributeError, TypeError):
        return None
    return token if isinstance(token, bytes) and len(token) == AUDIT_TOKEN_SIZE else None


class _Security:
    """The few Security and CoreFoundation calls the check needs, through ctypes."""

    def __init__(self) -> None:
        vp = ctypes.c_void_p
        self.sec = sec = ctypes.CDLL("/System/Library/Frameworks/Security.framework/Security")
        self.cf = cf = ctypes.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
        cf.CFDataCreate.restype, cf.CFDataCreate.argtypes = vp, [vp, ctypes.c_char_p, ctypes.c_long]
        cf.CFDictionaryCreate.restype = vp
        cf.CFDictionaryCreate.argtypes = [vp, ctypes.POINTER(vp), ctypes.POINTER(vp), ctypes.c_long, vp, vp]
        cf.CFStringCreateWithCString.restype = vp
        cf.CFStringCreateWithCString.argtypes = [vp, ctypes.c_char_p, ctypes.c_uint32]
        cf.CFURLCreateFromFileSystemRepresentation.restype = vp
        cf.CFURLCreateFromFileSystemRepresentation.argtypes = [vp, ctypes.c_char_p, ctypes.c_long, ctypes.c_bool]
        cf.CFRelease.argtypes = [vp]
        for name, args in (("SecCodeCopyGuestWithAttributes", [vp, vp, ctypes.c_uint32, ctypes.POINTER(vp)]),
                           ("SecRequirementCreateWithString", [vp, ctypes.c_uint32, ctypes.POINTER(vp)]),
                           ("SecCodeCheckValidity", [vp, ctypes.c_uint32, vp]),
                           ("SecStaticCodeCreateWithPath", [vp, ctypes.c_uint32, ctypes.POINTER(vp)]),
                           ("SecCodeCopyDesignatedRequirement", [vp, ctypes.c_uint32, ctypes.POINTER(vp)])):
            fn = getattr(sec, name)
            fn.restype, fn.argtypes = ctypes.c_int32, args
        self.audit_key = vp.in_dll(sec, "kSecGuestAttributeAudit")
        self.key_callbacks = ctypes.c_void_p.in_dll(cf, "kCFTypeDictionaryKeyCallBacks")
        self.value_callbacks = ctypes.c_void_p.in_dll(cf, "kCFTypeDictionaryValueCallBacks")

    def release(self, *refs: Any) -> None:
        for ref in refs:
            if ref:
                self.cf.CFRelease(ref)

    def requirement(self, text: str) -> int | None:
        string = self.cf.CFStringCreateWithCString(None, text.encode(), _UTF8)
        req = ctypes.c_void_p()
        try:
            return req.value if self.sec.SecRequirementCreateWithString(string, 0, ctypes.byref(req)) == 0 else None
        finally:
            self.release(string)

    def designated(self, bundle: Path) -> int | None:
        """The designated requirement of the app at `bundle`, as its signature states it."""
        raw = str(bundle).encode()
        url = self.cf.CFURLCreateFromFileSystemRepresentation(None, raw, len(raw), True)
        static, req = ctypes.c_void_p(), ctypes.c_void_p()
        try:
            if self.sec.SecStaticCodeCreateWithPath(url, 0, ctypes.byref(static)) != 0:
                return None
            if self.sec.SecCodeCopyDesignatedRequirement(static, 0, ctypes.byref(req)) != 0:
                return None
            return req.value
        finally:
            self.release(url, static.value)

    def satisfies(self, token: bytes, requirement: int) -> bool:
        """The running process with this audit token is validly signed and satisfies the requirement."""
        data = self.cf.CFDataCreate(None, token, len(token))
        keys, values = (ctypes.c_void_p * 1)(self.audit_key.value), (ctypes.c_void_p * 1)(data)
        attributes = self.cf.CFDictionaryCreate(None, keys, values, 1, ctypes.addressof(self.key_callbacks),
                                                ctypes.addressof(self.value_callbacks))
        code = ctypes.c_void_p()
        try:
            if self.sec.SecCodeCopyGuestWithAttributes(None, attributes, 0, ctypes.byref(code)) != 0:
                return False
            return bool(self.sec.SecCodeCheckValidity(code, 0, requirement) == 0)
        finally:
            self.release(code.value, attributes, data)


class AppPeers:
    """`AppPeers()(token)`: whether the process behind this audit token is FERB.app, by its code signature against
    FERB.app's own designated requirement. Which FERB.app (`mode`):
    - "bundled": the daemon runs from inside FERB.app: that bundle;
    - "installed": the daemon runs from FERB's own venv (~/.ferb/venv, how `ferb setup --update` installs it): the
      installed FERB.app (setup records no location: /Applications/FERB.app, then ~/Applications/FERB.app, with FERB's
      bundle id); none found: nobody passes;
    - "checkout": the daemon runs from a source checkout's `.venv` (the developer toolbox): the notch's identifiers.
    The requirement is derived again whenever the bundle's signature changes (its CodeResources or executable), so an
    app update never locks the notch out until a daemon restart. A token's verdict is kept until then (a token names
    one process)."""

    def __init__(self, bundle: Path | None = None, *, prefix: Path | None = None,
                 candidates: tuple[Path, ...] | None = None, security: Any = None) -> None:
        here = Path(prefix if prefix is not None else sys.prefix).resolve()
        self.bundle = bundle if bundle is not None else app_bundle(here)
        if self.bundle is not None:
            self.mode = "bundled"
        elif _checkout(here):
            self.mode = "checkout"
        else:
            self.mode = "installed"
        self.candidates = candidates if candidates is not None else (
            Path("/Applications/FERB.app"), Path.home() / "Applications" / "FERB.app")
        self._make_security = security or _Security
        self._security: Any = None
        self._requirement: int | None = None
        self._stamp: tuple[Any, ...] | None = ()      # the signature the requirement was derived from (() = never)
        self._seen: dict[bytes, bool] = {}

    def _app(self) -> Path | None:
        """The FERB.app whose signature counts (None: checkout mode, or no installed FERB.app)."""
        if self.mode == "bundled":
            return self.bundle
        if self.mode == "installed":
            return installed_app(self.candidates)
        return None

    def _required(self) -> tuple[Any, int] | None:
        if self._security is None:
            self._security = self._make_security()
        app = self._app()
        stamp = _signature_stamp(app) if app is not None else None
        if stamp != self._stamp:                  # first use, or FERB.app was updated, moved or removed
            self._stamp, self._seen = stamp, {}
            if self.mode == "checkout":
                self._requirement = self._security.requirement(DEV_REQUIREMENT)
            else:
                self._requirement = self._security.designated(app) if app is not None else None
        return (self._security, self._requirement) if self._requirement is not None else None

    def __call__(self, token: bytes | None) -> bool:
        if not token:
            return False
        try:
            required = self._required()
            if token not in self._seen:
                self._seen[token] = bool(required and required[0].satisfies(token, required[1]))
        except Exception:                        # anything that can't be checked fails closed
            return False
        return self._seen[token]


def _checkout(prefix: Path) -> bool:
    """A source checkout's own venv (`<repo>/.venv` beside its pyproject.toml), never FERB's installed one."""
    try:
        if prefix.is_relative_to(ferb_home().resolve()):
            return False
    except OSError:
        return False
    return prefix.name == ".venv" and (prefix.parent / "pyproject.toml").is_file()


def _signature_stamp(app: Path) -> tuple[Any, ...] | None:
    """What changes when the bundle is re-signed or replaced: its CodeResources and main executable (inode, size,
    modification time); None when it can't be read."""
    try:
        with (app / "Contents" / "Info.plist").open("rb") as f:
            executable = str(plistlib.load(f).get("CFBundleExecutable") or "")
        found = []
        for path in (app / "Contents" / "_CodeSignature" / "CodeResources", app / "Contents" / "MacOS" / executable):
            st = path.stat()
            found.append((str(path), st.st_ino, st.st_size, st.st_mtime_ns))
        return tuple(found)
    except (OSError, ValueError, plistlib.InvalidFileException):
        return None
