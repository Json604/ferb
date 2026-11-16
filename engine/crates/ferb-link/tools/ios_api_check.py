#!/usr/bin/env python3
"""Checks an iOS build of the phone library against the iOS SDK (kb PHN-478).

Usage: python3 -I engine/crates/ferb-link/tools/ios_api_check.py LIBRARY.dylib [--min-ios X.Y]

For every symbol the library imports (`nm -um`), the iOS SDK's own headers are asked through clang: one Objective-C
file takes the address of each symbol under the headers of the library it comes from, compiled for the library's
platform (device or simulator) and minimum iOS. clang then says, per symbol:
  - unavailable    the SDK marks it API_UNAVAILABLE(ios) (or prohibited): App Store review may refuse the app;
  - undeclared     no public header of the iOS SDK declares it: a private API, unless it is one of RUNTIME below;
  - needs iOS N    introduced after the minimum: a strong import that would fail to load on an older iOS.
Imports from libc++ (C++ names) are listed, not checked. Exit 1 on any unavailable or undeclared import, or any
import that needs a newer iOS than --min-ios (default: the library's own minimum).

It also lists what the library uses of Apple's "required reason" APIs (a privacy manifest, PrivacyInfo.xcprivacy,
must name a reason for each category; developer.apple.com, "Describing use of required reason API"): imported
functions and Objective-C selectors and classes named in the library.
"""

import argparse
import os
import re
import subprocess
import sys
import tempfile

# Symbols the toolchain itself emits (stack protector, thread-local storage, the dyld stub binder, the unwinder Rust
# panics use, Clang blocks). They come from libSystem and have no API header; none is a call FERB's code chose.
RUNTIME = {
    "__stack_chk_fail", "__stack_chk_guard", "_tlv_atexit", "_tlv_bootstrap", "dyld_stub_binder",
    "__chkstk_darwin", "_NSConcreteStackBlock", "_NSConcreteGlobalBlock", "_Block_copy", "_Block_release",
}
# C code built with _FORTIFY_SOURCE (SQLCipher's, in ferb-mobile-ffi) calls the checked string and print functions
# clang emits for its __builtin___*_chk builtins (secure/_string.h, secure/_stdio.h declare them only as builtins), and a
# CFSTR literal refers to CoreFoundation's constant-string class. Each is exported for iOS: the fortify functions by the
# iOS SDK's usr/lib/libSystem.B.tbd (checked 2026-11-16, iOS 27.2 SDK), the class by CoreFoundation.framework's tbd.
RUNTIME |= {
    "__memccpy_chk", "__memcpy_chk", "__memmove_chk", "__memset_chk", "__snprintf_chk", "__sprintf_chk",
    "__stpcpy_chk", "__stpncpy_chk", "__strcat_chk", "__strcpy_chk", "__strlcat_chk", "__strlcpy_chk",
    "__strncat_chk", "__strncpy_chk", "__vsnprintf_chk", "__vsprintf_chk",
    "__CFConstantStringClassReference",
}
RUNTIME_PREFIXES = ("_Unwind_",)

# The headers that declare libSystem's and the other system libraries' public API on iOS.
SYSTEM_HEADERS = """
stdlib.h stdio.h string.h strings.h unistd.h fcntl.h errno.h signal.h time.h math.h dlfcn.h dirent.h sched.h poll.h
pthread.h netdb.h ifaddrs.h spawn.h copyfile.h crt_externs.h execinfo.h libgen.h locale.h wchar.h xlocale.h
sys/types.h sys/socket.h sys/stat.h sys/mman.h sys/ioctl.h sys/sysctl.h sys/event.h sys/time.h sys/uio.h sys/wait.h
sys/mount.h sys/statvfs.h sys/attr.h sys/random.h sys/resource.h sys/param.h sys/un.h
net/if.h netinet/in.h arpa/inet.h mach/mach.h mach/mach_time.h mach-o/dyld.h malloc/malloc.h os/log.h os/lock.h
dispatch/dispatch.h Block.h libkern/OSAtomic.h CommonCrypto/CommonRandom.h CommonCrypto/CommonDigest.h
objc/runtime.h objc/message.h
""".split()
LIBRARY_HEADERS = {"libresolv": ["resolv.h"], "libobjc": ["objc/runtime.h", "objc/message.h"]}

