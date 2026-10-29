# One-flow motion, notch tear-out and compact reply bubbles

## Goal
Owner feedback: remove double movement during opening/closing, make the physical
notch drag into a pill instead of resizing, and fit short reply bubbles to text.

## Architecture
Restore main's body-expansion behavior in NotchPresentation/PanelCoordinator.
Keep current geometry when reversing a live transition and move origin on the same
spring progress as surface size. Give the camera footprint plus below-camera strip
priority over PanelResizeOverlay. Separate full-width message-row alignment from
content-sized bubble painting in ConversationTranscript and MarkdownContent.

## Spec
- Both docked notch and movable pill open/close as one surface.
- Drag at the actual notch or its immediate underside tears out the pill.
- Other panel edges retain resizing; chat controls retain normal interaction.
- Short reply background hugs its content; long prose and rich content wrap.
- Do not change user-selected panel dimensions.

## Global Constraints
Only ui/ux; no main merge. No visual inspection/screenshots/UI interaction.
The owner verifies appearance. No daemon changes or full Python rerun.

## Review Focus
Interrupted opening/closing; native/SwiftUI geometry agreement; camera drag versus
resize hit priority; long/code/table content measurement; continuous text selection.

## Task 1 — Motion and drag
Fresh implementer motion_drag_fix: diagnose against main; failing geometry/hit
regressions; implement; focused and full Swift checks; fresh read-only review.

## Task 2 — Bubble width
Fresh implementer bubble_width: read-only diagnosis while Task1 runs, then implement
after Task1 validation; content-fitted background plus constrained Markdown width;
offscreen layout regressions for short greeting and long/rich content; review.

## Self-Review
Screenshot indicates reply-bubble width, not panel height. Preserve panel sizing.
Native offscreen test layout is permitted; screen inspection is explicitly forbidden.

## Owner follow-ups in the same session
### Task 3 — Video preview hover dismissal
Trace actual popover windows and close after leaving notch and preview, allowing the
pointer to cross their gap. Remove permanent dismissal blockers and make lifecycle
cleanup idempotent. Detaching must not pin the notch. Fresh implementer/reviewer.

### Task 4 — Retained selectable text redraw
Owner reports blank text with bubble intact, repaired by double-click. Data fallback
is intact; rendering cause is inferred, not reproduced. Add narrow redraw/layout
invalidation after retained hosting-view presentation and focus transitions, preserving
selection and text identity. Validate offscreen lifecycle behavior. Owner confirmation
of the intermittent symptom remains necessary; do not claim visual verification.

### Task 5 — Continuous attachment and whole-camera hover
Owner observes abrupt concave shoulder appearance and camera-top/middle hover failure.
Replace the <=3pt/fixed-radius shoulder gate with continuously growing geometry
driven by the existing shared spring. Preserve all-edge union fill and direct contact.
Connect native indicator enter/move/exit tracking to existing hover state, and include
the physical top boundary in docked hit testing. Do not introduce another timer,
separate motion clock or polling loop. Fresh implementer/reviewer; offscreen geometry
and event-delivery tests plus full Swift. Owner verifies motion and physical hover.

### Task 6 — Cursor-locked drag, display fit, native resize cursors
Owner observes pointer drift while dragging and expanded panel overflowing the screen.
Trace event coordinate conversion and preserve the grabbed point using stable global
coordinates; keep the connected surface one window. Bound effective panel and transcript
layout by the active display, including attachment space, rather than adding arbitrary
product caps. Respect saved preferred size when display space permits. Verify reported
notch geometry and pill fallback on notchless displays with geometry tests. Replace
SF Symbol corner cursors with public AppKit frame resize cursors on macOS 15+, keeping
a native fallback for earlier supported systems. Fresh implementer and reviewer; tests
only, no visual inspection; build and reload development notch after validation.

