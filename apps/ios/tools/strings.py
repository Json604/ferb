#!/usr/bin/env python3
"""Writes the iPhone app's one string catalog from the Android app's strings (kb PHN-567).

The owner approves the app's words once for both phones: every string of apps/android/app/src/main/res/values/
strings.xml is taken VERBATIM, under the same name, except the few below whose platform words differ (Android's
fingerprint, screen lock, settings and Stay connected), the ones iOS has no place for, and the few only iOS needs.
Where Android says "your fingerprint", iOS says the phone's own kind through %@ (the app fills in "Face ID",
"Touch ID" or "your passcode"). Android's %1$s and %1$d become %1$@ and %1$lld.

Output: apps/ios/Packages/FerbApp/Sources/FerbAppModel/Resources/Localizable.xcstrings. `--check` fails when the
committed catalog is not what this writes (run it after the Android strings change); `--diff` prints every string that
differs from Android, for the owner.

Usage: python3 -I apps/ios/tools/strings.py [--check | --diff]
"""

import json
import os
import re
import sys
import xml.etree.ElementTree as ET

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
ANDROID = os.path.join(ROOT, "apps/android/app/src/main/res/values/strings.xml")
OUT = os.path.join(ROOT, "apps/ios/Packages/FerbApp/Sources/FerbAppModel/Resources/Localizable.xcstrings")

# The same message in iOS's words: (name, words). %@ in a fingerprint line is the phone's own kind.
IOS = {
    # PHN-8 on iOS: a passcode, Settings
    "no_lock_title": "Set a passcode first",
    "no_lock_text": "This app controls your Mac, so it opens only with Face ID, Touch ID or your passcode. This iPhone has no passcode. Set one in Settings, then come back.",
    # the system prompt shows one line of the app's (its reason); Face ID's own sheet shows the app's name
    "prompt_keys_subtitle": "Your Mac can then ask for %@ before FERB goes ahead",
    "pair_reason_keys": "This phone's passcode or %@ changed, so it needs pairing again.",
    "answer_needs_pairing": "This answer needs %@, and %@ on this phone changed since it was paired. Answer on your Mac, or pair this phone again.",
    "needs_pairing_again": "This needs %@, and %@ on this phone changed since it was paired. Pair this phone again to do it here.",
    "files_key_place": "Asks for %@ each time",
    "files_not_opened": "Not opened: %@ was cancelled.",
    "settings_note": "Each change asks for %@. The lists of apps and sites are changed in FERB on your Mac.",
    "set_days_text": "How many days FERB keeps these. Saving asks for %@.",
    "keys_row": "Use %@ for checks",
    "keys_row_detail": "Your Mac doesn't know %@ on this phone yet",
    "keys_registered": "Done. Your Mac can now ask for %@.",
    "question_yes_fingerprint": "Yes asks for %@.",
    "n_needs_fingerprint": "Needs %@. Tap to answer it in FERB.",
    "n_needs_fingerprint_title": "This one needs %@",
    # iOS can't blank screenshots (no FLAG_SECURE): the switch hides FERB while the screen is recorded or mirrored;
    # the app switcher is always covered (kb PHN-565; the owner's question)
    "profile_screenshots": "Hide in screen recordings",
    "profile_screenshots_detail": "Blank while the screen is recorded or mirrored",
    # no Stay connected on iPhone (kb PHN-566; the owner's question): notifications while FERB runs
    "notify_text": "FERB tells you on this phone when it needs an answer or finishes something, while it is open and for a short while after you leave it. iOS asks whether FERB may show notifications.",
    "notify_denied": "Notifications are off for FERB, so nothing can tell you. You can turn them on in Settings.",
    # SENSE-420 / GATE-613 on iOS: Apple's location service, iOS's own question
    "city_text": "FERB on your Mac then knows which city you're in, for things like local times and places nearby. Only the city goes to your Mac, never the exact spot. To name the city, Apple's own location service looks it up. iOS asks next; you can turn this off here any time.",
    "city_denied": "iOS didn't allow location for FERB, so FERB goes by the time zone.",
    # the first run's asks (kb PHN-636): iOS asks for the microphone and for speech recognition together
    "first_run_microphone": "Microphone and speech",
    "loc_why_denied": "Location wasn't allowed for FERB on the phone.",
    # speaking to FERB (kb PHN-604): Apple's speech recognition where the iPhone can't recognise the language itself;
    # iOS asks for the microphone and for speech recognition, two questions
    "voice_text_service": "Hold the microphone while you speak, and let go when you're done. Your words appear in the message box, so you can check them before you send. This iPhone turns speech into words with Apple's speech recognition, which may send the sound of your voice to Apple to do it. FERB records and keeps nothing, and your voice never goes to your Mac.",
    "voice_ask_next": "iOS asks next whether FERB may use the microphone and speech recognition.",
    "voice_denied": "The microphone or speech recognition is off for FERB, so it can't hear you. You can turn them on in Settings.",
    "profile_lock_why": "FERB always asks for Face ID, Touch ID or your passcode when it opens, so nobody else with your iPhone can reach your Mac. This can't be turned off.",
    "mac_needs_cancelled": "The unlock prompt was closed, so your Mac's chat is still locked.",
    "mac_needs_old": "Your Mac took a while to come online, so FERB needs to unlock once more.",
    "voice_language": "This iPhone has no speech for your language on the phone itself yet. Add it in Settings under General, Keyboard, Dictation Languages, then try again.",
    # kb PHN-765 on iOS: notifications off is the one thing he changes (no channels, Stay connected or battery setting)
    "cant_notify_off": "Notifications are off for FERB on this iPhone, so its questions can't reach you outside the app.",
    "cant_notify_open_settings": "Open Settings",
}

