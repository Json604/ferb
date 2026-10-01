"""Spike 20: background WhatsApp search, one method per run. Harmless text only; checks the search bar image and the
app's front state. Usage: python probe.py <method> <word> <out.png>"""
import sys, time
import ApplicationServices as AS
import Quartz
from AppKit import NSBitmapImageRep, NSPNGFileType
sys.path.insert(0, "spikes/18_background_input")
from ax import attr, app_el, pid_of, walk, keys_to_pid
from ferb.act.mac import accessibility as axm, apps

method, word, out = sys.argv[1], sys.argv[2], sys.argv[3]
pid = pid_of("net.whatsapp.WhatsApp"); app = app_el(pid); owner = apps.frontmost().bundle_id

def box():
    w = attr(app, "AXMainWindow") or attr(app, "AXFocusedWindow")
    els = [el for _, el in walk(w)]
    i = next(k for k, el in enumerate(els) if "New Chat" in str(attr(el, "AXDescription") or attr(el, "AXTitle") or ""))
    return next(el for el in els[i+1:] if attr(el, "AXRole") == "AXButton" and not (attr(el, "AXTitle") or attr(el, "AXDescription")))

def ax_keys(text):
    for ch in text:
        AS.AXUIElementPostKeyboardEvent(app, ord(ch), 0, True)   # deprecated but targets the app's focus
        AS.AXUIElementPostKeyboardEvent(app, ord(ch), 0, False)
        time.sleep(0.03)

def shot():
    wid = next(w["kCGWindowNumber"] for w in Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionAll, 0)
               if w["kCGWindowOwnerPID"] == pid and dict(w["kCGWindowBounds"])["Height"] > 400)
    img = Quartz.CGWindowListCreateImage(Quartz.CGRectNull, Quartz.kCGWindowListOptionIncludingWindow, wid,
                                         Quartz.kCGWindowImageBoundsIgnoreFraming)
    w, h = Quartz.CGImageGetWidth(img), Quartz.CGImageGetHeight(img)
    crop = Quartz.CGImageCreateWithImageInRect(img, Quartz.CGRectMake(0, 0, int(w * 0.5), int(h * 0.14)))
    NSBitmapImageRep.alloc().initWithCGImage_(crop).representationUsingType_properties_(NSPNGFileType, None).writeToFile_atomically_(out, True)

fronts = []
if method == "press_then_pid":
    print("press:", AS.AXUIElementPerformAction(box(), "AXPress")); time.sleep(0.6); fronts.append(apps.frontmost().bundle_id)
    keys_to_pid(pid, word)
elif method == "press_then_axkeys":
    print("press:", AS.AXUIElementPerformAction(box(), "AXPress")); time.sleep(0.6); fronts.append(apps.frontmost().bundle_id)
    ax_keys(word)
elif method == "menu_then_axkeys":
    axm.press_menu(pid, ["File", "Search"]); time.sleep(0.6); fronts.append(apps.frontmost().bundle_id)
    ax_keys(word)
time.sleep(1.5); fronts.append(apps.frontmost().bundle_id)
shot()
print("owner stayed in front:", all(f == owner for f in fronts))
