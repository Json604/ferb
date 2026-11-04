# FERB UI playground

A dev-only copy of FERB's notch app that runs the **real** first-run intro (notch, orb, narration, beats, sounds,
copy) with everything behind it faked. You change the intro's look and feel in the real source files, run one command
and see the change. No real FERB is involved: no daemon, no `~/.ferb`, no Keychain, no permission prompts.

## Whole-app review

The startup/setup concept is accepted (APP-618). There is no global Skip intro control. To review the rest of the app:

```sh
ui/mac/FERBNotch/playground.sh --arrival --app
```

**Explore FERB** opens the real notch/chat and app window. The controls reach Tasks, Schedules & Reminders, Noticed,
Memory, all Settings tabs and the circular task previews. Chat reply, Running task, Approval, Question, Finished result,
Reminder arriving, Scheduled result, Failed task and All message cards exercise the real UI through simulated RPCs.
Reminder arriving uses the notification poster with a simulated notification center; Scheduled result publishes a
background task result. Both close the sample chat first so the compact notch arrival can be reviewed, and keep the
reminder card and message in chat. The arrival uses that same rich card and stays visible until hover, click or
opening chat. Planning previews remain for six seconds after their final text.
Typed messages use the last selected sample state; no AI provider is called. Use **Finish running sample tasks** for a
held working state, or **Reset app playground** to restore the samples. Startup & setup remains available in its disclosure.

Sample schedules, reminders, memory and settings respond to their normal controls. App registration, updates, cleanup,
uninstall and diagnostic sending are simulated. Logs and artifacts point into the playground's temporary home. The
preview image is a drawn sample, never a screen capture. Unknown daemon operations are refused rather than claiming success.

The fake workspace is in memory and resets on launch. The notch's conversation uses this playground's own preferences.
`--app --reopen` keeps its conversation and setup state; **Reset app playground** starts fresh.

## The one command

From the repo root:

```sh
ui/mac/FERBNotch/playground.sh            # build, then open the playground fresh (intro from the top)
ui/mac/FERBNotch/playground.sh --reopen   # build, then open it keeping the last run's state
ui/mac/FERBNotch/playground.sh --build-only   # build "build/FERB Playground.app" without opening it
```

The script builds the package (debug), assembles `ui/mac/FERBNotch/build/FERB Playground.app` with its own bundle id
`dev.ferb.playground`, quits a playground that is still open, and opens the new one. Two windows show up: FERB itself
(the notch, then the orb) and a plain **controls** window, which isn't part of FERB.

Because it has its own bundle id, the playground never shares preferences, permission grants or Keychain access with
FERB.app (`com.ferb.FERB`) or the development notch (`dev.ferb.notch`). Its home is
`$(getconf DARWIN_USER_TEMP_DIR)ferb-playground` (or `FERB_HOME` if you set one). Both the script and the app refuse
to start if that path is your real `~/.ferb`, is inside it, or contains it.

## What's real and what's faked

