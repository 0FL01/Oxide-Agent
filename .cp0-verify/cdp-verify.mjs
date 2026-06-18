// CP0 P0.5 CDP verification script — throwaway, not production code.
// Connects to Chromium page target WebSocket, sends CDP commands, captures raw responses + latency.
// Node 22 has global WebSocket.

const PAGE_WS = process.argv[2] || "ws://127.0.0.1:9222/devtools/page/F02C8032454AAD0508CFA68E7D5E8931";
const TEST_PAGE = "file:///home/stfu/ai/Oxide-Agent/.cp0-verify/test-page.html";

let msgId = 1;
const pending = new Map(); // id -> {resolve, reject, timer}
const events = []; // captured CDP events

const ws = new WebSocket(PAGE_WS);

function send(method, params = {}, timeoutMs = 10000) {
  const id = msgId++;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`timeout: ${method} id=${id}`)), timeoutMs);
    pending.set(id, { resolve, reject, timer });
    ws.send(JSON.stringify({ id, method, params }));
  });
}

function logResult(label, result, ms) {
  console.log(`\n=== ${label} (${ms}ms) ===`);
  // truncate large responses for readability
  const json = JSON.stringify(result, null, 2);
  console.log(json.length > 4000 ? json.slice(0, 4000) + "\n...[truncated]" : json);
}

