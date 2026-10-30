"""Runtime prompts are executable input (CLAUDE.md). These pin the instructions the F4 live run showed missing (task
79bc5bfa, 2026-10-09): the agent the owner names does the work, conditions cover what the owner actually asked, reports
say who did each part, a lasting choice the owner didn't ask for is asked about, and FERB speaks to the owner as "you"
(V6, D34, P27). Whether a model follows them is a live check; these keep the words from silently disappearing."""
from pathlib import Path

import pytest

PROMPTS = Path(__file__).resolve().parents[1] / "ferb" / "orchestrator" / "prompts"


def _words(name: str) -> str:
    return " ".join((PROMPTS / name).read_text().split())


SENTENCES = [
    # live 2026-10-25 06:50: "whos this actor?" got a confident wrong name from the same frame twice; the owner: "just
    # add its probably X ... it should've just checked the cast"
    ("supervisor.md",
     'Naming a person from a face or picture is a likely match, never a fact: say "It\'s probably …" and what it is '
     'based on (the show, scene, caption). When the screen names the show, episode or event and you aren\'t sure, '
     'start a short task that checks its cast or credits and answer from that.'),
    ("supervisor.md",
     'Work FERB must repeat or keep true while the owner\'s request runs ("keep this tab playing", "loop until the '
     'checks pass", "every time a reply comes, answer it") is a plan with a repeating do step (a loop: its repeat rule '
     'set): goal is what to keep true or repeat, repeat.end_when is the owner\'s own end condition or empty when they '
     'gave none (then only the owner\'s stop ends it).'),
    ("supervisor.md",
     'Never make a step depend on a loop whose end_when is empty. A loop only acts through ordinary rounds; it grants '
     'nothing.'),
    # live 2026-10-25 (cc5debe4, a1f16a32, 4a8b1d73): every plan bound the tab in a do step before its loop (43-55 s),
    # and the step's tab was let go of when it ended, so the loop's first round did it all again
    ("supervisor.md",
     "A loop's first round does its own setup: it finds the target, starts or fixes it and arms its probe; the loop's "
     "rounds share one task and keep what they open. Plan an earlier step only for different work the loop needs."),
    # live 2026-10-24: the owner steered the visa task with "the username is …"; nothing was remembered, and a later task
    # typed the email as the username. L36 §5: the username alone, for the site's login record
    ("supervisor.md",
     'When the owner tells you a login name or username for a site or app ("the username is Asha Rao" while working '
     'on that site), remember it in the same turn as the task change: kind fact, subject "owner" or the project '
     '("project:<slug>"), a key naming the site ("usvisascheduling-username"), content: the username alone, exactly '
     'as they typed it ("Asha Rao"), and login_origin: the site\'s https:// address; FERB saves their whole line as '
     'the memory and the username for that site. Passwords and other secrets go through the hidden-input path. '
     'Otherwise login_origin is null.'),
    # scan m4: the remember bullet's empty-list rule reads as about remember, before the login sentence
    ("supervisor.md",
     'any rewording waits for their Yes. When nothing is worth remembering, remember is an empty list. When the owner '
     'tells you a login name or username'),
    # L36 §6: dated facts (the 10-11 wedding flight) never retired
    ("supervisor.md",
     'When the owner\'s words give the item an end ("until Sunday", "till the 31st"), add until: quote, the owner\'s '
     'exact date words, and only what they said in day (today, tomorrow, a weekday …), date ("MM-DD" or "YYYY-MM-DD") '
     'or in_days; FERB works out the date, and the item leaves memory after that day. Otherwise until is null.'),
    # live 2026-10-24 12:34: "pause the visa task for now, I'll say when to continue" stopped the task and left its
    # daily schedule active
    ("supervisor.md",
     'When the owner pauses or resumes a job that also has a schedule in the facts ("pause the visa task for now, '
     'I\'ll say when to continue"), pause or resume that schedule in the same turn, along with the task change; when '
     'they end the job for good ("stop checking for visa slots altogether"), cancel it.'),
    # owner 2026-10-24: "until I say stop" became end_when "You say to stop."; the owner's own stop is no end condition
    ("supervisor.md",
     'When the owner\'s only end is their own stop ("until I say stop", "keep it running"), end_when is empty.'),
    ("supervisor.md",
     'it is a loop that repeats a checked round until the owner says stop or end_when is verified, with code checking '
     'one exact fact between rounds;'),
    ("worker.md",
     'In a loop round (step_context has "loop"), act once toward the goal and verify it, then report loop_action and, '
     'with continue, a probe: one probe-safe observation tool (browser_tabs, browser_snapshot or browser_find; '
     'shell_list or shell_read; fs_list or fs_read; mac_snapshot), its arguments as a JSON object '
     'string, a dot path to one field of its result, and equals, not_equals or changed. Choose the field that shows '
     'when the job needs doing again.'),
    ("worker.md",
     'Report finished only with observed evidence that end_when holds; never when end_when is empty. Outside the '
     'loop, loop_action and probe are null.'),
    # live loops cc5debe4, a1f16a32 (2026-10-25): each round took a second snapshot to learn the reel played
    ("worker.md",
     "A snapshot's `playing` is true only when the media's time moved, so one snapshot shows whether it plays; take "
     "another only after you play, unmute or seek it."),
    # live loop cc5debe4 (2026-10-25): every round ended with keep_open, which let go of the owner's tab and paused it
    ("worker.md",
     'The loop holds for you the tabs and sessions its rounds open or borrow, across rounds until it stops: leave them '
     "held and use them again in the next round (step_context's held_tabs lists its tabs by id, ready for "
     'browser_snapshot).'),
    # review M2: "use keep_open when media should keep running" sat next to "leave them held"
    ("worker.md",
     'In a loop round, the loop keeps this tab held for you between rounds; leave it held, without keep_open, which is '
     'for work that ends.'),
    # final loops review (L31): shell_list's order isn't an identity; a command's own id is
    ("worker.md",
     'To watch a command you started, probe shell_read with its id (field running), not a shell_list index.'),
    # live 2026-10-24 (task 0a5fbe31): a paired profile that wasn't running was opened through Chrome's Profiles menu
    # and driven with Accessibility; the browser tools start it in the background
    ("worker.md",
     "Use the browser tools for a paired browser profile even when it isn't running or connected now: they start it in "
     "the background, or open its window when that doesn't connect (the owner accepted that focus shift), then work in "
     "hidden tabs. Open sites with browser_open "
     "or browser_navigate. Never drive a browser through Mac tools (its menus, address bar, clicks or keys) when a "
     "browser tool can do the step: that takes over the owner's screen. Mac input in the foreground asks the owner "
     "first, once per task for that app; prefer a background route. If a browser tool says a profile isn't paired, "
     "pair it with browser_pair (FERB asks the owner once); never work around it."),
    # live 2026-10-24 (task 4a8b1d73): probes on media.44 / media.51 watched another video as the feed loaded more
    ("worker.md",
     'To watch one video or audio element, probe browser_snapshot with media_by_id.<id>.playing, where <id> is that '
     "element's id in a fresh snapshot's media list: positions in the list shift as a page loads. To watch whether "
     "one of the owner's own tabs (one that browser_tabs lists) plays sound, probe browser_tabs with "
     'tabs_by_ref.<tab_ref>.audible, which is cheaper than a snapshot.'),
    # owner 2026-10-24: "will ferb click on the cloudflare check box??? it should since its just a box"; 2026-10-27:
    # checks go through the browser_verification ability ("we are using it now"), and one that still blocks goes to
    # the owner as one notification that opens the tab, with no "done" reply
    ("worker.md",
     "A verification check in the owner's own browser profile (a \"Verify you are human\" checkbox, pictured objects "
     "to pick, a slider) is handled with FERB's browser_verification ability: read its procedure with abilities_read "
     "and follow it with your browser tools, as the owner would, checking the page with a fresh snapshot after each "
     "input. If the check still blocks (a press-and-hold, a puzzle you can't resolve, the site's rejection), call "
     "browser_handoff with the page's own words: FERB sends the owner one notification that opens the tab and returns "
     "once the page lets them through. Never ask the owner to reply \"done\"."),
    # owner 2026-10-24, visa login: account_security declared on every sign-in step asked five times in four minutes
    ("worker.md",
     "`account_security` only for changing a password, two-factor or recovery settings, or deleting the account. "
     "Signing in with existing credentials (filling a saved or owner-entered password, a login code or existing "
     "security answers, and pressing the page's sign-in or Continue button) is not an account-security change: "
     "declare it `edit`."),
    # owner 2026-10-24, visa task: nine escape questions in two minutes, one per command reading a Chrome profile folder
    # hands decision 1 (2026-10-30): one read question per protected folder per task
    ("worker.md",
     "A command that needs escape (one shell_rules rule lifted) asks the owner. A read of a protected folder is asked "
     "once for the task: after the owner's Yes, later reads of that folder (commands or file tools) run without asking; "
     "a write or admin escape covers that exact command once. Before asking, look where no escape is needed: a public "
     "page about it (an extension's Web Store listing shows its permissions) or a tool that already reads it. When a "
     "protected folder really must be read, get everything you need from it in one command, never one command per file "
     "or step."),
    # hands decision 4 (2026-10-30): a refused write in a visible folder goes to file tools or a which-folder question
    ("worker.md",
     "Shell commands can't change the owner's visible folders (~/Documents, ~/Desktop, ~/Downloads, iCloud Drive, other "
     "disks) unless the task names the folder; change the owner's files there with fs_write, fs_edit or fs_trash (each "
     "keeps a recovery copy), or ask the orchestrator which folder the task is about, never through a terminal."),
    # live 2026-10-25 (task 6f5b8589): the sandbox refused `ps` and a socket, and the worker typed the command into
    # the owner's VS Code terminal unasked; owner: "it should've just said that I'm getting blocked I just wanna run
    # this can I?" (D31: the escape hatch, or a terminal input FERB asks about every time)
    ("worker.md",
     'When FERB\'s sandbox refuses a command you need for another reason (for example "Operation not permitted"), '
     "request the escape that shell_rules names for it, so the owner answers once for that exact command. When no "
     "shell_rules rule covers it, you may type the command into a terminal (a terminal app or an editor's terminal): "
     "FERB asks the owner first, showing the exact command, every time."),
        ("worker.md",
     'When the owner hands the work to a particular agent or person that does it on its own (a coding agent like Codex Astra, a colleague, a service), that one does it, not you. An app you drive as a tool (WhatsApp, Spotify, Gmail, a browser) is not that: you do the work through it. Your part is to start the agent, give it what it needs (answer its questions, supply data or files, do the browser or Mac steps it cannot), monitor it and check its result. If it is stuck, help it, or use owner_ask; do none of its work yourself unless the owner says so (the orchestrator relays the answer), and then say in your report that you did it.'),
    ("worker.md",
     "- summary: the concrete result, useful details and remaining limitations, in FERB's first-person voice. Say who "
     "did each part (FERB itself, or the agent, app or person that did it) and what was used, as the records show."),
        ("supervisor.md",
     'When the owner hands the work to a named agent or person that does it on its own (not an app FERB uses as a tool), the goal says that one does it and FERB starts it, helps it and checks its result without doing that work itself.'),
        ("supervisor.md",
     'When the owner hands the work to a named agent or person (not an app FERB uses as a tool), one condition is that they did it, not FERB or a substitute.'),
    ("supervisor.md",
     "monitor its result, inspect artifacts and report when it finishes; if it is stuck, FERB helps it or asks the "
     "owner before doing its work."),
    ("interaction.md",
     "A lasting choice the owner did not ask for and the request does not need (for example which submission counts as "
     "final, or changing a setting or default) is the owner's decision, never a side step: bring it to the orchestrator "
     "as a question instead of making it."),
    # a friend's feedback 2026-10-27, decided by the owner: multi-part requests are answered part by part
    ("interaction.md",
     "A request with several parts (several questions, or several things to do) is answered part by part, in the "
     "owner's order, one short labelled line or paragraph per part saying what was found or done, or plainly what "
     "wasn't; never merge the parts into one paragraph or leave one out."),
    # owner 2026-10-27: "its every question is too confusing. just ask simply what you want with a small reason below"
    ("question.md",
     'Write `display_text` as one short plain line saying what FERB needs from the owner (a choice or the missing input), then on a new line the reason in a few words.'),
    # live 2026-10-27 (a friend's FERB): a storage scan failed because FERB's own safety snapshot counted as a
    # change, and a Homebrew cleanup failed because Homebrew's own removal of five libraries, undone by the
    # worker, broke a safeguard the planner added; both did what was asked
    ("verifier.md",
     "FERB's own safety records are never a change by the task: the recovery snapshot FERB takes before shell work or a permanent delete (a local Time Machine snapshot named com.apple.TimeMachine.<date>.local, `recovery_snapshot` in a call's record) and FERB's own backup copies of changed or trashed files. Judge a no-change or read-only condition by the task's own actions."),
    ("verifier.md",
     "A safeguard the owner did not state (FERB's planner added it: not touching other items, no side effects) is judged on the end state: a side effect the worker caught and fully undid, restored to exactly what was there before, meets it when the report discloses it; give `met=false` only for a side effect that remains, or one the report leaves out. A condition in the owner's own words is judged as they wrote it."),
    ("supervisor.md",
     "FERB's own safety snapshots and backup copies are never a change by the task: never write a condition that they would break."),
    # live 2026-10-26 (task 7c1eca3a): every Indian OFC location said "No Slots Available" and the run reported
    # "failed" because its condition was "booked"; the six-hourly schedule keeps checking
    ("supervisor.md",
     'A run that checks for something that may not be available yet and acts only when it is (book an appointment once a slot opens, buy once the price drops), repeated by a schedule, has as its conditions that the check was made across everything asked and its answer reports what it found, plus the action when it was available; never the awaited result itself (booked, bought). Not available yet is a finished check that says when the next one runs, not a failed task.'),
    ("scheduled.md",
     'A run that checks for something that may not be available yet and acts only when it is (book an appointment once a slot opens, buy once the price drops), repeated by a schedule, has as its conditions that the check was made across everything asked and its answer reports what it found, plus the action when it was available; never the awaited result itself (booked, bought). Not available yet is a finished check that says when the next one runs, not a failed task.'),
    # live 2026-10-26 (task 7c1eca3a): the worker asked "should this wait until you're idle, should you open that
    # profile now, or should I skip" for a closed Chrome profile, as this file's old window rule told it to; FERB
    # opens a closed profile itself (owner: "if not open once that causes focus shift - ok")
    ("interaction.md",
     "A browser profile that isn't open is never a reason to ask: the browser tools start it in the background (or "
     "open its window when that doesn't connect), and browser_open puts the tab in one of its open windows or, when "
     "it has none, in FERB's own minimized window, made once (the owner accepted that focus shift). Never ask the owner to open a profile window, to wait until they're idle or to "
     "skip a step for it, unless a browser tool says the profile's extension isn't paired."),
    ("interaction.md",
     'Everything the owner reads (answers, summaries, questions, success conditions, stop reasons, and a check\'s '
     'evidence and how) speaks to them as "you" and "your", never "the owner" or "the user". In a success condition, '
     '"you" means the owner.'),
        ("dispatch.md",
     'When the owner handed the work to a named agent or person (not an app FERB uses as a tool), never instruct a worker to do that work itself: have it help the named one, or use action=ask to ask the owner whether FERB should take over, saying what is stuck.'),
        ("question.md",
     "A worker asking to do work the owner handed to a named agent or person (not an app FERB uses as a tool) is the owner's decision about who does the work, not a method question: ask_owner=true, saying what is stuck and what FERB would do instead."),
    # L36 §5: a resumed task re-asked for two secrets saved that morning; nothing listed them by site
    ("worker.md",
     'Before typing into a login form, call secrets_list for that site.'),
    ("worker.md",
     'It gives the username saved for that site and the names of its saved secrets, never their values, whatever your '
     'memory shows. Type a listed username like any text; fill a listed secret with browser_secret (or mac_secret) '
     'secret=<its name>; ask the owner, through the hidden input, only for what is not listed, and save it with '
     'remember=<a clear name>: "password", or the security question\'s own text.'),
        ("verifier.md",
     'A condition that the agent or person the owner handed the work to did it is met only by evidence that they produced it; work FERB did itself is a substitute: give `met=false` and say in the evidence that FERB did it. An app FERB used as a tool is not such an agent.'),
    # L39 (live 39e35d57, 2026-10-25): a perl replace of every "ID" also changed Table 10's header, the worker's own
    # check printed ID=2 / ID=1 and it reported "matching identity values"; PDF fields were covered by white boxes; an
    # unrequested typo fix left the PDF inconsistent; PyMuPDF was installed for what PDFKit does
    ("worker.md",
     "Edit the one intended occurrence: count the matches before a replace, and narrow a replace that matches more "
     "than you intend (a word like \"ID\" also appears in tables and headings)."),
    ("worker.md",
     "When your own check shows a mismatch (counts that differ between files, a placeholder still present), explain or "
     "fix it before you report."),
    ("worker.md",
     "Change only what the request covers; list other issues you notice for the owner in your answer instead of fixing "
     "them."),
    ("worker.md",
     "Prefer the Mac's built-in tools (textutil, PDFKit, sips) before installing packages."),
    ("worker.md",
     "Replace text in a PDF by removing it, never by drawing over it: a box over old text leaves the old text in the "
     "file."),
    ("verifier.md",
     "When your context lists the files this step changed, read the old-vs-new diff of each. An unrequested change (a "
     "replace that also changed a heading or a table, a fix the owner didn't ask for) or a mismatch in the worker's own "
     "check output (its counts or searches disagree with its report) is `met=false`: cite the diff or output line."),
    ("verifier.md",
     "Before any `met=true` on such a step, read each changed file (fs_read) or its change record (task_evidence with "
     "its span_id): FERB refuses a pass without that look."),
    # L39 fix round 1 (I1, M9): facts-only changes may be the owner's; a bulk step's records in one read
    ("verifier.md",
     "One task_evidence call may name several change records (span_ids). A change listed as \"changed during this "
     "step\" with no content may be the owner's or another task's: it is not by itself an unrequested change by the "
     "worker."),
    ("dispatch.md",
     "action=recheck: run only the check again, on the same work and its recorded file changes, with no worker run."),
    ("dispatch.md",
     "When FERB's fact says the check marked conditions met without reading the files the work changed, the work isn't "
     "what failed and a worker retry can't fix it: use recheck, or finish with result unconfirmed naming the files the "
     "check didn't read, or ask the owner."),
]


