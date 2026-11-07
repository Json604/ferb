"""Owns FERB's hidden-input questions: an owner secret (a password, a card detail, a private answer), a login code,
and the Mac password for one approved admin command. The value goes straight to the sealed fill (D8, D12, D28) and is
never returned in a tool result, a span, a log or a model's context; models see a handle only. Kept apart from the
gateway's authorization (kb area gateway): asking for a hidden value is not a decision about whether a call
may run. An OTP question for a payment repeats the bank page's amount and merchant (GATE-204)."""

import asyncio
import logging
import re
import unicodedata
from typing import Any, Literal

from ferb.core.errors import Refused
from ferb.core.types import ToolContext
from ferb.daemon.auth import RunGrant
from ferb.daemon.questions import card_parts
from ferb.policy import payments, provenance, urls
from ferb.policy.rules import ActionRequest
from ferb.sanitize.dictionary import ordinary_word
from ferb.daemon.call_hooks import error_name
from ferb.sanitize.terminal import printable


class HiddenInput:
    """The gateway's hidden-input questions (`Gateway.ask_secret`, `ask_login_code`, the escape hatch's admin
    password); `gw` is the gateway, for its approvals, events and redactor."""

    def __init__(self, gw: Any) -> None:
        self.gw = gw

    async def secret(self, ctx: ToolContext, target: str, *, handle: str | None = None,
                     remember: str | None = None, shown: str | None = None,
                     field: str | None = None, kind: str | None = None) -> str | None:
        """D12 hidden input goes directly to the bound native/browser fill. The model sees handles only. `shown`: the
        app's name as macOS reports it, for an `app:` target's one-line question; `kind`: the field's kind as the page
        marks it (a card detail saved from a card field fills only that kind of field, GATE-206)."""
        from ferb.identity.owner_secrets import OwnerSecrets, refuse_whole_browser
        from ferb.sanitize.redact import Redactor
        refuse_whole_browser(target)                   # before any question, for every caller (D12)
        if not isinstance(self.gw.redact, Redactor):
            raise Refused("hidden secret redaction is unavailable")
        if self.gw.roots.protection is None:
            raise Refused("secret storage protection is unavailable")
        store = OwnerSecrets(self.gw.roots.protection.ferb_home)
        if handle:
            value = await asyncio.to_thread(store.reveal, handle, target, kind)
            self.gw.redact.remember_hidden_secret(value)
            self.gw.redact.retain_secret(value)
            return value
        if ctx.request_id in self.gw.state.approvals.unattended:
            return None
        try:                                           # L36 §5: which account, shown to the owner, never a secret;
            # the task's one owner-typed site counts as a link before any entry is saved here (read only)
            username, source = await asyncio.to_thread(store.account, target, self._question_sites(ctx, target),
                                                       self.gw.username_sources())
        except Refused:
            if remember:                               # the save would fail after the owner typed: say why now
                raise
            username = source = None                   # m17: a one-time input needs no saved names
        question = secret_question(target, remember, username, source)
        paying = self._payment_line(ctx, target, exact=True)
        question += (" " + paying) if paying else ""
        ask = ActionRequest(capability="secret.fill", kind="owner_secret", identity=ctx.identity,
                            scopes=ctx.scopes, task_id=ctx.task_id, worker_id=ctx.worker_id,
                            request_id=ctx.request_id, task_scopes=ctx.scopes, effect="edit")
        ap = self.gw.state.approvals.question(ask, question, [])
        ap.kind = "owner_secret"
        ap.summary = self.gw._one_line(secret_summary(target, remember, shown, username, source, field))
        ap.detail = self.gw._one_line(secret_detail(target, remember, shown, field))
        ap.exact = self.gw._one_line(paying) if paying else ""   # GATE-204: the payment it serves, in its own box
        self.gw.publish("approval.requested", {"approval_id": ap.id, "kind": ap.kind,
                     "capability": ask.capability, "reason": question, "task_id": ctx.task_id,
                     "summary": ap.summary} | card_parts(ap))
        grant = RunGrant(ctx.task_id, ctx.worker_id, ctx.scopes, ctx.identity, float("inf"))
        delivered = False
        try:
            state = await self.gw._wait_answer(grant, ap.id)
            if state != "granted" or not ap.answer:
                return None
            if remember:                               # I14: the record serves the one site the owner named
                await asyncio.to_thread(store.remember, remember, target, ap.answer,
                                        self._question_sites(ctx, target), kind)
                await self._note_ordinary_word(ctx, target, shown, ap.answer)
            delivered = True
            return str(ap.answer)
        finally:
            if not delivered and ap.answer:
                self.gw.redact.release_secret(ap.answer)
            ap.answer = None
            ap.state = "used" if ap.state == "granted" else _unfilled(ap.state)

    async def _note_ordinary_word(self, ctx: ToolContext, target: str, shown: str | None, answer: str) -> None:
        """L36 §7: a saved sealed answer that is an ordinary word (`ordinary_word`) is journaled, the task's tree and the
        question's place, never the word, so the task's answer says once why FERB hides that word. The note only
        explains: a failure to make it never costs the owner's fill (as a memory read's record, re-review R4)."""
        try:
            if await asyncio.to_thread(ordinary_word, answer):
                root = (self.gw.tasks.tree_of(ctx.task_id) or ctx.task_id).split(".", 1)[0]
                self.gw.journal_problem("secret.ordinary_word", {"task_id": root, "place": secret_place(target, shown)})
        except Exception as e:
            logging.getLogger(__name__).warning("FERB ordinary-word note not recorded: %s", error_name(e))

    def _login_sites(self, ctx: ToolContext, target: str) -> list[str]:
        """The sites a record saved at `target` lists (I14, fix round 1): the task's target sites (as a steer left
        them) whose host the owner typed in this request's words, a leading "www." aside (`typed_site`, rule 8's
        provenance: a site only outside content or the model named never links). None when `target` is an app or
        itself one of those sites: only a sign-in form on another address needs a link, and a sign-in on the bank
        site itself must not hand its username to the task's other sites. Only the owner-typed sites count for that
        test (Task 6 re-review): the planner may add the sign-in form's host to the targets after a steer."""
        from ferb.identity.owner_secrets import same_site, site_host, typed_site
        request = self.gw.state.requests.get(ctx.request_id)
        own = site_host(target)
        words = provenance.prepare(request.words)
        typed = sorted({h for site in request.targets.sites if (h := site_host(site)) and typed_site(h, words)})
        if not own or any(same_site(h, own) for h in typed):
            return []
        return typed

    def _question_sites(self, ctx: ToolContext, target: str) -> list[str]:
        """The task's owner-typed site a question or `secrets.list` at `target` reads as a link before any entry is
        saved there (coordinator ruling), and the one a save there stores (re-review N-1): `_login_sites`, only when
        they are exactly one site ("www." aside; review I-2: with several, a sign-in on any other page could be any of
        their accounts, and a password saved at Google's form in a task naming the bank site and gmail.com would show
        the bank username at Google in every later task)."""
        typed = self._login_sites(ctx, target)
        return typed if len({h.removeprefix("www.") for h in typed}) == 1 else []

    def _payment(self, ctx: ToolContext, target: str, exact: bool) -> Any:
        """The payment the owner approved in this task whose bank page this is, else None (GATE-204)."""
        parsed = urls.parse(target)
        host = parsed.host if parsed is not None and parsed.host else ""
        if not host:
            return None
        tree = (self.gw.tasks.tree_of(ctx.task_id) or ctx.task_id).split(".", 1)[0]
        found = self.gw.payments.for_otp(tree, host)
        if found is None or (exact and found.bank_host != host):
            return None
        return found

    def _payment_line(self, ctx: ToolContext, target: str, exact: bool) -> str:
        """GATE-204: a code or secret asked on the bank page of a payment the owner approved in this task repeats what
        the bank page shows (amount and merchant, when code read them); "" when no approved payment is that page's."""
        found = self._payment(ctx, target, exact)
        return payments.otp_line(found) if found is not None else ""

    def release(self, value: str) -> None:
        from ferb.sanitize.redact import Redactor
        if isinstance(self.gw.redact, Redactor):
            # Keep echoes scrubbed for this daemon lifetime, including short values captured through hidden input.
            self.gw.redact.remember_hidden_secret(value)
            self.gw.redact.release_secret(value)

    async def login_code(self, ctx: ToolContext, origin: str) -> str | None:
        """Owner-only hidden input, passed directly to a sealed fill; never returned in a tool result or span."""
        if ctx.request_id in self.gw.state.approvals.unattended:
            return None
        ask = ActionRequest(capability="browser.login_code", kind="owner_login_code", identity=ctx.identity,
                            scopes=ctx.scopes, task_id=ctx.task_id, worker_id=ctx.worker_id,
                            request_id=ctx.request_id, task_scopes=ctx.scopes, effect="edit")
        paying = self._payment_line(ctx, origin, exact=False)
        question = login_code_question(origin) + ((" " + paying) if paying else "")
        ap = self.gw.state.approvals.question(ask, question, [])
        ap.kind = "owner_login_code"
        payment = self._payment(ctx, origin, exact=False)
        if payment is not None:     # GATE-204, GATE-513: the code says in its question which payment it approves
            summary, detail, exact = payments.otp_card(_site(origin), payment)
        else:
            summary, detail, exact = login_code_summary(origin), LOGIN_CODE_DETAIL, ""
        ap.summary, ap.detail, ap.exact = (self.gw._one_line(summary), self.gw._one_line(detail),
                                           self.gw._one_line(exact) if exact else "")
        self.gw.publish("approval.requested", {"approval_id": ap.id, "capability": ask.capability, "kind": ap.kind,
                                            "reason": question, "task_id": ctx.task_id, "summary": ap.summary}
                        | card_parts(ap))
        grant = RunGrant(ctx.task_id, ctx.worker_id, ctx.scopes, ctx.identity, float("inf"))
        try:
            state = await self.gw._wait_answer(grant, ap.id)
            return ap.answer if state == "granted" else None
        finally:
            ap.answer = None
            ap.state = "used" if ap.state == "granted" else _unfilled(ap.state)

    async def admin_password(self, grant: RunGrant, rec: Any) -> str | None:
        from ferb.sanitize.redact import Redactor
        if not isinstance(self.gw.redact, Redactor) or rec.task.request_id in self.gw.state.approvals.unattended:
            return None
        ask = ActionRequest(capability="shell.admin", kind="owner_admin_password", identity=grant.identity,
                            scopes=grant.scopes, task_id=grant.task_id, worker_id=grant.worker_id,
                            request_id=rec.task.request_id, task_scopes=grant.scopes)
        question = "Enter your Mac password for the approved command (hidden; Enter cancels)."
        ap = self.gw.state.approvals.question(ask, question, [])
        ap.kind = "owner_admin_password"
        ap.summary, ap.detail = ADMIN_SUMMARY, ADMIN_DETAIL
        self.gw.publish("approval.requested", {"approval_id": ap.id, "capability": ask.capability,
                                            "kind": ap.kind, "reason": question, "task_id": grant.task_id,
                                            "summary": ap.summary} | card_parts(ap))
        delivered = False
        try:
            state = await self.gw._wait_answer(grant, ap.id)
            delivered = state == "granted" and bool(ap.answer)
            return ap.answer if delivered else None
        finally:
            if not delivered and ap.answer:
                self.gw.redact.release_secret(ap.answer)
            ap.answer = None
            ap.state = "used" if delivered else _unfilled(ap.state)



