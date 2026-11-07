"""Own the words of where FERB browses when it has no browser profile of its own (APP-777, refining APP-1). Must not
decide anything: the daemon's `_browsing` (ferb/daemon/areas/browsers.py) decides from config and pairing facts, and
the supervisor applies its answer to a task (`_browse_as`)."""

BROWSING_ASK = "browsing.choose"     # FERB's one question: which of the owner's profiles it uses for web work
NONE = "None"                        # the question's last option: no profile; web work goes without a browser
KIND = "browsing"                    # its `memory_questions.kind`
ASKED = "owner_profile_asked"        # [browsing] key: the owner answered the question (a profile, or None)

QUESTION = ("FERB doesn't have a browser profile of its own. Which of your profiles should it use for web work, like "
            "searches? It works there in the background, as you: its searches show in that profile's history and "
            "sites see your accounts there. Anything risky still asks you first. With None, web tasks are slower and "
            "less reliable. You can change this anytime in Settings → Browsers.")
GONE = "The profile FERB used for web work, {label}, isn't there any more. "
CHOSEN = "FERB will do its web work in {label} from now on. You can change it in Settings → Browsers."
NONE_CHOSEN = ("OK: FERB won't use any of your profiles for web work. Some tasks will be much slower and less "
               "reliable: without a browser, FERB fetches pages directly, and pages that need JavaScript, a sign-in "
               "or a bot check can't be read that way. You can pick a profile anytime in Settings → Browsers.")
NO_BROWSER_RUN = ("This run had no browser: you haven't picked which of your profiles FERB uses for web work, so it "
                  "read pages without one (slower and less reliable). I'll ask you the next time a task needs it.")
GONE_RUN = ("This run had no browser: the profile FERB used for web work, {label}, isn't there any more, so it read "
            "pages without one (slower and less reliable). I'll ask you the next time a task needs it.")

# The planner's facts (availability's Unavailable line for FERB's own browser), so it can say why when asked
UNASKED_FACT = ("not set up; when a task does web work as FERB, FERB asks the owner once which of their browser "
                "profiles to use, and works there as the owner")
CHOSEN_FACT = "not set up; web work as FERB runs as the owner in their {label}, the profile they picked"
NONE_FACT = ("not set up, and the owner chose to give FERB none of their profiles: web work as FERB has no browser, so "
             "pages are fetched directly, pages that need JavaScript, a sign-in or a bot check can't be read, and it "
             "is slower and less reliable. The owner can pick a profile in Settings → Browsers")