def test_the_effect_field_says_a_sign_in_is_not_an_account_security_change() -> None:
    """The tool schema's `effect` help reaches the worker on every call: it says what the worker prompt says."""
    from ferb.daemon.gateway import EFFECT_HELP
    assert ("account_security: changing a password, two-factor or recovery settings, or deleting the account; signing "
            "in with existing credentials is edit") in EFFECT_HELP
    assert "sign-in, two-factor" not in EFFECT_HELP and "sign-in, two-factor" not in _words("worker.md")


@pytest.mark.parametrize("name, sentence", SENTENCES, ids=[f"{n}:{s[:40]}" for n, s in SENTENCES])
def test_the_prompt_carries_the_instruction(name: str, sentence: str) -> None:
    assert " ".join(sentence.split()) in _words(name)


def test_the_old_monitoring_line_still_says_launching_is_not_completion() -> None:
    # The new paragraph sits after it; the existing instruction is kept, not replaced.
    assert "launching it is not completion." in _words("worker.md")


@pytest.mark.parametrize("name, sentence", [
    ("worker.md", "An aggregator's InStock metadata is a discovery lead, not proof that an official seller currently has tickets available."),
    ("verifier.md", "A caveat at the end does not repair unsupported availability or price claims in individual rows."),
    ("supervisor.md", "For exhaustive comparisons, require substantive coverage of every requested source, not just an inventory or keyword-ranked excerpts."),
    ("worker.md", "Do not invent a numerical ranking score without a reproducible rubric and supporting observations."),
    ("verifier.md", "Inventory counts, opening/closing excerpts and a shortlist do not establish that all source content was analyzed."),
    ("worker.md", "For unclassified split boxes, pass their exact fresh snapshot IDs in split_elements, in code order, with element set to the first ID."),
])
def test_owner_report_audit_contract(name, sentence):
    # Real owner traces: session review 726ab98b and ticket list 6eb4bffd.
    # These guard executable instructions; live model adherence remains separately audited.
    assert sentence in _words(name)


