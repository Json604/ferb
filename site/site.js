// An illustrated walkthrough using sample data. No live assistant or network requests.
const documentView = `<span class="document-label">LAUNCH NOTES</span><h3>A little room to think.</h3><p>The next big idea starts here.</p><div class="writing-lines"><i></i><i></i><i></i></div><p class="typing-line">And a clear afternoon to make it happen.<span class="caret"></span></p>`;
const scenes = [
  {
    key: "background",
    mode: "background",
    title: "You work. FERB works alongside you.",
    detail: "Research, files and app tasks happen in the background.",
    state: "Preparing your meeting brief",
    caption: "Your work, uninterrupted.",
    window: "Your document",
    view: documentView,
    work: "Preparing your brief",
  },
  {
    key: "input",
    mode: "background",
    title: "Your keyboard is still yours.",
    detail: "FERB notices when you’re typing and stays out of your way.",
    state: "You’re typing. Staying out of your way.",
    caption: "Same cursor. Same focus. Keep going.",
    window: "Your document · editing",
    view: documentView.replace(
      "And a clear afternoon to make it happen.",
      "Here’s the idea I don’t want to lose…",
    ),
    work: "Working in the background",
  },
  {
    key: "secrets",
    mode: "chat",
    title: "Private input. Protected before AI.",
    detail:
      "Passwords, codes and card details use a sealed path. The model gets a handle, not the secret.",
    state: "Sensitive input protected",
    request: "Sign in to my project account.",
    reply:
      "Your saved credential can be filled without exposing it to the model.",
    result: "Secret stays sealed.",
    resultDetail: "The AI sees a reference, not the value.",
    caption: "Illustration of the protected input path.",
    window: "Account sign-in · sample",
    view: '<span class="document-label">ACCOUNT SIGN-IN</span><h3>Welcome back.</h3><p>Password</p><div class="secure-field">••••••••••••</div><p>Filled through the sealed path.</p>',
    work: "Protected input",
  },
  {
    key: "local",
    mode: "chat",
    title: "Your history lives on your Mac.",
    detail:
      "Local storage. Relevant, redacted context can go to your chosen AI provider for processing.",
    state: "Stored on this Mac",
    request: "What do you remember about my work?",
    reply: "Your preferences and task history are stored here, on your Mac.",
    result: "Local records. Your control.",
    resultDetail: "Review, correct or remove what FERB remembers.",
    caption: "Local storage does not mean offline AI.",
    window: "FERB · local records",
    view: '<span class="document-label">ON THIS MAC</span><h3>Your work has a home.</h3><div class="data-pills"><span>Task history</span><span>Preferences</span><span>Memory</span></div><div class="writing-lines"><i></i><i></i></div>',
    work: "Local records",
  },
  {
    key: "reminder",
    mode: "reminder",
    title: "A nudge, right when you need it.",
    detail:
      "Reminders and scheduled work arrive from the notch and stay in your chat.",
    state: "Reminder",
    result: "Take a little breather.",
    resultDetail: "Today, 3:00 PM · Your next meeting is in 15 minutes.",
    caption: "A reminder without opening the whole chat.",
    window: "Your document",
    view: documentView,
    work: "Morning brief · scheduled",
  },
  {
    key: "memory",
    mode: "chat",
    title: "A little more you, every day.",
    detail:
      "Preferences and lessons from repeated work help FERB adapt. You can change what it remembers.",
    state: "Memory",
    request: "Keep my morning briefs short. Decisions first.",
    reply: "Got it. I’ll lead with what needs your attention.",
    result: "Preference remembered.",
    resultDetail: "Yours to review, correct or forget.",
    caption: "Remembered preferences, not repeated instructions.",
    window: "Your preferences",
    view: '<span class="document-label">MEMORY</span><h3>Decisions first.</h3><p>Keep morning briefs short.</p><div class="data-pills"><span>Always on</span><span>Editable</span></div>',
    work: "Learning your preferences",
  },
  {
    key: "approval",
    mode: "chat",
    title: "You have the last word.",
    detail:
      "FERB prepares consequential actions, asks when required, and lets you stop a task.",
    state: "Needs your decision",
    request: "Send the meeting brief to Alex.",
    reply:
      "The brief is ready. Review the destination before it leaves your Mac.",
    result: "Send the brief to Alex?",
    resultDetail: "Waiting for your approval. Nothing is sent in this demo.",
    caption: "A prepared action. A clear decision.",
    window: "Brief · ready to review",
    view: '<span class="document-label">MEETING BRIEF</span><h3>Ready when you are.</h3><p>Two decisions. Three project updates.</p><div class="writing-lines"><i></i><i></i><i></i></div>',
    work: "Awaiting approval",
  },
  {
    key: "ai",
    mode: "chat",
    title: "Your assistant. Your choice of AI.",
    detail:
      "Connect ChatGPT, Claude, or an API key. FERB works with the service you choose.",
    state: "Connected to your AI",
    request: "Let’s get started.",
    reply: "Pick your AI, connect it, and give me something to do.",
    result: "At home on your Mac.",
    resultDetail: "Tasks, reminders and memory in one place.",
    caption: "Bring the AI you already use.",
    window: "Choose your AI",
    view: '<span class="document-label">CONNECTIONS</span><h3>A familiar connection.</h3><div class="data-pills"><span>ChatGPT</span><span>Claude</span><span>API key</span></div>',
    work: "Ready to help",
  },
];
const tabs = [...document.querySelectorAll("[data-scene]")];
const reduced = matchMedia("(prefers-reduced-motion: reduce)");
const panel = document.querySelector("#demo-content");
const desktop = document.querySelector(".desktop-scene");
const play = document.querySelector("#play-tour");
let current = 0,
  timer,
  playing = false;
