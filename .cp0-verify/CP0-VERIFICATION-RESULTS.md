# CP0 — P0.5 CDP Verification on Real Chromium

Date: 2026-06-18
Chromium: Chrome/149.0.7827.102 (headless=new, --no-sandbox)
Protocol-Version: 1.3
Verification scripts: `.cp0-verify/cdp-verify.mjs`, `.cp0-verify/cdp-verify-click-uid.mjs`

## V1-7: /json/list discovery ✓

```
GET http://127.0.0.1:9222/json/list → [
  {
    "id": "F02C8032454AAD0508CFA68E7D5E8931",
    "type": "page",
    "url": "about:blank",
    "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/F02C8032454AAD0508CFA68E7D5E8931"
  }
]
```

`/json/version` → Browser=Chrome/149.0.7827.102, User-Agent contains "HeadlessChrome/149.0.0.0" (stealth patch target).

## V1-6: Runtime.evaluate (returnByValue, awaitPromise) ✓

```
Runtime.evaluate({expression:"1+2", returnByValue:true})
→ {result:{type:"number", value:3, description:"3"}}        // 7ms

Runtime.evaluate({expression:"new Promise(r=>setTimeout(()=>r({a:1,b:'hello'}),50))", returnByValue:true, awaitPromise:true})
→ {result:{type:"object", value:{a:1, b:"hello"}}}          // 53ms
```

## V1-2: Page.navigate + Page.loadEventFired ✓

```
Page.navigate({url:"file://test-page.html"})
→ {frameId:"F02C...", loaderId:"A0A0...", isDownload:false}  // 10ms

Page.loadEventFired event:
→ {method:"Page.loadEventFired", params:{timestamp:99999.08}}  // 23ms after navigate
```

Also captured: `Page.domContentEventFired` (for "domcontentloaded" wait_until), `Page.frameNavigated`, `Page.frameStartedLoading`, `Page.frameStoppedLoading`.

## V1-1: Accessibility.getFullAXTree ✓

```
Accessibility.enable → {}
Accessibility.getFullAXTree → {nodes: [...36 nodes...]}     // 4ms first, 2.6ms avg
```

**Node shape (verified):**
```json
{
  "nodeId": "11",
  "ignored": false,
  "role": {"type": "role", "value": "button"},
  "name": {"type": "computedString", "value": "Login", "sources": [...]},
  "properties": [{"name": "focusable", "value": {"type": "booleanOrUndefined", "value": true}}],
  "parentId": "8",
  "childIds": ["36"],
  "backendDOMNodeId": 11,
  "frameId": "F02C..."
}
```

**Ignored node shape:**
```json
{
  "nodeId": "4",
  "ignored": true,
  "ignoredReasons": [{"name": "uninteresting", "value": {"type": "boolean", "value": true}}],
  "role": {"type": "role", "value": "none"},
  "parentId": "2",
  "childIds": ["8"],
  "backendDOMNodeId": 4
}
```

**Key findings:**
- `role.type` can be `"role"` or `"internalRole"` — noise filter must check `role.value`, not `role.type`.
- `ignored: true` nodes have `role.value: "none"` — confirms noise filter rule "skip ignored + skip role none".
- `backendDOMNodeId` is present on all nodes — stable UID source.
- Button "Login": `backendDOMNodeId=11` → UID = `n11`.
- `name.value` can be empty string "" (paragraph, generic containers).

## V1-4 + V1-3: Click-by-uid path ✓

**CORRECTED:** `DOM.requestNode` takes `objectId`, NOT `backendNodeId` (P0.5 caught this).

**Initialization sequence (P0.5 caught):** `DOM.getDocument({depth:0})` MUST be called before `DOM.pushNodesByBackendIdsToFrontend`, else error: "Document needs to be requested first".

**Verified path A (preferred, 1 call):**
```
DOM.getDocument({depth:0})                                    // required first
DOM.pushNodesByBackendIdsToFrontend({backendNodeIds:[187]})
  → {nodeIds:[10]}                                            // 2ms
DOM.getBoxModel({nodeId:10})
  → {model:{content:[16,116.875,48.625,116.875,48.625,131.875,16,131.875], width:49, height:21}}
  → center = ((16+48.625)/2, (116.875+131.875)/2) = (32.3125, 124.375)   // 1ms
Input.dispatchMouseEvent({type:"mousePressed", x:32.3, y:124.4, button:"left", clickCount:1})  // 2ms
Input.dispatchMouseEvent({type:"mouseReleased", x:32.3, y:124.4, button:"left", clickCount:1}) // 2ms
```