### Task 7 — Live drag-side adaptation and magnetic docking feedback
Owner requests the open chat to reposition around the dragged pill in real time to
avoid screen cutoff, plus magnetic animation/sound near the hardware notch. Keep
the grabbed pill locked to the pointer; smoothly retarget the connected chat and
concave attachment as direction changes. Fit content to the current display while
dragging, including smaller displays. Do not defer edge choice until mouse-up.
Use a proximity affordance tied to actual camera geometry: restrained attraction/
stretch/glow, one short magnetic sound on entry (respect feedback settings), reset
on exit, dock on release in the indicated capture zone. No automatic docking while
the button is held. Preserve interruptibility, reduce-motion handling, existing
whole-camera hover and one-window attachment. No private APIs or arbitrary product
caps. Fresh implementer/reviewer, regression tests for drag geometry, edge changes,
bounds and proximity enter/exit/release lifecycle; full Swift then build/reload.
No visual inspection, screenshots, UI interaction, or daemon restart.

### Task 8 — Resize cursor before first click
Owner reports resizing works but border hover initially does not change cursor.
Trace non-key border tracking and cursor-rect invalidation as presentation changes
geometry. Make native directional cursor follow the actual resize hit region on
first hover, including after reopen/morph, without stealing text/interior cursors
or notch/pill drag targets. Keep exclusions and native corners. Fresh implementer
and reviewer; offscreen event/geometry regressions, full Swift, build/reload.
No visual inspection or UI interaction. Owner's second motion clarification pending.

Owner clarified Task8 motion: chat repositions around pill; pill follows cursor.
Audit actual AppKit canvas/control screen coordinates, not just pure drag helper.
During side interpolation move the chat surface within a stable drag canvas so a
window-origin change cannot move the grabbed control independently of cursor.
Preserve bounds, continuous connection, magnetic feedback and reduced motion.
Add offscreen native coordinate regression covering relative card movement with
stationary pointer plus real pointer translation.

Task8 owner steering: remove the current magnetic proximity animation and sound.
Await owner's new walkthrough before designing a replacement. Delete proximity
glow/logo deformation, generated chirp and enlarged magnetic capture region; restore
basic docking against the existing physical-notch/pull-strip target. Preserve basic
swallow docking, live chat positioning, and resize cursor fixes. Remove tests specific
to the withdrawn feature while keeping geometry and docking regression coverage.

Task8 owner steering supersedes Task7 magnetic feedback: remove the proximity
animation and sound entirely pending the owner's walkthrough. Restore release
capture to `NotchDocking.accepts` (physical camera plus lower pull strip), preserving
the pre-existing swallow transition and live chat-side placement.

### Task 9 — Four direct inward-facing chat states
Owner rejects continuous following/orbiting and supplies top/bottom/right examples.
Use nearest screen edge to select exactly four states: top→chat below, bottom→above,
left→right, right→left. Remove available-room scoring as orientation selector.
Within one state pill and chat track pointer directly, with only necessary screen-fit
constraints. Animate only discrete state changes using a short interruptible direct
transition from current geometry to new anchored geometry; no perimeter route or
continuous chase filter. Preserve cursor lock, connected rounded surface, bounded
content, stable native canvas, first-hover resize cursors and basic notch docking.
Magnetism remains removed. Fresh implementer/reviewer, tests first including nearest
edge mappings, same-state direct tracking, interrupted state changes, bounds/contact.
Full Swift/build/reload; no visual inspection, screenshots, UI or audio interaction.

### Task 10 — Soften accepted four-state transitions
Owner approves Task9 placement exactly; only movement feels too direct. Preserve
all four states, nearest-edge selection, same-state direct pointer tracking, stable
canvas and no magnetism. Replace abrupt 180ms cubic ease-out with a restrained,
non-bouncy settling motion with smooth acceleration/deceleration; preserve velocity
on interruption where practical. Keep state changes short and responsive, reduced
motion immediate. Fresh implementer/reviewer; focused motion regressions and full
Swift/build/reload. No visual inspection or UI/audio interaction.

### Task 11 — Floating rendering performance
Owner accepts Task10 motion but observes poor FPS while floating. Diagnose source
and measure offscreen rendering work; do not blame hardware or claim measured live
FPS without evidence. Preserve accepted states, spring, cursor lock and appearance.
Investigate duplicate native mask/tint rebuilds, synchronous display/layout, published
SwiftUI invalidations, stationary full-display canvas mask cost. Optimize actual hot
paths with minimal changes, coalescing/caching unchanged updates and avoiding
forced redraw where retained layers suffice. Keep blur and connected clipping correct.
Fresh implementer/reviewer; regression tests for invalidation and geometry; report
before/after comparable code-level timing/counts, full Swift then build/reload. No
screenshots, visual inspection, UI/audio interaction or daemon restart.

