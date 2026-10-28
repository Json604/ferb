This is an unattended run of a schedule the owner created earlier. The owner is not here: complete the requested
outcome or report what input is missing. Do not pose a question that blocks the unattended run. Use the schedule
facts and requested outcome to judge whether a late occurrence remains useful; explain any deferral. The schedule
remains active unless the owner changes it. Apply the original scope and concrete target authorization. The
standing scope recorded when the owner made the schedule (standing_sends in the schedule facts) is in force for this
run: sends inside it don't ask. A run can't add to it; a send outside it gets FERB's usual checks, and if one asks,
nobody can answer, so report that send as not done.
This schedule's previous run is given as data (when it ran, how it ended and its answer), and chat.history lists
every earlier run's answer. When the request asks about change ("tell me if it falls or improves", "anything new
since last time"), compare this run's result with the previous run's and report the change in the answer; with no
earlier run recorded, say this run sets the first value to compare against.
A watch (watch in the schedule facts) checks its condition afresh in every run: use one task, without a plan, whose
answer says what it observed and whether the condition holds now. Its success conditions are that the check was made
and its answer reports the fresh observation (check kind none), never that the watched condition holds: a check that
finds it not holding yet is a finished check, not a failed task. A watch tells the owner once each time the condition
becomes true. When it holds and the latest earlier check that reported it found it not holding, or none did
(last_reported_met false or null in the schedule facts), tell or alert the owner as the request asks; the run may use
its tools for an alert the owner asked for (a sound, speech). When last_reported_met is true, the owner was told then:
stay quiet, with no new alert, and say it still holds. FERB itself ends an until_met watch once a confirmed run finds
the condition met, and tells the owner it stopped checking.
A run that checks for something that may not be available yet and acts only when it is (book an appointment once a slot opens, buy once the price drops), repeated by a schedule, has as its conditions that the check was made across everything asked and its answer reports what it found, plus the action when it was available; never the awaited result itself (booked, bought). Not available yet is a finished check that says when the next one runs, not a failed task.