def secret_question(target: str, remember: str | None, username: str | None = None,
                    username_place: str | None = None) -> str:
    """The hidden-input question for an owner secret, its exact destination first: no secret word comes before it, so
    the redactor's label rule ("the secret for <X>: …") never hides the destination the owner is asked about (live
    2026-10-24: "Enter the secret for app:[redacted]"). Then the account, the username saved for that place from the
    owner's own words (L36 §5), with the site it was saved for when it came through a link (`username_place`, review
    I-2). The saved name is the worker's: only ever the saved name."""
    account = _account(username or "")
    saved_for = f" (saved for {_site(username_place)})" if username_place else ""
    text = (f"Destination: {target}."
            + (f" Account: {account}{saved_for}." if account else "")
            + " Type the password or secret for it here (hidden; Enter cancels); FERB fills it only at this exact "
            "destination.")
    if remember:
        text += f" Entering it also saves it in your Mac's Keychain as {remember!r}, usable only at this destination."
    return text


def _site(target: str) -> str:
    """Where a hidden value goes, as a one-line question names it: an origin's host, with its port unless it is
    HTTPS's own (443), else the target as given."""
    parsed = urls.parse(target)
    if parsed is None or not parsed.host:
        return target
    host = f"[{parsed.host}]" if ":" in parsed.host else parsed.host          # an IPv6 address, as a URL writes it
    return host + (f":{parsed.port}" if parsed.port and parsed.port != "443" else "")