### Task 12 — Native regular-window finish
Owner asks for a good native macOS finish on the current regular FERB window and
authorizes continuation. Use native System Settings-like navigation: a single sidebar
with app destinations and settings categories, no nested horizontal seven-tab frame.
Keep existing selection/deep-link entry points, all settings actions, persistence and
page lifecycle. Use unified native toolbar/title, readable sidebar labels and consistent
content insets/grouped forms with semantic system colors. Preserve floating chat
unchanged. Files: FERBWindow.swift, SettingsPages.swift, relevant window tests.
Fresh implementer: first add meaningful navigation/deep-link regressions, implement,
focused/full Swift tests with anonymous SwiftPM and diff check. Fresh read-only
reviewer checks behavior preservation, native layout and accessibility. Build/reload
dev UI only after passing. No screen inspection or live UI tests; owner verifies visuals.
No daemon changes, commits, merges or pushes; stay on ui/ux. Update current docs.

### Task 13 — Compact sites, hover artifact previews, terminal-event opening
Owner requests: show exactly first three visited sites inline with +N button revealing
full list; hover every response artifact shows a small preview window; chat opens on
completion, failure or question. Preserve existing streaming arrival and question
behavior, focus/background policy and no replay of historical results. Completion
glow can retain existing expiry; expansion must not depend on glow.
Files: ConversationTranscript.swift, new narrow artifact preview helper if needed,
NotchModel/DaemonConnection and tests. Use native Quick Look embedded preview or
thumbnail (no shell reading or live screen capture); unsupported/missing files show
honest fallback. Preserve click Open/Quick Look/Finder/drag. Hover preview should
not steal focus, clip inside chat, or leave chat stuck open; reuse existing preview
window lifecycle/hover integration. Plus list keyboard accessible and scrollable.
Fresh implementer writes failing behavior regressions first, full anonymous Swift
suite and diff check; fresh reviewer; build/reload only after pass. No visual
inspection, daemon restart, commit/merge/push. Same ui/ux branch. Owner visual QA.

### Task 14 — Reliable file hover and source chips
Owner reports hover preview fails and click opens Markdown in Xcode; wants native
Mac viewing. Main artifact click uses existing Quick Look; retain explicit default-app
Open and Finder/drag in context menu. Do not alter system file associations. Diagnose
hover source in non-key notch, implement reliable native tracking if SwiftUI onHover
is insufficient, integrated nonactivating native preview lifecycle if needed; no
visual inspection. Preserve teardown/Space/hover exit/drag and missing-file fallback.
Owner references compact citation-style pill: first site plus remaining count replaces
three inline domains (new reference supersedes Task13 format), full source-list popover
with separated rows. Use actual known host metadata only, no invented page titles or
fetched favicons; generic native icons are fine. Hover/click opens, keyboard usable,
full list retained and scrollable. Tests for actual tracking setup and click route,
site counts and lifecycle. Fresh implementer/reviewer, full Swift/diffcheck/build/reload.
Same ui/ux, no daemon restart/visual inspection/commit/merge/push.

Task14 additional owner steering: attachments must be cards inside rounded composer
above text input, following supplied ChatGPT reference, not a separate Shared with
FERB row below. Extend to NotchView composer and NotchModel chrome sizing; compact
horizontal scroll of native file-icon cards/filename/remove button, no attachment
count cap. Preserve empty input size, add/remove/submit and preview actions. Account
for strip height in existing screen-fit/transcript calculation. Tests cover attachment
height/removal and no card strip when empty.

