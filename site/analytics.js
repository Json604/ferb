// The site's analytics (REL-435, REL-436). The build adds this script to the pages only when PostHog is set up, and
// it loads PostHog only after the visitor presses OK on the banner. Before an answer, after "No thanks", and whenever
// the browser sends Do Not Track or Global Privacy Control, nothing is loaded, stored or sent; the only thing kept is
// the answer itself, in this browser. The site's own server pages (/api/: sign-in, download, unsubscribe and the
// owner's list) carry no script at all.

export const CHOICE_KEY = "ferb-analytics";

// Query parameters that can carry a sign-in's or a link's secret, or a person; cut from every URL PostHog keeps.
export const PRIVATE_PARAMS = [
  "t",
  "token",
  "code",
  "state",
  "nonce",
  "ph",
  "email",
  "id_token",
  "access_token",
  "refresh_token",
  "authuser",
  "session_state",
];

export const STRINGS = {
  banner: "FERB uses analytics and session recordings to improve the site.",
  details: "Details",
  ok: "OK",
  no: "No thanks",
  on: "Analytics and session recordings are on in this browser.",
  off: "Analytics and session recordings are off in this browser.",
  undecided: "You haven't chosen yet.",
  refused: "Off: your browser asks sites not to track you (Do Not Track or Global Privacy Control).",
};

export function refusesTracking(nav = {}, win = {}) {
  const dnt = [nav.doNotTrack, nav.msDoNotTrack, win.doNotTrack].some((v) => v === "1" || v === "yes" || v === 1);
  return dnt || nav.globalPrivacyControl === true;
}

export const excluded = (pathname) => /^\/api(\/|$)/.test(pathname || "/");

export function readChoice(storage) {
  try {
    const v = storage?.getItem(CHOICE_KEY);
    return v === "yes" || v === "no" ? v : null;
  } catch {
    return null;
  }
}

function saveChoice(storage, value) {
  try {
    storage?.setItem(CHOICE_KEY, value);
  } catch {
    // No storage (a private window, blocked site data): the answer holds for this page only.
  }
}

export function cleanUrl(value) {
  if (typeof value !== "string" || !/^https?:\/\//i.test(value)) return value;
  let url;
  try {
    url = new URL(value);
  } catch {
    return value;
  }
  let changed = false;
  for (const name of [...url.searchParams.keys()]) {
    if (PRIVATE_PARAMS.includes(name.toLowerCase())) {
      url.searchParams.delete(name);
      changed = true;
    }
  }
  if (url.hash.includes("=")) {
    url.hash = "";
    changed = true;
  }
  return changed ? url.toString() : value;
}

// PostHog's before_send: every URL in an event's properties, and the page address a recording starts with, loses
// its private parameters.
export function cleanEvent(event) {
  if (!event) return event;
  for (const bag of [event.properties, event.$set, event.$set_once]) {
    if (!bag || typeof bag !== "object") continue;
    for (const [k, v] of Object.entries(bag)) bag[k] = cleanUrl(v);
  }
  const snapshots = event.properties?.$snapshot_data;
  if (Array.isArray(snapshots)) {
    for (const s of snapshots) if (s?.data && typeof s.data.href === "string") s.data.href = cleanUrl(s.data.href);
  }
  return event;
}

// The pinned bundle already holds every part PostHog needs, so the only script PostHog may fetch from its servers is
// its toolbar, which opens only when the owner launches it from his PostHog project (heatmaps on the live page).
// Its remote settings then come as data (JSON), never as code.
export function allowExternalScript(script) {
  let path = "";
  try {
    path = new URL(script?.src || "").pathname;
  } catch {
    path = "";
  }
  return /\/toolbar\.js$/.test(path) ? script : null;
}

// What PostHog does once the visitor has said OK.
export function settings({ host }) {
  return {
    api_host: host,
    defaults: "2026-08-30",
    persistence: "localStorage+cookie",
    // The cookie stays on this site's own address, never the parent domain's other sites.
    cross_subdomain_cookie: false,
    secure_cookie: true,
    person_profiles: "identified_only",
    respect_dnt: true,
    capture_pageview: true,
    capture_pageleave: true,
    autocapture: true,
    rageclick: true,
    capture_dead_clicks: true,
    enable_heatmaps: true,
    capture_performance: { web_vitals: true, network_timing: true },
    capture_exceptions: true,
    disable_session_recording: false,
    // The privacy page doesn't list the page's console, so recordings never carry it.
    enable_recording_console_log: false,
    session_recording: {
      maskAllInputs: true,
      maskInputOptions: { password: true, email: true },
      // Text a page marks data-private (none does yet) is masked like an input.
      maskTextSelector: "[data-private]",
      recordHeaders: false,
      recordBody: false,
    },
    mask_personal_data_properties: true,
    custom_personal_data_properties: PRIVATE_PARAMS,
    before_send: cleanEvent,
    prepare_external_dependency_script: allowExternalScript,
  };
}

function el(doc, tag, props = {}, children = []) {
  const node = doc.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === "text") node.textContent = v;
    else node.setAttribute(k, v);
  }
  for (const child of children) node.appendChild(child);
  return node;
}

