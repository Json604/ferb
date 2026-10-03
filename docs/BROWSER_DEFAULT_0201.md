# Browser defaults and the disconnected Brave task — 0.20.1

> Historical design. The owner replaced action/identity/exposure/phrase gates in 0.22; see [the current contract](ORCHESTRATED_CONTROL_022.md). Platform evidence below remains version-specific.


The owner's rule: basic browser work uses FERB's own profile. Personal profiles are used only when explicitly
requested. The Zomato retry violated that rule: it was classified as owner-browser work, received all owner scopes,
then tried Work and scaler. A phone number, saved address/payment or an earlier agent-selected profile is not
permission to use those browsers.

The later request, “keep scrolling in my current brave profile,” explicitly authorized Brave. Its trace showed an
empty tab inventory, a Mac Accessibility read of YouTube, refused Mac scroll/pointer calls, and a click that pressed
a web area without establishing scroll progress. It did not reach the last comment. The startup message about the
old Zomato attempt was interruption recovery following a daemon restart.

Read-only diagnostics found Work, scaler and FERB's own profile paired but disconnected, with no extension version
or rejection recorded by the current daemon. Native-host manifests and launcher were present and pointed at the
installed FERB environment. No native-host process was running when sampled. The available evidence does not
identify whether the extension was suspended, had stopped after an earlier rejection, or failed before greeting.
Reloading the existing FERB extension is the first recovery step; re-pairing is not established as necessary.
No personal tab was opened, read or acted on during this diagnosis; existing task traces and extension metadata
were inspected. A targeted stop request for the earlier Zomato attempt found it had already ended.

## Changes

- Prompts make FERB's own profile the browser default. Misclassified web-only owner tasks get one repair before
  work starts. Missing capabilities must be reported without switching to a personal profile.
- Owner tasks lose personal-browser scopes unless the owner's words explicitly name a browser/profile (including
  “my browser” / “current browser”). The gateway blocks native browser fallback when those scopes are absent.
  General app/file access does not automatically grant browser access.
- Profile account/domain matching is not authority. The explicit-name mode uses browser names and explicit
  profile references; available profile lists and model-written goals do not count.
- A retry can correct an earlier owner route to FERB; original explicit owner directions remain context. Existing
  data/identity boundaries, unknown-action replay blocks and per-action floors remain enforced.
- Tab listing reports disconnected pairings and any recorded rejection instead of an empty list. No tab opening
  or wake attempt is performed by this diagnostic.
- A task may try loading its selected profile without a startup window. On timeout it reports the connection
  problem. The old visible wake tab, followed by restoring the owner's app, is removed from automatic startup.
  Explicit owner-run setup/pairing may still open a page.

## Limits and recovery

FERB's identity profile still supports only FERB's accounts. Browser location and the identity signing into a site
are distinct; a requested owner login unsupported in the isolated profile needs clarification, not a silent switch.
Mixed owner-file/browser tasks cannot gain personal-profile access just by being labelled owner tasks.

A windowless launch is not proof that macOS can never switch Spaces. The owner's observed shifts override the lab's
limited frontmost-app polling. No real browser/app probes were run for this patch. Browser connection recovery and
owner-visible acceptance remain outstanding.

The running daemon is unchanged. When ready, the owner runs `ferb setup --update`. The extension remains 0.9.0 /
protocol 5. In the requested Brave profile, open `brave://extensions`, enable/reload FERB, then retry the current-tab
request. Reload FERB in its own Chrome profile too before ordinary web tasks. No pairing reset is required unless
subsequent diagnostics establish that problem.

Offline regressions cover profile authority, legacy retry correction, destination safety, disconnected tab listing
and refusal to create a visible wake tab. **516 offline tests passed, 50 deselected** (116.60 s); core mypy and `git diff --check` passed.
