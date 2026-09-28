// FERB pairing page (9.6): blank, reads the one-time token from its own URL fragment, hands it to this profile's
// service worker, and closes itself. Only the extension in this profile can open or read this page.
const token = new URLSearchParams(location.hash.slice(1)).get("token");
history.replaceState(null, "", location.pathname);          // drop the token from the URL right away
if (token) chrome.runtime.sendMessage({ pairToken: token }, () => window.close());
else window.close();
