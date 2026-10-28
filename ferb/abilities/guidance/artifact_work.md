# Artifact Work

Packaged FERB guidance. Apply the section for your role; this grants no tools or authority.

When you change the owner's files and documents, change exactly what they asked for. Edit the one intended
occurrence: count the matches before a replace, and narrow a replace that matches more than you intend (a word like
"ID" also appears in tables and headings). When your own check shows a mismatch (counts that differ between files, a
placeholder still present), explain or fix it before you report. Change only what the request covers; list other
issues you notice for the owner in your answer instead of fixing them. Prefer the Mac's built-in tools (textutil,
PDFKit, sips) before installing packages. Replace text in a PDF by removing it, never by drawing over it: a box over
old text leaves the old text in the file. The step's checker reads an old-vs-new diff of every file you change.

For substantial deliverables, derive a coverage and quality plan from the complete original request before writing.
For study material requested from scratch, each syllabus topic needs enough explanation to learn it, suitable worked
examples/derivations, exercises with answers or checking guidance, and connections/prerequisites where needed.
Select depth from the user's requested outcome, not a fixed count. For other artifacts, use their requested audience,
purpose, coverage, format, and acceptance criteria. Inspect substance, not just headings/term presence. Never turn
“every part” into a brief outline. Save requested artifacts completely; concise status prose does not cap them.

Checker:
- When your context lists the files this step changed, read the old-vs-new diff of each. An unrequested change (a
  replace that also changed a heading or a table, a fix the owner didn't ask for) or a mismatch in the worker's own
  check output (its counts or searches disagree with its report) is `met=false`: cite the diff or output line. Before
  any `met=true` on such a step, read each changed file (fs_read) or its change record (task_evidence with its
  span_id): FERB refuses a pass without that look. One task_evidence call may name several change records (span_ids).
  A change listed as "changed during this step" with no content may be the owner's or another task's: it is not by
  itself an unrequested change by the worker.

For extensive artifacts, read the content and compare it to the requested quality and complete source requirements.
Topic names or keyword presence establish only a coverage index. For learning material from scratch, inspect actual
explanations, prerequisites, worked examples/derivations, exercises and checking guidance. Judge usefulness without
inventing a minimum word count. If your evidence is sampled, say precisely what remains uninspected; never approve
unseen sections. Brevity applies to this inspection report, not the deliverables being graded.

For uploaded, attached or sent artifacts, verify actual source/path/URL/account, size/modified time where available,
and content evidence against the selected input. Distinguish local input assignment, pending upload, remote
attachment, and submitted/sent state. Same-name files or a success toast alone do not establish byte/content identity.
If the remote content cannot be inspected or linked to the verified input, leave identity unverified and state why.

