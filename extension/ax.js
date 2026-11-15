// Owns AX-to-DOM evidence mapping; must not infer target identity or silently discard candidates.
// Chromium's accessibility tree supplies names, roles and order. The isolated DOM walker supplies identity,
// geometry, hit testing, forms and redacted values. All actions use that same DOM reference; never a blind AX click.
const AX_ROLES = new Set(["button", "link", "textbox", "searchbox", "combobox", "checkbox", "radio", "menuitem", "tab",
  "option", "switch", "slider", "treeitem", "listitem", "row", "cell", "gridcell", "heading", "menuitemcheckbox", "menuitemradio"]);
const bindingOf = (node) => {                    // the snapshot's binding attribute (protocol.js BIND_ATTR)
  const attrs = node && node.attributes || [];
  for (let i = 0; i + 1 < attrs.length; i += 2) if (attrs[i] === BIND_ATTR) return attrs[i + 1];
  return null;
};
const ADOPT_BATCH = 1000;                        // backend ids per DOM.pushNodesByBackendIdsToFrontend (protocol.js)
const axToken = () => Array.from(crypto.getRandomValues(new Uint8Array(8)), b => b.toString(16).padStart(2, "0")).join("");

// Extension 0.19.6: the same elements, in the same order, with the same ids as one node after another gave, in three
// passes instead of five round trips per node: (1) each frame's AX tree and every candidate's DOM description, asked
// together; (2) every candidate the DOM walker didn't bind, adopted together: one push of their backend ids per debugger
// session, one binding attribute each, then one script in every frame adopting them all in order (ids as before);
// (3) the candidates in the AX tree's order, bound or adopted, as before. A 2,000-cell table took 6.3 s per read that
// way (lab 2026-11-08: 4,200 adoptions, each four CDP calls and a whole-tab script). A command given up (`run.aborted`)
// stops between passes. Returns the counts for the read's `steps`.
async function accessibilitySnapshot(tabId, gen, frames, run) {
  const st = tabState(tabId), bindings = new Map(), docs = new Map(), frameOwners = new Map();
  const live = () => { if (run && run.aborted) throw new Aborted("given up during the accessibility pass"); };
  for (const f of frames) {
    docs.set(f.document_binding, f);
    for (const e of f.elements) if (e.binding) bindings.set(e.binding, {frame: f, element: e});
    for (const owner of f.iframes || []) if (owner.binding) frameOwners.set(owner.binding, {frame: f, owner});
  }
  const jobs = new Map(), byCdpFrame = new Map(), ordered = new Map();
  const sessions = [undefined, ...st.sessions.keys()];
  // Auto-attach only covers immediate children; each attached target enables it recursively.
  await Promise.all(st.sessions.values());
  for (const sessionId of sessions) {
    try {
      const {frameTree} = await cdp(tabId, "Page.getFrameTree", {}, sessionId);
      const visit = (tree, parentId) => {
        if (!tree) return;
        jobs.set(tree.frame.id, {frameId: tree.frame.id, parentId: tree.frame.parentId || parentId, sessionId});
        for (const child of tree.childFrames || []) visit(child, tree.frame.id);
      };
      visit(frameTree);
    } catch (e) { st.axErrors.push(String(e.message || e)); }
  }
  // (1) per frame: its AX tree, then its candidates' descriptions (in flight together, answered in order)
  let candidates = 0;
  for (const job of jobs.values()) {
    live();
    const {frameId, sessionId} = job;
    job.candidates = [];
    try {
      const {nodes} = await cdp(tabId, "Accessibility.getFullAXTree", {frameId}, sessionId);
      const root = (nodes || []).find(n => n.role && n.role.value === "RootWebArea");
      if (root && root.backendDOMNodeId) {
        const {node} = await cdp(tabId, "DOM.describeNode", {backendNodeId: root.backendDOMNodeId, depth: 1}, sessionId);
        const f = docs.get(bindingOf(node)) || (node.children || []).map(n => docs.get(bindingOf(n))).find(Boolean);
        if (f) byCdpFrame.set(frameId, f);
      }
      const wanted = (nodes || []).filter(n => !n.ignored && AX_ROLES.has(n.role && n.role.value) && n.backendDOMNodeId);
      const described = await Promise.allSettled(wanted.map(n =>
        cdp(tabId, "DOM.describeNode", {backendNodeId: n.backendDOMNodeId, depth: 0}, sessionId)));
      wanted.forEach((n, i) => {
        const d = described[i];
        if (d.status === "rejected") { st.axErrors.push(String(d.reason && d.reason.message || d.reason)); return; }
        job.candidates.push({n, binding: bindingOf(d.value.node)});
      });
      candidates += wanted.length;
    } catch (e) { st.axErrors.push(String(e.message || e)); }
  }
  // (2) the unbound ones, adopted together, in the order one after another adopted them (a node met twice, once)
  live();
  const adopted = new Map();                     // backend node id -> {frame, element} (null: none could be adopted)
  const bySession = new Map();                   // session -> backend ids, in order
  for (const job of jobs.values()) for (const c of job.candidates) {
    const id = c.n.backendDOMNodeId;
    if (bindings.has(c.binding) || adopted.has(id)) continue;
    adopted.set(id, null);
    if (!bySession.has(job.sessionId)) bySession.set(job.sessionId, []);
    bySession.get(job.sessionId).push(id);
  }
  const tokens = new Map();                      // backend node id -> its binding token, once its attribute is set
  for (const [sessionId, ids] of bySession) {
    live();
    try {
      await cdp(tabId, "DOM.getDocument", {depth: 0}, sessionId);
      for (let i = 0; i < ids.length; i += ADOPT_BATCH) {
        const some = ids.slice(i, i + ADOPT_BATCH);
        const {nodeIds} = await cdp(tabId, "DOM.pushNodesByBackendIdsToFrontend", {backendNodeIds: some}, sessionId);
        const set = await Promise.allSettled(some.map((id, k) => {
          if (!nodeIds || !nodeIds[k]) return Promise.resolve(null);
          const token = axToken();
          return cdp(tabId, "DOM.setAttributeValue", {nodeId: nodeIds[k], name: BIND_ATTR, value: token}, sessionId)
            .then(() => token);
        }));
        some.forEach((id, k) => { if (set[k].status === "fulfilled" && set[k].value) tokens.set(id, set[k].value); });
      }
    } catch (e) { st.axErrors.push(String(e.message || e)); }
  }
  const order = [...adopted.keys()].filter(id => tokens.has(id));
  if (order.length) {
    live();
    const list = order.map(id => tokens.get(id));
    let results = [];
    try {
      results = await script({target: {tabId, allFrames: true},
        func: (g, t, a) => window.__ferb.adoptAll(g, t, a), args: [gen, list, BIND_ATTR]});
    } catch (e) { st.axErrors.push(String(e.message || e)); }
    order.forEach((id, k) => {                    // each token is in one document: the first frame answering it
      for (const r of results) {
        const el = r && Array.isArray(r.result) ? r.result[k] : null;
        if (!el) continue;
        const frame = frames.find(f => f.frame_id === r.frameId);
        if (!frame) continue;
        frame.elements.push(el);
        adopted.set(id, {frame, element: el});
        break;
      }
    });
  }
  // (3) in the AX tree's order, as before
  live();
  let count = 0;
  for (const job of jobs.values()) for (const {n, binding} of job.candidates) {
    const bound = bindings.get(binding) || adopted.get(n.backendDOMNodeId);
    if (!bound) { st.axErrors.push("AX target has no accessible DOM binding"); continue; }                       // stale/hidden/closed shadow root: never invent a click target
    const {frame, element} = bound;
    byCdpFrame.set(job.frameId, frame);
    element.role = n.role.value;
    if (!element.capture_masked && n.name && n.name.value) element.label = String(n.name.value).replace(/\s+/g, " ");
    // Never copy AX values: password and OTP metadata lives in the DOM, including autocomplete/name/type.
    element.ax = true;
    // The node's own id in the renderer, stable while it lives (a fresh snapshot of the same page finds it again):
    // P19's identity for an act on it (tools.BrowserAct.ledger_args), never a reference to act with.
    element.backend_node = n.backendDOMNodeId;
    const list = ordered.get(frame) || [];
    if (!list.includes(element)) list.push(element);
    ordered.set(frame, list);
    count++;
  }
  for (const [id, f] of byCdpFrame) {
    if (f.frame_id === 0) continue;
    const job = jobs.get(id), parent = jobs.get(job.parentId);
    if (!parent) continue;
    try {
      const {backendNodeId} = await cdp(tabId, "DOM.getFrameOwner", {frameId: id}, parent.sessionId);
      const {node} = await cdp(tabId, "DOM.describeNode", {backendNodeId, depth: 0}, parent.sessionId);
      const bound = frameOwners.get(bindingOf(node));
      if (bound) { f.parent_frame_id = bound.frame.frame_id; f.owner_binding = bound.owner.binding; }
    } catch (e) { st.axErrors.push(String(e.message || e)); }
  }
  for (const f of frames) {
    const primary = ordered.get(f) || [];
    f.elements = primary.concat(f.elements.filter(e => !primary.includes(e)));
    f.ax = primary.length > 0;
  }
  return {ax_nodes: candidates, ax_bound: count, adopted: [...adopted.values()].filter(Boolean).length};
}
