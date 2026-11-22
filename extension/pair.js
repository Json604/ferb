// Owns explicit setup token delivery to this profile; must not select a profile or reveal the token.
// FERB pairing page: reads the one-time token from its own URL fragment, hands it to this profile's service worker,
// and closes itself. Only the extension in this profile can open or read this page.
// HAND-680 (0.19.14): opened without a token (typed by hand, from history), it closes nothing: as a window's only tab
// a close took the whole window. It says what it is instead.
const token = new URLSearchParams(location.hash.slice(1)).get("token");
history.replaceState(null, "", location.pathname);          // drop the token from the URL right away
if (token) chrome.runtime.sendMessage({ pairToken: token }, () => window.close());
else document.addEventListener("DOMContentLoaded", () => {
  const p = document.createElement("p");
  p.textContent = "This is FERB's pairing page. FERB opens it by itself when it pairs this profile, so there's " +
    "nothing to do here. You can close this tab.";
  p.style.cssText = "font: 15px/1.5 -apple-system, system-ui, sans-serif; max-width: 34em; margin: 64px auto; " +
    "padding: 0 16px; color: #333";
  document.body.append(p);
});