# Plurals (Android <plurals>): the same, with %2$@ where Android says "your fingerprint" (L.plural fills it).
IOS_PLURALS = {
    "recent_skipped": {"one": "%1$lld more is left out: it needs %2$@, or is no longer on your Mac.",
                       "other": "%1$lld more are left out: they need %2$@, or are no longer on your Mac."},
    "search_skipped": {"one": "%1$lld place that needs %2$@ wasn't searched.",
                       "other": "%1$lld places that need %2$@ weren't searched."},
    "files_tree_skipped": {"one": "%1$lld file that needs %2$@ was left out; open it on its own.",
                           "other": "%1$lld files that need %2$@ were left out; open them one by one."},
}

# Android-only: no place on iOS (the system draws its own prompt titles and Cancel; there are no notification
# channels, no Stay connected service, no Android settings).
DROP = {
    "prompt_unlock_title", "prompt_yes_title", "prompt_keys_title", "prompt_cancel",
    "ch_questions", "ch_questions_desc", "ch_checks", "ch_checks_desc", "ch_done", "ch_done_desc", "ch_link",
    "ch_link_desc", "ch_stay", "ch_stay_desc", "stay_title", "stay_text",
    "cant_notify_channel", "cant_notify_stay", "cant_notify_battery",
}

# Only iOS: what replaces Stay connected, the phone's own kind of lock, Info.plist-free words the app needs.
NEW = {
    "bio_face": "Face ID",
    "bio_touch": "Touch ID",
    "bio_optic": "Optic ID",
    "bio_passcode": "your passcode",
    "background_title": "While FERB is closed",
    "background_detail": "Questions show when you open FERB",
    "background_text": "On iPhone, FERB's questions and news reach you while FERB is open and for a short while after you leave it. Once iOS closes it, they wait on your Mac and show the moment you open FERB again. Your Mac never stops for this: what it is doing goes on.",
    "scan_no_camera": "This iPhone has no camera FERB can use here. Type the code instead.",
    "recording_cover": "FERB is hidden while the screen is recorded or mirrored.",
    # This phone's screenshot row says whether this iOS keeps Watch live's picture out of screenshots (PHN-597)
    "profile_screenshots_detail_shield": "Blank while the screen is recorded or mirrored. Watch live's picture of your Mac stays out of screenshots.",
    "profile_screenshots_detail_no_shield": "Blank while the screen is recorded or mirrored. Screenshots can still show FERB on this iPhone.",
    # the lab's Mac page and This phone rows name the fingerprint (Android) or Face ID (the lab's iPhone): the phone's own
    "mac_settings_detail": "Each change asks for %@",
    "profile_lock": "%@ to open",
    "pick_photos": "Photos",
    "pick_files": "Files",
}

