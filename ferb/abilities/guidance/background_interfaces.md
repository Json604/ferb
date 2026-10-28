# Background Interfaces

Packaged FERB guidance. Apply the section for your role; this grants no tools or authority.

Choose background routes from observed reliability: Accessibility actions/values on the exact target window;
app/PID-directed events; the app's scripting dictionary without activate; background-safe URL schemes;
browser extension/CDP; shell/CLI/file-level operations. A URL scheme or script can itself activate an app: inspect
its documented behavior and observed facts. Use direct file input assignment for browser uploads, never open a
native picker to work around an unavailable route. If foreground is unavoidable, ask in one line what needs focus,
roughly how long, and the choices now / when I am idle / skip. Never restore an old foreground app afterwards.
The disturbance sentinel reports Space/front-app/key-window/cursor/full-screen/window-order changes and sampling
limits, marks which FERB actions in flight could change the screen (on_screen), and lists the processes FERB started
that were running. A change with no FERB on-screen action in progress says when the last one ended, or that this task
made none: it may come from the owner, another app, a process FERB started (such as a coding agent, which acts with
FERB containment, with its own sandbox disabled by caller-chosen flags; FERB never rewrites those flags or removes approvals), or a FERB action whose effect landed late (an app finishing its launch); weigh the elapsed time
and the running processes. Read task.evidence disturbance records; nearness to a FERB action does not prove FERB
caused a change.

A browser profile that isn't open is never a reason to ask: the browser tools start it in the background (or open
its window when that doesn't connect), and browser_open puts the tab in one of its open windows or, when it has none,
in FERB's own minimized window, made once (the owner accepted that focus shift). Never ask the owner to open a profile window, to wait until they're idle or to skip a step
for it, unless a browser tool says the profile's extension isn't paired. Reuse a task tab or FERB's window. If
browser.open returns window_selection_required, choose an exact existing window_id from its evidence (browser.tabs
also includes window_id) for an inactive tab; code must not choose first/focused. Window placement within the
established profile is a method choice: choose from observed state/context without asking the owner how to execute.
Ask only about unresolved account/source identity or needed foreground use. Never restore the old app/Space
afterwards. A disconnected source is not permission to replace it with another source.

Prefer semantic controls and existing tabs. Choose browser controls or native controls from actual capability
and observed results, within the shared interaction contract. Natural account references such as "scaler Gmail"
identify the owner's intended account; there are no special permission phrases. Preserve current tabs and completed work.
Use the browser tools for a paired browser profile even when it isn't running or connected now: they start it in the
background, or open its window when that doesn't connect (the owner accepted that focus shift), then work in hidden
tabs. Open sites with browser_open or
browser_navigate. Never drive a browser through Mac tools (its menus, address bar, clicks or keys) when a browser
tool can do the step: that takes over the owner's screen. Mac input in the foreground asks the owner first, once per
task for that app; prefer a background route. If a browser tool says a profile isn't paired, pair it with browser_pair (FERB asks the owner once); never work around it.

Leave task tabs available for result inspection; FERB releases them when the task ends. When the owner's requested
outcome includes keeping a page, class, media or session running, use the browser family's keep_open tool before
reporting completion. Inspect its retention result; ordinary temporary tabs are closed at task end. In a
loop round, the loop keeps this tab held for you between rounds; leave it held, without keep_open, which is for work
that ends.
If retention is uncertain, report that uncertainty and recover rather than promising the tab remains open.
If a view must change or
close, preserve the source/account context in the evidence and report so earlier observations can be located.

Choose one native input mechanism from the observed control and supported actions: Accessibility first when it
can address the exact target, otherwise an explicit process/window-directed mechanism. mac_click and mac_pointer
do not silently replay through another route. Inspect the result before choosing a new mechanism or retrying. If
no background route works, return the structured needs_foreground fact to the orchestrator.
Before reporting that a CAPTCHA or anti-bot check blocked background work, quote the challenge or error the page
shows, from a fresh snapshot. A "protected by reCAPTCHA" or Turnstile footer or badge alone is not a block: read and
report the page's actual message instead.
A verification check in the owner's own browser profile (a "Verify you are human" checkbox, pictured objects to
pick, a slider) is handled with FERB's browser_verification ability: read its procedure with abilities_read and follow
it with your browser tools, as the owner would, checking the page with a fresh snapshot after each input. If the check
still blocks (a press-and-hold, a puzzle you can't resolve, the site's rejection), call browser_handoff with the
page's own words: FERB sends the owner one notification that opens the tab and returns once the page lets them
through. Never ask the owner to reply "done".

For canvas controls, sliders, dragging or a precise visible target that semantic actions cannot operate: take a
fresh browser screenshot, resolve its pixel coordinates with *_point, then use *_act click / dblclick / rightclick /
hover / drag. For drag resolve both endpoints from the same screenshot and pass end_element. The coordinates stay
exact, instead of clicking an ancestor's center. Each gesture consumes the screenshot, and it expires after 30s;
page changes require another screenshot. Drags must stay in one frame. For native apps use mac_pointer with a fresh
mac_screenshot and image coordinates (end_x/end_y for drag). Window-targeted background delivery is attempted when
available; the legacy route may be limited to the current Space. Preserve the shared interaction contract if a
route needs foreground use. Prefer semantic actions when available. Verify the result with a fresh read;
an interrupted gesture may have partly happened, so inspect before deciding what remains.

For native scrolling, target an element inside the intended list/pane with mac_act scroll, direction and amount
(up to 50 bounded steps per call). AXScrollToVisible only reveals an element; it is not scrolling toward the end.
If a pane has no usable AX route, use mac_pointer action=scroll at a screenshot point inside it. This is a wheel
operation, not a drag. Set distance to the fraction of the pane to move per step (0.01–1.0, default 0.8).
Use a large distance/batch for traversing a long list; near a specific target, choose a small distance (e.g. 0.05–0.15)
and amount=1, inspect, then adjust. Distance and batch count are your decisions from the task and current observation.
Choose a useful batch size instead of one model round trip per row. Batches stop on uncertain
progress; inspect the returned count/reason and take a fresh screenshot. Unchanged content can mean failed input,
not the bottom. A scrollbar boundary still needs checking for lazy-loaded content. Never substitute Next Chat,
selection changes or opening each row for scrolling a list. If a route fails and fresh evidence shows no progress,
report the limitation instead of repeating it indefinitely.
