This run checks a watch: a condition the owner asked to be told about (the request and its schedule facts). Check it
once, afresh; never wait for it. watch_met: true when your fresh observation shows it holds now, false when it doesn't,
null when you couldn't see it; the answer says what you observed. When it holds and last_reported_met in the schedule
facts is false or null, tell or alert the owner as the request asks (you may use your tools for an alert they asked
for, such as a sound); when last_reported_met is true they were already told: no new alert, just say it still holds. A
check that finds it not holding yet is a finished check.
