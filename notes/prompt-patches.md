# Prompt patches: mapping table (step 10a, for your review)

**Status:** approved by the owner 2026-10-31 (notes/prompts.md decision 4) and applied in step 10b: every row went to
its destination (the rules R1-R10 in the prompts named below, tool manuals in tool or schema descriptions, code-covered
and dropped lines removed, the memory items seeded once). Two placements differ slightly from the table: row 7's
password-words line stays one line in supervisor.md's "clarify" (clarify is a value of `kind`, which has no field of its
own to describe) and in owner.ask's description; row 20's "email me" is the seeded memory item plus one line in the
task's targets ("me" for "email me", the address from memory, asked when missing). The live re-test list is in
notes/tests.md "Live re-test list (step 13)".

Prompts decision 2: patches leave the prompts. Each one becomes a memory item, folds into a general rule, moves to a
tool's description, is already covered by code, or is dropped. You review this table and confirm the memory items
(memory decisions 15 and 18: there are no Keep questions, so this review is where you confirm them). Step 10b then
rewrites the prompts from it, and step 13 re-runs the tasks in the "Re-test" column live. After this, a correction
goes into FERB's memory ("remember …"), never into a prompt.

How it was built: I read every line of the runtime prompts (`ferb/orchestrator/prompts/*.md`, `ferb/memory/*.md`,
`ferb/abilities/**`). For each patch, `git log -S` found the commit that added it. The task behind it comes from the
commit message, or from the FERB database by text and time (read-only). Where no message names the task, "likely"
marks a match by date and wording.

