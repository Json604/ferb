/* Curated architecture explanations, not a live trace replay. No personal message bodies or credentials. */
(function (root) {
const nodes = [
 ['owner','Owner / CLI','Your request and the final answer. Chat can disconnect while a task continues.','ferb/cli/chat.py','interface'],
 ['rpc','Daemon / RPC','Authenticates local calls and delivers progress, questions, stop and results.','ferb/daemon/main.py','code'],
 ['supervisor','Supervisor','Builds context, invokes decision models, owns attempts and reconciles the result.','ferb/orchestrator/supervisor.py','code'],
 ['context','Context / registry','Relevant memory, bounded chat history, daily activity, available tools and live device facts.','ferb/registry.py','data'],
 ['planner','Decision model','Chooses reply, clarification or task. Recovery and question decisions also use no-tool model calls.','ferb/orchestrator/prompts/supervisor.md','model'],
 ['worker','Worker runtime','Configured Codex/Claude adapter runs the tool-using agent; the model chooses the next action.','ferb/orchestrator/worker.py','model'],
 ['mcp','MCP transport','Per-run token connects the worker’s tool calls back to the daemon through stdio and RPC.','ferb/cli/mcp.py','code'],
 ['gateway','Action gateway','Validates arguments, task ownership and stop state; locks resources, executes, redacts and records.','ferb/daemon/gateway.py','code'],
 ['browser','Browser transport','Daemon bridge → native host → extension 0.12.0 / protocol 7 → per-tab CDP, AX + DOM.','ferb/act/browser/ext_bridge.py','platform'],
 ['mac','Native Mac driver','AX reads and app/window-directed input; exact-window screenshots. Support depends on the app and OS.','ferb/act/mac/driver.py','platform'],
 ['shell','Shell / PTY','Tracked commands and external coding sessions. This Claude process is separate from FERB’s internal worker.','ferb/act/shell/manager.py','platform'],
 ['files','Files / artifacts','Workspace, requested deliverables, process logs, recordings and receipts.','ferb/act/fs/files.py','data'],
 ['recording','Recording manager','Samples one pinned target. Stop encodes a silent MP4; inspect decodes the saved pixels.','ferb/act/recording.py','code'],
 ['questions','Questions / sealed OTP','Ordinary questions go to the orchestrator first. Supported OTP input goes directly to hidden owner input.','ferb/daemon/questions.py','code'],
 ['checker','Completion checks','Mechanical checks and, when needed, a checker model. False/unknown conditions prevent success.','ferb/orchestrator/verify.py','model'],
 ['cleanup','Release / recovery','Stops recordings before targets. Releases tabs/processes, reviews outcomes; dispatch chooses retry, ask or finish.','ferb/orchestrator/supervisor.py','code'],
 ['store','Stores / observability','SQLite tasks, spans, ledger and results; event bus and daily JSON activity. Images are not saved in tool spans.','ferb/store/spans.py','data'],
 ['timers','Timers / sensing','Reminders, scheduled tasks and filtered background activity. Optional enrichment is separate from task routing.','ferb/clock/timers.py','code'],
 ['provider','Model services','External Codex / Claude inference and authentication. Provider errors become runtime failures.','ferb/runtimes/codex_exec.py','external'],
 ['secrets','Sealed credentials','Origin-bound secret handling and Keychain-backed identity. Raw OTPs do not go to the model.','ferb/identity','data']
].map(([id,label,description,source,kind])=>({id,label,description,source,kind}));
const edges = [['owner','rpc'],['rpc','supervisor'],['supervisor','context'],['context','store'],['supervisor','planner'],['planner','provider'],['planner','supervisor'],['supervisor','worker'],['worker','provider'],['worker','mcp'],['mcp','rpc'],['rpc','gateway'],['gateway','browser'],['gateway','mac'],['gateway','shell'],['gateway','files'],['gateway','questions'],['questions','supervisor'],['questions','owner'],['questions','secrets'],['gateway','store'],['gateway','recording'],['recording','browser'],['recording','mac'],['recording','files'],['supervisor','checker'],['checker','mcp'],['supervisor','cleanup'],['cleanup','recording'],['cleanup','browser'],['cleanup','shell'],['cleanup','planner'],['cleanup','supervisor'],['supervisor','store'],['store','rpc'],['timers','supervisor'],['timers','rpc'],['mac','timers'],['browser','timers'],['timers','store'],['shell','files']];
const step=(from,to,title,detail,kind='code',proof='')=>({from,to,title,detail,kind,proof});
const intake=[
 step('owner','rpc','A request arrives','Chat sends your words over authenticated local RPC. A voice turn joins the same planning path.'),
 step('rpc','supervisor','Keep the request with its conversation','The daemon hands the request to Supervisor.send. Disconnecting chat is different from cancelling the task.'),
 step('supervisor','context','Retrieve relevant context','Search memory, recent conversation/tasks, activity and the action registry. Context is bounded, not a complete replay of everything you have done.'),
 step('supervisor','planner','Decide how to respond','A no-tool model chooses reply, clarify or task plus success conditions. Shared interaction instructions carry background and profile preferences.','model'),
 step('planner','supervisor','Validate the plan','Code validates the schema and task references. One repair call is allowed for invalid output; failure here means no worker starts.'),
 step('supervisor','store','Create the task and attempt','Persist task specification and success conditions; register a task tree, fresh run token and liveness tracking.'),
 step('supervisor','worker','Start the chosen worker','The worker gets the installed action registry, owner words, relevant context and success conditions. Retry attempts retain earlier artifacts and uncertain actions.','model'),
 step('worker','mcp','Choose a tool','The worker discovers actions and chooses arguments. This is a model decision, not a keyword-based permission classifier.','model'),
 step('mcp','rpc','Transport the call','FERB MCP sends the run token and tool call over the daemon socket.'),
 step('rpc','gateway','Execute through the gateway','Validate task/token, stop state, schema and capability scopes. Take resource locks. Consequential actions are ledgered before execution.')
];
const finish=[
 step('gateway','store','Record the observation','Redacted tool input/output enters the span store and action events. Ledger outcome is done, failed or unknown. A lost acknowledgment is not proof nothing happened.'),
 step('worker','supervisor','Return a claim and evidence','The worker reports status, summary, artifacts and evidence. This is a claim, not yet the final answer.','model'),
 step('supervisor','checker','Check success conditions','Mechanical file/URL checks run first. Judgment-based conditions use a checker with the same capability scopes, read-only by prompt. It can retrieve earlier task evidence.','model'),
 step('supervisor','cleanup','Release before promising success','Stop unfinished recordings before their targets. Close ordinary owned tabs, detach borrowed/kept tabs, stop ordinary managed processes, retain background=true processes.'),
 step('cleanup','supervisor','Reconcile final resource state','Nonempty cleanup observations trigger a no-tool resource review. If keeping a page open failed, the final answer cannot truthfully claim it is still open.','model'),
 step('supervisor','store','Persist final status','Stop, verification and cleanup affect the final outcome. Final idempotent cleanup also runs on all exit paths.'),
 step('store','rpc','Deliver the result','Attached chat gets the result. Detached/scheduled work is saved for later delivery; cancelled results do not produce a finished notification.'),
 step('rpc','owner','Speak as FERB','One answer should say what was established, where artifacts are, and what remains uncertain. A verified flag alone does not make a claim true.','model')
];
const cases=[];
function add(id,title,prompt,coverage,basis,steps,verdict){cases.push({id,title,prompt,coverage,basis,steps,verdict});}
add('calendar','01 · Check a calendar','Check my calendar in the account I name.','Browser • account evidence • checking','Reconstructed from your pasted calendar conversation; current-code path, not an exact replay.',[...intake,
 step('gateway','browser','Choose the requested profile','The model inventories connected profiles and selects the one you named. FERB’s own profile is the default when no personal profile was requested.','model'),
 step('browser','worker','Read account, date and events','AX/DOM observations must establish the account and requested day. A blank view alone does not prove every calendar is empty.','platform'),...finish],
 'Your pasted run first checked scaler, then found three meetings in the requested account but still reported an unreadable-account check. The map separates profile selection, observed identity and completion judgment.');
add('claude','02 · Open interactive Claude','Can you run an interactive Claude Code session?','Shell • PTY • readiness • lifetime','Local trace inspected: 2a2a77edd48f4f9793caf45f7fbcea18 · 5 Oct 2026. Summarized observations, not every polling call.',[...intake,
 step('gateway','shell','Confirm Claude is installed','shell.start ran command -v claude && claude --version; shell.wait returned exit 0 and Claude Code 2.1.288.','observed','Recorded tool output'),
 step('gateway','shell','Launch a retained process','shell.start ran claude in FERB’s workspace with background=true. It returned process p-61494c72. This is a managed PTY, not an automatically opened Terminal window.','observed','Recorded launch arguments'),
 step('shell','worker','Read the workspace trust screen','shell.read returned the trust prompt. A running process was not yet a ready coding prompt.','observed','Recorded PTY text'),
 step('gateway','shell','Handle the interactive prompt','shell.send sent a down arrow and newline, then further confirmations. The final carriage return was followed by Claude’s welcome screen and input prompt.','observed','Recorded input and output'),
 step('shell','worker','Establish actual readiness','The saved output showed Claude Code, Opus 5.5 with high effort, the workspace and the input prompt. No project task was typed in these recorded calls.','observed','Recorded output after carriage return'),...finish],
 'Trace supports reaching the Claude prompt. It does not establish a visible owner-facing terminal or continued availability now. The checker’s process searches were inconclusive; historical readiness must not be stretched into a live availability claim.');
add('class','03 · Leave the class running','Join the CV class in scaler and let it run.','Browser • retained tabs • cleanup','Based on your pasted class incident; steps explain the current implementation.',[...intake,
 step('gateway','browser','Find and join the class','Use the explicitly requested profile, inspect the class and join. Playback/session evidence is separate from opening a URL.','model'),
 step('gateway','browser','Keep the exact result tab','browser.keep_open transfers the page’s lifetime to the owner. Owned ordinary tabs otherwise close at cleanup.','code'),
 step('browser','worker','Inspect the class state','A room title, screen share and timer support the current class state. Retaining a tab does not prove uninterrupted playback forever.','platform'),...finish],
 'The original contradiction was “left running” followed by cleanup closing the page. Current cleanup and resource review must include the actual keep-open outcome.');
add('otp','04 · Login needs an OTP','Log in to Zomato; ask me when the code arrives.','Sealed OTP • unsupported field • recovery','Based on your pasted OTP failure; split-field limitation remains in current code.',[...intake,
 step('gateway','browser','Request login, then inspect the code fields','Worker actions should observe that the OTP screen actually appeared. Sending the request is not proof of login.','model'),
 step('browser','secrets','Validate the supported secret target','The login-code driver validates task-owned reference, HTTPS origin and recognized field metadata. Unlabelled split boxes can fail here, before any owner question.'),
 step('gateway','questions','Supported branch: hidden code prompt','For a supported field, the gateway asks via hidden terminal input. This bypasses the ordinary question-decision model.'),
 step('questions','secrets','Fill without exposing the OTP','The code travels to the pending secure reply and origin-bound fill, not model text or normal tool logs.'),
 step('worker','supervisor','Unsupported branch: report a blocker','On invalid fields the worker reports the actual limitation; it must not say it asked for a code when it did not.','model'),
 step('cleanup','planner','Choose a supported next approach','Dispatch can retry, ask or finish. Recovery gets the same background/profile instructions; it does not invent permission to take focus.','model'),...finish],
 'These supported/unsupported steps explain alternatives, not both outcomes of one login. The previously rejected six-box OTP remains an explicit limitation; the recording update does not fix it.');
add('whatsapp','05 · Search SST groups','List all WhatsApp group names containing SST.','Native app • scrolling • historical evidence','Based on your pasted WhatsApp thread and owner observations, not a fresh scan.',[...intake,
 step('gateway','mac','Observe the native app','Read the app’s AX tree and exact window. Use the owner-requested WhatsApp context; do not silently substitute Messages.','model'),
 step('gateway','mac','Search and scroll the relevant pane','The model chooses scroll distance and batch count. Pointer input binds to a fresh screenshot/window. A delivered event alone does not prove movement.','model'),
 step('mac','worker','Establish coverage','Inspect loaded results, group/community distinctions and changes after scrolling. One viewport or a frozen lazy-loading list does not establish all matches.','platform'),...finish],
 'A quick historical reply may reuse a previous result. It should say that, instead of claiming a fresh complete search. Earlier snapshots can support specific names but not unseen groups.');
add('record','06 · Record two tabs correctly','Record both requested tabs while I keep working.','New recording • saved pixels • partial capture','Current 0.25.0 implementation walkthrough. Live owner acceptance is still pending.',[...intake,
 step('gateway','browser','Bind the two requested tabs','Each recording uses its own observed task-owned tab reference and original browser/profile identity. No foreground tab selection is required.'),
 step('gateway','recording','Start one recording per target','browser.record returns a task-owned handle. Default 2 samples/second, silent; .mp4 output and receipt must be new paths.'),
 step('recording','browser','Sample independently while actions continue','record_frame checks frame inventory and sensitive fields before/after CDP capture. Its own buffer does not invalidate action references. Capture errors halt sampling rather than substitute the desktop.'),
 step('recording','files','Stop and encode the MP4','recording.stop preserves sample timing and writes target/timing receipts. Encoded repeated frames are not extra captured information. Failure is partial, not success.'),
 step('gateway','recording','Inspect the saved pixels','recording.inspect decodes selected times from the actual MP4 to a contact sheet. Compare target identity and important actions, not just file size/duration.','model'),...finish],
 'The old demo recording captured VS Code despite successful page actions. The new target recording and decoded inspection address that mismatch. Automated checks pass; undisturbed live behavior still needs your observation.');
add('native-record','07 · Record a native window','Record this application window.','Native window/PID • privacy • artifact','Implementation example for the new recording capability; not an observed live run.',[...intake,
 step('gateway','mac','Take a fresh window screenshot','The screenshot identifies the actual window and observed app PID. Screen Recording permission must exist.'),
 step('gateway','recording','Pin the observed window','mac.record uses the screenshot reference. An app restart, wrong window or privacy-sensitive surface cannot silently become a new target.'),
 step('recording','mac','Capture that window without input','Sampling revalidates PID/window and privacy before/after capture. It does not inject clicks or update pointer references.'),
 step('recording','files','Finalize and inspect','Stop produces the video and receipt; inspect decodes saved frames. A failed capture is reported as partial.'),...finish],
 'This sandbox lacks native capture permission. This path is documented from code and covered by offline contracts, not live owner acceptance.');
add('assignment','08 · Read context, create an artifact','Read assignment context, have a coding agent work, report when it finishes.','Cross-app • external agent • files • evidence','Based on your FMI conversation; illustrative current flow, not a replay or a new assignment run.',[...intake,
 step('gateway','browser','Read the requested source account','Observe the email/brief in the selected profile. Store actionable requirements and source evidence, not just a vague task title.','model'),
 step('gateway','files','Write a concrete brief','Create the requested workspace artifact with requirements. A file path is useful evidence only when the file actually exists.'),
 step('gateway','shell','Launch the external coding session','A tracked process receives the brief. The external Claude/Codex session is a workload, distinct from the internal worker that calls shell tools.','model'),
 step('gateway','shell','Read output and wait for completion','Process IDs and exit status establish progress, not artifact correctness. Save usable session/log identifiers where available.'),
 step('gateway','files','Inspect the produced files','Check the deliverables and calculations/layout as appropriate. A passing repository unit test is not an end-to-end FERB benchmark score.','model'),...finish],
 '“Started Claude”, “Claude finished”, “the PDF exists”, and “the work is correct” require different evidence. The final report should make those distinctions and link the artifacts.');
add('schedule','09 · Replace a recurring job','Cancel the recurring reminder and run it once on 7 Oct at noon.','Schedule changes • timers • unattended task','Based on your pasted scheduling exchange; no message is sent by this site.',[
 ...intake.slice(0,5),
 step('supervisor','store','Apply requested schedule changes','The validated decision updates the saved schedule and cancels the old one. Report the actual IDs/results, not just the model’s intention.'),
 step('timers','supervisor','At due time, plan the saved request','A one-shot timer is finished or recurrence rearmed before the scheduled request is planned. Its task uses unattended mode.'),
 ...intake.slice(5),
 step('gateway','questions','If a missing choice requires the owner','An unattended task cannot wait for an interactive owner answer. A needed choice/code becomes a blocker, not a guessed answer.'),...finish],
 'Creating a schedule proves it was scheduled; it does not prove the future action ran. Inspect the occurrence’s own task trace after its due time.');
add('context','10 · Remember and answer from history','Do you still have the syllabus link?','Memory • background activity • direct reply','Based on your syllabus and quick SST reply exchanges; illustrates the no-worker branch.',[
 step('mac','timers','Background context is observed','Filtered device/browser observations can feed background activity. This is separate from running an owner task.'),
 step('timers','store','Persist relevant context','Activity is mirrored to daily JSON and stored history; memory distillation uses model adapters. Optional Jev enrichment is not the planner.'),
 ...intake.slice(0,5),
 step('planner','supervisor','Choose a historical reply','The planner may answer from relevant stored evidence without launching a worker. Current-state claims still require fresh observation.','model'),
 step('supervisor','store','Store requested memory separately','A planner-applied memory change can exist outside the worker’s action list. A checker that examines only worker calls can miss that evidence.'),
 step('rpc','owner','Label the answer as historical','Say whether the link/result was confirmed, previously rejected or still uncertain. Speed is not evidence of a new live search.','model')],
 'Your transcript showed memory-save events before the worker. The later claim “no worker store action” did not by itself prove the link was not durably saved.');
add('question','11 · Resolve a question / recover','Prepare the deck for Rahul; ask before sending.','Question routing • clarification • retry','Illustrative case from FERB’s authorization contract; no external action is executed.',[...intake,
 step('gateway','questions','Worker asks a question','owner.ask first routes through the orchestrator, not directly to the owner.'),
 step('questions','supervisor','Can context settle it?','question.md has no tools. If the owner already asked to send, the orchestrator can use that direction; if the outcome was only preparation, owner input may be needed.','model'),
 step('questions','owner','Ask only the unresolved choice','Interactive questions use chat/voice with a 600-second expiry. The registered owner wait pauses the silence watchdog.'),
 step('worker','supervisor','If the method fails, preserve findings','The worker returns a failure/blocker with artifacts and uncertain actions. The next attempt must not blindly replay a possibly completed send.','model'),
 step('cleanup','planner','Recovery chooses retry, ask or finish','dispatch.md has no tools. Retry selects the next worker/method; ask returns needs_user and ends the attempt rather than waiting inside it.','model'),...finish],
 'Execution has no fixed action-count budget. It does have schema checks, ownership, stop handling, completion conditions and a liveness watchdog.');
add('stop','12 · Stop, provider failure or restart','Stop this task.','Cancellation • unknown actions • restart','Stop is based on the audited demo run; failure/restart are source-based alternate paths.',[
 ...intake.slice(0,7),
 step('owner','rpc','Explicit stop interrupts active work','Ctrl-C/task.stop is different from Ctrl-D leaving chat. The audited first demo attempt had an owner-stop event before any tools ran.','observed'),
 step('rpc','supervisor','Stop wins over recovery','Mark the tree stopped, revoke tokens, cancel runtime handles and prevent new recovery attempts.'),
 step('supervisor','gateway','Cancel and preserve uncertainty','Cancel in-flight calls; a consequential action may remain unknown if acknowledgment was lost. Do not report it as definitely undone.'),
 step('supervisor','cleanup','Release resources on exit','Idempotent cleanup still runs. Owned tabs/processes follow their lifetime settings; recording finalizes before target release.'),
 step('provider','supervisor','Alternative: provider or liveness failure','A provider error is a runtime failure, not automatically owner cancellation. Silence monitoring polls every 5 seconds and cancels after 180 seconds excluding registered waits.'),
 step('rpc','store','Alternative: daemon restart','Active tasks are marked interrupted; unresolved ledger actions become unknown. Startup does not blindly replay them.'),
 step('rpc','owner','Report the actual status','Stopped, failed, needs_user and interrupted are different outcomes. Don’t replace them all with “Stopped.”','model')],
 'This scenario deliberately displays alternate exit paths. They are not all events in one run. No new live task is started by this walkthrough.');
root.FERB_GUIDE={version:'0.25.0',nodes,edges,cases};
if(typeof module!=='undefined') module.exports=root.FERB_GUIDE;
})(globalThis);
