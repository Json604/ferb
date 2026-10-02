// FERB watch probe: no logic. `ferb watch` drives chrome.* calls through this worker over the debugging pipe.
self.detaches = [];
chrome.debugger.onDetach.addListener((src, reason) => self.detaches.push({ tabId: src.tabId, reason, t: Date.now() }));