def unescape(text):
    return text.replace("\\'", "'").replace('\\"', '"')


def android_strings():
    tree = ET.parse(ANDROID)
    out = {}
    for el in tree.getroot().findall("string"):
        out[el.get("name")] = unescape("".join(el.itertext()))
    return out


def android_plurals():
    tree = ET.parse(ANDROID)
    out = {}
    for el in tree.getroot().findall("plurals"):
        out[el.get("name")] = {i.get("quantity"): unescape("".join(i.itertext())) for i in el.findall("item")}
    return out


def swift_format(s):
    s = re.sub(r"%(\d+)\$s", r"%\1$@", s)
    s = re.sub(r"%(\d+)\$d", r"%\1$lld", s)
    return s


def catalog():
    android = android_strings()
    missing = sorted(set(IOS) - set(android))
    if missing:
        sys.exit(f"strings.py: IOS names strings Android no longer has: {missing}")
    words = {}
    for name, text in android.items():
        if name in DROP:
            continue
        words[name] = swift_format(IOS.get(name, text))
    clash = sorted(set(NEW) & set(words))
    if clash:
        sys.exit(f"strings.py: NEW names strings Android has: {clash}")
    words.update(NEW)
    plurals = {}
    for name, forms in android_plurals().items():
        if name in words:
            sys.exit(f"strings.py: {name} is both a string and a plural")
        plurals[name] = {q: swift_format(t) for q, t in IOS_PLURALS.get(name, forms).items()}
    gone = sorted(set(IOS_PLURALS) - set(plurals))
    if gone:
        sys.exit(f"strings.py: IOS_PLURALS names plurals Android no longer has: {gone}")
    strings = {}
    for name in sorted(words):
        strings[name] = {"extractionState": "manual",
                         "localizations": {"en": {"stringUnit": {"state": "translated", "value": words[name]}}}}
    for name in sorted(plurals):
        strings[name] = {"extractionState": "manual", "localizations": {"en": {"variations": {"plural": {
            q: {"stringUnit": {"state": "translated", "value": t}} for q, t in sorted(plurals[name].items())}}}}}
    strings = dict(sorted(strings.items()))
    return json.dumps({"sourceLanguage": "en", "strings": strings, "version": "1.0"}, indent=2, ensure_ascii=False) + "\n"


def diff():
    android = android_strings()
    print("Changed from Android (same name, iOS words):")
    for name in sorted(IOS):
        print(f"  {name}\n    Android: {android[name]}\n    iOS:     {IOS[name]}")
    plurals = android_plurals()
    for name in sorted(IOS_PLURALS):
        for q in sorted(IOS_PLURALS[name]):
            print(f"  {name} ({q})\n    Android: {plurals[name][q]}\n    iOS:     {IOS_PLURALS[name][q]}")
    print("Android only (no place on iOS):")
    for name in sorted(DROP):
        print(f"  {name}: {android.get(name, '')}")
    print("iOS only:")
    for name in sorted(NEW):
        print(f"  {name}: {NEW[name]}")


def main():
    args = sys.argv[1:]
    if "--diff" in args:
        diff()
        return
    text = catalog()
    old = open(OUT, encoding="utf-8").read() if os.path.exists(OUT) else None
    if "--check" in args:
        if old != text:
            sys.exit("Localizable.xcstrings differs from apps/ios/tools/strings.py's output: run it and commit both")
        return
    if old != text:
        os.makedirs(os.path.dirname(OUT), exist_ok=True)
        with open(OUT, "w", encoding="utf-8") as f:
            f.write(text)
        print(f"wrote {os.path.relpath(OUT, ROOT)}")


if __name__ == "__main__":
    main()