**Verified path B (alternative, 2 calls):**
```
DOM.resolveNode({backendNodeId:187}) → {object:{objectId:"..."}}    // 1ms
DOM.requestNode({objectId:"..."}) → {nodeId:10}                     // 1ms
```

Both paths yield the same nodeId=10. Path A is simpler.

## V1-5: Page.captureScreenshot ✓

```
Page.captureScreenshot({format:"png"})
→ {data:"iVBORw0KGgo..."} (base64, 19948 bytes)                    // 53ms first, 38.2ms avg

Page.captureScreenshot({format:"jpeg", quality:80})
→ {data:"/9j/4AAQ..."} (base64, 20644 bytes)                       // 31ms
```

## Q1: Baseline latency (direct CDP, no pipe) ✓

```
Accessibility.getFullAXTree:  3, 3, 2, 3, 2 ms  (avg=2.6ms)
Page.captureScreenshot(png): 40, 32, 51, 34, 34 ms (avg=38.2ms)
Runtime.evaluate(1+1):       1, 1, 1, 1, 1 ms  (avg=1.0ms)
```

**Concurrent vs sequential (a11y + screenshot + eval):**
```
sequential: 49ms
concurrent:  30ms    → ~1.6x speedup
```

Confirms plan's claim that concurrent CDP commands on a single WebSocket give meaningful speedup on observe.

## Stealth-safe capture ✓

**Console interceptor (no Runtime.enable):**
```
Runtime.evaluate({
  expression: "install console.log/warn interceptor → window.__cp0_console_captured",
  returnByValue: true
})
→ "installed"                                                    // 1ms

// After console.log("intercepted log"):
Runtime.evaluate({expression:"JSON.stringify(window.__cp0_console_captured)"})
→ '[{"level":"log","args":["intercepted log"]}]'               // 3ms
```

**Network capture (Network.enable, no Runtime.enable):**
```
Network.enable → {}
// After navigation:
// 5 network events captured: requestWillBeSent, responseReceived, dataReceived, loadingFinished, policyUpdated
```

**Confirmed: `Runtime.enable` is NOT needed for either console or network capture.**

## Events captured on single WebSocket

23 events from one connection:
- Page: frameStartedNavigating, frameStartedLoading, frameNavigated, domContentEventFired, loadEventFired, frameStoppedLoading
- DOM: setChildNodes, documentUpdated
- Network: requestWillBeSent, responseReceived, dataReceived, loadingFinished, policyUpdated
- Accessibility: loadComplete

All on a single WebSocket — confirms G3 (single CDP connection) is viable.

## Architectural findings for implementation

1. **DOM domain init sequence:** `DOM.getDocument({depth:0})` before any `DOM.pushNodesByBackendIdsToFrontend` call. Must be done once per navigation (document is invalidated on navigation — `DOM.documentUpdated` event fires).
2. **`DOM.requestNode` takes `objectId`, not `backendNodeId`.** Use `DOM.pushNodesByBackendIdsToFrontend({backendNodeIds:[N]})` for backendNodeId→nodeId.
3. **AX `role.type` can be `"internalRole"` (RootWebArea, chromeRole) or `"role"` (button, heading, paragraph, none, StaticText, InlineTextBox).** Noise filter checks `role.value`.
4. **Concurrent CDP on single WebSocket:** ~1.6x speedup on observe. Confirmed.
5. **Stealth-safe capture:** console interceptor via `Runtime.evaluate` + `Page.addScriptToEvaluateOnNewDocument`; network via `Network.enable`. No `Runtime.enable` needed.
6. **All latencies are very low** (direct CDP): a11y ~2.6ms, eval ~1ms, screenshot ~38ms, click ~4ms, DOM ops ~1-2ms. The Python sidecar's pipe overhead (~30-50ms per call) is indeed the bottleneck, not Chrome processing.