def test_a_declared_post_names_no_audience_in_its_destination() -> None:
    """`destination` holds recipients, account, amount and item, and no audience field. A post's audience
    ("public", "followers") put into recipients would ask the new-recipient question, which a campaign scope with
    recipients [] never covers, so a scheduled post would be declined (D30). Every text that tells the worker what goes
    in `destination` says the same: no recipients for a public post, and the account it goes out from."""
    from ferb.daemon.gateway import DESTINATION_SCHEMA
    interaction, worker = _words("interaction.md"), _words("worker.md")
    assert ("with its recipients (none for a public post) and the account it goes out from in `destination`"
            in interaction)
    assert "audience in `destination`" not in interaction
    assert "`recipients` for a message or email (none for a public post)" in worker
    assert "`account` the post, message or payment goes out from" in worker
    described = " ".join(str(DESTINATION_SCHEMA["description"]).split())
    assert "recipients: the people a message or email goes to (none for a public post)" in described
    assert "audience" not in described and "audience" not in DESTINATION_SCHEMA["properties"]


# Owner live use, 2026-10-14 (task 1b79b873, 20:15): EF's "Retrieve Saved Application" page showed "Error 6732: Oops!
# We could not find a saved application for this email address"; the worker read the page's reCAPTCHA footer as a
# block and asked to take the foreground. A block is reported with the page's own words, never inferred from a badge.
CAPTCHA_BLOCK = ("Before reporting that a CAPTCHA or anti-bot check blocked background work, quote the challenge or "
                 "error the page shows, from a fresh snapshot. A \"protected by reCAPTCHA\" or Turnstile footer or "
                 "badge alone is not a block: read and report the page's actual message instead.")