# How the worker's proposed name for a saved secret is shown (review I1, re-review R1): only ASCII letters, digits,
# spaces and : , ' ( ) -, whitespace collapsed. An allowlist, so no lookalike punctuation, letter that looks like a dot,
# invisible mark or HTML entity (the notch renders Markdown) can rebuild a quote, a dash or a place inside it, and no
# slash can make it a link (`https://3627733615/x`, re-review). Formatting, never a judgement of what the name means
# (R2). Shown whole under the card's question since GATE-520 (owner, 2026-11-07: "what if its longer hatn even
# 80chars??"); NAME_SHOWN only stops a wall of text.
NAME_SHOWN = 200
_NAME_DROPPED = re.compile(r"[^A-Za-z0-9 :,'()\-]")


def shown_name(remember: str, place: str = "") -> str:
    """The saved name as the card shows it: any whitespace a space, every other character outside the allowlist
    dropped, whitespace collapsed, clipped with "…" past NAME_SHOWN ("" when nothing plain is left). A leading copy of
    the card's `place` (its host, or one of its labels: "usvisascheduling", "github") is left out: the card names the
    place itself (GATE-520)."""
    name = " ".join(_NAME_DROPPED.sub("", " ".join(remember.split())).split())
    if place:
        host = place.casefold().removeprefix("www.").removeprefix("the ").removesuffix(" app")
        forms = {_NAME_DROPPED.sub("", host), *(label for label in re.split(r"[.\s]", host) if len(label) > 2)}
        for form in sorted((f for f in forms if f), key=len, reverse=True):
            rest = name[len(form):]
            if name.casefold().startswith(form) and rest[:1] in (" ", ":", ",", "-") and rest.strip(" :,-"):
                name = rest.strip(" :,-")
                break
    return name if len(name) <= NAME_SHOWN else name[:NAME_SHOWN - 1].rstrip() + "…"