**Counts.** 74 patches (where one rule appears in several files, the copies share a row): **35 rule**, **17 code**,
**12 tool**, **6 memory** (9 memory items, because some rows give one), **4 drop**.
**Size.** The patch passages come to ≈50.7k chars: supervisor.md 11.2k, worker.md 23.9k, verifier.md 8.6k,
interaction.md 5.2k, question/scheduled/runtime_tools 1.8k. About 13k of that is tool manual, which moves into tool
descriptions (some of it is already there). The 10 rules below take ≈4k. Net, about 46k chars leave the role
prompts. This is wider than prompts.md's ≈16k of "one-off patches" because it also counts repeated rules and tool
manuals, as decision 2 asks. memory/*.md and the ability's SKILL.md have no patches.

Not in this table, because the skeleton rewrite handles them (prompts.md problems 1, 3, 4, 5 and 8): worker types
described as capabilities (worker.md:1-5), the internal word "steering" (supervisor.md:304), the done/blocked
statuses, and the checker's harsh lines.
Correction to prompts.md problem 9: the map/reduce input lists are **live**. Orchestration decision 5 and step 8a
kept "do with an input list", so supervisor.md:224-236 and worker.md:246-248 stay.

## Proposed general rules (full text)

Each rule goes once, in the place named. Role-specific wording is fine, but the meaning stays the same.

**R1. Do the whole request, as stated.** (planner conditions, worker, checker)
Carry every part of the owner's request into the work and its conditions: each named app, tool, agent, account,
folder, source and date range, and each quality or limit they state for the result, in their words ("score well",
"no overfitting", "easy to understand", "every part of the syllabus"). Never narrow it (read-only, a sample, a
shortlist, "as far as possible"). Never add a cap the owner didn't choose (top-N, excerpts, file size, time). Words
about how to work ("in parallel", "one by one") shape the plan and are never a condition. Do the work in full,
without caveats or a lesser offer.

**R2. Settle which one from evidence.** (shared prefix; question router)
Work out which app, account, profile, source, contact or file the owner means from their words, their memory and
what you can observe. Never infer it from a generic word ("messages" names no app unless memory says so), from the
account an earlier task or agent used, or from a matching name or a default. Confirm the account by the label or
address the page shows, and say in the answer which account and source you used. A same-named item in another place
(a local file and a Drive file) is a different item until shown otherwise. If several still fit, ask one question
naming each candidate and its evidence.

**R3. Prove it with a fresh, primary look.** (worker; checker. Replaces the recency paragraph that appears 3 times)
State as done or true only what you observed: a fresh read of the result, the primary source (the seller's page, the
official group, the email itself), and every source and date range the request covers. Leads are not proof: a
listing or "InStock" tag, a search hit, an old message, an earlier artifact, a file's existence, a keyword or a
heading. For "latest", "new" or "current", use the source's dates to show both which item it is and that it is the
current one. Read what the work depends on first (an assignment's requirements before doing it). Keep a coverage
record and name what you couldn't see instead of claiming "all checked". For a deliverable, inspect its substance,
not its headings.

**R4. Say how you know.** (planner reply; worker answer)
Name where an answer comes from (the screen, memory or general knowledge) and say a guess as a guess ("It's probably
…, going by …"). When a quick look would settle it (the show's cast list, listening to the tab the owner is on), take
it instead of guessing. Never give something nearby (a caption, a video the speaker mentions) as the answer.
Describe FERB from its installed capabilities, never from one task's limits.

**R5. A named doer does its own work.** (shared prefix, once)
When the owner hands work to a named agent or person (a coding agent, a colleague, a service), that one does it. An
app FERB drives as a tool doesn't count. FERB starts it with the settings the owner named (model and reasoning names
are settings, not things to find), gives it what it needs, monitors it to the end and checks its result. Starting it
is not finishing. If it is stuck, help it or ask the owner whether FERB should take over; never quietly do its work.
The answer says who did each part.

**R6. Change only what was asked.** (worker; the checker judges it)
Change exactly what the request covers, in the intended place: count the matches first and narrow a replace that
hits more than you mean. A fix removes or replaces the old content and never covers it up: a box drawn over PDF text
leaves the text in the file. List other problems you notice instead of fixing them. If your own check shows a
mismatch, fix or explain it before you report. A lasting choice the request doesn't need (which submission counts,
a setting or a default) is a question for the owner, not a side step.

**R7. Read what the owner wants for a running job.** (planner)
Stop a task only when the owner's words ask for nothing but stopping. "Leave it, but tell me why" narrows it: no
more changes, keep diagnosing, report. Doubts or second thoughts pause it. A job and its schedule move together:
pausing or resuming the job pauses or resumes its schedule in the same turn, and ending the job for good cancels the
schedule.

**R8. Judge the owner's outcome, not FERB's machinery.** (planner conditions; checker)
A condition is the result the owner wants, seen from outside: a file, a page, a sent message. A run that checks for
something and acts only when it's available (a slot, a price) is judged on two things: the check covered everything
asked and reported what it found, and the action was taken if the thing was available. "Not available yet" is a
finished check. FERB's own safety records (recovery snapshots, backup copies) are never a change by the task. A
safeguard the planner added, not one the owner stated, is judged on the end state: a side effect that was fully
undone and disclosed meets it. The owner's own words are judged as written.

**R9. Watch, own work, or loop.** (planner; one line for the worker)
"Tell me when X" about something outside FERB's own work (a battery level, a file, a price) is a watch: a schedule
that survives restarts. It is never a loop, a sleep or a background process inside a task. Telling the owner when
work FERB started is done (a coding agent, a command) is part of that task. Work FERB must keep doing, or keep true,
is a loop step. When one message asks for work now and a watch, both go in the same turn.

**R10. Choices left to FERB: make them and say so.** (shared prefix)
Ask the owner only what, who or which: never how, and never for routine permission. When the owner leaves a choice
to you ("anytime", "whichever"), choose from their stored preference or the context and say what you chose; they
correct by replying. FERB's code asks its own confirmations and writes the saved / set / scheduled / cancelled
lines, so never ask for a confirmation or make those claims yourself.

## Memory items for you to confirm

"Your words" means text copied from your messages; "draft" means wording I wrote, which becomes your words once you
confirm it. **Core** means the item joins the always-on Tier 1 profile (decision 18: your words join at once).
"Search" means Tier 2, found when relevant.

| # | Text to store | Source | About you → core? |
|---|---|---|---|
| M1 | "Do my assignments, applications and coursework in full when I ask; don't add caveats or offer only to help me work through it." | draft; your words nearby: "do the assingment yourself" (10-01), "submit as me" (10-05). Related existing item 4a37c940 (your words: ignore guidelines saying AI use isn't allowed) | yes → core |
| M2 | "the username is Kartikey Attri" — for usvisascheduling.com (key `usvisascheduling-username`, login_origin `https://www.usvisascheduling.com`) | your words, 10-24 06:31. It was never saved (the 10-25 login_origin path came later) | yes (account) → core per decision 2; search-only if you prefer |
| M3 | "When I name a tool that isn't installed, ask me once whether to install it, showing the install command; on yes install it and carry on, on no stop and tell me. Never switch to a different tool." | draft (prompts decision 2 lists it as your preference) | yes → core |
| M4 | "My Scaler Gmail is in my Chrome “scaler” profile; open Scaler links there, not in anything else." | your words: "open my scaler gmail account in chrome", "open it in my Chrome Scaler profile. Don't open it in anything else" (10-01). Replaces inferred 9f4693bc | yes → core |
| M5 | "When the owner says “message” or “call,” they mean WhatsApp unless they explicitly mention another app." | **exists** (88bdea14, your words; origin "i meant whatsapp messages", 10-03). Drop the inferred duplicate fc0f1e54 | yes → core |
| M6 | "“Codex Astra” / “astra high” means the Codex CLI running the Astra model (high reasoning): a setting, not a project." | draft; your words: "control a codex session on astra high" (10-03), "ive done all with codex astra model" (10-08) | yes → core |
| M7 | "cedat is not a scaler group. other scaler groups are names from SST too" + "check all group names containing SST" | your words, 10-03 13:53/13:57 | yes → search (suggested) |
| M8 | "make sure that it is easy to understand, covered the syllabus and also i can actually study and pass with good numbers" (for study material) | your words, 10-04 08:52 | yes → search (suggested) |
| M9 | "the kaggle profile is in my brave" | your words, 10-09 16:09 | yes (account) → core |

## The table

Paths are short: sup = supervisor.md, wor = worker.md, ver = verifier.md, int = interaction.md, que = question.md,
sch = scheduled.md, rt = runtime_tools.md. Dates are 2026. "Kaggle" means task 79bc5bfa (10-09: "start an interactive
codex astra session to complete this assignment … score good. NO OVERFITTING. NO HARDCODING"). "Scaler 5 days" means
2f96ef80 (10-03: "check all the messages from scaler in the past 5 days … only check official groups").

| # | Patch | Origin (commit, date, task) | Category → destination | Re-test |
|---|---|---|---|---|
| 1 | sup:7-9 "Naming a person from a face or picture is a likely match, never a fact" | 2003754f 10-25; "whos this actor?" got two confident wrong names; you: "it should've just checked the cast" | rule R4 | "who's this actor?" on a show frame |
| 2 | sup:23-25 "What is this song/sound?" answered from the page only when it names the song | 4c8aa85e 10-15; Instagram reel 708acb0e answered from "Original audio"; "hear and tell" 3a1c9a44 named a mentioned video | rule R4 (+ audio_listen description) | both 10-15 requests |
| 3 | sup:33-35 "The owner decides what's appropriate for their own work — assignments, … coursework" | 291f3804 10-01; FMI assignment through a Claude session ("do the assignment yourself") | memory M1 (+ R1) | an assignment request done in full |
| 4 | sup:43-44 "Never present the limits of one task (e.g. a read-only research step)" | 14e3e981 10-01; likely "compare yourself with dots" (10-01) | rule R4 | "how are you better than dots?" |
| 5 | sup:57-62 login name: key "usvisascheduling-username", content "Asha Rao" | f747d37c 10-24 (visa 0a5fbe31: "the username is …" not saved; a later task typed the email); bcfed4e4 10-25 | memory M2; the login_origin field stays as one schema line (code: daemon/areas/memory.py:957), example removed | give a username mid-task; a later task logs in with it |
| 6 | sup:68, 146-147, 177-178, 193-195 "Don't say in text whether it was saved / scheduled / set"; "I'll remind you on October 30…" | 8abad86f 10-14; 11b6e66d 10-15 (your 03:36 case); 8e055ba6 10-24 | code: memory.py:920 "Memory: noted…", supervisor.py:242 "Reminder set…", schedule lines (8abad86f) → one line (R10) | a reminder, a schedule, a "remember" |
| 7 | sup:11-13 clarify never uses "password, passcode, passphrase, PIN, pwd or passwd" | 31acc745 10-20, 6243f38d 10-21 | code: sanitize/redact.py:246, labels.py:444 hide the typed answer → one line in the clarify field's description | – |
| 8 | sup:83, 120 "one condition is that they did it", 331-333 "Starting another agent is not task completion" | 1ca394d8, 6b7eb857 10-09; Kaggle | rule R5 | Kaggle |
| 9 | sup:85-87 "<tool> isn't installed — should I install it?" | 6d97c8ce 10-14; Mole task 2dd99563 failed "not installed" | memory M3 | "storage is 80% again, use mole" |
| 10 | sup:91-94 "\"scaler Gmail\" is meaningful without magic words" | 4e0976a2 10-03; 10-01 "open my scaler gmail account in chrome" (ae1e1085, fdcc8142 failed) | memory M4 (+ R2) | "open my scaler gmail, find the latest FMI assignment" |
| 11 | sup:95-98 "Generic words such as \"messages\" do not name the macOS Messages app"; an earlier task's account isn't the new one's | 13677223 10-03; Scaler 5 days used Messages; you: "i meant whatsapp messages" | rule R2 (+ M5, exists) | Scaler 5 days |
| 12 | sup:99-101 "Permissions are granted during setup … follow the shared confirmation contract" | fd7d7f88 10-05 (0.26 hardening) | code: policy/confirm.py floors → R10 line | – |
| 13 | sup:118-120 "audience, depth and usability, especially \"from scratch\", \"every part\", or study-ready" | fd7d7f88 10-05; 1-week study material, 10-04 ("each and every part of the syllabus … pass with good numbers") | rule R1 + R3 (M8 optional) | the 1-week study material request |
| 14 | sup:120 "\"score well\", \"no overfitting\", \"no hardcoding\" is its own condition" | 6b7eb857 10-09; Kaggle | rule R1 (M9 from the same message) | Kaggle |
| 15 | sup:124 + sch:21 (same text) check-and-act run: "never the awaited result itself (booked, bought)" | 4b40d593 10-26; visa slot check 7c1eca3a "failed" on "No Slots Available" | rule R8 (planner only; the sch copy goes) | six-hourly visa check with no slots |
| 16 | sup:125 + ver:49-52 "safety snapshots"; "com.apple.TimeMachine.<date>.local" | f9c87a71 10-27; a friend's "reduce my storage" failed on FERB's own snapshot; a Homebrew cleanup | rule R8 (code labels it: gateway.py:216 `recovery_snapshot`); snapshot name dropped | read-only "reduce my storage" scan + a brew cleanup |
| 17 | sup:126-127 "Do not replace \"all official groups for five days\" with \"some relevant previews\"" | 13677223 10-03; Scaler 5 days, a95b1bc6 | rule R3 (M7) | Scaler 5 days |
| 18 | sup:140 every_minutes "at least 15" | 2d501b72 09-30 (build step 15) | code: schedules.py:75, 87 (> 0); prompts decision 3 → remove | "every 5 minutes …" accepted |
| 19 | sup:157-160 "Turn on Low Power Mode and alert me at 10%"; "start Codex on it and tell me when it's done" | 1bd9cdc0, 1611d121 10-21; your 10-15 case c38b6a2e: the alert ran as a shell loop that died on restart | rule R9 | "take mac to low power mode … loud warning at 10%" |
| 20 | sup:171-172 "email me": address from memory; "remember it (key \"email\")" | 2d501b72 09-30 | rule (the remember contract covers it; key detail dropped) + your call on the address | "every morning at 9 email me …" |
| 21 | sup:174-176 "pause the visa task for now…"; "stop checking for visa slots altogether" | 76798b8d 10-24; 12:34 pause left schedule sc-eec2a61cbf active | rule R7 | pause, then resume the visa job: the schedule follows |
| 22 | sup:211-214 "anytime": key "time:anytime", else choose and say | 11b6e66d 10-15; "remind me about ths show on oct30 anytime" | rule R10 | that reminder |
| 23 | sup:237-241 "do this for all 15 assignments in parallel" | d6d5946e 10-19; 891b3eaf made "in parallel" a condition and failed after 9 min | rule R1 (the one-step-per-item plan line stays) | "do this for all 14 tabs in parallel" |
| 24 | sup:266-268 campaign scope: "browser.act, … (x.com), its recipients []" | c429f816 10-14 (campaign path; launch-post planning 9523bbed) | tool: standing_sends schema description | an X post from Brave Work (8247cd9a), owner-approved run only |
| 25 | sup:307-309 "don't worry, leave it, it works now, but if you can find the reason, tell me" | 527e8ee7 10-15; hotspot task f98cae75 was stopped | rule R7 | hotspot question → "leave it, but tell me why" |
| 26 | sup:321-322 "Correct an unauthorized owner-browser route to identity ferb" | c0b454ab 10-02 | rule R2 (see owner call 6) | a web task after one in a personal profile |
| 27 | sup:330-331 "Reading an assignment email must establish requirements…"; 333-334 + wor:7-11 "Model names/reasoning settings … not a project to find" | 4e0976a2 10-03 10:17; 08:29 "check in scaler gmail about the FMI assignment 2 … control a codex session on astra high" | rule R3 + R5; memory M6 | that FMI Assignment 2 request |
| 28 | sup:344-348 = wor:274-278 = ver:105-109 recency paragraph (identical ×3) | 46c0be76 10-06 00:14; 10-05 23:47 "no theres a new assingemtn, check scaler mail" | rule R3 | "check scaler mail for the new assignment" |
| 29 | sup:374-377 + wor:296-312 + ver:131-142 exhaustive comparisons; tickets / "InStock" | ed7e797c 10-11; concert tickets 6eb4bffd; "analyze all local coding agent cli sessions" 726ab98b | rule R3 + R1 (no caps) | both 10-11 requests |
| 30 | wor:13 + 263 named agent; "Do not claim a requested agent finished…" | 1ca394d8 10-09 | rule R5 | Kaggle |
| 31 | wor:15-21 shell_start: "no nohup, trailing &…", shell_wait | c42444ce 10-03 | tool: shell_start / shell_wait (act/shell/tools.py:83 has part of it) | – |
| 32 | wor:22-27 watch vs. a process the task started | 1bd9cdc0, 1611d121 10-21 | rule R9 | as 19 |
| 33 | wor:28-31 proxy; "Set external_source to the actual source" | 5ae6308a 10-14 | tool: shell external_source field (tools.py:98) | – |
| 34 | wor:32-36 protected-folder read asked once; "an extension's Web Store listing shows its permissions" | 659b8ae2 10-24 (visa extension: 9 asks in 2 min); d189db24 10-30 | code: confirm.py:84 (one read question per folder per task, hands decision 1); "look where no escape is needed" → escape tool description; example dropped | the visa step that inspects the slot extension |
| 35 | wor:37-43 visible folders; sandbox refusal; terminal typing | d189db24 10-30; 3f9a180b 10-25 (6f5b8589 typed `codex` into VS Code's terminal unasked) | code: shell/sandbox.py:135-140, policy/confirm.py:627 + policy/terminal.py | the Codex startup-failure fix (6f5b8589) |
| 36 | wor:45-53 + ver:17-20 recording, recording_inspect | 88368e8c 10-05 | tool: browser_record / mac_record / recording_inspect (+ R3) | – |
| 37 | wor:55-64 audio_listen; check the media is playing | be9969f3 10-14 | tool: audio_listen, browser_media, the snapshot's media field | as 2 |
| 38 | wor:65-68 "can't recognise music by its sound … never name something else" | 4c8aa85e 10-15 | rule R4 + tool (audio_listen) | as 2 |
| 39 | wor:77-80 "Do not ask for routine permission"; 82-87 "Never ask for confirmation yourself" | 4e0976a2 10-03; 6d97c8ce 10-14 | code: confirm.py → R10 line | – |
| 40 | wor:89-93 "`brew install mole`" | 6d97c8ce 10-14; Mole task 2dd99563 | memory M3 | as 9 |
| 41 | wor:95-101 "a word like \"ID\" also appears in tables"; "never by drawing over it"; textutil/PDFKit/sips | b9af4aac 10-25; 39e35d57: reported "ID=2 / ID=1" as matching; white boxes over PDF text; an unrequested typo fix | rule R6 (+ fs_edit description: count the matches) | "unzip these 3 … make sure they're correct" |
| 42 | wor:103-110 study material "from scratch"; intake receipts | fd7d7f88 10-05 | rule R1 + R3 (the intake line becomes a code fact) | as 13 |
| 43 | wor:112-120 browser routes; "scaler Gmail"; paired profile; "Never drive a browser through Mac tools" | d8ad3d9d 10-24 (0a5fbe31 drove Chrome through its menus); 3b9b76e4 10-26; 4e0976a2 | tool: browser_open / browser_pair; code: ext_bridge.py:934 `_connected`, confirm.py:128 foreground floor; "scaler Gmail" → M4 | visa task in Chrome New_Personal with the profile closed |
| 44 | wor:122-125 "messages"; "Confirm the visible account label" | 13677223 10-03 | rule R2 (+ M5) | Scaler 5 days |
| 45 | wor:127-134 group/date coverage; posting date vs. event date | 13677223 10-03 | rule R3 | Scaler 5 days |
| 46 | wor:135-142 keep_open; tabs a loop holds | 51e82443 10-03 + loop work | tool: browser keep_open; code holds a loop's tabs | "keep it running, only stop when I say" (10-24) |
| 47 | wor:144-147 choosing a native input mechanism | fd7d7f88 10-05 | tool: mac_act / mac_click | – |
| 48 | wor:148-150 = int:27-29 (+ ver:175-176) "a \"protected by reCAPTCHA\" … footer alone is not a block" | 721f0b86 10-14; EF "Retrieve Saved Application" (1b79b873): the footer was read as a block | code/ability: act/browser/snapshot.py:23-132 `bot_check` fact; SKILL.md:17 (a vendor badge alone is no challenge). No prompt wording (hands decision 6). See owner call 9 | the EF saved-application page |
| 49 | wor:151-156 browser_verification ability, browser_handoff fallback | c11900e1 10-27 (you: "we are using it now") | code/ability: snapshot.py:130-132 names the ability; browser_handoff description; SKILL.md | visa six-hour check through the bot check (cd80f4ee) |
| 50 | wor:158-164 + ver:171-173 snapshots/find; "FERB refuses to repeat an action whose earlier outcome is unknown" | 5a34bfcb 10-08 | code: gateway.py:94-127 (RETRY_ASK and its refusal text), ledger.py:59; tool: browser_snapshot / browser_find | – |
| 51 | wor:165-179 login code, browser_secret, split_elements, secrets_list, owner_ask options | 4e0976a2; 8192405a 10-24; 31acc745 10-20 | tool: browser_login_code / browser_secret / mac_secret / secrets_list / owner_ask; code: redact.py:246 | visa login (3 security questions) |
| 52 | wor:181-189 pointer gestures, the 30 s screenshot | 4f2790e3 10-02 | tool: *_point / *_act / mac_pointer | – |
| 53 | wor:191-201 scrolling; "Never substitute Next Chat" | 9591c831 10-03 07:58; likely the WhatsApp list work that week | tool: mac_act scroll / mac_pointer scroll | Scaler 5 days (scrolling groups) |
| 54 | wor:204-219 + int:43-46 effect declaration; "Signing in … is not an account-security change" | 6d97c8ce, c429f816 10-14; 1cd326d6 10-24 (visa sign-in declared account_security: 5 asks in 4 min) | tool: EFFECT_HELP gateway.py:2325, DESTINATION_SCHEMA :2332 (already there) | visa login asks nothing for the sign-in |
| 55 | wor:241 "The checker checks this wording" | e14d691d 10-11 | drop (prompts.md problem 5) | – |
| 56 | ver:30-32 "calls before then are labelled an earlier message's run" | e03faefd 10-24 ("keep it running, only stop when i say") | code: evidence.py:301, spans.py:125 → one line | as 46 |
| 57 | ver:36-38 "all groups or all five days" | 13677223 10-03 | rule R3 | Scaler 5 days |
| 58 | ver:42-48 old-vs-new diff; 179-180 a check that didn't look | b9af4aac, 5587ae2a 10-25 | code: supervisor.py:5291 CHANGED_FILES, :5299 REFUSED_PASS, :5302 NO_LOOK (+ R6: an unrequested change fails the check) | as 41 |
| 59 | ver:53-56 "A safeguard the owner did not state … judged on the end state" | f9c87a71 10-27 | rule R8 | as 16 |
| 60 | ver:77-78 "audience, depth, usability"; 84-88 learning material "from scratch" | fd7d7f88 10-05 | rule R1 (judged as stated) + R3 | as 13 |
| 61 | ver:90-93 identity of an uploaded artifact; que:9 "A prior answer about a local file never settles a same-name Drive file" | fd7d7f88 10-05; FMI Assignment 2 form, Drive vs. local PDF (16f2e0c4, 4138cec8) | rule R2 | "did the PDF I uploaded come from Drive or my Mac?" |
| 62 | ver:111-116 "is a substitute, not the target"; work FERB did itself is a substitute | bba6d1c1, 6b7eb857 10-09 | rule R5 + R2 | Kaggle |
| 63 | ver:118-127 "My report to you says truthfully…" | a6494d6e 10-09 | code: supervisor.py:105 adds the condition; the checker keeps fix_answer | – |
| 64 | ver:174 + que:15 named agent | 6b7eb857, 1ca394d8 10-09 | rule R5 | Kaggle |
| 65 | ver:177-178 + que:12-14 a named tool that isn't installed | 6d97c8ce 10-14 | memory M3 | as 9 |
| 66 | int:31-38 target identity before upload / send / pay ("A same filename … never establishes identity") | 88368e8c, fd7d7f88 10-05 | rule R2 | as 61 |
| 67 | int:36 "route_unavailable" | fd7d7f88 10-05 | drop (no code emits it; needs_foreground is a driver fact, mac/driver.py:1132) | – |
| 68 | int:38-46 confirmation contract | 6d97c8ce 10-14 | code: confirm.py → R10 line | – |
| 69 | int:47-48 "which submission counts as final" | 6b7eb857 10-09; Kaggle | rule R6 | Kaggle |
| 70 | int:56-57 "the choices now / when I am idle / skip" | fd7d7f88 10-05 | code: confirm.py:128 foreground_need (one Yes per task per app) → drop | – |
| 71 | int:61-62 coding agent "with its own sandbox disabled by caller-chosen flags" | 3fb1e7c2 10-11; likely Kaggle's "give it yolo persmission" | drop (the disturbance facts already list FERB-started processes) | – |
| 72 | int:72-80 a profile that isn't open; "window_selection_required" | 46c0be76 10-06; 3f9fb6a4 10-26 (you: "if open then use") | code: ext_bridge.py:934 `_connected`, :993-1000 FERB's own minimized window → drop | as 43 |
| 73 | int:114-127 "Visible answers" (repeats :86-111) | 8e055ba6 10-24 and earlier | drop (the shared prefix keeps one copy) | – |
| 74 | rt:5-9 "A visible tool_search helper can mean FERB tools are deferred" | 17ebb1f2 10-07 | code: worker.py:35, :340 NO_TOOLS_LOADED rerun | – |

## Repetition clusters: the one place each keeps

| Cluster | Copies today | Keeps it |
|---|---|---|
| CAPTCHA / bot check | int:27-29, wor:148-156, ver:175-176 | code: the `bot_check` snapshot fact + SKILL.md. No prompt wording |
| Confirmation contract | int:38-46, wor:77-87, ver:159-161, que:8-9, sup:99-101 | one fact line in the shared prefix (R10) |
| "Don't claim saved/set/scheduled" | sup:68, 146-147, 177-178, 193-195 | one line in the planner's side-outputs section (R10) |
| Effect declaration | wor:204-219, int:43-46, EFFECT_HELP | EFFECT_HELP / DESTINATION_SCHEMA only (gateway.py:2325-2340) |
| Named agent does the work | sup:83, 120, 331-333; wor:13, 263; ver:116, 174; que:15 | R5, once in the shared prefix |
| Recency paragraph | sup:344-348, wor:274-278, ver:105-109 (identical) | R3 (worker + checker) |
| Named tool not installed | sup:85-87, wor:89-93, ver:177-178, que:12-14 | memory M3 |
| Unknown-outcome refusal | wor:160-164, wor:270-272, ver:171-173 | code: the refusal text (gateway.py:94-127) |
| Exhaustive review / tickets | sup:374-377, wor:296-312, ver:131-142 | R3 + R1 |
| Study depth / "from scratch" | sup:118-120, wor:103-108, ver:77-78, 84-88, int:68-70 | R1 + R3 |
| Groups × five days | sup:126-127, wor:127-133, ver:36-38 | R3 (+ M7) |
| Check-and-act run | sup:124, sch:21 | R8 in the planner |
| Safety snapshots | sup:125, ver:49-52 | R8 |
| Watch vs. own work | sup:151-160, wor:22-27 | R9 |
| "messages" / account choice | sup:95-98, wor:122-125 | R2 + M5 |
| "scaler Gmail" | sup:92, wor:113-114 | M4 |
| Password words in questions | sup:11-13, wor:177-179 | code (redact.py:246) + a line in the clarify / owner_ask descriptions |
| Foreground use | int:9-11, 56-57, 74; wor:119-120 | code (confirm.py:128) + one fact line |
| Visible-answer style | int:86-111, int:114-127 | the shared prefix, once |

## Re-test list for step 13 (deduplicated)

1. "who's this actor?" on a show frame. 2. "what is this song?" on a reel; "hear and tell". 3. An assignment done in
full (FMI-style). 4. "how are you better than dots?". 5. A username given mid-task, then a later login. 6. Kaggle via
Codex Astra (you control the browser; score well, no overfitting). 7. "storage is 80% again, use mole" (not
installed). 8. "open my scaler gmail, latest FMI assignment". 9. Scaler 5 days, official groups. 10. The 1-week study
material. 11. Six-hourly visa check with no slots; pause and resume the visa job; the visa login; the visa check
through the bot check; the visa task with Chrome New_Personal closed. 12. A friend-style "reduce my storage" scan +
brew cleanup. 13. "every 5 minutes …" schedule. 14. Low Power Mode + alert at 10%. 15. "email me every morning …".
16. "remind me about this show on oct30 anytime". 17. "do this for all the tabs in parallel". 18. Hotspot → "leave
it, but tell me why". 19. FMI Assignment 2 with "astra high". 20. "check scaler mail for the new assignment". 21.
Concert tickets now; "analyze all coding agent sessions". 22. Three ZIPs with name and student ID checked. 23. The
Codex startup fix in VS Code (terminal asks). 24. The EF saved-application page (footer badge only). 25. "keep it
running, only stop when I say" follow-up. 26. Drive vs. local PDF upload question. 27. An X post from Brave Work
(owner-approved run only).

## Your calls

1. **"email me" address**: no memory item holds your email for "email me". Which address should FERB store?
2. **"anytime"**: keep "choose from context and say which" (your 10-15 revision: no hardcoded default), or store a
   default time?
3. **M2 visa username**: core profile (as decision 2's example says) or search only?
4. **M7, M8, M9**: core or search? (Suggested: M7 and M8 search, M9 core.)
5. **M1**: should it also include "DONT MENTION ANYWHERE AI OR REFLECT AI SOLUTION, submit as me" (10-05), or is
   item 4a37c940 enough?
6. **Conflict: default browser for ordinary web work.** sup:91 and int:18 say "default ordinary web work to FERB's
   profile". Your stored words (bac03d3b) say "Most of your things are in the Brave profile with your kartikey060105@gmail
   account." Which one decides where web work goes when you name no profile?
7. **M6 wording**: does "astra high" mean the Astra model with high reasoning?
8. **Memory clean-up**: drop the inferred duplicates fc0f1e54 (messages) and 9f4693bc (Scaler, replaced by M4)?
9. **Bot check vs. footer badge (row 48)**: a reCAPTCHA v3 / Turnstile footer badge is itself a frame on
   google.com/recaptcha/ (or challenges.cloudflare.com), so the `bot_check` fact will name "reCAPTCHA" on pages like
   the EF one, where nothing blocked. With the prompt line gone, only SKILL.md:17 says a badge alone isn't a
   challenge, and the worker reads that only if it opens the ability. Keep it that way (re-test 24 will show), or
   make the code fact say "a widget frame is present, not that it blocks"?