ws.addEventListener("open", async () => {
  console.log("WebSocket connected to:", PAGE_WS);

  // Listen for all messages (responses + events)
  ws.addEventListener("message", (ev) => {
    const data = JSON.parse(ev.data);
    if (data.id && pending.has(data.id)) {
      const { resolve, reject, timer } = pending.get(data.id);
      clearTimeout(timer);
      pending.delete(data.id);
      if (data.error) reject(new Error(JSON.stringify(data.error)));
      else resolve(data.result);
    } else if (!data.id) {
      // CDP event
      events.push(data);
    }
  });

  try {
    // ── V1-6: Runtime.evaluate (returnByValue, awaitPromise) ──
    {
      const t = Date.now();
      const r = await send("Runtime.evaluate", {
        expression: "1 + 2",
        returnByValue: true,
      });
      logResult("V1-6a Runtime.evaluate returnByValue (1+2)", r, Date.now() - t);
    }
    {
      const t = Date.now();
      const r = await send("Runtime.evaluate", {
        expression: "new Promise(r => setTimeout(() => r({a:1,b:'hello'}), 50))",
        returnByValue: true,
        awaitPromise: true,
      });
      logResult("V1-6b Runtime.evaluate awaitPromise (Promise→{a:1,b:'hello'})", r, Date.now() - t);
    }

    // ── V1-2: Page.navigate + Page.loadEventFired ──
    // Enable Page domain first to get events
    await send("Page.enable");
    {
      const t = Date.now();
      const navResult = await send("Page.navigate", { url: TEST_PAGE });
      logResult("V1-2a Page.navigate (file://test-page.html)", navResult, Date.now() - t);
    }
    // Wait for Page.loadEventFired event
    {
      const t = Date.now();
      const loaded = await new Promise((resolve, reject) => {
        const to = setTimeout(() => reject(new Error("timeout: Page.loadEventFired")), 10000);
        const handler = (ev) => {
          const d = JSON.parse(ev.data);
          if (d.method === "Page.loadEventFired") {
            clearTimeout(to);
            ws.removeEventListener("message", handler);
            resolve(d);
          }
        };
        ws.addEventListener("message", handler);
      });
      logResult("V1-2b Page.loadEventFired event", loaded, Date.now() - t);
    }

    // ── V1-7: /json/list already verified via curl, record here ──
    console.log("\n=== V1-7 /json/list ===");
    console.log("Verified via curl: GET http://127.0.0.1:9222/json/list returns array with page target webSocketDebuggerUrl");

    // ── V1-1: Accessibility.getFullAXTree ──
    {
      await send("Accessibility.enable");
      const t = Date.now();
      const r = await send("Accessibility.getFullAXTree");
      logResult("V1-1a Accessibility.getFullAXTree (full response)", r, Date.now() - t);
      // Show first few nodes to verify shape
      if (r && r.nodes) {
        console.log(`\n--- V1-1b AX tree: ${r.nodes.length} nodes, first 5 ---`);
        r.nodes.slice(0, 5).forEach((n, i) => {
          console.log(`  [${i}] nodeId=${n.nodeId} backendDOMNodeId=${n.backendDOMNodeId} ignored=${n.ignored} role=${n.role?.value} name=${JSON.stringify(n.name?.value)} childIds=${JSON.stringify(n.childIds)}`);
        });
        // Find the button node
        const btn = r.nodes.find(n => n.role?.value === "button" && n.name?.value === "Login");
        if (btn) {
          console.log(`\n--- V1-1c button "Login" node ---`);
          console.log(`  nodeId=${btn.nodeId} backendDOMNodeId=${btn.backendDOMNodeId} ignored=${btn.ignored} role=${btn.role?.value} name=${JSON.stringify(btn.name?.value)}`);
          console.log(`  → UID format will be: n${btn.backendDOMNodeId}`);
        }
      }
    }

    // ── V1-4: DOM.requestNode + DOM.getBoxModel (uid→coords) ──
    {
      // First get document
      const doc = await send("DOM.getDocument", { depth: 0 });
      const rootId = doc.root.nodeId;
      // Find the button by selector
      const t0 = Date.now();
      const queryResult = await send("DOM.querySelector", {
        nodeId: rootId,
        selector: "#btn-login",
      });
      const btnNodeId = queryResult.nodeId;
      console.log(`\n=== V1-4a DOM.querySelector #btn-login → nodeId=${btnNodeId} (${Date.now()-t0}ms) ===`);

      if (btnNodeId) {
        // Resolve node to get backendNodeId
        const t1 = Date.now();
        const resolved = await send("DOM.resolveNode", { nodeId: btnNodeId });
        logResult("V1-4b DOM.resolveNode (backendNodeId)", resolved, Date.now() - t1);

        // Get box model for click coordinates
        const t2 = Date.now();
        const box = await send("DOM.getBoxModel", { nodeId: btnNodeId });
        logResult("V1-4c DOM.getBoxModel (click coords)", box, Date.now() - t2);
        if (box && box.model) {
          const [x1,y1,x2,y2,x3,y3,x4,y4] = box.model.content;
          const cx = (x1+x3)/2, cy = (y1+y3)/2;
          console.log(`  → click center: (${cx}, ${cy})`);
        }
      }
    }

    // ── V1-3: Input.dispatchMouseEvent (click) ──
    {
      // Use coordinates from button box model above (approximate, should be in viewport)
      // Button is near top of page
      const t = Date.now();
      const r = await send("Input.dispatchMouseEvent", {
        type: "mousePressed",
        x: 50,
        y: 130,
        button: "left",
        clickCount: 1,
      });
      logResult("V1-3a Input.dispatchMouseEvent mousePressed", r, Date.now() - t);
      const r2 = await send("Input.dispatchMouseEvent", {
        type: "mouseReleased",
        x: 50,
        y: 130,
        button: "left",
        clickCount: 1,
      });
      logResult("V1-3b Input.dispatchMouseEvent mouseReleased", r2, Date.now() - t);
    }

    // ── V1-5: Page.captureScreenshot ──
    {
      const t = Date.now();
      const r = await send("Page.captureScreenshot", { format: "png" });
      logResult("V1-5a Page.captureScreenshot (png)", { ...r, data: r.data?.slice(0, 50) + "...[base64, len=" + r.data?.length + "]" }, Date.now() - t);
    }
    {
      const t = Date.now();
      const r = await send("Page.captureScreenshot", { format: "jpeg", quality: 80 });
      logResult("V1-5b Page.captureScreenshot (jpeg q=80)", { ...r, data: r.data?.slice(0, 50) + "...[base64, len=" + r.data?.length + "]" }, Date.now() - t);
    }

    // ── Q1 baseline: latency measurements ──
    console.log("\n=== Q1 baseline latency (5 iterations each) ===");
    for (const [label, method, params] of [
      ["Accessibility.getFullAXTree", "Accessibility.getFullAXTree", {}],
      ["Page.captureScreenshot(png)", "Page.captureScreenshot", { format: "png" }],
      ["Runtime.evaluate(1+1)", "Runtime.evaluate", { expression: "1+1", returnByValue: true }],
    ]) {
      const times = [];
      for (let i = 0; i < 5; i++) {
        const t = Date.now();
        await send(method, params);
        times.push(Date.now() - t);
      }
      console.log(`  ${label}: ${times.join(", ")} ms (avg=${(times.reduce((a,b)=>a+b,0)/times.length).toFixed(1)})`);
    }

    // ── Q1: concurrent vs sequential (3 CDP commands) ──
    console.log("\n=== Q1 concurrent vs sequential (a11y + screenshot + eval) ===");
    // Sequential
    {
      const t = Date.now();
      await send("Accessibility.getFullAXTree");
      await send("Page.captureScreenshot", { format: "png" });
      await send("Runtime.evaluate", { expression: "document.title", returnByValue: true });
      console.log(`  sequential: ${Date.now() - t}ms`);
    }
    // Concurrent
    {
      const t = Date.now();
      await Promise.all([
        send("Accessibility.getFullAXTree"),
        send("Page.captureScreenshot", { format: "png" }),
        send("Runtime.evaluate", { expression: "document.title", returnByValue: true }),
      ]);
      console.log(`  concurrent:  ${Date.now() - t}ms`);
    }

    // ── Console capture verification (stealth-safe approach) ──
    console.log("\n=== Console capture (injected interceptor approach, NO Runtime.enable) ===");
    {
      // Inject a console interceptor without Runtime.enable
      const t = Date.now();
      const r = await send("Runtime.evaluate", {
        expression: `
          (function() {
            if (window.__cp0_console_captured) return "already installed";
            window.__cp0_console_captured = [];
            const origLog = console.log;
            const origWarn = console.warn;
            console.log = function(...args) { window.__cp0_console_captured.push({level:"log",args:args.map(String)}); origLog.apply(console, args); };
            console.warn = function(...args) { window.__cp0_console_captured.push({level:"warn",args:args.map(String)}); origWarn.apply(console, args); };
            return "installed";
          })()
        `,
        returnByValue: true,
      });
      logResult("V1-stealth console interceptor install (no Runtime.enable)", r, Date.now() - t);
      // Trigger a log
      await send("Runtime.evaluate", { expression: 'console.log("intercepted log")', returnByValue: true });
      // Read captured
      const r2 = await send("Runtime.evaluate", { expression: "JSON.stringify(window.__cp0_console_captured)", returnByValue: true });
      logResult("V1-stealth console interceptor captured logs", r2, Date.now() - t);
    }

    // ── Network capture (Network.enable, no Runtime.enable) ──
    console.log("\n=== Network capture (Network.enable, no Runtime.enable) ===");
    {
      await send("Network.enable");
      const t = Date.now();
      // Navigate to trigger network events
      await send("Page.navigate", { url: TEST_PAGE + "?cachebust=" + Date.now() });
      await new Promise(r => setTimeout(r, 1000)); // wait for events
      const netEvents = events.filter(e => e.method?.startsWith("Network."));
      console.log(`  Network events captured: ${netEvents.length}`);
      netEvents.slice(0, 3).forEach((e, i) => {
        console.log(`  [${i}] ${e.method} url=${e.params?.request?.url || e.params?.response?.url || "(none)"}`);
      });
    }

    console.log("\n=== CP0 VERIFICATION COMPLETE ===");
    console.log(`Total events captured: ${events.length}`);
    console.log(`Event methods: ${[...new Set(events.map(e => e.method))].join(", ")}`);
    ws.close();
    process.exit(0);
  } catch (err) {
    console.error("VERIFICATION ERROR:", err.message);
    ws.close();
    process.exit(1);
  }
});

ws.addEventListener("error", (err) => {
  console.error("WebSocket error:", err.message || err);
  process.exit(1);
});

setTimeout(() => {
  console.error("Global timeout (30s)");
  process.exit(1);
}, 30000);