@pytest.mark.parametrize("name", ["worker.md", "interaction.md"])
def test_a_captcha_block_is_reported_with_the_pages_own_words(name: str) -> None:
    assert CAPTCHA_BLOCK in _words(name)


def test_the_orchestrator_checks_a_reported_captcha_block_against_the_page() -> None:
    assert ("A reported CAPTCHA or anti-bot block that doesn't quote the page's challenge or error is not yet a "
            "blocker: have the worker read the page and report what it actually says.") in _words("dispatch.md")


def test_a_reworded_worker_question_keeps_the_workers_reason() -> None:
    """Owner live use, 2026-10-14: the worker's "EF's CAPTCHA blocked the background interaction. Can I bring the EF
    tab forward…" reached the owner as "Choose: use the EF tab briefly now, wait until you're idle, or skip…", so the
    owner couldn't see, or correct, the reason."""
    assert ("Both `question` and `display_text` keep the worker's stated reason for asking, in one short clause, "
            "not only the choices, so the owner can see it and correct it.") in _words("question.md")


NARROWING_STEER = ("Use stop only when the owner's words ask only to stop. A message that tells FERB to stop changing "
                   "things but still wants an answer (\"don't worry, leave it, it works now, but if you can find the "
                   "reason, tell me\") narrows the task: continue it with the instruction to stop making changes, keep "
                   "diagnosing and report what it finds.")


