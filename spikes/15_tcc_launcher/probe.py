"""Spike 15 probe: permission status as seen by this process (ctypes only, so any interpreter works)."""
import ctypes, json, os, sys
AS = ctypes.CDLL("/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices")
AS.AXIsProcessTrusted.restype = ctypes.c_bool
CG = ctypes.CDLL("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics")
CG.CGPreflightScreenCaptureAccess.restype = ctypes.c_bool
out = {"accessibility": bool(AS.AXIsProcessTrusted()), "screen_recording": bool(CG.CGPreflightScreenCaptureAccess()),
       "interpreter": os.path.realpath(sys.executable), "pid": os.getpid(), "ppid": os.getppid()}
with open(sys.argv[1], "w") as f:
    json.dump(out, f)