# Apple's required-reason APIs: C functions, and Foundation selectors or classes.
REASON_FUNCTIONS = {
    "File timestamp (NSPrivacyAccessedAPICategoryFileTimestamp)": [
        "getattrlist", "getattrlistbulk", "fgetattrlist", "stat", "fstat", "fstatat", "lstat", "getattrlistat"],
    "System boot time (NSPrivacyAccessedAPICategorySystemBootTime)": ["mach_absolute_time"],
    "Disk space (NSPrivacyAccessedAPICategoryDiskSpace)": [
        "statfs", "statvfs", "fstatfs", "fstatvfs", "getattrlist", "fgetattrlist", "getattrlistat"],
}
REASON_OBJC = {
    "File timestamp (NSPrivacyAccessedAPICategoryFileTimestamp)": [
        "creationDate", "modificationDate", "fileModificationDate", "NSURLContentModificationDateKey",
        "NSURLCreationDateKey", "NSFileCreationDate", "NSFileModificationDate"],
    "System boot time (NSPrivacyAccessedAPICategorySystemBootTime)": ["systemUptime"],
    "Disk space (NSPrivacyAccessedAPICategoryDiskSpace)": [
        "NSURLVolumeAvailableCapacityKey", "NSURLVolumeAvailableCapacityForImportantUsageKey",
        "NSURLVolumeAvailableCapacityForOpportunisticUsageKey", "NSURLVolumeTotalCapacityKey",
        "NSFileSystemFreeSize", "NSFileSystemSize"],
    "Active keyboards (NSPrivacyAccessedAPICategoryActiveKeyboards)": ["activeInputModes"],
    "User defaults (NSPrivacyAccessedAPICategoryUserDefaults)": ["NSUserDefaults", "standardUserDefaults"],
}


def run(*cmd):
    return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout


def platform_and_minimum(lib):
    """The library's platform (iphoneos or iphonesimulator) and minimum iOS, from its load commands."""
    text = run("otool", "-l", lib)
    m = re.search(r"cmd LC_BUILD_VERSION\n.*?platform (\d+)\n.*?minos ([\d.]+)", text, re.S)
    if m:
        return ("iphonesimulator" if m.group(1) == "7" else "iphoneos"), m.group(2)
    m = re.search(r"cmd LC_VERSION_MIN_IPHONEOS\n.*?version ([\d.]+)", text, re.S)
    if m:
        return "iphoneos", m.group(1)
    sys.exit(f"{lib}: not an iOS library (no LC_BUILD_VERSION or LC_VERSION_MIN_IPHONEOS)")


def imports(lib):
    """(C name, library) for each undefined symbol."""
    out = []
    for line in run("nm", "-um", lib).splitlines():
        m = re.match(r"\s*\(undefined\) (?:weak )?external (\S+)(?: \(from (\S+)\))?", line)
        if m:
            name = m.group(1)
            name = name[1:] if name.startswith("_") else name
            out.append((name.split("$")[0] if not name.startswith("OBJC_") else name, m.group(2) or "?"))
    return sorted(set(out))


def version_tuple(v):
    return tuple(int(x) for x in v.split("."))


