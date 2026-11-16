# FerbDesign

The glass lab's phone design as a SwiftUI library for FERB's iPhone app (stage 4 of the Rust rewrite, slice I3), and
FerbGallery, which shows all of it with invented data. The iPhone and Android apps look the same (kb PHN-416); on the
phone the lab's `Ink` colours are the authority over `tokens.json`'s warm ramp, which is the Mac's.

## Where it comes from

`~/Desktop/ferb-glass-lab` (read only, never built from here) at commit **23a0fde**
(`23a0fdeabe53a85a124c744b7117c07af58869b5`, recorded in `LabSource.commit` with the SHA-256 of every lab file used).
Android's design module copied the same commit; `LabNumbersTests` fails when the two differ.

| Here | From the lab | How |
|---|---|---|
| `Lab/Theme.swift`, `Lab/AstraSky.swift`, `Lab/Sky.swift` | `Sources/FerbGlass/` | Unchanged (a header added) |
| `Lab/Data.swift` | `Sources/FerbGlass/Data.swift` | Only FileKind and FileIcon (the rest is the Mac's) |
| `Lab/Mobile.swift` | `Sources/FerbGlassLab/Mobile.swift` | Public; scaffolding removed; callbacks added |
| `Lab/MobilePieces.swift` | `MobileScreens.swift`, `Conversation.swift` | The pieces the screens draw inline, taken out as views |
| `Lab/MobileScreens.swift` | `Sources/FerbGlassLab/MobileScreens.swift` | Stateless pages and sheets, data and callbacks as parameters |
| `Lab/Season.swift` | `Sources/FerbGlassLab/Weather.swift` | The season layer only (weather is paused, PHN-412) |
| `Lab/Words.swift`, `Resources/corpus.json` | `Words.swift`, `words/corpus.json` | Words.swift ported as Android's app did (PHN-507); corpus byte for byte |
| `LabSample.swift` | `MobileScreens.swift` | The lab's invented sample data and the twelve screens (`LabScreenView`) |
| `Tokens.swift` | `tokens.json` | Radius, spacing and the Mac's type ramp (the lab's Swift has no constants for them) |

Each copied file says at its top which lab file it came from and what changed; the lab's comments stay.

## What changed, and why

- **The lab's scaffolding is gone**: `OS`, `Device`, `StatusBar`, `LabFonts` and Roboto, `MobileStage`, `LabModel`,
  the Mac's notch, window and book, the Words scene. On the iPhone the system draws the status bar and the face is the
  system's. `os.top` and `os.bottom` are `PhoneInsets` (the lab's iPhone numbers, 58 and 30, by default); the lab's
  iPhone words are used throughout (Face ID).
- **Public and stateless, with Android's parameters** (kb PHN-509): every page, sheet and piece takes its data and
  optional callbacks (`onTab`, `onProfile`, `onRow`, `onSend`, `onDismiss`, the link picture's `name` and `status`, the
  relay sheet's `RelayCopy`, the profile's `more` rows, a note under the link, ...). Their defaults draw the lab's
  sample exactly; a callback only adds touch (a press that sinks to 0.97, Theme.swift's numbers), checked by
  `testCallbacksDoNotChangeTheDrawing`.
- **Names**: the lab's `Tab` is `PhoneTab` (SwiftUI has a `Tab`); the kinds are `MarkKind`, `PillKind`, `LinkKind`
  (Android's names; `Mark.Kind`, `Pill.Kind`, `LinkPicture.Kind` and `Switch` remain as aliases); the lab's `Switch` is
  `PhoneSwitch`; Android's `FingerprintNote` is `BiometricNote`.
- **Lists scroll when they don't fit** (`FitOrScroll`): the chat, tasks and files pages, and the settings sheet. A list
  that fits is drawn as the lab draws it, with no scroll view.
- **Still motion**: the running mark, the typing dots and the season layer read one clock. `stillMotion()` (and Reduce
  Motion) draws them at the lab's still moment, t = 1000 s, so pictures are repeatable.
- **The relay sheet's words**: the lab's rows said files and pictures go as "Small ones" and Watch live is "Off", its
  old rule. The owner's rule is that the shared relay carries text, questions and Stop only (PHN-410, PHN-444), so the
  default `LabSample.relay` says "Not on the shared relay" for both and its explanation says why, exactly as Android's
  sample does. The lab's rows are kept as `LabSample.labRelayRows`.
- **Two fixes against the lab** (each found in a render): the question sheet's No and Yes both stretch with Yes given
  the layout priority, so No is squeezed to nothing; No now takes its words' width and Yes the rest (as Android draws
  it). The pairing code's digits had no colour of their own (the lab's dark stage would draw them white on light
  grey); they are black.
