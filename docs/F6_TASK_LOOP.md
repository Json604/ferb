# F6 task loop — pending final checks and owner acceptance

F6 adds optional dependency plans to the existing task supervisor. Simple tasks still use the ordinary worker and
checker. A plan has `do`, `map`, `reduce`, `wait`, `ask`, and `converse` steps. Code checks the dependency graph,
inventories declared input paths, checks complete map coverage and conservative batch context estimates, and returns
concrete repair facts to the planner. It does not choose ranking criteria or a worker-count cap.

Each map batch writes complete input notes with evidence references. Code checks that every assigned input has one
note before reduction. Notes are stored under the task's workspace folder and in redacted durable step output. Each
executable step uses the ordinary worker, result checks, resource cleanup, and gateway. A final worker inspects the
combined result and writes the evidence report and owner answer in the same call; the checker judges that answer.

Migration 0028 keeps plans, step results and observed wait events. On daemon startup an unfinished plan resumes using
the same task and step identifiers. Completed steps are retained; interrupted steps receive previous output and the
existing uncertain-action ledger. A wait can name a time or an event with exact field matches. Restarts retain a
wait's event cursor. An explicit stop cancels the tree; shutdown suspends it for restart. This does not assert that an
unobserved external action succeeded, and it does not restore transient login secrets or escape approvals.

Big jobs display the plan and estimated time/cost before starting. Standing send scopes require an explicit Yes to
the displayed task, tool, identity, account, site, recipients and body. A null body is allowed only through the display
that explicitly says **any message body**; omitted bodies are not wildcards. Semantic task constraints stay with the
agent. Payment/account-security confirmations, prior No decisions, private/outside provenance, test-mode recipients
and unknown-action checks remain in the gateway. Scope use is traced, send activity is recorded for the final digest,
and grants are revoked when the task ends. No scope carries to another task.

Context has labelled identity, situation, recent activity, memory, complete request/conversation, and task-state
layers. Identity/request/state remain protected. Last-ten-minute activity is grouped by app and repeated identical
states compacted; optional activity yields context space before memory. Workers get their own plan step and handed
results, with full sources available through existing readers. The skills catalog and stable owner identity guidance
are shared across runtimes. Procedure candidates and selected procedure bodies are data, with applicability and
selection hooks supplied by F8; selecting a procedure never bypasses tool policy or verification.

`ferb chat` keeps accepting input while background streams run. Messages enter planning in order, then their tasks
run independently. `/tasks` lists active tasks, `/stop [id]` stops work directly, and Escape on an empty input stops
the latest task. Ctrl-C clears a nonempty input; two presses on an empty input close chat while tasks continue. Owner
questions appear above the input. Use `/answer <approval-id> <answer>`, or reply normally when the intended question
is clear. Private questions wait for Enter on an empty input before opening hidden input. Password spaces are kept.
`/memory confirm <id>` confirms a proposal delivered by F8's memory digest.

`ferb attach <process-id>` watches the existing redacted PTY log. Add `--take` for exclusive owner input. Ctrl-]
detaches and returns input to the worker. The attachment stream owns the lease, so disconnect also releases input;
watching alone does not pause worker input. This never drives the owner's Terminal application.

Owner skills live in `~/.ferb/skills/<name>/SKILL.md`, with frontmatter `name` and `description`, followed by the body.
The always-loaded catalog contains metadata only. `skills.read` loads the body or an explicitly named supporting
file through a directory-relative no-symlink reader. FERB does not install skills from the internet. Skills are owner
guidance; an external document that calls itself a skill retains its external trust.

Build validation is compile-only by the owner's instruction. The new offline tests cover session-file inventory and
coverage, dependency execution, event matching and durable waits, exact standing scopes, owner input leases,
nonblocking chat, hidden input, context allocation and the separate checked owner answer. They have been authored,
not run. The controller runs the final integrated checks; live behavior remains pending owner acceptance.
