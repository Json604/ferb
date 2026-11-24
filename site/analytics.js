// The site's analytics (REL-435, REL-438, REL-439). The build adds this script to the pages only when PostHog is set
// up. Every visit is counted anonymously: PostHog in cookieless mode, nothing stored in the browser, no person, no
// recording, and only page views, page leaves and Continue with Google clicks (ANONYMOUS_EVENTS). That is all there
// is before the visitor answers the bar, after "No thanks", and whenever the browser sends Do Not Track or Global
// Privacy Control (no bar then). After OK, PostHog runs in full: its cookie and storage, autocapture, heatmaps, web
// vitals, errors and session recordings. The only thing the site itself keeps is the answer, in this browser. The
// site's server pages (/api/: sign-in, download, unsubscribe and the owner's list) carry no script at all.

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

// What an anonymous visit sends; every other event (clicks, heatmaps, vitals, errors, recordings) waits for OK.
export const ANONYMOUS_EVENTS = ["$pageview", "$pageleave", "continue with google clicked"];

export const STRINGS = {
  banner: "FERB counts visits anonymously. With your OK it also uses analytics and session recordings to improve the site.",
  details: "Details",
  ok: "OK",
  no: "No thanks",
  on: "Analytics and session recordings are on in this browser.",
  off: "Only the anonymous visit count is on in this browser.",
  undecided: "You haven't chosen yet, so only the anonymous visit count is on.",
  refused: "Only the anonymous visit count is on: your browser asks sites not to track you (Do Not Track or Global Privacy Control).",
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

// Without the visitor's OK only ANONYMOUS_EVENTS leave the page, and none of them makes a person.
export function anonymousOnly(event) {
  if (!event || !ANONYMOUS_EVENTS.includes(event.event)) return null;
  event.properties = { ...event.properties, $process_person_profile: false };
  delete event.$set;
  delete event.$set_once;
  return event;
}

// How PostHog runs. Cookieless mode "on_reject" with opt_out_capturing_by_default: until PostHog is told the visitor
// agreed (opt_in_capturing), it counts anonymously, hashing on its servers and storing nothing here; after it, it runs
// in full. `full()` says whether the visitor has said OK (and the browser doesn't refuse tracking).
export function settings({ host }, full = () => false) {
  return {
    api_host: host,
    defaults: "2026-08-30",
    cookieless_mode: "on_reject",
    opt_out_capturing_by_default: true,
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
    before_send: (event) => cleanEvent(full() ? event : anonymousOnly(event)),
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
    if (!link || !ph?.__loaded) return;
    const file = new URL(link.getAttribute("href"), "https://site.invalid").searchParams.get("file");
    ph.capture("continue with google clicked", { file }, { transport: "sendBeacon" });
  });

  load(s);
  if (s.choice === null) showBanner(s);
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
  // PostHog not started yet: its `loaded` callback reads the answer.
  if (s.win.posthog?.__loaded) follow(s, s.win.posthog);
}

const full = (s) => s.choice === "yes" && !s.refused;

// Puts PostHog in the mode the answer asks for.
function follow(s, ph) {
  const granted = ph.get_explicit_consent_status() === "granted";
  if (full(s) && !granted) ph.opt_in_capturing();
  if (!full(s) && granted) {
    // Back to the anonymous count: recording stops, and PostHog's cookie, storage and identifier go. Clearing the
    // answer it kept leaves it where a visitor who never answered is, which counts as a refusal here.
    ph.opt_out_capturing();
    ph.clear_opt_in_out_capturing();
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
    ph.init(s.cfg.key, { ...settings(s.cfg, () => full(s)), loaded: (instance) => follow(s, instance) });
  });
  s.doc.head.appendChild(script);
}

if (typeof document !== "undefined" && typeof window !== "undefined") start(document, window);
