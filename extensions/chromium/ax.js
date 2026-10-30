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

async function accessibilitySnapshot(tabId, gen, frames) {
  const st = tabState(tabId), bindings = new Map(), docs = new Map(), frameOwners = new Map();
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
  for (const job of jobs.values()) {
    const {frameId, sessionId} = job;
    try {
      const {nodes} = await cdp(tabId, "Accessibility.getFullAXTree", {frameId}, sessionId);
      const root = (nodes || []).find(n => n.role && n.role.value === "RootWebArea");
      if (root && root.backendDOMNodeId) {
        const {node} = await cdp(tabId, "DOM.describeNode", {backendNodeId: root.backendDOMNodeId, depth: 1}, sessionId);
        const f = docs.get(bindingOf(node)) || (node.children || []).map(n => docs.get(bindingOf(n))).find(Boolean);
        if (f) byCdpFrame.set(frameId, f);
      }
      for (const n of nodes || []) {
        const role = n.role && n.role.value;
        if (n.ignored || !AX_ROLES.has(role) || !n.backendDOMNodeId) continue;
        const {node} = await cdp(tabId, "DOM.describeNode", {backendNodeId: n.backendDOMNodeId, depth: 0}, sessionId);
        let bound = bindings.get(bindingOf(node));
        if (!bound) bound = await adoptAXNode(tabId, gen, n.backendDOMNodeId, sessionId, frames);
        if (!bound) { st.axErrors.push("AX target has no accessible DOM binding"); continue; }                       // stale/hidden/closed shadow root: never invent a click target
        const {frame, element} = bound;
        byCdpFrame.set(frameId, frame);
        element.role = role;
        if (!element.capture_masked && n.name && n.name.value) element.label = String(n.name.value).replace(/\s+/g, " ");
        // Never copy AX values: password and OTP metadata lives in the DOM, including autocomplete/name/type.
        element.ax = true;
        // The node's own id in the renderer, stable while it lives (a fresh snapshot of the same page finds it again):
        // P19's identity for an act on it (tools.BrowserAct.ledger_args), never a reference to act with.
        element.backend_node = n.backendDOMNodeId;
        const list = ordered.get(frame) || [];
        if (!list.includes(element)) list.push(element);
        ordered.set(frame, list);
      }
    } catch (e) { st.axErrors.push(String(e.message || e)); }
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
}

async function adoptAXNode(tabId, gen, backendNodeId, sessionId, frames) {
  await cdp(tabId, "DOM.getDocument", {depth: 0}, sessionId);
  const {nodeIds} = await cdp(tabId, "DOM.pushNodesByBackendIdsToFrontend", {backendNodeIds: [backendNodeId]}, sessionId);
  if (!nodeIds || !nodeIds[0]) return null;
  const token = Array.from(crypto.getRandomValues(new Uint8Array(8)), b => b.toString(16).padStart(2, "0")).join("");
  await cdp(tabId, "DOM.setAttributeValue", {nodeId: nodeIds[0], name: BIND_ATTR, value: token}, sessionId);
  const results = await chrome.scripting.executeScript({target: {tabId, allFrames: true},
    func: (g, t, a) => window.__ferb.adopt(g, t, a), args: [gen, token, BIND_ATTR]});
  for (const r of results) {
    if (!r.result) continue;
    const frame = frames.find(f => f.frame_id === r.frameId);
    if (!frame) continue;
    frame.elements.push(r.result);
    return {frame, element: r.result};
  }
  return null;
}
