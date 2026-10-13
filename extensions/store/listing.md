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

**Official URL / Homepage:** https://kartikey.xyz/ferb (or the GitHub repository once it's public on Oct 21)

**Support URL:** the GitHub repository's Issues page (once public), or a contact page on kartikey.xyz

## Privacy practices tab

**Single purpose:**
Lets the FERB assistant app installed on the user's Mac see and operate web pages, in the browser profiles the user
pairs with it, to carry out tasks the user asks for.

**Permission justifications:**

- **tabs:** FERB lists, opens, switches and closes the tabs a task needs, and reads their URL and title to know where
  it is.
- **scripting:** injects FERB's bundled content script into the task's page, to read the page and to click, type and
  scroll on it.
- **debugger:** uses a fixed allowlist of DevTools Protocol commands, enforced inside the extension:
  - trusted input events (`Input.dispatchMouseEvent`, `Input.dispatchKeyEvent`, `Input.insertText`), which many sites
    require instead of synthetic DOM events;
  - the accessibility tree (`Accessibility.getFullAXTree`);
  - tab screenshots (`Page.captureScreenshot`);
  - downloads without a Save dialog (`Fetch.*`, `IO.*`);
  - file uploads (`DOM.setFileInputFiles`) and JavaScript dialogs (`Page.handleJavaScriptDialog`).

  No arbitrary script evaluation is allowed (`Runtime.evaluate` is not in the allowlist). The only injected script is
  one bundled in the package.
- **nativeMessaging:** connects to the FERB app on the same Mac. This is the extension's only channel; it has no
  network backend.
- **alarms:** a periodic check that reconnects to the FERB app after the browser suspends the service worker.
- **storage:** keeps the pairing state and the list of tabs FERB is keeping open for a task.
- **Host permission `<all_urls>`:** tasks can involve any website the user asks about. The extension acts only on
  tabs a FERB task opened or claimed, in profiles the user paired.

**Remote code:** No, I am not using remote code. All scripts are in the package; the debugger allowlist has no
script-evaluation command.

**Data usage** (tick these; the extension handles them only to perform the user's task):
- Personally identifiable information (it can appear on pages a task reads)
- Authentication information (passwords and codes are filled through the sealed path; never stored by the
  extension)
- Personal communications (when a task reads email or messages in the browser)
- Web history (URLs and titles of tabs a task uses)
- User activity (clicks and typing the extension performs for a task)
- Website content (page text, accessibility tree, screenshots of the task's tab)

**Certify all three:**
- I do not sell or transfer user data to third parties, outside of the approved use cases.
- I do not use or transfer user data for purposes that are unrelated to my item's single purpose.
- I do not use or transfer user data to determine creditworthiness or for lending purposes.

**Privacy policy URL:** host `privacy.html` at, for example, https://kartikey.xyz/ferb/privacy

## Distribution

- **Visibility:** Public. Whether an unlisted item can be installed through the external-install request isn't
  documented, so Public is the safe choice.
- **Regions:** all.

## Test instructions for the reviewer (optional field)

The extension is idle without the FERB macOS app, which talks to it through native messaging (host
`com.ferb.native_host`). The source is in the FERB repository (`extensions/chromium/`); the repository becomes public
on 2026-10-21. The debugger allowlist is in `protocol.js` (`CDP_ALLOWED`).

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
