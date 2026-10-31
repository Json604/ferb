"""Invented texts shaped like the audit's username case (L36, Ruling 25): two owner messages that continue a running
appointment task (the first started it, the second corrects it), that task's goal and targets, and the research step
goal that typed the owner's email instead of the username. Shared by the search and the worker-memory tests."""
from ferb.orchestrator.schemas import TargetsSpec

# Invented text only (the owner's chat lines never enter the repository). The messages name no site and share no word
# with the username fact; the goal holds the ordinary words of a long research-and-booking goal (memory_corpus).
MSG_1217 = ("not sure that plan is right for this, slots can open anytime. search online properly for this first and "
            "then propose a good strategy")
MSG_1222 = ("also, book whichever center has one free. dont hold out for one city. document check first, then the "
            "interview. find out what info the booking step needs. if nothing is needed from me just book it. dont "
            "wait for me. book it and tell me after")
PERMIT_GOAL = ("Update the ongoing permit appointment strategy work for your `owner@example.com` account in Chrome "
               "`Home_Personal`: optimize for actually securing an appointment without paying an agent. Do not restrict "
               "the search to one city; look for the earliest available valid document-check appointment first and "
               "the corresponding interview across any locations the official system permits, with any date "
               "acceptable. Research exactly what information, documents, selections, or decisions the official "
               "booking flow requires before the final booking step. Use your securely stored login and security "
               "answers where applicable. If no new information or decision is required from you when a valid "
               "appointment pair appears, book it immediately without waiting for you, then report the exact "
               "locations, dates, and confirmation details. If the booking flow requires a genuinely missing personal "
               "choice or information, ask only for that. Also determine and implement a monitoring frequency that "
               "materially improves your chance of getting a slot while respecting official limits and avoiding "
               "account blocks, bots, paid agents, or unsafe extensions.")
PERMIT_TARGETS = TargetsSpec(apps=["Chrome Home_Personal"], sites=["bookslots.com"])
RESEARCH_OFFICIAL = ("Collect current official guidance on permit appointment availability, scheduling rules, "
                     "rescheduling limits, account access, and automation or abuse restrictions from official "
                     "government and scheduling sources.")
