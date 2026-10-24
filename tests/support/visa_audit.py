"""The audit's real texts for the visa username case (L36, Ruling 25): the owner's 12:17 and 12:22 messages, the visa
task's goal and targets, and the research_official step goal that typed the owner's email (live task 0a5fbe31,
2026-10-24). Shared by the search and the worker-memory tests."""
from ferb.orchestrator.schemas import TargetsSpec

# The audit's real texts (scratchpad memaudit/repro.py and the live task 0a5fbe31, 2026-10-24); only the owner's email
# address is replaced (Ruling 25). The owner's 12:17 message created the task; 12:22 corrected it.
MSG_1217 = ("hmm idk if thats s agood strategy for this, cause slots can pen anytime. search online extensilvely for "
            "this and then propose what is a good strategy")
MSG_1222 = ("also, wherever its available, book it. dont just wait for chennai. first ofc then counselar. search what "
            "info you’d require at booking the slot stpe. if nothting is req from me then just booook it. dont wait for "
            "me. book and then tell me")
VISA_GOAL = ("Update the ongoing US B1/B2 appointment strategy work for your `owner@example.com` account in Chrome "
             "`New_Personal`: optimize for actually securing an appointment without paying an agent. Do not restrict "
             "the search to Chennai—look for the earliest available valid OFC appointment first and corresponding "
             "consular interview across any Indian locations the official system permits, with any date acceptable. "
             "Research exactly what information, documents, selections, or decisions the official booking flow "
             "requires before the final booking step. Use your securely stored login and security answers where "
             "applicable. If no new information or decision is required from you when a valid appointment pair "
             "appears, book it immediately without waiting for you, then report the exact OFC and consular locations, "
             "dates, and confirmation details. If the booking flow requires a genuinely missing personal choice or "
             "information, ask only for that. Also determine and implement a monitoring frequency that materially "
             "improves your chance of getting a slot while respecting official limits and avoiding account blocks, "
             "bots, paid agents, or unsafe extensions.")
VISA_TARGETS = TargetsSpec(apps=["Chrome New_Personal"], sites=["usvisascheduling.com"])
RESEARCH_OFFICIAL = ("Collect current official guidance on India B1/B2 appointment availability, scheduling rules, "
                     "rescheduling limits, account access, and automation or abuse restrictions from official US "
                     "government and scheduling sources.")