export function start(doc, win) {
  const tag = doc.querySelector("script[data-posthog-key]");
  if (!tag || excluded(win.location?.pathname)) return null;
  let storage = null;
  try {
    storage = win.localStorage;
  } catch {
    storage = null;
  }
  const s = {
    doc,
    win,
    storage,
    banner: null,
    cfg: {
      key: tag.getAttribute("data-posthog-key"),
      host: tag.getAttribute("data-posthog-host"),
      lib: tag.getAttribute("data-posthog-lib"),
      integrity: tag.getAttribute("data-posthog-integrity"),
    },
    refused: refusesTracking(win.navigator, win),
  };
  s.choice = s.refused ? "no" : readChoice(storage);

  // The privacy page's "Change your choice" opens the banner again.
  for (const button of doc.querySelectorAll("[data-analytics-choice]")) {
    button.hidden = s.refused;
    button.addEventListener("click", () => showBanner(s));
  }
  showState(s);

  // "Continue with Google" is the download's first step: a named event, sent as the page leaves for Google.
  doc.addEventListener("click", (event) => {
    const link = event.target?.closest?.('a[href^="/api/auth/google"]');
    const ph = s.win.posthog;
    if (!link || s.choice !== "yes" || !ph?.__loaded) return;
    const file = new URL(link.getAttribute("href"), "https://site.invalid").searchParams.get("file");
    ph.capture("continue with google clicked", { file }, { transport: "sendBeacon" });
  });

  if (s.choice === "yes") load(s);
  else if (s.choice === null) showBanner(s);
  return s;
}

function showState(s) {
  const text = s.refused ? STRINGS.refused : s.choice === "yes" ? STRINGS.on : s.choice === "no" ? STRINGS.off : STRINGS.undecided;
  for (const node of s.doc.querySelectorAll("[data-analytics-state]")) node.textContent = text;
  for (const node of s.doc.querySelectorAll("[data-analytics-controls]")) node.hidden = false;
}

function showBanner(s) {
  if (s.refused || s.banner) return;
  const ok = el(s.doc, "button", { type: "button", text: STRINGS.ok });
  const no = el(s.doc, "button", { type: "button", text: STRINGS.no });
  const details = el(s.doc, "a", { href: "/privacy#analytics", text: STRINGS.details });
  const text = el(s.doc, "p", {}, [s.doc.createTextNode(STRINGS.banner + " "), details]);
  s.banner = el(s.doc, "div", { class: "consent", role: "region", "aria-label": "Analytics" }, [
    text,
    el(s.doc, "div", { class: "consent-actions" }, [ok, no]),
  ]);
  ok.addEventListener("click", () => decide(s, "yes"));
  no.addEventListener("click", () => decide(s, "no"));
  s.doc.body.appendChild(s.banner);
}

function decide(s, choice) {
  s.choice = choice;
  saveChoice(s.storage, choice);
  s.banner?.remove();
  s.banner = null;
  showState(s);
  const ph = s.win.posthog;
  if (choice === "yes") {
    if (!ph?.__loaded) return load(s);
    ph.set_config({ disable_persistence: false });
    ph.opt_in_capturing();
  } else if (ph?.__loaded) {
    // Stop capture and recording, and remove PostHog's cookie and stored identifier from this browser (turning its
    // storage off deletes the cookie and localStorage; the tab's own window ids go here). PostHog keeps only its
    // opt-out mark.
    ph.stopSessionRecording?.();
    ph.opt_out_capturing();
    ph.set_config({ disable_persistence: true });
    try {
      const session = s.win.sessionStorage;
      for (const k of Object.keys(session)) if (k.startsWith("ph_")) session.removeItem(k);
    } catch {
      // No session storage to clear.
    }
  }
}

function load(s) {
  if (s.loading) return;
  s.loading = true;
  const script = s.doc.createElement("script");
  script.src = s.cfg.lib;
  script.integrity = s.cfg.integrity;
  script.crossOrigin = "anonymous";
  script.async = true;
  script.addEventListener("load", () => {
    const ph = s.win.posthog;
    if (!ph?.init) return;
    ph.init(s.cfg.key, {
      ...settings(s.cfg),
      loaded: (instance) => {
        // An earlier "No thanks" left PostHog's own opt-out mark; this OK replaces it.
        if (instance.has_opted_out_capturing()) instance.opt_in_capturing();
      },
    });
  });
  s.doc.head.appendChild(script);
}

if (typeof document !== "undefined" && typeof window !== "undefined") start(document, window);
