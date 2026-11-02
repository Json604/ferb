// A small, deliberate preview. No autoplay, network calls or live assistant behind it.
const scenes = {
  task: {
    state: "Working quietly",
    request: "Tidy my Downloads. Group the files by project.",
    reply: "I’ll sort them into folders and keep a record of what moved.",
    title: "A little less clutter.",
    detail: "Files organized. Your workspace is still yours.",
  },
  reminder: {
    state: "Right on time",
    request: "Remind me to take a break at 3.",
    reply: "Of course. I’ll give you a nudge when it’s time.",
    title: "Time for a little breather.",
    detail: "Today, 3:00 PM · Still here when you get back.",
  },
  memory: {
    state: "Getting to know you",
    request: "Keep my morning briefs short. Decisions first.",
    reply: "Got it. I’ll lead with what needs your attention.",
    title: "A little more you.",
    detail: "Your preferences, remembered. Yours to change.",
  },
};
const tabs = [...document.querySelectorAll("[data-scene]")];
const panel = document.querySelector("#demo-content");
const reduced = matchMedia("(prefers-reduced-motion: reduce)");
let transition;
function select(tab, animate = true) {
  if (tab.getAttribute("aria-selected") === "true") return;
  tabs.forEach((item) => {
    const active = item === tab;
    item.setAttribute("aria-selected", String(active));
    item.tabIndex = active ? 0 : -1;
  });
  panel.setAttribute("aria-labelledby", tab.id);
  const scene = scenes[tab.dataset.scene];
  Object.entries(scene).forEach(([key, value]) => {
    document.getElementById(`demo-${key}`).textContent = value;
  });
  transition?.cancel();
  if (animate && !reduced.matches)
    transition = panel.animate(
      [
        { opacity: 0.55, transform: "translateY(3px)" },
        { opacity: 1, transform: "translateY(0)" },
      ],
      { duration: 220, easing: "cubic-bezier(.23,1,.32,1)" },
    );
}
tabs.forEach((tab, index) => {
  tab.addEventListener("click", (event) => select(tab, event.detail !== 0));
  tab.addEventListener("keydown", (event) => {
    let next;
    if (event.key === "ArrowRight") next = (index + 1) % tabs.length;
    if (event.key === "ArrowLeft")
      next = (index + tabs.length - 1) % tabs.length;
    if (event.key === "Home") next = 0;
    if (event.key === "End") next = tabs.length - 1;
    if (next === undefined) return;
    event.preventDefault();
    tabs[next].focus();
    select(tabs[next], false);
  });
});
reduced.addEventListener("change", () => {
  if (reduced.matches) transition?.cancel();
});