| Real (the product's own code, unchanged) | Faked (in `Sources/FERBNotch/Playground/`) |
|---|---|
| The notch: `PanelCoordinator`, `NotchModel`, `NotchView`, its springs, the menu-bar item | The daemon: `PlaygroundDaemon` answers on the same `DaemonRPCTransport` seam as `DaemonRPC` (no socket) |
| The intro: `AppSetupController`, `IntroController`, every beat, `IntroCopy`, `IntroNarration` | `intro.suggest` (the daemon's two general tasks) and `intro.preview` (a canned plan, ~3.5 s planner wait) |
| The orb: `OrbController`, `OrbPanel`, `OrbView`, `OrbMotion`, its drag card | System Settings: a stand-in window with FERB's switch (the orb docks beside it) |
| Sounds: `FerbSoundEngine` and the bundled sound files; system UI sound settings and alert volume | Call activity (idle unless *Simulate a call* is on), so host speech services cannot silence the audition; macOS permission popups: answered by the panel's "When FERB asks" choice |
| `SetupModel` (the grant, confirm and resume logic) and `SetupView` (the compact window after a skip) | `ferb setup --service` / `--codex-login`, Claude sign-in, API keys (never stored), Login Items, launchd restart |
| The launch decision (`AppSetupPlan`), resume after a quit (`IntroProgress`) | Browsers, pairing, FERB's email, the Mac scan, the microphone (dictation "hears" a fixed sentence) |
| The real keyboard controls (double-tap right ⌥, hold right ⌘⌥) | The Try task and the stop practice: a simulated task whose steps tick by (Stop ends it); nothing real runs |

Not in the playground: a live Sparkle updater, registered login items or background services, or real macOS notification banners. The app window and Settings use simulated services in whole-app review. The playground code is compiled into debug builds only (`FERB_PLAYGROUND` in
`Package.swift`); FERB.app is built release by `scripts/build-app.sh` and contains none of it.

Things it can't fake, and why:

- **The real System Settings and macOS's own permission popups.** The playground's bundle isn't in any privacy list
  and must never ask macOS for anything, so popups are invisible and answered by the panel; list panes use the
  stand-in window. Visuals of macOS's own dialogs aren't FERB's to change anyway.
- **"No hardware notch" on a Mac that has one.** The notch geometry, the orb's home and the sound placement read
  `NSScreen.safeAreaInsets` directly; there is no seam to pretend. Use the controls' Display picker with an external
  display to see the no-notch layout.
- **The notch's own springs at another speed.** The speed control scales the tour's waits, the orb's clock and the
  fake daemon, not `NotchPresentation`'s animations.

## The controls window

- **Intro**: the current beat and line; *Restart from the top* (a fresh first run, with the first-launch clip);
  *Quit & reopen FERB* (quits and relaunches keeping the state: the resume path); *Jump to* any beat (quits
  and reopens at that beat, optionally marking the steps before it done).
- **Permissions**: each of the seven as Not asked / On / Off; *When FERB asks, the owner* allows after a moment, says
  no, or waits for you (questions then appear in the panel with Allow / Don't Allow, and list panes wait for the
  stand-in's switch); *apply only after a restart* (Screen Recording, Input Monitoring, Full Disk Access, as on real
  macOS: the stand-in then offers macOS's "Quit & Reopen", which relaunches the playground); *Restart background
  service* (applies those switches without quitting).
- **Background service, sign-in, browsers**: install fails; Login Items needs your OK (and allowed); sign-in fails;
  sign out; pairing fails.
- **Try**: suggestions and the plan preview each Normal / Slow / Fails; whether the Mac scan "saw" something.
- **Controls**: buttons for each control the Control beat teaches, open/close the notch (Stop is on the tour's own line, on the simulated task).
- **Look**: sound on/off; *Simulate a call* to check the real call-quieting policy (off on launch, independent of the host microphone); speed 0.25×–4×; where the notch sits (in the notch, or an edge) and on which display.
- **Log**: what the fake daemon was asked and what the fakes did.

## Where to change what

All paths are under `ui/mac/FERBNotch/Sources/`. Edit these; the playground picks them up on the next
`playground.sh`.

| To change | Edit |
|---|---|
| Where the notch sits by default, docking into the camera housing | `FERBNotch/PanelCoordinator.swift` (`updateScreen`; a first run docks in the notch, or sits at the top centre without one: `NotchDocking.isDocked`, `edge`, APP-597), `FERBNotch/NotchDocking.swift`, `NotchCore/NotchGeometry.swift` |
| Dragging the notch / edge handle | `FERBNotch/PanelCoordinator.swift` (`configureIndicator`, `moveIndicator`, `snapIndicator`), `FERBNotch/FerbIndicator.swift`, `FERBNotch/IndicatorPlacement.swift`, `FERBNotch/MagneticDocking.swift`, `FERBNotch/LiveDragLayout.swift` |
| The notch's look and its open/close springs | `FERBNotch/NotchView.swift`, `FERBNotch/NotchBackdrop.swift`, `FERBNotch/NotchPresentation.swift`, `FERBNotch/NotchTracer.swift` (glow) |
| How an intro line shows in the notch (text, buttons, typing field, the swell) | `FERBNotch/IntroNarration.swift` (`IntroNarrationView`, `pulseIntro`), `FERBNotch/NotchView.swift` |
| The orb's look, its bubble and drag card | `FERBNotch/OrbView.swift`, `FERBNotch/OrbPanel.swift` |
| The orb's motion (springs, fades, hop, flight length, sound timing) | `NotchCore/OrbMotion.swift`, `FERBNotch/OrbController.swift`; where it docks beside Settings: `NotchCore/SettingsAnchor.swift`; where it points at a browser while pairing: `FERBNotch/IntroAppTargets.swift` |
| What FERB says (every line and button label) | `FERBNotch/IntroCopy.swift` |
| Beat order and flow | `NotchCore/IntroPlan.swift` (`IntroBeat` order), `FERBNotch/IntroController.swift` (`perform`, timings in `Timing`), `FERBNotch/IntroSetupBeats.swift` (sign-in, Allow all, browsers, email), `FERBNotch/IntroDemoBeats.swift` (Try, Control); launch and resume decisions: `NotchCore/AppSetup.swift` |
| The permission step | `FERBNotch/IntroSetupBeats.swift` (`permissions`, `listRow`, `promptRow`), its lines in `IntroCopy.swift`, the grant/confirm service in `FERBNotch/SetupModel.swift` (`grant`, `grantAndConfirm`, `allowAllSequence`), the compact window after a skip in `FERBNotch/SetupView.swift` |
| Sounds | `FERBNotch/FerbSoundEngine.swift` (playback, placement, room), `FERBNotch/FerbSoundFiles.swift` and `FERBNotch/Resources/Sounds/` (the files), `NotchCore/SoundPolicy.swift`, `FERBNotch/SoundFeedback.swift` (everyday sounds) |
| The demo's preview rows in the chat | `FERBNotch/ConversationTranscript.swift` (`turn.preview`), `FERBNotch/TurnCardView.swift` (`preview` / `previewRunning` rows), `NotchCore/ConversationArchive.swift` (`beginPreview`, `applyPreview`, `stopPreview`); what the demo does around them: `FERBNotch/IntroDemoBeats.swift` |

To change what the fake world *does* (a new canned plan, a slower sign-in, another failure mode), edit
`FERBNotch/Playground/` (`PlaygroundDaemon.swift` for daemon replies, `PlaygroundState.swift` for knobs,
`PlaygroundPanel.swift` for the controls).

## Starting to-do list: the owner's feedback (2026-11-01, verbatim)

> "the notch was on the side, not movable, the orb was not pretty enough, the flow is good but the permission
> accepting, reopen and quit is broken, no good feedback in the demo etc etc so many wrong things."

- [x] The notch was on the side (fixed: a first run docks in the notch, APP-597).
- [x] Not movable (fixed: with a tour line showing, a drag moved a hidden handle window, APP-597).
- [ ] The orb isn't pretty enough.
- [x] Permission accepting is broken (fixed: leaving System Settings restarts the service to read a switch that needs it; closing the stand-in counts as leaving, APP-598).
- [x] Reopen and quit is broken (fixed: a resumed tour waits for, and starts, the background service before any beat, APP-515, APP-598; the controls' *Quit & reopen FERB* and the stand-in's *Quit & Reopen* exercise it).
- [ ] No good feedback in the demo (Try: the preview rows, the orb at the app, the lines while it thinks).

## Rules for the UI agent

1. Edit the real source files listed above. The playground runs the product's own code, so there is nothing to wire
   up; don't copy UI into `Playground/` and don't add playground-only branches to product code.
2. The product must behave exactly as before outside the playground. The only product seams are
   `AppSetupController.introSleeper` (nil in FERB.app) and the playground's entry in `Application.swift`
   (`#if FERB_PLAYGROUND`, debug builds only, and only under the bundle id `dev.ferb.playground`).
3. Never point the playground at real FERB: no `~/.ferb`, no Keychain, no real permission requests, no `tccutil` or
   `launchctl`. Don't install, restart or stop FERB.app, its daemon or the owner's browsers.
4. Tests: `source scripts/swiftpm-anonymous.sh` first, then from the repo root
   `swift test --package-path ui/mac/FERBNotch --scratch-path /tmp/ferbnotch-tests` (a scratch path outside `.build`
   keeps test builds from fighting the playground's build). Don't test exact copy or wording.
5. The owner judges the look live in the playground; a passing test never proves the intro looks right.