def test_a_narrowing_message_steers_instead_of_stopping() -> None:
    """Owner live use, 2026-10-15 02:46: "don't worry, leave it — it works now, but if you can find an answer, tell me"
    stopped the hotspot-diagnosis task ("Stopped, as you asked.") instead of narrowing it to diagnosis only."""
    assert NARROWING_STEER in _words("supervisor.md")
    assert "Use stop only when the owner asks to stop," not in _words("supervisor.md")


ANY_TIME = ("When the owner says any time is fine (\"anytime\", \"whenever\", \"doesn't matter\"), don't ask for a time: "
            "use their stored preference for that phrase (memory key \"time:anytime\" and the like) if there is one, "
            "else choose a sensible time from context (the event's own time if you know it, or a reasonable time for "
            "that kind of reminder), and say in your reply which time you chose.")
QUOTE_ONE = ("Quote the single message that asked for the reminder when it holds the whole request; when the owner "
             "completed it over several of their messages (a time given after your question), quote each of those "
             "messages exactly, separated by a blank line.")
NO_SET_CLAIM = ("Never write that the reminder is set (\"I'll remind you on October 30 at 7 PM\", \"Reminder set for "
                "…\"): FERB's own line says whether it was set, or why not.")


def test_any_time_is_fine_picks_a_time_from_context_and_says_it() -> None:
    """Owner live case 2026-10-15 03:36: "anytime" got the question asked again. No clock value is pinned here: the
    owner doesn't want a hardcoded default."""
    words = _words("supervisor.md")
    assert ANY_TIME in words
    assert not __import__("re").search(r"\b\d{1,2}:\d{2}\b", ANY_TIME)


