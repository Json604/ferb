"""fill_secret: write a FERB secret into one element of one frame, only if that frame's origin is allowed.

Two checks: (1) the frame's securityOrigin from CDP before revealing the secret; (2) `location.origin`
inside the same JS call that sets the value, in an isolated world (page scripts can't tamper with it).
The value is set on that exact element, not typed into "whatever has focus", so focus-stealing or a
navigation between check and write cannot redirect the secret. Results and errors never contain it.
"""

from dataclasses import dataclass
from urllib.parse import urlsplit

from cdp_pipe import CdpError, Page
from secrets_kc import KeychainSecretStore, SecretRef


@dataclass(frozen=True)
class FillTarget:
    frame_id: str
    selector: str


@dataclass(frozen=True)
class FillResult:
    ok: bool
    reason: str
    audit_key: str  # ref.key only — the one thing recorded about the secret


_SET_IF_ORIGIN = """
function(expectedOrigin, selector, value) {
  if (location.origin !== expectedOrigin) return "origin_changed";
  const el = document.querySelector(selector);
  if (!el) return "no_element";
  if (!(el instanceof HTMLInputElement)) return "not_input";
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(el, value);
  el.dispatchEvent(new Event("input", {bubbles: true}));
  el.dispatchEvent(new Event("change", {bubbles: true}));
  return "ok";
}
"""


def normalize_origin(origin: str) -> str:
    u = urlsplit(origin)
    port = u.port
    default = {"http": 80, "https": 443}.get(u.scheme)
    host = u.hostname or ""
    return f"{u.scheme}://{host}" + (f":{port}" if port and port != default else "")


def fill_secret(page: Page, target: FillTarget, ref: SecretRef, store: KeychainSecretStore) -> FillResult:
    def refuse(reason: str) -> FillResult:
        return FillResult(False, reason, ref.key)

    if not store.has(ref):
        return refuse("unknown_secret")
    allowed = {normalize_origin(o) for o in store.spec(ref).allowed_origins}

    frame = page.frames().get(target.frame_id)
    if frame is None:
        return refuse("no_frame")
    session, info = frame
    origin = normalize_origin(info.get("securityOrigin", ""))
    if origin not in allowed:
        return refuse("origin_not_allowed")

    try:
        ctx = page.send("Page.createIsolatedWorld",
                        {"frameId": target.frame_id, "worldName": "ferb-fill"}, session)["executionContextId"]
        win = page.send("Runtime.evaluate", {"expression": "window", "contextId": ctx}, session)
        r = page.send("Runtime.callFunctionOn", {
            "functionDeclaration": _SET_IF_ORIGIN,
            "objectId": win["result"]["objectId"],
            "arguments": [{"value": origin}, {"value": target.selector}, {"value": store._reveal(ref)}],
            "returnByValue": True,
        }, session)
    except (CdpError, KeyError):
        return refuse("fill_failed")  # CdpError text never includes our arguments
    status = r.get("result", {}).get("value")
    return FillResult(status == "ok", status or "fill_failed", ref.key)
