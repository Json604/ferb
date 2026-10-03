# 0.23 — managed extension updates and connection recovery

## What actually broke in scaler

The browser still had a FERB pairing recorded in the daemon, but its current extension storage contained a new
installation UUID, no matching pairing key and sequence zero. The live connection was unpaired. The prior record
and cached version made `ferb browsers` say “paired, not connected, extension 0.9.0.” This was not a task-intent
gate. The earlier protocol mismatch was separate; repeating Reload did not restore the missing pairing.

The owner authorized one visible pairing tab. `ferb browsers pair scaler --browser chrome` completed, and a fresh
RPC confirmed scaler **paired and connected**. No browser restart or daemon reinstall was performed for that repair.
At the final check Chrome was no longer running, so the connection was absent again, but its repaired identity,
key and sequence still matched the daemon. A closed browser is distinct from a lost pairing.
The evidence does not establish why extension storage had been reset; do not label every disconnect a reinstall.

## Implemented behavior

- Setup/update publishes the extension at `~/.ferb/extensions/chromium`, independent of the source checkout.
  The wheel includes the bundle, so installed commands can prepare it too. Files are staged first and replaced
  individually, with the new manifest/version published last.
- `ferb browsers extension install` prepares that bundle and the Chrome/Brave native-host registrations without
  opening browsers or restarting the daemon. `ferb browsers extension path` prints the managed path.
- Extension **0.10.0**, protocol **5**, checks its own local manifest on the existing 30-second alarm. A newer
  version triggers `runtime.reload()` after commands, gestures, attached debugger sessions and owned task tabs
  have finished. Pairing credentials are preserved. Repeated failed reloads of the same version pair do not loop.
- A refusal no longer permanently disables reconnecting. A handshake that gets no response closes and retries;
  refusal hellos retry on the alarm. Chrome's disconnect reason and current connection state are retained in
  extension session storage; errors are also logged to its service-worker console. This does not make an offline
  extension's internal error remotely readable by the daemon.
- Unknown live installs receive `pairing_required`. Bad-key/replay hellos receive an explicit repair diagnostic,
  while action execution still requires a valid pairing. `ferb browsers` distinguishes current connected version
  from the last connected version and counts unpaired connections by browser. It does not guess which profile a
  new unknown installation belongs to.

Automatic reload requires Developer mode for an unpacked extension. Reloading a disabled extension, repairing
lost credentials, adding new permissions and installing in a new profile remain separate operations. There is no
browser restart, profile switching, policy installation, preference-file editing or silent account selection in
the production updater. Version changes must accompany extension releases; same-version source edits are not updates.

## One-time migration and routine updates

Run from the branch checkout:

```sh
.venv/bin/ferb browsers extension install
```

In each profile already using FERB, open its extension management page, enable Developer mode, and **Load unpacked**
from `~/.ferb/extensions/chromium`. Do not remove the old extension first. The fixed manifest key preserves the
extension ID; the real Chrome migration test also confirmed that loading the new path preserved the pairing.
Check `ferb browsers`; repair with `ferb browsers pair <profile> --browser chrome|brave` only if credentials are lost.
New profiles still need initial installation and pairing. Installation does not authorize using a personal profile
for an unrelated task; the agent follows the owner's account/profile direction.

Then the owner can run `.venv/bin/ferb setup --update` to install 0.23 and restart FERB. Migrating first avoids the
new daemon refusing the old 0.9 extension before it gains the update code. Subsequent setup updates refresh the
same managed folder; running, idle profiles reload automatically, and closed profiles load the new files next time.
The first bootstrap from 0.9 cannot be automated by update code that 0.9 does not contain.

## Research and deployment boundary

- [OpenClaw's current setup](https://docs.openclaw.ai/tools/chrome-extension) registers a native host and requests
  its published Store extension across Chrome profiles on macOS. Chrome discovers that request at startup and
  controls per-profile approval. OpenClaw does not restart Chrome or approve that prompt automatically.
- [Chrome's external installation mechanism](https://developer.chrome.com/docs/extensions/how-to/distribute/install-extensions)
  requires a Chrome Web Store update URL on macOS. FERB has no published Store listing, so writing an external
  install request today would not install this local bundle. Store publishing is a separate owner decision.
- [Chrome runtime behavior](https://developer.chrome.com/docs/extensions/reference/api/runtime) treats unpacked
  `runtime.reload()` as an update. [The update lifecycle](https://developer.chrome.com/docs/extensions/develop/concepts/extensions-update-lifecycle)
  distinguishes local reload from downloading a Store release.
- [Chrome's agent extension tools](https://developer.chrome.com/docs/devtools/agents/extensions) support unpacked
  installation/reload in a debugging session. FERB uses that only for isolated tests; it does not restart personal
  Chrome with remote-debugging flags to force installation.

## Verification

Offline regressions cover lost-pairing diagnostics, refused reconnect, valid handshakes, deferred updates, a command
starting during asynchronous update checks, no same-version/downgrade reload, preserved pairing storage, packaged
bundle resolution, and publishing the manifest last.

`tests/test_extension_update_real.py` runs a disposable Chrome and daemon. It pairs once, moves the unpacked
extension to the managed path, then publishes 0.10.1 and waits for the actual alarm. The extension reloads and
reconnects under the same installation identity; the fixed key is proved by the authenticated connection. No
visible window opens and the front-app detector does not observe Chrome taking focus. This verifies Chrome in
this setup; Brave and the owner's full-task experience remain separate acceptance checks.

Initial real attempts exposed a lab prerequisite: CDP's first load can succeed without Developer mode, but Chrome
disables the extension on reload in that state. The final lab configures Developer mode using Chrome's own settings
API in a hidden, test-owned page. No owner browser preferences are edited.

Final validation: 531 offline tests passed, 52 deselected (112.59 s); the combined real migration/update passed
in 32.98 s. Core mypy, JS syntax, wheel build, isolated installed-package resource lookup and whitespace checks
passed. The scaler pairing repair is preserved; installing this new daemon release remains an owner-controlled step.