def check(lib, min_ios):
    sdk, lib_min = platform_and_minimum(lib)
    min_ios = min_ios or lib_min
    syms = imports(lib)
    frameworks = sorted({src for _, src in syms if src not in ("libSystem", "libresolv", "libobjc", "libc++", "?")})
    headers = SYSTEM_HEADERS + [h for src in LIBRARY_HEADERS for h in LIBRARY_HEADERS[src]]
    lines = ["#pragma clang diagnostic warning \"-Wunguarded-availability\""]
    for h in headers + [f"{f}/{f}.h" for f in frameworks]:
        lines += [f"#if __has_include(<{h}>)", f"#import <{h}>", "#endif"]
    first = len(lines) + 1
    checked, skipped = [], []
    for name, src in syms:
        if src == "libc++" or name.startswith("_Z"):
            skipped.append((name, src))
            continue
        if name.startswith("OBJC_CLASS_$_") or name.startswith("OBJC_METACLASS_$_"):
            cls = name.split("$_", 1)[1]
            lines.append(f"Class ferb_ref_{len(checked)}(void) {{ return [{cls} class]; }}")
        else:
            lines.append(f"void *ferb_ref_{len(checked)}(void) {{ return (void *)&{name}; }}")
        checked.append((name, src))
    # Compiled for the older of the two minimums, so clang names the version each newer import needs.
    floor = min(lib_min, min_ios, key=version_tuple)
    triple = f"arm64-apple-ios{floor}" + ("-simulator" if sdk == "iphonesimulator" else "")
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "imports.m")
        with open(path, "w") as f:
            f.write("\n".join(lines) + "\n")
        proc = subprocess.run(
            ["xcrun", "--sdk", sdk, "clang", "-x", "objective-c", "-fsyntax-only", "-fmodules", "-ferror-limit=0",
             "-target", triple, "-Wno-incompatible-function-pointer-types", "-Wunguarded-availability", path],
            capture_output=True, text=True)
    found = {}
    stray = [m.group(0) for m in re.finditer(r"imports\.m:(\d+):\d+: (?:fatal )?error: .*", proc.stderr)
             if not 0 <= int(m.group(1)) - first < len(checked)]
    if stray or (proc.returncode != 0 and "error:" not in proc.stderr):
        sys.exit("the check itself did not compile:\n" + ("\n".join(stray) or proc.stderr))
    for m in re.finditer(r"imports\.m:(\d+):\d+: (error|warning): (.*)", proc.stderr):
        i = int(m.group(1)) - first
        if not 0 <= i < len(checked):
            continue
        msg = m.group(3)
        name, src = checked[i]
        if "unavailable" in msg or "prohibited" in msg:
            found.setdefault(name, ("unavailable", src, msg))
        elif "undeclared" in msg or "implicit declaration" in msg or "unknown receiver" in msg:
            if name in RUNTIME or name.startswith(RUNTIME_PREFIXES):
                continue
            found.setdefault(name, ("undeclared", src, msg))
        elif (v := re.search(r"only available on iOS ([\d.]+)", msg)):
            found.setdefault(name, (f"needs iOS {v.group(1)}", src, msg))
    print(f"{lib}\n  {sdk}, minimum iOS {lib_min}, checked against iOS {min_ios}: "
          f"{len(syms)} imports, {len(checked)} checked, {len(skipped)} C++ not checked")
    for src in sorted({s for _, s in syms}):
        print(f"    {sum(1 for _, s in syms if s == src):4} from {src}")
    failed = False
    needs = [kind.split()[-1] for kind, _, _ in found.values() if kind.startswith("needs iOS")]
    for name, (kind, src, msg) in sorted(found.items()):
        if kind.startswith("needs iOS") and version_tuple(kind.split()[-1]) <= version_tuple(min_ios):
            continue
        failed = True
        print(f"  FAIL {kind}: {name} (from {src})")
    if not failed:
        print("  every checked import is declared and available on iOS")
    print(f"  the imports need iOS {max(needs, key=version_tuple) if needs else floor} or newer")

    # Required-reason APIs.
    names = {n for n, _ in syms}
    try:
        methnames = set(run("otool", "-v", "-s", "__TEXT", "__objc_methname", lib).split())
    except subprocess.CalledProcessError:
        methnames = set()
    strings = set(run("strings", "-a", lib).split())
    print("  required-reason APIs (PrivacyInfo.xcprivacy):")
    any_reason = False
    for cat in sorted(set(REASON_FUNCTIONS) | set(REASON_OBJC)):
        used = sorted({f for f in REASON_FUNCTIONS.get(cat, []) if f in names}
                      | {s for s in REASON_OBJC.get(cat, [])
                         if s in methnames or f"OBJC_CLASS_$_{s}" in names or s in strings})
        if used:
            any_reason = True
            print(f"    {cat}: {', '.join(used)}")
    if not any_reason:
        print("    none")
    return failed


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("library")
    ap.add_argument("--min-ios", help="the oldest iOS the app supports (default: the library's own minimum)")
    a = ap.parse_args()
    sys.exit(1 if check(a.library, a.min_ios) else 0)


if __name__ == "__main__":
    main()
