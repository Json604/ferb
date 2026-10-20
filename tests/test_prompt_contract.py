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
    ("interaction.md",
     'Everything the owner reads (answers, summaries, questions, success conditions, stop reasons, and a check\'s '
     'evidence and how) speaks to them as "you" and "your", never "the owner" or "the user". In a success condition, '
     '"you" means the owner.'),
        ("dispatch.md",
     'When the owner handed the work to a named agent or person (not an app FERB uses as a tool), never instruct a worker to do that work itself: have it help the named one, or use action=ask to ask the owner whether FERB should take over, saying what is stuck.'),
        ("question.md",
     "A worker asking to do work the owner handed to a named agent or person (not an app FERB uses as a tool) is the owner's decision about who does the work, not a method question: ask_owner=true, saying what is stuck and what FERB would do instead."),
        ("verifier.md",
     'A condition that the agent or person the owner handed the work to did it is met only by evidence that they produced it; work FERB did itself is a substitute: give `met=false` and say in the evidence that FERB did it. An app FERB used as a tool is not such an agent.'),
]


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
                   "reason, tell me\") narrows the task: steer it to stop making changes, keep diagnosing and report "
                   "what it finds.")


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
