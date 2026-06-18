// CP0 supplement 2: verify correct click-by-uid path.
// DOM.requestNode takes objectId, NOT backendNodeId.
// Correct path: DOM.pushNodesByBackendIdsToFrontend(backendNodeIds) → nodeIds → DOM.getBoxModel
const PAGE_WS = process.argv[2];
const ws = new WebSocket(PAGE_WS);
let msgId = 1;
const pending = new Map();

ws.addEventListener("message", (ev) => {
  const data = JSON.parse(ev.data);
  if (data.id && pending.has(data.id)) {
    const { resolve, reject, timer } = pending.get(data.id);
    clearTimeout(timer); pending.delete(data.id);
    if (data.error) reject(new Error(JSON.stringify(data.error)));
    else resolve(data.result);
  }
});

function send(method, params = {}, timeoutMs = 5000) {
  const id = msgId++;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`timeout: ${method}`)), timeoutMs);
    pending.set(id, { resolve, reject, timer });
    ws.send(JSON.stringify({ id, method, params }));
  });
}

ws.addEventListener("open", async () => {
  try {
    await send("Page.enable");
    await send("Page.navigate", { url: "file:///home/stfu/ai/Oxide-Agent/.cp0-verify/test-page.html" });
    await new Promise(r => setTimeout(r, 500));
    await send("DOM.enable");
    await send("DOM.getDocument", { depth: 0 }); // required before pushNodesByBackendIdsToFrontend
    await send("Accessibility.enable");
    const ax = await send("Accessibility.getFullAXTree");
    const btn = ax.nodes.find(n => n.role?.value === "button" && n.name?.value === "Login");
    console.log("Button AX: backendDOMNodeId=" + btn.backendDOMNodeId);

    // Path A: DOM.pushNodesByBackendIdsToFrontend
    const t1 = Date.now();
    const pushed = await send("DOM.pushNodesByBackendIdsToFrontend", { backendNodeIds: [btn.backendDOMNodeId] });
    const domNodeId = pushed.nodeIds[0];
    console.log(`DOM.pushNodesByBackendIdsToFrontend([${btn.backendDOMNodeId}]) → nodeId=${domNodeId} (${Date.now()-t1}ms)`);

    const t2 = Date.now();
    const box = await send("DOM.getBoxModel", { nodeId: domNodeId });
    const [x1,y1,,,x3,y3] = box.model.content;
    const cx = (x1+x3)/2, cy = (y1+y3)/2;
    console.log(`DOM.getBoxModel → center=(${cx}, ${cy}) (${Date.now()-t2}ms)`);

    // Click
    const t3 = Date.now();
    await send("Input.dispatchMouseEvent", { type: "mousePressed", x: cx, y: cy, button: "left", clickCount: 1 });
    await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: cx, y: cy, button: "left", clickCount: 1 });
    console.log(`Click at (${cx}, ${cy}) → success (${Date.now()-t3}ms)`);

    // Path B: DOM.resolveNode + DOM.requestNode (objectId)
    const t4 = Date.now();
    const resolved = await send("DOM.resolveNode", { backendNodeId: btn.backendDOMNodeId });
    const objectId = resolved.object.objectId;
    const rn = await send("DOM.requestNode", { objectId });
    console.log(`DOM.resolveNode+requestNode → nodeId=${rn.nodeId} (${Date.now()-t4}ms)`);

    // Verify both give same box model
    const box2 = await send("DOM.getBoxModel", { nodeId: rn.nodeId });
    console.log("Path A nodeId=" + domNodeId + " vs Path B nodeId=" + rn.nodeId + " match=" + (domNodeId === rn.nodeId));

    // Also test DOM.describeNode with backendNodeId
    const desc = await send("DOM.describeNode", { nodeId: domNodeId, depth: 0 });
    console.log("DOM.describeNode: tagName=" + desc.node?.tagName + " backendNodeId=" + desc.node?.backendNodeId);

    console.log("\nCLICK-BY-UID PATH VERIFIED:");
    console.log("  uid=n{backendDOMNodeId} → DOM.pushNodesByBackendIdsToFrontend({backendNodeIds:[N]}) → nodeIds[0] → DOM.getBoxModel({nodeId}) → coords → Input.dispatchMouseEvent");
    console.log("  Alternative: DOM.resolveNode({backendNodeId:N}) → objectId → DOM.requestNode({objectId}) → nodeId");
    ws.close();
    process.exit(0);
  } catch (err) {
    console.error("ERROR:", err.message);
    process.exit(1);
  }
});
setTimeout(() => { console.error("timeout"); process.exit(1); }, 10000);
