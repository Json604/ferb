# Voice and audio: decisions and fix list

Area: `ferb/voice/*` (controller, mic, push_to_talk, speak), `ferb/sense/transcribe.py`, `ferb/act/listen.py`,
`ferb/act/recording.py`, `ferb/act/mac/audio.py`, voice/hearing wiring in `ferb/daemon/main.py`, the CLI voice parts,
and the FERB Voice helper (`macapp/FERB/main.swift`). The notch app (ui/) is the owner's side; read only for the RPCs
and dictation it already has.
Basis: a read of every line (2026-10-30), read-only DB counts. Nothing was recorded or spoken.

## Owner decisions (2026-10-30)
1. **Push-to-talk setup moves into the app**; no CLI path (notes/daemon.md decision 3).
2. **Speech output: macOS's native voice now; other TTS providers pluggable later**, like the runtime adapters.
   Owner, 2026-10-30: keep Apple's voice and the app's dictation for now; local human-like voices (Kokoro,
   Chatterbox) and a local dictation engine (Parakeet/Whisper + cleanup) are deferred until the owner raises them.
3. **One push-to-talk path: the app's dictation** (hold Right Command + Option, the Mac's language). It gains an
   option to send on release, and replies are spoken through the pluggable speaker. The daemon's push-to-talk stack
   goes: `voice/controller.py`, `mic.py`, `push_to_talk.py`, the voice role and `voice.*` RPCs, `owner_input.on_key`
   forwarding, FERB Voice's `serve`/`Listener`/`request` modes, `ferb setup --voice`, `ferb voice`. FERB Voice keeps
   `transcribe`, `status`, `request-speech` for hearing. (App changes are the owner's side.)
4. **Music recognition: not now.** Workers keep being told FERB can't recognise music by its sound.
5. **Muting while FERB listens: not now.** Today's behavior stays (FERB hears the whole app, unmuted).

## What exists today (verified)

### Two separate push-to-talk systems
- **Daemon push-to-talk**: hold **Right Option** (push_to_talk.py:10); the daemon's input tap forwards keys while
  `push_to_talk` is on; FERB Voice (macapp/FERB, macOS 14.2+) records the chosen mic (never Bluetooth) and recognizes
  on-device, **hard-coded to en-US** (main.swift:117); the transcript runs a chat turn in session "voice"; the reply is
  spoken with `say`. Enabling it needs `ferb setup --voice` (writes `voice.requirement`); the app calls no `voice.*`
  RPC. **Not used since 10-02** (34 `voice.attached` on 10-01/02, 0 voice turns, `push_to_talk = false`).
- **The app's own dictation**: hold **Right Command + Option**, `AVAudioEngine`, on-device, the Mac's current locale;
  the text goes into the draft and is not sent (ui/…/NotchShortcutState.swift:35-47, Microphone.swift).
- Daemon voice replies are rendered through the **CLI's** rendering and string-matched CLI prompts (main.py:681;
  controller.py:134-139): removing the CLI breaks them. Strings point at "ferb chat's hidden prompt" and
  "`ferb setup --voice`".

### Speech output
- `Speaker` protocol (say/stop/wait), `SaySpeaker` (one `say` process per utterance), `Silent` (voice/speak.py).
  Used only for daemon voice-session replies and `ferb chat --speak`; hard-coded construction; no voice, rate or
  locale. `voice.said` is published with the text.

### Hearing and recordings
- ScreenCaptureKit captures **one app's** audio (16 kHz mono WAV; audio.py:182-209); a tab listen hears the whole
  browser (every tab and profile of that instance). FERB Voice transcribes on-device through a lease-file protocol,
  codesign-checked; the sound file is deleted after transcription; transcripts redacted.
- Live: 5 `audio.listen` calls (all by tab): 2 captured nothing, 3 captured sound, 1 non-empty transcript; 2 strong
  signals with empty transcripts (consistent with music); 5 browser recordings.

### Music recognition
- **No ShazamKit or music-recognition code anywhere.** worker.md:61-64 tells workers FERB can't recognise music.
- (a) **Control Center "Recognize Music"**: no public API; only by pressing the Control Center item through
  accessibility and reading the result notification (Full Disk Access already reads Notification Center). Needs the
  owner to add the item; drives system UI (may open a panel, against background-first); can't target one app; writes
  to the owner's Shazam history. Unclear (live check): whether it hears app audio or only the mic; whether a macOS
  Shortcuts "Recognize Music" action exists (`shortcuts run` would be a cleaner trigger).
- (b) **ShazamKit fed FERB's captured app audio** (a FERB Voice `recognize` mode reusing the lease pattern): fully
  background, hears exactly one app, works with headphones. **Catalog matching likely requires the ShazamKit App
  Service on an explicit App ID from a paid Apple Developer team**; FERB is signed ad-hoc / with a self-made
  certificate today. Sends an audio signature to Apple's Shazam service (a new hosted destination, rule 15).

### Muting while FERB listens
- Today's mute sets the page's media element muted (content.js:657-658): FERB hears nothing either.
- `chrome.tabs.update({muted:true})`: Chrome mutes before output, so expect silence (live check).
- `chrome.tabCapture`: one tab, local playback stops during capture (muted for the owner, heard by FERB), **but needs
  a user gesture on that tab** (extension action/shortcut) and a protocol change to carry audio.
- Core Audio process tap with `muteBehavior = mutedWhenTapped` (macOS 14.2+, new "System Audio Recording"
  permission): per process. **Native apps (Music, Spotify, players) work cleanly.** **FERB's own browser is the same
  Chrome process as the owner's** (launched with `--profile-directory` in the owner's data folder,
  act/browser/profiles.py:189-199), so tapping it would mute the owner's Chrome too; isolation needs FERB's browser
  as its own app or `--user-data-dir` instance (real rework). Live checks: which process owns Chrome's audio; whether
  capture still works while a tap mutes.

### Bugs (confirmed by code)
1. A failed push-to-talk start keeps the last utterance; on release FERB Voice returns it and it runs again as a new
   chat turn (main.swift:139-152, 186-195; controller.py:87); also blocks the helper 3 s.
2. Mic list reported only at attach (hot-plug unseen).
3. `codesign` runs synchronously on every voice connection; FERB Voice opens a new connection per transcript.
4. Push-to-talk English only.
5. The app can't enable push-to-talk.
6. CLI-pointing strings; daemon voice path depends on CLI rendering.
- Suspected: both hotkeys can start at once (Right Command then Right Option); no transcription timeout; quadratic
  `SequenceMatcher` on the event loop for long listens; `_quit_voice_app` kills an in-flight transcription; IOProc
  reads only the first buffer; `browser.record with_audio` lacks listen's excluded-host check.

## Target design
1. **One push-to-talk path**: the app's dictation (decision 3).
2. **Speaker factory**: `[voice] tts = "macos"` (+ voice, rate); providers registered like runtimes; a hosted
   provider only ever receives redacted text; speech text from structured events, not CLI strings.
3. Hearing keeps ScreenCaptureKit app capture, FERB Voice `transcribe`/`status`/`request-speech` and the lease
   protocol; transcription timeout; redaction alignment off the loop.

## Fix list
- [ ] Remove the daemon push-to-talk stack (decision 3); daemon side of "speak this reply" for the app's voice turns.
- [ ] Speaker factory with the macOS provider; config keys; redaction for hosted providers.
- [ ] Voice replies from structured events (no CLI rendering); strings point to the app.
- [ ] Transcription timeout; alignment off the loop; `_quit_voice_app` spares transcriptions; record-with-audio
      excluded-host check.