### Task 15 — Immediate acknowledgement preview and hover feedback
Owner requests ack text as soon as it arrives, briefly emerging from FERB without
opening whole chat. Show fresh ack in small text-only nonactivating anchored bubble
when chat closed; when chat open show immediately in transcript (remove artificial
QuickReplyBubble delay). Preserve ack in archive; never replay old acks; dismiss bubble
on full chat opening/answer/question/failure, replace safely on next ack. Brief timed
presentation with restrained motion/reduced-motion handling, fit active screen and
anchor correctly docked/floating, track positioning and teardown/Space/sleep. No
focus/input stealing and no change to terminal/question full opening.
Owner also wants pointing-hand cursor and highlight for file/source hover. Use native
cursor rects/tracking without corrupting resize/drag cursors; reset on exit/removal.
Sites popover closes immediately on leaving trigger (no cross-gap retention for sites),
while click/keyboard disclosure remains usable for full list; file preview behavior
owner said is good, retain its cross-gap grace. Reconcile hover-dismiss with keyboard
click activation as appropriate; no sticky anchor state.
Fresh implementer writes regressions for ack no-expand/focus, immediate transcript,
replacement/history/dismissal and native cursor/source-hover lifecycles. Fresh review,
full anonymous Swift suite/diffcheck/build/reload. ui/ux only, no visual inspection,
daemon restart, commits/merge/push. Update current docs.

### Task 16 — Recurring blank selectable reply text
Owner reports previous fix insufficient: blank upper answer while later paragraph
paints; never seen before UI work. Read-only offscreen diagnosis found actual
MarkdownContent uses private SwiftUI SelectionTextField (NSTextField), not NSTextView
used by prior regression. Text strings/geometry intact through numerical reveal
probe; exact missing-pixel failure not reproduced (no visual inspection allowed).
Implement narrow native-text rebuild on genuine completed collapsed→expanded reveal:
model render generation conveyed through environment; id only MarkdownContent block
container so native selectable fields remount while transcript/composer/scroll and
message identities remain stable. No remount per resize/status/drag/side switch, no
per-frame full layout. Old hidden text selection ends at close/reopen; open-chat
selection remains untouched. This trades hidden selection for fresh field/cell/backing
state and directly addresses suspected stale private renderer without claiming proof.
Regression must use actual production MarkdownContent, verify native field identities
change once on reveal, content/style/links remain, same-open/drag retains identity.
Test lifecycle callback/revision interruption avoids lost/double refresh. Fresh
implementer/reviewer, anonymous full Swift/diffcheck/build/reload with Task15. No
visual inspection, daemon restart, commit/merge/push; owner verifies symptom.

### Task 17 — Source hover union and substantive pre-worker preview
Owner corrects strict-exit behavior: allow pointer onto full sites list and scrolling;
treat trigger+popover as continuous hover region with brief cross-gap grace, dismiss
after leaving both, prevent underlying artifact preview from taking over. Preserve
keyboard disclosure and cleanup. Owner clarifies caption should present substantive
model response before worker invocation (plan event), never canned ack. Read actual
event producer to confirm correct signal, preview fresh plan after foreground restored
without expanding/focusing chat. Owner follow-up removes canned ack text and sound for now:
keep backend/archive ack contract, hide canned text in transcript, never caption/sound it. No daemon changes.
Tests for source anchor→popover→scroll lifecycle/underlying preview competition and
ack-no-caption versus plan-caption/duplicate/wrongrequest/background/terminal cleanup.
Fresh implementation/review, full Swift/diffcheck, build combined afterTask18.

### Task 18 — Replace unreliable selectable message renderer
Owner again reports blank text, now QuickReplyBubble too (outside Markdown-only
rebuild), so previousTask16 is insufficient. Fresh read-only diagnosis of common
selectable renderer/compositing path first; implement robust controlled native text
rendering or minimal evidence-based common fix rather than widening guessed redraw
workarounds. Preserve copy/selection, formatting/safe links, content-sized bubbles,
scroll/drag perf and supported Markdown. No visual inspection. Tests must exercise
actual production ack/plan/answer views with collapse/resize/reveal and assert native
layout/content/drawing lifecycle. Explicitly do not claim visual reproduction if none.
One implementation at a time afterTask17; fresh review fullSwift/build/reload.
All workui/ux no commit/merge/push/daemonrestart.

Task17 owner steering: hide short fixed/canned acknowledgement text for now; owner
will revisit with voice. Suppress canned-ack transcript rendering and its accepted
sound/preview, retain backend/archive event data unchanged. Substantive plan remains
visible/previewed. This overrides earlier instruction keeping canned ack inline.