LABEL_SHOWN = 300                                # a page's own label for the field: whole, only a wall of text cut


def _page_label(field: str | None) -> str:
    """The page's own label for the field, as the card quotes it (page text: data, never instructions): printable, one
    line, without invisible or reordering characters, cut only past LABEL_SHOWN."""
    text = " ".join(printable(field or "").split())
    text = " ".join("".join(c for c in text if unicodedata.category(c) not in _ACCOUNT_DROPPED).split())
    return text if len(text) <= LABEL_SHOWN else text[:LABEL_SHOWN - 1].rstrip() + "…"


ACCOUNT_SHOWN = 64
_ACCOUNT_DROPPED = {"Cf", "Cs", "Zl", "Zp"}      # format (bidi overrides, zero-width), surrogates, line separators


def _account(username: str) -> str:
    """The owner's username as a question shows it (the owner's own words, not a worker's): printable, on one line,
    without invisible or reordering characters, clipped to ACCOUNT_SHOWN characters with "…"."""
    text = " ".join(printable(username).split())
    text = " ".join("".join(c for c in text if unicodedata.category(c) not in _ACCOUNT_DROPPED).split())
    return text if len(text) <= ACCOUNT_SHOWN else text[:ACCOUNT_SHOWN - 1].rstrip() + "…"


