"""Spike 18: does input reach an app in the background? WhatsApp, draft-only: the New Chat search box (messages no one).
(a) Accessibility: press buttons, set a text value; (b) keystrokes posted to the app's process (CGEventPostToPid).
Each step checks the frontmost app never changed and whether the app really reacted (the contact list filters)."""
import json
import sys
import time

from ax import BUNDLE, actions, attr, fields, find, front, keys_to_pid, label, pid_of, press, rows, set_value

results: dict = {}

owner_front = front()
pid = pid_of(BUNDLE)
step = lambda name, **kw: results.__setitem__(name, {**kw, "front_unchanged": front() == owner_front})

# a1: press a harmless button in the background (the "Starred" tab, then back to "Chats")
starred = find(lambda el: attr(el, "AXRole") == "AXButton" and "Starred" in label(el))
r = press(starred[0]) if starred else None
time.sleep(1)
heading = [attr(el, "AXValue") or attr(el, "AXTitle") for el in find(lambda el: attr(el, "AXRole") == "AXHeading")]
step("a1_press_button", ax_error=r, heading_after=heading)
chats = find(lambda el: attr(el, "AXRole") == "AXButton" and label(el).endswith("Chats"))
press(chats[0]); time.sleep(1)

# open New Chat (a search box appears)
before_rows = rows()
new_chat = find(lambda el: "NewChatButton" in label(el))
press(new_chat[0]); time.sleep(1.5)
fs = fields()
step("new_chat_panel", fields=[label(f)[:60] for f in fs], rows_before=before_rows, rows_now=rows())
if not fs:
    print(json.dumps(results, indent=1, default=str)); sys.exit(0)
box = fs[0]
base = rows()

# a2: set the value through Accessibility
err = set_value(box, "zzqx")
time.sleep(1.5)
step("a2_set_value", ax_error=err, value_read_back=attr(box, "AXValue"), rows_before=base, rows_after=rows())
set_value(box, ""); time.sleep(1)

# b: keystrokes to the process while it's in the background (focus the box via AX first)
import ApplicationServices as AS
AS.AXUIElementSetAttributeValue(box, "AXFocused", True)
keys_to_pid(pid, "zzqx")
time.sleep(1.5)
step("b_keys_to_pid", value_read_back=attr(box, "AXValue"), rows_before=base, rows_after=rows(),
     focused=attr(box, "AXFocused"))

# clean up: clear, close the panel, back to Chats
set_value(box, "")
for f in fields():
    if "AXCancel" in actions(f):
        AS.AXUIElementPerformAction(f, "AXCancel")
time.sleep(0.5)
chats = find(lambda el: attr(el, "AXRole") == "AXButton" and label(el).endswith("Chats"))
if chats:
    press(chats[0])
results["front_at_end"] = front() == owner_front
print(json.dumps(results, indent=1, default=str, ensure_ascii=False))