### Task 19 — Latest-response site hover by composer
Owner reports sitepill hover fails only latest reply immediately above composer,
works after another owner message. Diagnose real transcript scroll bounds/tracking
area/popover anchor and composer overlap using offscreen actualviews; do not assume
spacing without evidence. Fix minimal common hover/layout cause, preserve source
unionhover/scrolling/occlusion protection, native text renderer and compact layout.
Regression latestreply withsites followed directly bycomposer and no ownerreply;
verify native tracking visibleRect/anchor inside transcript and native event path.
Fresh implementer/reviewer, full anonymousSwift/diffcheck/build/reload. UIbranchonly,
no visualinspection/screenshots/UI/windowordering/daemonrestart/commit/merge/push.

### Task 20 — Connected compact streaming pre-worker response
Owner rejects separate caption window. Preview must be existing connected FERB chat
surface (not detached toast), only substantive text, appears on first response chunk
and updates as streamed, closes3s after stream finishes. Preserve user-open full
chat; deliberate interaction promotes to fullchat/cancels auto-close; later worker
completion/failure/questions still full-open per prior request. Remove separatecaption
window. Same accepted notch/pill morph, reducedmotion, screenfit, nofocus onarrival.
Current daemon ReplyDraft streams only pure reply/clarify; pre-workerplan arrives
final-only. Async owner question pending to permit small supporting preworkerstream
feed beyondUI-only. No backend implementation without answer; UI can support existing
plan event fallback/stream event interface while waiting. Retain redaction/schema
validation and no raw modelJSON or reasoning exposure; final plan remains authoritative.
Fresh implementer/reviewer afterTask19. Tests for compactfirstchunk/update/3safterfinal
replacement/promotion/failure and controlled nofocus; backend tests if ownerauthorizes.
No visuals/commit/merge/push. Daemon restart requires authorized streamscope if needed.

Task20 owner answered yes to supporting daemon pre-worker stream onui/ux. Backend
scope now authorized for redacted provisional pre-worker text delta/final/abort feed
and UI integration. Preserve finalschema validation/action execution sequencing and
redaction, no raw structured fragments/reasoning. Run meaningful streamsecurity tests,
fullPython suite nice-n10 (one only), mypyferb/core, fullSwift/diffcheck. Reload updated
developmentdaemon only as necessary to make authorized feed live; no releaseinstall.


### Task 21 — Reject clipped native hover/cursor geometry
Owner reports launched FERB immediately stops. Read crash evidence and reproduce native
source tracker geometry from restored conversation offscreen; add failing regression
before implementation. Normalize empty/null/nonfinite hover geometry, skip both cursor
and tracking registrations, restore tracking on visibility, preserve anchor bounds and
popover ownership. Files: ArtifactPreview.swift and LatestSourceHoverTests.swift.
Fresh review, focused and full anonymous Swift tests and diff check; no backend changes,
commit, merge or visual inspection. Parent builds/relaunches after review. Self-review:
assert actual clipped transcript behavior and cursor lifecycle, not only synthetic sizes.


### Task 22 — Keep native source hover aligned while the transcript scrolls
Owner recording shows latest source pill misses hover and activation happens below it;
older source anchors sometimes show the same displaced pointer. Full unordered NotchView
measurements agree between SwiftUI and native settled geometry, so do not add padding or
move the composer. Restore native automatic visible-rectangle tracking while normalizing
SwiftUI's viewport-sized native visibleRect to the anchor's own finite clipped bounds.
Files: ArtifactPreview.swift, LatestSourceHoverTests.swift, relevant existing hover tests.
Fail first on the real full NotchView host's oversized native visibleRect, then verify
scroll clipping, restored visibility, close/reopen, native cursor registration and retained
popover crossing/ownership behavior. Preserve Task21 null/nonfinite protections. Fresh
review; anonymous focused/full Swift tests and diff check. Parent builds/reloads; no daemon
changes, commits or live screen inspection. Supplied recording is explicitly authorized.
Self-review: do not claim unordered tests reproduce WindowServer event delivery; report
that remaining live validation separately from geometry/tracking regression coverage.
