# 0.21.0 — window-targeted input and observable scrolling

Status: implemented on `astra/version`; automated contracts pass. **Owner acceptance pending.** No daemon update or profile changes. The owner requested a
one-step-at-a-time visual verification loop, including real keyboard focus rather than just frontmost-app polling.

## What changed

- `mac.act scroll` resolves the intended container (including AXList/AXTable/AXOutline). A window with several panes
  requires an element. It no longer mistakes `AXScrollToVisible` for a directional scroll. It prefers the container's
  scrollbar; Page keys are limited to an explicitly targeted AXTextArea. It never navigates chat rows to scroll.
- A call runs at most 50 bounded steps, checking position after each. A scrollbar boundary, batch limit, owner input
  or unconfirmed movement stops the batch. No-change does not prove the end; lazy-loaded lists still require a fresh
  content check even at a scrollbar boundary. Unknown positions never count as a boundary.
- `mac.pointer action=scroll` sends vertical/horizontal viewport-sized pixel steps to a captured point. Direction and amount appear
  in the CLI trace with an arrow. It uses the same 30-second screenshot lease and window/owner checks as gestures.
  Progress includes a bounded fingerprint of visible child geometry and hashed identity when bars expose no value.
  Without measurable AX container evidence, it sends one step and asks for a fresh screenshot rather than continuing
  blind. This is a CLI indicator, not an on-screen overlay or permanent cursor icon.
- `window_input.py` loads optional SkyLight window-addressed mouse delivery. Missing symbols retain the legacy public
  PID route and its observed same-Space limitation. Available symbols permit an attempt on another Space; this is not
  proof that any particular app accepts it. Events carry the exact PID/window ID and FERB marker. No activation,
  focus-transfer, global event-posting or Space-changing API is called. Delivery happens once, never through two
  transports. Drag release remains inside the GUI lock even on cancellation.

## Owner verification — use this checkout, without installing it

Run from the repository:

```sh
.venv/bin/python -c 'from ferb.device.watch import main; raise SystemExit(main())' --mac --only M14
```

This selects setup, one window-targeted wheel tick in a new disposable TextEdit document, and cleanup last. Existing
windows are not accepted as disposable if creation fails. Keep working in another app during each action; answer the
CLI after each step. For M14, also inspect whether the document actually moved and confirm that keyboard focus stayed
with your app. `u` (unsure) is pending acceptance, never a pass. Results are stored in `~/.ferb/watch/`.

First validate a single tick; then check a bounded batch, cancellation, a second scrollable pane and the other-Space
case with the owner watching. Only after that should the owner update the installed daemon. Live tests in everyday apps require the owner
to name the app and supervise the interaction; the owner explicitly requested a WhatsApp scroll test in this session.

Automated tests cover correct targeting, no reveal-as-scroll, bounded progress, ambiguous panes, unknown endpoints,
route availability, exact routing stamps, no duplicate transport, stale windows, partial/cancel cleanup and watch
setup/cleanup ordering. They do not establish app delivery, lack of focus theft, cross-Space support or actual speed.

## Resources considered, not trusted as guarantees

The owner's Brave ChatGPT conversation linked several open-source agents. Primary-source review found:

- [MacOS-Use scroll implementation](https://github.com/Jeomon/MacOS-Use/blob/c88574c0a70534a21e9490e2118f1fce04e16904/macos_use/agent/desktop/service.py#L238)
  moves the global cursor and posts wheel events, so it does not establish background Mac scrolling.
- [iphone-use verification](https://github.com/leeguooooo/iphone-use/blob/16af06893f6ce9bc0aefaddc96183e43e812e58b/crates/server/src/http.rs#L4638)
  and [bounded execution](https://github.com/leeguooooo/iphone-use/blob/16af06893f6ce9bc0aefaddc96183e43e812e58b/crates/server/src/http.rs#L6189)
  distinguish trustworthy observations, partial execution and unknown outcomes. Useful principles; its WDA transport
  is not a native Mac input mechanism.
- [Cua's native input](https://github.com/trycua/cua/tree/4cac25bc4a02ff2a52f35c9690d4d9601ccea11c/libs/cua-driver/rust/src/input)
  supplies the window-routing ABI used here (MIT attribution in `THIRD_PARTY_NOTICES.md`). Its wheel implementation
  posts via both public and private transports; FERB deliberately uses one. Its focus-restoration code is not evidence
  that a window never stole focus. FERB does not import the synthetic focus-transfer/activation recipes.

These projects do not establish how ChatGPT's visible dots are implemented.

## Live check, 2026-10-03 — explicit owner request

The owner observed no disturbance from the repeated TextEdit-running check, then requested WhatsApp instead.
TextEdit document creation was cancelled. WhatsApp was closed and was launched with `open -g` for this authorized
check. Its chat-list geometry was read, a fresh window screenshot captured, and the branch driver sent exactly one
downward wheel tick while its window was off the current Space. No chats were opened and no text was entered.

Driver result: one tick, one observed container-position change; before/after window-image bytes differed.
The detector logged no activation or Space-change notification. Cursor positions differed; whether that was the
owner's movement or an input side effect is unresolved. VS Code was the initial detector sample before dispatch,
not an observed activation caused by the event. **Owner confirmed: the chat list scrolled and there was no disturbance; cursor/app movement was theirs.**
No background-success claim is made from these readings alone.

Automated checks: 527 offline tests passed (50 deselected, 117.35 s), then the added interrupted-owner-verification
regression passed with its targeted suite (3 tests). Core mypy and whitespace checks passed.

Follow-up: a five-tick line-unit batch stopped after two events because its AX position evidence stopped changing;
the owner reported too little movement/poor speed. Changed native pointer scrolling to 80% viewport-sized pixel
steps (default 80% of the viewport, capped at 800 points), with bounded visible-child geometry/identity evidence for apps with unusable bars. The next
three-step run completed in 1.5 seconds on another Space, three observed changes, fixed cursor, unchanged front app,
no activation/Space notifications. **Owner confirmed: the revised scroll worked without disturbance.** The owner's next queued test is their current
Brave YouTube tab: reach the comments' actual end and correctly identify the last displayed comment.

Distance is now a model-selected viewport fraction (`distance=0.01..1.0`, default `0.8`), separate from `amount`
(the batch count). A fine adjustment can use `distance=0.05, amount=1`; reaching an end can use larger steps and a
bounded batch. Worker guidance requires using the goal and visible proximity to choose distance, then observing the
result. The follow-up 5% WhatsApp test dispatched 34 pixels in 0.59 s, observed one change, cursor/front app fixed;
owner confirmed it was a very small movement. This is the same native route as the accepted 548-pixel steps, not an app-specific branch.

Latest checks: 531 offline tests passed (50 deselected, 118.21 s); subsequent distance-control changes passed the
40-test targeted scroll/pointer/watch/background suite. No daemon restart or installation.

YouTube follow-up: the owner confirmed the existing Brave tab reached the last available comment. Independent
read matched the reported final author/comment. The installed checker falsely failed because it could not access
the worker's tab; branch 0.21.1 fixes that handoff and enforces read-only checker effects. See
`OWNER_ACCEPTANCE_LOG.md` for scope and remaining live checker acceptance.