def test_a_reminder_quote_is_the_owners_message_and_the_reply_never_claims_it_was_set() -> None:
    words = _words("supervisor.md")
    assert QUOTE_ONE in words and NO_SET_CLAIM in words


SONG_PLANNER = ("\"What is this song/sound?\" is answered from the page only when the page names the song; otherwise "
                "it needs listening (a task) on the tab the owner is looking at (their front window, or the tab already "
                "identified in this conversation), never another tab.")
SONG_WORKER = ("FERB's hearing turns speech into text; it can't recognise music by its sound. Asked what a song or sound "
               "is, listen on the tab the owner is looking at (the one named in the task), not another; if what you "
               "hear is speech, or music without lyrics you can identify, say so plainly and never name something "
               "else (a video or product the speaker mentions) as the song.")


def test_what_is_this_song_listens_on_the_owners_tab_and_never_names_something_else() -> None:
    """Live use: "what is this song?" on a reel was answered from the page's "Original audio by @…" label without
    listening; on "hear and tell" a 12 s speech-only voiceover was heard on a different tab, and the video the speaker
    mentioned was named as the song."""
    assert SONG_PLANNER in _words("supervisor.md")
    assert SONG_WORKER in _words("worker.md")


PARALLEL_STEPS = ("When the owner asks for several items to be done in parallel (\"do this for all 15 assignments in "
                  "parallel\"), make a plan with one step per item (steps that don't depend on each other run "
                  "independently); never send one worker told to be parallel, since a worker can't start workers. "
                  "\"Parallel\" is how to work, never a success condition: if the steps can't run at the same time, do "
                  "the items one by one and say so, and never fail a task only because of how the work was split.")