let animations = [];
let hasStarted = false;
const text = (id, value) =>
  (document.getElementById(id).textContent = value || "");
function show(index, animate = true) {
  current = index;
  const scene = scenes[index];
  animations.forEach((animation) => animation.cancel());
  animations = [];
  desktop.dataset.mode = scene.mode;
  text("demo-state", scene.state);
  text("demo-request", scene.request);
  text("demo-reply", scene.reply);
  text("demo-title", scene.result);
  text("demo-detail", scene.resultDetail);
  text("story-title", scene.title);
  text("story-detail", scene.detail);
  text("scene-caption", scene.caption);
  text("window-label", scene.window);
  text("work-title", scene.work);
  text("scene-number", `${String(index + 1).padStart(2, "0")} / 08`);
  // Only locally authored templates enter this illustration; no external input.
  document.getElementById("workspace-body").innerHTML = scene.view;
  tabs.forEach((tab, i) => {
    tab.setAttribute("aria-selected", String(i === index));
    tab.tabIndex = i === index ? 0 : -1;
  });
  panel.setAttribute("aria-labelledby", tabs[index].id);
  if (animate && !reduced.matches) {
    if (scene.key === "input") {
      animations.push(
        document
          .querySelector(".typing-line")
          .animate(
            [
              { clipPath: "inset(0 100% 0 0)" },
              { clipPath: "inset(0 0% 0 0)" },
            ],
            { duration: 2100, easing: "steps(38, end)", fill: "backwards" },
          ),
      );
    }
    if (scene.key === "background") {
      document.querySelectorAll(".work-row").forEach((row, i) =>
        animations.push(
          row.animate(
            [
              { opacity: 0.25, transform: "translateY(5px)" },
              { opacity: 1, transform: "translateY(0)" },
            ],
            {
              duration: 500,
              delay: i * 450,
              easing: "ease-out",
              fill: "backwards",
            },
          ),
        ),
      );
    }

    animations.push(
      panel.animate(
        [
          { opacity: 0.55, transform: "translateY(-6px)" },
          { opacity: 1, transform: "translateY(0)" },
        ],
        { duration: 280, easing: "cubic-bezier(.23,1,.32,1)" },
      ),
    );
    animations.push(
      document.querySelector(".workspace-body").animate(
        [
          { opacity: 0.5, transform: "translateY(5px)" },
          { opacity: 1, transform: "translateY(0)" },
        ],
        { duration: 300, easing: "cubic-bezier(.23,1,.32,1)" },
      ),
    );
  }
}
function stop() {
  clearTimeout(timer);
  playing = false;
  play.textContent =
    current === scenes.length - 1 ? "Replay tour" : "Play tour";
  play.setAttribute("aria-pressed", "false");
}
function schedule() {
  clearTimeout(timer);
  timer = setTimeout(() => {
    if (current === scenes.length - 1) {
      stop();
      return;
    }
    show(current + 1);
    schedule();
  }, 6500);
}
play.addEventListener("click", (event) => {
  hasStarted = true;
  if (playing) {
    stop();
    return;
  }
  if (current === scenes.length - 1) show(0, event.detail !== 0);
  playing = true;
  play.textContent = "Pause tour";
  play.setAttribute("aria-pressed", "true");
  schedule();
});
tabs.forEach((tab, index) => {
  tab.addEventListener("click", (event) => {
    hasStarted = true;
    stop();
    show(index, event.detail !== 0);
  });
  tab.addEventListener("keydown", (event) => {
    let next;
    if (event.key === "ArrowRight") next = (index + 1) % tabs.length;
    if (event.key === "ArrowLeft")
      next = (index + tabs.length - 1) % tabs.length;
    if (event.key === "Home") next = 0;
    if (event.key === "End") next = tabs.length - 1;
    if (next === undefined) return;
    event.preventDefault();
    hasStarted = true;
    stop();
    show(next, false);
    tabs[next].focus();
  });
});
new IntersectionObserver(
  (entries) => {
    const entry = entries[0];
    if (!entry.isIntersecting) {
      stop();
      return;
    }
    if (!hasStarted && !reduced.matches) {
      hasStarted = true;
      playing = true;
      play.textContent = "Pause tour";
      play.setAttribute("aria-pressed", "true");
      schedule();
    }
  },
  { threshold: 0.55 },
).observe(desktop);
document.addEventListener("visibilitychange", () => {
  if (document.hidden) stop();
});
reduced.addEventListener("change", () => {
  stop();
  animations.forEach((animation) => animation.cancel());
});
show(0, false);
