# Plan: the independent security audit (T-1098)

Run at the end, only when the owner says so (owner 2026-11-25: "security audit at the end when i say, not now"), by
an independent Fable agent with the live access he grants then (owner 2026-11-14: "a full device access security audit
by an independent fable agent ... i will give it my mac, and my moto and any other access it needs to do the live
security testing at the extreme").

## Scope (owner 2026-11-25: "security is the main thing anyways - covering this everywhere")
1. **The `ferb` program in every FERB.app** ("since every installed version of ferb on any mac will be able to run its
   cli commands, how safe is that???"). Known before the audit: of the daemon's 188 methods, 159 accept the local
   `cli` role and 88 of those are not FERB.app-only (code-signature checked), among them memory.add/edit/forget,
   settings.set, schedules.change, runtimes.use, browsers.pair/unpair, stop.lift and setup.permission_request. Any
   program running as the owner can call them through the bundled program or the socket. chat.send, chat.answer,
   chat.approve and chat.correct are FERB.app-only. The audit judges each method: what a same-user program gains
   through it beyond what it could already do, and which must become FERB.app-only or ask the owner.
2. **Screenshots and screen pictures**: what FERB captures, where it is stored and for how long, what reaches a model
   provider, the phones (Watch live, previews), screenshot blocking, redaction of what is on screen.
3. **Claims that aren't true** ("us claiming something that isn't happening"): every security and privacy statement
   FERB makes (FERB.app's words, the site and privacy page, the phones, README, SECURITY.md, kb decisions marked as
   built) checked against the code and a live run.
4. **Any major flaw anywhere**: the daemon's socket and roles, the phone link and pairing, the site's sign-in and
   tokens, secrets and the sealed fill, the sandbox and proxy, the extension and native host, updates and signing,
   the stored data, prompt injection from what FERB reads, and the live attack scenarios of TEST-507.
