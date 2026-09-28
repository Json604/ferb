// Bare functional minimum (CLAUDE.md): hand the one-time token to the background, then close.
const token = new URLSearchParams(location.hash.slice(1)).get("token");
chrome.runtime.sendMessage({ pairToken: token }).finally(() => window.close());
