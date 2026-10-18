# FERB — Chrome Web Store submission

Copy each block into the Developer Dashboard field with the same name.

## Package

- File: `ferb-extension-<version>.zip`. The manifest is at the zip's root and there is no `key` field.
- Build it with `ferb browsers extension package --out ~/Desktop`. Until that command is released, use the zip in the
  submission kit.

## Store listing

**Name:** FERB

**Summary** (132 characters max):
Lets the FERB assistant on your Mac read and operate the browser profiles you pair it with, for tasks you ask it to do.

**Description:**
FERB is an open-source personal assistant for macOS. It works in the background while you keep working: you ask in
plain words, and it does the task the way a person would, in your apps and in your browser.

This extension is FERB's hands in Chromium browsers. On its own it does nothing. It connects only to the FERB app on
your Mac, through the browser's native messaging, and only in profiles you pair with FERB. When you give FERB a task
that involves the web, the extension:
• opens, reads and switches the tabs the task needs;
• reads the page (its text and accessibility tree) so FERB knows what's there;
• clicks, types, scrolls, uploads and downloads on the page as the task requires;
• takes screenshots of the task's tab when FERB needs to see it.

In paired profiles it also:
• tells the FERB app when a tab is opened, loads a page, becomes active or is closed (the address, with password
  and token parameters removed, and the title), for FERB's activity history on your Mac, which keeps the page loads
  and tab switches. Private (incognito) windows are never recorded;
• finds where password and code fields are on the tab you can see when FERB takes a screenshot of the browser window,
  so FERB can paint over them. It reads their position only, never their contents.

What stays private:
• Passwords and one-time codes are filled through a sealed path. They are encrypted between the FERB app and the
  extension and never shown to the AI model.
• The extension talks only to the FERB app on your Mac. It has no servers of its own, sends no analytics and runs no
  code from the internet.
• Everything FERB does is recorded on your Mac, so you can see what happened.

Requires the FERB app for macOS.

**Category:** Productivity → Workflow & Planning (or Tools)

**Language:** English

**Store icon:** `icon128.png` (128×128)

**Screenshot:** `screenshot-1280x800.png`. Replace it with a real capture of FERB at work when you have one; the
current image illustrates the notch UI.

**Small promo tile:** `promo-440x280.png`

**Official URL / Homepage:** https://ferb.kartikey.xyz

**Support URL:** mailto:kartikey060105@gmail.com until the GitHub repository is public (then its Issues page)

## Privacy practices tab

**Single purpose:**
Lets the FERB assistant app installed on the user's Mac see and operate web pages, in the browser profiles the user
pairs with it, to carry out tasks the user asks for, and keep the user's tab activity in those profiles in the app's
activity history on their Mac.

**Permission justifications:**

- **tabs:** FERB lists, opens, switches and closes the tabs a task needs, and reads their URL and title to know where
  it is. In paired profiles the extension also reports each page load and tab switch (the URL with secret-looking
  parameters removed, and the title; never for incognito tabs) to the FERB app on the same Mac, for the user's
  activity history kept on their Mac.
- **scripting:** injects FERB's bundled content script into the task's page, to read the page and to click, type and
  scroll on it; and into the visible tab of a window FERB is about to screenshot, to find where password and code
  fields are (their positions only) so FERB can paint over them.