- **The home screen's words**: `HomeTexts(Moment)` speaks the corpus with what the phone really measures (the sky, FERB's
  counts, the link, first run, just paired, the open before this one and its zone). Weather, the owner's usual hours and
  late nights are not measured, so their rules never speak (PHN-507, as Android). `HomeTexts.lab(_:)` is the lab's
  stand-in (SkyNow's greeting and sun line), which the screenshots use, as Android's do.
- **The sky's place without a location** (PHN-420, PHN-459): `SkyPlace.fromZone` reads `Resources/zone-places.json`,
  made by `tools/zone_places.py` from the IANA tz database that macOS ships (2026f), the same way as Android's
  `ZonePlaces.kt`; `ZonePlacesTests` checks the two tables entry for entry. A zone with no place uses its standard
  offset's meridian on the equator.

## Not from the lab (`NotFromLab.swift`, and FerbGallery)

Built only from the lab's own pieces and numbers, the same set Android built, until the lab draws them: `QrFrame`
(the scanner's frame), `FolderList` (an open folder on the Files page), `LivePage` (Watch live), `SettingsSheet`
(FERB's settings from the Mac page), `MessagePage` (the unlock page's layout for other messages); `TransferCard`'s
`note` and `actions`; `FilesPage`'s `transfers`, `content` and `recentNote`; `MacPage`'s `note`; the press;
accessibility labels; `LiveSky` (the sky's clock, once a minute) and `SkyMoment`. Their words are parameters.

## Tests

Simulator (the device "FERB iOS design" or any iOS 17+ simulator), from this folder:

```sh
source ../../../../scripts/swiftpm-anonymous.sh
xcodebuild test -scheme FerbDesign-Package -destination 'platform=iOS Simulator,id=<device>' \
  -derivedDataPath ../../build/dd-design CODE_SIGNING_ALLOWED=NO
```

(The `FerbDesign` scheme builds but has no test action; `FerbDesign-Package` has.) The arithmetic, words and tables
also run on the Mac: `swift test --scratch-path <a folder outside the repo>`.

- `LabNumbersTests`: `Ink`, `Tone`, `Lamp`, radii, spacing, type and file badges against Android's
  `apps/android/design/src/test/resources/lab/lab-numbers.json`; the lab commit and file hashes; every colour literal
  of the lab's phone files is in these sources or listed with why not (weather, the phones' frames).
- `SkyMathTests`: sun, sunrise and sunset, moon, the lab's skies, the owner's chosen sky, Astra's schedule, seasons and
  the season layer's numbers against Android's `sky-golden.json`, to 1e-9.
- `ZonePlacesTests`, `WordsTests`.
- `ScreenshotTests`: the twelve screens at dawn, noon and night in Delhi on 2026-11-14 (6.1, 12.5, 22.5 local, Android's
  moments) at 402 by 874 points, one pixel a point, against `Tests/FerbDesignTests/Screenshots/screens/<screen>-<time>.png`
  (Android's names); nine component showcases against `Screenshots/components`. Under 0.1 % of pixels may differ
  (Android's threshold). A failure writes the actual picture and a diff to the simulator's temporary folder and attaches
  them to the result.

**Re-recording goldens** after a deliberate change (look at them first):

```sh
TEST_RUNNER_FERB_RECORD_SNAPSHOTS=1 xcodebuild test -scheme FerbDesign-Package ...   # writes the PNGs
xcodebuild test -scheme FerbDesign-Package ...                                      # must pass unchanged
```

The pictures are made with SwiftUI's `ImageRenderer`: the package's tests have no host app, and without one a window
has no render server (`drawHierarchy` draws nothing). `ImageRenderer` leaves out what UIKit draws (scroll views, text
fields, materials), which is why a list that fits is drawn without a scroll view and the editable composer and glass
panels are not shot. Two recordings in a row are byte-identical.

## iPhone and Android, side by side

What differs between this package's goldens and Android's (`apps/android/design/src/test/screenshots/screens`), and why:

- **The frame**: the iPhone is the lab's 402 by 874 with 58 above and 30 below; Android is 412 by 915 with 44 and 24.
  Everything below the status bar sits 14 points lower on the iPhone, and the sky's sun, moon, stars and the season's
  leaves are placed by fractions of the screen, so they land on different things (the noon sun crosses "It opens with
  Face ID." on the unlock page; the moon shows above the tasks card on the iPhone and behind it on Android).
- **The status bar**: Android's goldens draw the lab's Android status bar, camera hole and gesture bar (previews only);
  the iPhone's draw none (the system draws them).
- **The face**: SF Pro (the system's) against Roboto. SF is wider at the same size, so lines wrap or cut sooner:
  "Summarise this week's newsletters" takes two lines, "The site asked for a code that never..." and "Questions arrive
  while the app i..." are cut; the relay and question sheets are taller and cover more of the page behind.
- **Symbols**: SF Symbols on the iPhone, Material Symbols on Android (Apple's licence, PHN-458).
- **Platform words**: Face ID on the iPhone where Android says fingerprint (the lab's own two versions).
- **The composer**: typed text shows its end on the iPhone (the lab's `truncationMode(.head)`); Android shows its start.
- **Blur and shadows**: SwiftUI's own here; Android reads SwiftUI's radius as a Gaussian's sigma (unchecked there).