def test_a_parallel_request_is_one_plan_step_per_item_and_never_a_condition() -> None:
    """Owner live use 2026-10-18 (task 891b3eaf): the planner wrote "All 15 assignments analyzed in parallel, one per
    agent" as a condition and sent one worker, which reported blocked twice; the task failed after 9 minutes with
    nothing produced."""
    assert PARALLEL_STEPS in _words("supervisor.md")


@pytest.mark.parametrize("name, sentence", [
    ("worker.md", "Give owner_ask options when the answer is one of a few (yes or no, which of these): a picked option "
                  "reaches you as picked, while a typed answer to a question that mentions passwords, passcodes or "
                  "PINs is hidden whole. Never ask for a password in owner_ask; the sealed tools ask for it."),
    # re-review 4 (M3): the planner's clarify has no options field, so it never uses the words at all
    ("supervisor.md", "A clarify question never asks for a secret (FERB's sealed input does) and never uses the words "
                      "password, passcode, passphrase, PIN, pwd or passwd: a typed reply to a question with them is "
                      "hidden from you."),
])
def test_questions_about_passwords_offer_options_and_never_ask_for_one(name: str, sentence: str) -> None:
    """Re-review 3 (R1-a, R1-c): a typed answer to a question with a password word is hidden whole."""
    assert " ".join(sentence.split()) in _words(name)


def test_the_shared_rules_never_offer_now_idle_or_skip_for_a_closed_profile() -> None:
    assert "offer now / idle / skip" not in _words("interaction.md")