- **debugger:** used only on tabs a FERB task opened or claimed, with a fixed allowlist of DevTools Protocol commands
  whose parameters are checked inside the extension (`CDP_ALLOW` in `protocol.js`); every other command is refused:
  - `Page.enable`: receives JavaScript dialog events.
  - `Page.setInterceptFileChooserDialog`: lets FERB answer a file picker with the file the task names, instead of a
    system dialog.
  - `Page.handleJavaScriptDialog`: accepts or dismisses an alert, confirm or prompt as the task decides.
  - `Page.captureScreenshot`: a screenshot of the task's tab.
  - `Page.getFrameTree`: the tab's frames, to place masks over password fields inside embedded frames.
  - `Page.addScriptToEvaluateOnNewDocument`: allowed only with exactly one bundled script (FERB's page hook, compared
    character for character), which lets FERB catch a download or a popup the page opens with script.
  - `Page.removeScriptToEvaluateOnNewDocument`: removes that page hook.
  - `Emulation.setFocusEmulationEnabled`: makes typed input land in a tab that isn't focused, so FERB works in the
    background.
  - `Emulation.setDeviceMetricsOverride`: a fixed 1280×900 layout, so the page doesn't change with the window size.
  - `Target.setAutoAttach`: reaches out-of-process frames through the same tab session.
  - `Target.getTargetInfo`: the tab's own id and its opener's id (never its URL or title), to know which tab opened a
    popup.
  - `Input.dispatchMouseEvent`, `Input.dispatchKeyEvent`, `Input.insertText`: trusted clicks, keys and typing, which
    many sites require instead of synthetic DOM events.
  - `Accessibility.getFullAXTree`: the accessibility tree, for pages built on canvas or ARIA.
  - `DOM.getFrameOwner`, `DOM.describeNode`, `DOM.pushNodesByBackendIdsToFrontend`, `DOM.getBoxModel`: find an element
    FERB marked and where it is on the page.
  - `DOM.setAttributeValue`: only the `data-ferb-ax` marker attribute, with a 16-character hex value.
  - `DOM.getDocument` (depth 0, the document node only), `DOM.querySelector`, `DOM.performSearch`,
    `DOM.getSearchResults`, `DOM.discardSearchResults`: find the one element FERB marked (`data-ferb-node`) for an
    upload.
  - `DOM.setFileInputFiles`: attaches the file the task names to that upload field.
  - `Fetch.enable` (download responses only), `Fetch.continueRequest`, `Fetch.failRequest`,
    `Fetch.takeResponseBodyAsStream`, `IO.read`, `IO.close`: save a download without a Save dialog; a held response is
    only read, never rewritten or redirected.

  `Runtime.evaluate` and every other script-evaluation command are not in the allowlist. The one script-evaluation
  command allowed, `Page.addScriptToEvaluateOnNewDocument`, is pinned to one bundled function.
- **nativeMessaging:** connects to the FERB app on the same Mac. This is the extension's only channel; it has no
  network backend.
- **alarms:** a periodic check that reconnects to the FERB app after the browser suspends the service worker.
- **storage:** keeps the pairing state and the list of tabs FERB is keeping open for a task.
- **Host permission `<all_urls>`:** tasks can involve any website the user asks about. The extension reads page
  content and acts (clicks, typing) only on tabs a FERB task opened or claimed, in profiles the user paired. In those
  profiles it also reports each tab's URL (secret-looking parameters removed) and title for the user's local activity
  history, and finds password-field positions in the visible tab before FERB screenshots the window.

**Remote code:** No, I am not using remote code. All scripts are in the package. The debugger allowlist has one
script-evaluation command, `Page.addScriptToEvaluateOnNewDocument`, and it accepts only one bundled function (FERB's
page hook, compared character for character); `Runtime.evaluate` and all other evaluation commands are refused.

**Data usage** (tick these; the extension handles them only to perform the user's tasks and to keep the user's tab
activity in the FERB app's activity history on their Mac):
- Personally identifiable information (it can appear on pages a task reads)
- Authentication information (passwords and codes are filled through the sealed path; never stored by the
  extension)
- Personal communications (when a task reads email or messages in the browser)
- Web history (URLs and titles of tabs a task uses, and, in paired profiles, of each tab that loads a page or becomes
  active, with secret-looking URL parameters removed; never incognito tabs)
- User activity (clicks and typing the extension performs for a task)
- Website content (page text, accessibility tree, screenshots of the task's tab; positions of password and code fields
  in the visible tab before a window screenshot)

**Certify all three:**
- I do not sell or transfer user data to third parties, outside of the approved use cases.
- I do not use or transfer user data for purposes that are unrelated to my item's single purpose.
- I do not use or transfer user data to determine creditworthiness or for lending purposes.

**Privacy policy URL:** https://ferb.kartikey.xyz/privacy (the `site/` folder, deployed on Vercel)

## Distribution

- **Visibility:** Public. Whether an unlisted item can be installed through the external-install request isn't
  documented, so Public is the safe choice.
- **Regions:** all.

## Test instructions for the reviewer (optional field)

The extension is idle without the FERB macOS app, which talks to it through native messaging (host
`com.ferb.native_host`). The source is in the FERB repository (`extensions/chromium/`); the repository becomes public
on 2026-10-21. The debugger allowlist is in `protocol.js` (`CDP_ALLOW`, 32 commands).

## After approval

1. Copy the item ID (32 letters a–p) from the dashboard.
2. Add it to `~/.ferb/config.toml`:

   ```toml
   [browser]
   store_extension_id = "<id>"
   ```

3. Run `ferb setup --update`. Setup writes the install request for each supported browser and registers the native
   host for both IDs.
4. Reopen the browsers setup lists. Turn FERB on in each profile when the browser offers it.
5. Pair each profile with `ferb browsers pair --browser <browser> "<profile>"`.