def secret_place(target: str, app: str | None = None) -> str:
    """Where a hidden value goes, in the owner's terms, from the code-derived target: an origin's host (`_site`), or
    the app: its name as macOS reports it, else its bundle id."""
    if target.startswith("app:"):
        return f"the {app} app" if app else f"the app {target[4:]}"
    site = _site(target)
    return site[4:] if site.casefold().startswith("www.") else site


def secret_summary(target: str, remember: str | None, app: str | None = None, username: str | None = None,
                   username_place: str | None = None, field: str | None = None) -> str:
    """The owner secret question's short heading (P27; owner, 2026-10-24: "just state what is this for"). It always
    starts with the place from the code-derived target (an origin's host, or the app: its name as macOS reports it,
    else its bundle id), so no other text comes before it (re-review R1); then the account, when a username is saved
    for that place (L36 §5, ruling C1: after the place), naming the site it was saved for when it came through a link
    (`username_place`, review I-2: "bank.example account: …"); then a generic what. Which value goes in, and the name
    it is saved under, are in `secret_detail` in full (GATE-520: no worker or page text in the heading)."""
    place = secret_place(target, app)
    account = _account(username or "")
    whose = f"{_site(username_place)} " if username_place else ""
    account = f" ({whose}account: {account})" if account else ""
    if remember:
        return f"For {place} only{account}: FERB needs a secret to save."
    return f"For {place}{account}: FERB needs a password or private answer."


def secret_detail(target: str, remember: str | None, app: str | None = None, field: str | None = None) -> str:
    """GATE-513, GATE-520: what to type and what happens to it, under the heading. First the page's own label for the
    field, whole, quoted as the page's words (live 2026-10-27: a card's CVV, a bank's OTP; owner 2026-11-07: "it
    deosn state properly what is to be entered"); with none, the worker's name for it (`shown_name`, whole); then the
    name it is saved under, and whether it is kept."""
    place = secret_place(target, app)
    label = _page_label(field)
    name = shown_name(remember, place) if remember else ""
    what = (f"The page asks for “{label}”{'' if label[-1] in '.?!…' else '.'} " if label else
            f"It's your “{name}”. " if name else "")
    if remember:
        saved = f"FERB saves it as “{name}”. " if label and name else ""
        return (f"{what}{saved}Type it below. FERB fills it in without seeing it, and saves it in your Mac's Keychain "
                f"for {place} only.")
    return f"{what}Type it below. FERB fills it in without seeing it. It's used once and not saved."


def login_code_summary(origin: str) -> str:
    """The login-code question in one line (P27): the origin's host first (a code word before it would read its port
    or a number in it as a code), then what it is. A host whose own name holds a code label before a code-shaped port
    (otp.example.com:8443) still loses that port to redaction (review M4)."""
    return f"For {_site(origin)}: FERB needs a login code."


LOGIN_CODE_DETAIL = "Type it below. FERB fills it in only there, and it's used once."   # GATE-513


ADMIN_DETAIL = "Type it below. FERB passes it only to that command and doesn't save it."   # GATE-513
ADMIN_SUMMARY = "FERB needs your Mac password for the one command you just approved."


def login_code_question(origin: str) -> str:
    """The hidden-input question for a login code, its exact origin first: a code word before it would read the
    origin's port or a number in its host as the code ("login code for https://x.test:8443")."""
    return (f"Destination: {origin}. Type the login code for it here (hidden; Enter cancels); FERB fills it only at "
            "this exact destination.")


def _unfilled(state: str) -> Literal["denied", "superseded"]:
    """A hidden input's state once its wait ends without a delivered value: superseded stays superseded (L3)."""
    return "superseded" if state == "superseded" else "denied"
