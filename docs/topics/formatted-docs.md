# Changes Summary

35 files changed, +2395/-451 lines

---

## CHANGELOG.md

**Lines changed:** +1 additions, 0 deletions

```diff
@@ -21,6 +21,7 @@ Docs: https://docs.openclaw.ai
 - Gateway: add SecretRef support for gateway.auth.token with auth-mode guardrails. (#35094) Thanks @joshavant.
 - Plugins/hook policy: add `plugins.entries.<id>.hooks.allowPromptInjection`, validate unknown typed hook names at runtime, and preserve legacy `before_agent_start` model/provider overrides while stripping prompt-mutating fields when prompt injection is disabled. (#36567) thanks @gumadeiras.
 - Tools/Diffs guidance: restore a short system-prompt hint for enabled diffs while keeping the detailed instructions in the companion skill, so diffs usage guidance stays out of user-prompt space. (#36904) thanks @gumadeiras.
+- Telegram/ACP topic bindings: accept Telegram Mac Unicode dash option prefixes in `/acp spawn`, support Telegram topic thread binding (`--thread here|auto`), route bound-topic follow-ups to ACP sessions, add actionable Telegram approval buttons with prefixed approval-id resolution, and pin successful bind confirmations in-topic. (#36683) Thanks @huntharo.

### Breaking
```

---

## docs/channels/telegram.md

**Lines changed:** +7 additions, 0 deletions

```diff
@@ -524,6 +524,13 @@ curl "https://api.telegram.org/bot<bot_token>/getUpdates"

     This is currently scoped to forum topics in groups and supergroups.

     **Thread-bound ACP spawn from chat**:
     - `/acp spawn <agent> --thread here|auto` can bind the current Telegram topic to a new ACP session.
     - Follow-up topic messages route to the bound ACP session directly (no `/acp steer` required).
     - OpenClaw pins the spawn confirmation message in-topic after a successful bind.
     - Requires `channels.telegram.threadBindings.spawnAcpSessions=true`.
     Template context includes:
 
     - `MessageThreadId`
```

---

## docs/tools/acp-agents.md

**Lines changed:** +7 additions, 2 deletions

```diff
@@ -79,11 +79,14 @@ Required feature flags for thread-bound ACP:
 - `acp.dispatch.enabled` is on by default (set `false` to pause ACP dispatch)
 - Channel-adapter ACP thread-spawn flag enabled (adapter-specific)
   - Discord: `channels.discord.threadBindings.spawnAcpSessions=true`
   - Telegram: `channels.telegram.threadBindings.spawnAcpSessions=true`

 ### Thread supporting channels
 
 - Any channel adapter that exposes session/thread binding capability.
-- Current built-in support: Discord.
+- Current built-in support:
+  - Discord threads/channels
+  - Telegram topics (forum topics in groups/supergroups and DM topics)
 - Plugin channels can add support through the same binding interface.
 
 ## Channel specific settings
 @@ -303,7 +306,9 @@ If no target resolves, OpenClaw returns a clear error (`Unable to resolve sessio
 Notes:
 
 - On non-thread binding surfaces, default behavior is effectively `off`.
-- Thread-bound spawn requires channel policy support (for Discord: `channels.discord.threadBindings.spawnAcpSessions=true`).
+- Thread-bound spawn requires channel policy support:
+  - Discord: `channels.discord.threadBindings.spawnAcpSessions=true`
+  - Telegram: `channels.telegram.threadBindings.spawnAcpSessions=true`
 
 ## ACP controls
```

---

## extensions/acpx/src/runtime-internals/test-fixtures.ts

**Lines changed:** +4 additions, 0 deletions

```diff
@@ -223,6 +223,10 @@ if (command === "prompt") {
     process.exit(1);
   }
   if (stdinText.includes("permission-denied")) {
+    process.exit(5);
+  }
+  if (stdinText.includes("split-spacing")) {
     emitUpdate(sessionFromOption, {
       sessionUpdate: "agent_message_chunk",
```

---

## extensions/acpx/src/runtime.test.ts

**Lines changed:** +36 additions, 0 deletions

```diff
@@ -224,6 +224,42 @@ describe("AcpxRuntime", () => {
     });
   });

+  it("maps acpx permission-denied exits to actionable guidance", async () => {
+    const runtime = sharedFixture?.runtime;
+    expect(runtime).toBeDefined();
+    if (!runtime) {
+      throw new Error("shared runtime fixture missing");
+    }
+    const handle = await runtime.ensureSession({
+      sessionKey: "agent:codex:acp:permission-denied",
+      agent: "codex",
+      mode: "persistent",
+    });
+    const events = [];
+    for await (const event of runtime.runTurn({
+      handle,
+      text: "permission-denied",
+      mode: "prompt",
+      requestId: "req-perm",
+    })) {
+      events.push(event);
+    }
+    expect(events).toContainEqual(
+      expect.objectContaining({
+        type: "error",
+        message: expect.stringContaining("Permission denied by ACP runtime (acpx)."),
+      }),
+    );
+    expect(events).toContainEqual(
+      expect.objectContaining({
+        type: "error",
+        message: expect.stringContaining("approve-reads, approve-all, deny-all"),
+      }),
+    );
+  });
   it("supports cancel and close using encoded runtime handle state", async () => {
     const { runtime, logPath, config } = await createMockRuntimeFixture();
     const handle = await runtime.ensureSession({
```

---

## extensions/acpx/src/runtime.ts

**Lines changed:** +28 additions, 2 deletions

```diff
@@ -42,10 +42,30 @@ export const ACPX_BACKEND_ID = "acpx";

 const ACPX_RUNTIME_HANDLE_PREFIX = "acpx:v1:";
 const DEFAULT_AGENT_FALLBACK = "codex";
+const ACPX_EXIT_CODE_PERMISSION_DENIED = 5;
 const ACPX_CAPABILITIES: AcpRuntimeCapabilities = {
   controls: ["session/set_mode", "session/set_config_option", "session/status"],
 };

+function formatPermissionModeGuidance(): string {
+  return "Configure plugins.entries.acpx.config.permissionMode to one of: approve-reads, approve-all, deny-all.";
+}
+function formatAcpxExitMessage(params: {
+  stderr: string;
+  exitCode: number | null | undefined;
+}): string {
+  const stderr = params.stderr.trim();
+  if (params.exitCode === ACPX_EXIT_CODE_PERMISSION_DENIED) {
+    return [
+      stderr || "Permission denied by ACP runtime (acpx).",
+      "ACPX blocked a write/exec permission request in a non-interactive session.",
+      formatPermissionModeGuidance(),
+    ].join(" ");
+  }
+  return stderr || `acpx exited with code ${params.exitCode ?? "unknown"}`;
+}
 export function encodeAcpxRuntimeHandleState(state: AcpxHandleState): string {
   const payload = Buffer.from(JSON.stringify(state), "utf8").toString("base64url");
   return `${ACPX_RUNTIME_HANDLE_PREFIX}${payload}`;
@@ -333,7 +353,10 @@ export class AcpxRuntime implements AcpRuntime {
       if ((exit.code ?? 0) !== 0 && !sawError) {
         yield {
           type: "error",
-          message: stderr.trim() || `acpx exited with code ${exit.code ?? "unknown"}`,
+          message: formatAcpxExitMessage({
+            stderr,
+            exitCode: exit.code,
+          }),
         };
         return;
       }
@@ -639,7 +662,10 @@ export class AcpxRuntime implements AcpRuntime {
     if ((result.code ?? 0) !== 0) {
       throw new AcpRuntimeError(
         params.fallbackCode,
-        result.stderr.trim() || `acpx exited with code ${result.code ?? "unknown"}`,
+        formatAcpxExitMessage({
+          stderr: result.stderr,
+          exitCode: result.code,
+        }),
       );
     }
     return events;
```

---

## src/auto-reply/commands-registry.data.ts

**Lines changed:** +3 additions, 2 deletions

```diff
@@ -354,7 +354,8 @@ function buildChatCommands(): ChatCommandDefinition[] {
     defineChatCommand({
       key: "focus",
       nativeName: "focus",
-      description: "Bind this Discord thread (or a new one) to a session target.",
+      description:
+        "Bind this thread (Discord) or topic/conversation (Telegram) to a session target.",
       textAlias: "/focus",
       category: "management",
       args: [
@@ -369,7 +370,7 @@ function buildChatCommands(): ChatCommandDefinition[] {
     defineChatCommand({
       key: "unfocus",
       nativeName: "unfocus",
-      description: "Remove the current Discord thread binding.",
+      description: "Remove the current thread (Discord) or topic/conversation (Telegram) binding.",
       textAlias: "/unfocus",
       category: "management",
     }),
```

---

## src/auto-reply/reply/discord-context.ts → channel-context.ts

**Lines changed:** +15 additions, 5 deletions

**File renamed:** `src/auto-reply/reply/discord-context.ts` → `src/auto-reply/reply/channel-context.ts`

```diff
@@ -17,19 +17,29 @@ type DiscordAccountParams = {
 };

 export function isDiscordSurface(params: DiscordSurfaceParams): boolean {
   return resolveCommandSurfaceChannel(params) === "discord";
 }
+export function isTelegramSurface(params: DiscordSurfaceParams): boolean {
+  return resolveCommandSurfaceChannel(params) === "telegram";
+}
 export function resolveCommandSurfaceChannel(params: DiscordSurfaceParams): string {
   const channel =
     params.ctx.OriginatingChannel ??
     params.command.channel ??
     params.ctx.Surface ??
     params.ctx.Provider;
-  return (
-    String(channel ?? "")
-      .trim()
-      .toLowerCase() === "discord"
-  );
+  return String(channel ?? "")
+    .trim()
+    .toLowerCase();
 }

 export function resolveDiscordAccountId(params: DiscordAccountParams): string {
   return resolveChannelAccountId(params);
 }
+export function resolveChannelAccountId(params: DiscordAccountParams): string {
+  const accountId = typeof params.ctx.AccountId === "string" ? params.ctx.AccountId.trim() : "";
+  return accountId || "default";
+}
```

---

## src/auto-reply/reply/commands-acp.test.ts

**Lines changed:** +150 additions, 9 deletions

```diff
@@ -118,7 +118,7 @@ type FakeBinding = {
   targetSessionKey: string;
   targetKind: "subagent" | "session";
   conversation: {
-    channel: "discord";
+    channel: "discord" | "telegram";
     accountId: string;
     conversationId: string;
     parentConversationId?: string;
@@ -242,7 +242,11 @@ function createSessionBindingCapabilities() {

 type AcpBindInput = {
   targetSessionKey: string;
-  conversation: { accountId: string; conversationId: string };
+  conversation: {
+    channel?: "discord" | "telegram";
+    accountId: string;
+    conversationId: string;
+  };
   placement: "current" | "child";
   metadata?: Record<string, unknown>;
 };
@@ -251,14 +255,22 @@ function createAcpThreadBinding(input: AcpBindInput): FakeBinding {
   const nextConversationId =
     input.placement === "child" ? "thread-created" : input.conversation.conversationId;
   const boundBy = typeof input.metadata?.boundBy === "string" ? input.metadata.boundBy : "user-1";
+  const channel = input.conversation.channel ?? "discord";
   return createSessionBinding({
     targetSessionKey: input.targetSessionKey,
-    conversation: {
-      channel: "discord",
-      accountId: input.conversation.accountId,
-      conversationId: nextConversationId,
-      parentConversationId: "parent-1",
-    },
+    conversation:
+      channel === "discord"
+        ? {
+            channel: "discord",
+            accountId: input.conversation.accountId,
+            conversationId: nextConversationId,
+            parentConversationId: "parent-1",
+          }
+        : {
+            channel: "telegram",
+            accountId: input.conversation.accountId,
+            conversationId: nextConversationId,
+          },
     metadata: { boundBy, webhookId: "wh-1" },
   });
 }
@@ -297,6 +309,31 @@ function createThreadParams(commandBody: string, cfg: OpenClawConfig = baseCfg)
   return params;
 }

+function createTelegramTopicParams(commandBody: string, cfg: OpenClawConfig = baseCfg) {
+  const params = buildCommandTestParams(commandBody, cfg, {
+    Provider: "telegram",
+    Surface: "telegram",
+    OriginatingChannel: "telegram",
+    OriginatingTo: "telegram:-1003841603622",
+    AccountId: "default",
+    MessageThreadId: "498",
+  });
+  params.command.senderId = "user-1";
+  return params;
+}
+function createTelegramDmParams(commandBody: string, cfg: OpenClawConfig = baseCfg) {
+  const params = buildCommandTestParams(commandBody, cfg, {
+    Provider: "telegram",
+    Surface: "telegram",
+    OriginatingChannel: "telegram",
+    OriginatingTo: "telegram:123456789",
+    AccountId: "default",
+  });
+  params.command.senderId = "user-1";
+  return params;
+}
 async function runDiscordAcpCommand(commandBody: string, cfg: OpenClawConfig = baseCfg) {
   return handleAcpCommand(createDiscordParams(commandBody, cfg), true);
 }
@@ -305,6 +342,14 @@ async function runThreadAcpCommand(commandBody: string, cfg: OpenClawConfig = ba
   return handleAcpCommand(createThreadParams(commandBody, cfg), true);
 }

+async function runTelegramAcpCommand(commandBody: string, cfg: OpenClawConfig = baseCfg) {
+  return handleAcpCommand(createTelegramTopicParams(commandBody, cfg), true);
+}
+async function runTelegramDmAcpCommand(commandBody: string, cfg: OpenClawConfig = baseCfg) {
+  return handleAcpCommand(createTelegramDmParams(commandBody, cfg), true);
+}
 describe("/acp command", () => {
   beforeEach(() => {
     acpManagerTesting.resetAcpSessionManagerForTests();
@@ -448,10 +493,70 @@ describe("/acp command", () => {
     expect(seededWithoutEntry?.runtimeSessionName).toContain(":runtime");
   });

+  it("accepts unicode dash option prefixes in /acp spawn args", async () => {
+    const result = await runThreadAcpCommand(
+      "/acp spawn codex \u2014mode oneshot \u2014thread here \u2014cwd /home/bob/clawd \u2014label jeerreview",
+    );
+    expect(result?.reply?.text).toContain("Spawned ACP session agent:codex:acp:");
+    expect(result?.reply?.text).toContain("Bound this thread to");
+    expect(hoisted.ensureSessionMock).toHaveBeenCalledWith(
+      expect.objectContaining({
+        agent: "codex",
+        mode: "oneshot",
+        cwd: "/home/bob/clawd",
+      }),
+    );
+    expect(hoisted.sessionBindingBindMock).toHaveBeenCalledWith(
+      expect.objectContaining({
+        placement: "current",
+        metadata: expect.objectContaining({
+          label: "jeerreview",
+        }),
+      }),
+    );
+  });
+  it("binds Telegram topic ACP spawns to full conversation ids", async () => {
+    const result = await runTelegramAcpCommand("/acp spawn codex --thread here");
+    expect(result?.reply?.text).toContain("Spawned ACP session agent:codex:acp:");
+    expect(result?.reply?.text).toContain("Bound this conversation to");
+    expect(result?.reply?.channelData).toEqual({ telegram: { pin: true } });
+    expect(hoisted.sessionBindingBindMock).toHaveBeenCalledWith(
+      expect.objectContaining({
+        placement: "current",
+        conversation: expect.objectContaining({
+          channel: "telegram",
+          accountId: "default",
+          conversationId: "-1003841603622:topic:498",
+        }),
+      }),
+    );
+  });
+  it("binds Telegram DM ACP spawns to the DM conversation id", async () => {
+    const result = await runTelegramDmAcpCommand("/acp spawn codex --thread here");
+    expect(result?.reply?.text).toContain("Spawned ACP session agent:codex:acp:");
+    expect(result?.reply?.text).toContain("Bound this conversation to");
+    expect(result?.reply?.channelData).toBeUndefined();
+    expect(hoisted.sessionBindingBindMock).toHaveBeenCalledWith(
+      expect.objectContaining({
+        placement: "current",
+        conversation: expect.objectContaining({
+          channel: "telegram",
+          accountId: "default",
+          conversationId: "123456789",
+        }),
+      }),
+    );
+  });
   it("requires explicit ACP target when acp.defaultAgent is not configured", async () => {
     const result = await runDiscordAcpCommand("/acp spawn");

     expect(result?.reply?.text).toContain("ACP target agent is required");
     expect(result?.reply?.text).toContain("ACP target harness id is required");
     expect(hoisted.ensureSessionMock).not.toHaveBeenCalled();
   });

@@ -528,6 +633,42 @@ describe("/acp command", () => {
     expect(result?.reply?.text).toContain("Applied steering.");
   });

+  it("resolves bound Telegram topic ACP sessions for /acp steer without explicit target", async () => {
+    hoisted.sessionBindingResolveByConversationMock.mockImplementation(
+      (ref: { channel?: string; accountId?: string; conversationId?: string }) =>
+        ref.channel === "telegram" &&
+        ref.accountId === "default" &&
+        ref.conversationId === "-1003841603622:topic:498"
+          ? createSessionBinding({
+              targetSessionKey: defaultAcpSessionKey,
+              conversation: {
+                channel: "telegram",
+                accountId: "default",
+                conversationId: "-1003841603622:topic:498",
+              },
+            })
+          : null,
+    );
+    hoisted.readAcpSessionEntryMock.mockReturnValue(createAcpSessionEntry());
+    hoisted.runTurnMock.mockImplementation(async function* () {
+      yield { type: "text_delta", text: "Viewed diver package." };
+      yield { type: "done" };
+    });
+    const result = await runTelegramAcpCommand("/acp steer use npm to view package diver");
+    expect(hoisted.runTurnMock).toHaveBeenCalledWith(
+      expect.objectContaining({
+        handle: expect.objectContaining({
+          sessionKey: defaultAcpSessionKey,
+        }),
+        mode: "steer",
+        text: "use npm to view package diver",
+      }),
+    );
+    expect(result?.reply?.text).toContain("Viewed diver package.");
+  });
   it("blocks /acp steer when ACP dispatch is disabled by policy", async () => {
     const cfg = {
       ...baseCfg,
```

---

## src/auto-reply/reply/commands-acp/context.test.ts

**Lines changed:** +18 additions, 0 deletions

```diff
@@ -108,4 +108,22 @@ describe("commands-acp context", () => {
     });
     expect(resolveAcpCommandConversationId(params)).toBe("-1001234567890:topic:42");
   });
+  it("resolves Telegram DM conversation ids from telegram targets", () => {
+    const params = buildCommandTestParams("/acp status", baseCfg, {
+      Provider: "telegram",
+      Surface: "telegram",
+      OriginatingChannel: "telegram",
+      OriginatingTo: "telegram:123456789",
+    });
+    expect(resolveAcpCommandBindingContext(params)).toEqual({
+      channel: "telegram",
+      accountId: "default",
+      threadId: undefined,
+      conversationId: "123456789",
+      parentConversationId: "123456789",
+    });
+    expect(resolveAcpCommandConversationId(params)).toBe("123456789");
+  });
 });
```

---

## src/auto-reply/reply/commands-acp/context.ts

**Lines changed:** +20 additions, 10 deletions

```diff
@@ -6,6 +6,7 @@ import { DISCORD_THREAD_BINDING_CHANNEL } from "../../../channels/thread-binding
 import { resolveConversationIdFromTargets } from "../../../infra/outbound/conversation-id.js";
 import { parseAgentSessionKey } from "../../../routing/session-key.js";
 import type { HandleCommandsParams } from "../commands-types.js";
+import { resolveTelegramConversationId } from "../telegram-context.js";

 function normalizeString(value: unknown): string {
   if (typeof value === "string") {
@@ -40,19 +41,28 @@ export function resolveAcpCommandThreadId(params: HandleCommandsParams): string
 export function resolveAcpCommandConversationId(params: HandleCommandsParams): string | undefined {
   const channel = resolveAcpCommandChannel(params);
   if (channel === "telegram") {
+    const telegramConversationId = resolveTelegramConversationId({
+      ctx: {
+        MessageThreadId: params.ctx.MessageThreadId,
+        OriginatingTo: params.ctx.OriginatingTo,
+        To: params.ctx.To,
+      },
+      command: {
+        to: params.command.to,
+      },
+    });
+    if (telegramConversationId) {
+      return telegramConversationId;
+    }
     const threadId = resolveAcpCommandThreadId(params);
     const parentConversationId = resolveAcpCommandParentConversationId(params);
     if (threadId && parentConversationId) {
       const canonical = buildTelegramTopicConversationId({
         chatId: parentConversationId,
         topicId: threadId,
       });
       if (canonical) {
         return canonical;
       }
     }
     if (threadId) {
-      return threadId;
+      return (
+        buildTelegramTopicConversationId({
+          chatId: parentConversationId,
+          topicId: threadId,
+        }) ?? threadId
+      );
     }
   }
   return resolveConversationIdFromTargets({
```

---

## src/auto-reply/reply/commands-acp/lifecycle.ts

**Lines changed:** +32 additions, 22 deletions

```diff
@@ -37,7 +37,7 @@ import type { CommandHandlerResult, HandleCommandsParams } from "../commands-typ
 import {
   resolveAcpCommandAccountId,
   resolveAcpCommandBindingContext,
-  resolveAcpCommandThreadId,
+  resolveAcpCommandConversationId,
 } from "./context.js";
 import {
   ACP_STEER_OUTPUT_LIMIT,
@@ -123,37 +123,39 @@ async function bindSpawnedAcpSessionToThread(params: {
   }

-  const currentThreadId = bindingContext.threadId ?? "";
-  if (threadMode === "here" && !currentThreadId) {
+  const currentConversationId = bindingContext.conversationId?.trim() || "";
+  const requiresThreadIdForHere = channel !== "telegram";
+  if (
+    threadMode === "here" &&
+    ((requiresThreadIdForHere && !currentThreadId) ||
+      (!requiresThreadIdForHere && !currentConversationId))
+  ) {
     return {
       ok: false,
       error: `--thread here requires running /acp spawn inside an active ${channel} thread/conversation.`,
     };
   }

-  const threadId = currentThreadId || undefined;
-  const placement = threadId ? "current" : "child";
+  const placement = channel === "telegram" ? "current" : currentThreadId ? "current" : "child";
   if (!capabilities.placements.includes(placement)) {
     return {
       ok: false,
       error: `Thread bindings do not support ${placement} placement for ${channel}.`,
     };
   }
   const channelId = placement === "child" ? bindingContext.conversationId : undefined;
-  if (placement === "child" && !channelId) {
+  if (!currentConversationId) {
     return {
       ok: false,
       error: `Could not resolve a ${channel} conversation for ACP thread spawn.`,
     };
   }

   const senderId = commandParams.command.senderId?.trim() || "";
-  if (threadId) {
+  if (placement === "current") {
     const existingBinding = bindingService.resolveByConversation({
       channel: spawnPolicy.channel,
       accountId: spawnPolicy.accountId,
-      conversationId: threadId,
+      conversationId: currentConversationId,
     });
     const boundBy =
       typeof existingBinding?.metadata?.boundBy === "string"
@@ -162,19 +164,13 @@ async function bindSpawnedAcpSessionToThread(params: {
     if (existingBinding && boundBy && boundBy !== "system" && senderId && senderId !== boundBy) {
       return {
         ok: false,
-        error: `Only ${boundBy} can rebind this thread.`,
+        error: `Only ${boundBy} can rebind this ${channel === "telegram" ? "conversation" : "thread"}.`,
       };
     }
   }

   const label = params.label || params.agentId;
-  const conversationId = threadId || channelId;
-  if (!conversationId) {
-    return {
-      ok: false,
-      error: `Could not resolve a ${channel} conversation for ACP thread spawn.`,
-    };
-  }
+  const conversationId = currentConversationId;

   try {
     const binding = await bindingService.bind({
@@ -344,12 +340,13 @@ export async function handleAcpSpawnAction(
     `✅ Spawned ACP session ${sessionKey} (${spawn.mode}, backend ${initializedBackend}).`,
   ];
   if (binding) {
-    const currentThreadId = resolveAcpCommandThreadId(params) ?? "";
+    const currentConversationId = resolveAcpCommandConversationId(params)?.trim() || "";
     const boundConversationId = binding.conversation.conversationId.trim();
-    if (currentThreadId && boundConversationId === currentThreadId) {
-      parts.push(`Bound this thread to ${sessionKey}.`);
+    const placementLabel = binding.conversation.channel === "telegram" ? "conversation" : "thread";
+    if (currentConversationId && boundConversationId === currentConversationId) {
+      parts.push(`Bound this ${placementLabel} to ${sessionKey}.`);
     } else {
-      parts.push(`Created thread ${boundConversationId} and bound it to ${sessionKey}.`);
+      parts.push(`Created ${placementLabel} ${boundConversationId} and bound it to ${sessionKey}.`);
     }
   } else {
     parts.push("Session is unbound (use /focus <session-key> to bind this thread/conversation).");
@@ -360,6 +357,19 @@ export async function handleAcpSpawnAction(
     parts.push(`ℹ️ ${dispatchNote}`);
   }

+  const shouldPinBindingNotice =
+    binding?.conversation.channel === "telegram" &&
+    binding.conversation.conversationId.includes(":topic:");
+  if (shouldPinBindingNotice) {
+    return {
+      shouldContinue: false,
+      reply: {
+        text: parts.join(" "),
+        channelData: { telegram: { pin: true } },
+      },
+    };
+  }
   return stopWithText(parts.join(" "));
 }
```

---

## src/auto-reply/reply/commands-acp/shared.test.ts

**Lines changed:** +22 additions, 0 deletions

```diff
@@ -0,0 +1,22 @@
+import { describe, expect, it } from "vitest";
+import { parseSteerInput } from "./shared.js";
+describe("parseSteerInput", () => {
+  it("preserves non-option instruction tokens while normalizing unicode-dash flags", () => {
+    const parsed = parseSteerInput([
+      "\u2014session",
+      "agent:codex:acp:s1",
+      "\u2014briefly",
+      "summarize",
+      "this",
+    ]);
+    expect(parsed).toEqual({
+      ok: true,
+      value: {
+        sessionToken: "agent:codex:acp:s1",
+        instruction: "\u2014briefly summarize this",
+      },
+    });
+  });
+});
```

---

## src/auto-reply/reply/commands-acp/shared.ts

**Lines changed:** +36 additions, 14 deletions

```diff
@@ -11,7 +11,7 @@ export { resolveAcpInstallCommandHint, resolveConfiguredAcpBackendId } from "./i

 export const COMMAND = "/acp";
 export const ACP_SPAWN_USAGE =
-  "Usage: /acp spawn [agentId] [--mode persistent|oneshot] [--thread auto|here|off] [--cwd <path>] [--label <label>].";
+  "Usage: /acp spawn [harness-id] [--mode persistent|oneshot] [--thread auto|here|off] [--cwd <path>] [--label <label>].";
 export const ACP_STEER_USAGE =
   "Usage: /acp steer [--session <session-key|session-id|session-label>] <instruction>";
 export const ACP_SET_MODE_USAGE =
@@ -77,6 +77,9 @@ export type ParsedSetCommandInput = {
   sessionToken?: string;
 };

+const ACP_UNICODE_DASH_PREFIX_RE =
+  /^[\u2010\u2011\u2012\u2013\u2014\u2015\u2212\uFE58\uFE63\uFF0D]+/;
 export function stopWithText(text: string): CommandHandlerResult {
   return {
     shouldContinue: false,
@@ -118,9 +121,9 @@ function readOptionValue(params: { tokens: string[]; index: number; flag: string
       error?: string;
     }
   | { matched: false } {
-  const token = params.tokens[params.index] ?? "";
+  const token = normalizeAcpOptionToken(params.tokens[params.index] ?? "");
   if (token === params.flag) {
-    const nextValue = params.tokens[params.index + 1]?.trim() ?? "";
+    const nextValue = normalizeAcpOptionToken(params.tokens[params.index + 1] ?? "");
     if (!nextValue || nextValue.startsWith("--")) {
       return {
         matched: true,
@@ -152,6 +155,18 @@ function readOptionValue(params: { tokens: string[]; index: number; flag: string
   return { matched: false };
 }

+function normalizeAcpOptionToken(raw: string): string {
+  const token = raw.trim();
+  if (!token || token.startsWith("--")) {
+    return token;
+  }
+  const dashPrefix = token.match(ACP_UNICODE_DASH_PREFIX_RE)?.[0];
+  if (!dashPrefix) {
+    return token;
+  }
+  return `--${token.slice(dashPrefix.length)}`;
+}
 function resolveDefaultSpawnThreadMode(params: HandleCommandsParams): AcpSpawnThreadMode {
   if (resolveAcpCommandChannel(params) !== DISCORD_THREAD_BINDING_CHANNEL) {
     return "off";
@@ -164,16 +179,17 @@ export function parseSpawnInput(
   params: HandleCommandsParams,
   tokens: string[],
 ): { ok: true; value: ParsedSpawnInput } | { ok: false; error: string } {
+  const normalizedTokens = tokens.map((token) => normalizeAcpOptionToken(token));
   let mode: AcpRuntimeSessionMode = "persistent";
   let thread = resolveDefaultSpawnThreadMode(params);
   let cwd: string | undefined;
   let label: string | undefined;
   let rawAgentId: string | undefined;

-  for (let i = 0; i < tokens.length; ) {
-    const token = tokens[i] ?? "";
+  for (let i = 0; i < normalizedTokens.length; ) {
+    const token = normalizedTokens[i] ?? "";

-    const modeOption = readOptionValue({ tokens, index: i, flag: "--mode" });
+    const modeOption = readOptionValue({ tokens: normalizedTokens, index: i, flag: "--mode" });
     if (modeOption.matched) {
       if (modeOption.error) {
         return { ok: false, error: `${modeOption.error}. ${ACP_SPAWN_USAGE}` };
@@ -190,7 +206,11 @@ export function parseSpawnInput(
       continue;
     }

-    const threadOption = readOptionValue({ tokens, index: i, flag: "--thread" });
+    const threadOption = readOptionValue({
+      tokens: normalizedTokens,
+      index: i,
+      flag: "--thread",
+    });
     if (threadOption.matched) {
       if (threadOption.error) {
         return { ok: false, error: `${threadOption.error}. ${ACP_SPAWN_USAGE}` };
@@ -207,7 +227,7 @@ export function parseSpawnInput(
       continue;
     }

-    const cwdOption = readOptionValue({ tokens, index: i, flag: "--cwd" });
+    const cwdOption = readOptionValue({ tokens: normalizedTokens, index: i, flag: "--cwd" });
     if (cwdOption.matched) {
       if (cwdOption.error) {
         return { ok: false, error: `${cwdOption.error}. ${ACP_SPAWN_USAGE}` };
@@ -217,7 +237,7 @@ export function parseSpawnInput(
       continue;
     }

-    const labelOption = readOptionValue({ tokens, index: i, flag: "--label" });
+    const labelOption = readOptionValue({ tokens: normalizedTokens, index: i, flag: "--label" });
     if (labelOption.matched) {
       if (labelOption.error) {
         return { ok: false, error: `${labelOption.error}. ${ACP_SPAWN_USAGE}` };
@@ -251,7 +271,7 @@ export function parseSpawnInput(
   if (!selectedAgent) {
     return {
       ok: false,
-      error: `ACP target agent is required. Pass an agent id or configure acp.defaultAgent. ${ACP_SPAWN_USAGE}`,
+      error: `ACP target harness id is required. Pass an ACP harness id (for example codex) or configure acp.defaultAgent. ${ACP_SPAWN_USAGE}`,
     };
   }
   const normalizedAgentId = normalizeAgentId(selectedAgent);
@@ -271,12 +291,13 @@ export function parseSteerInput(
   tokens: string[],
 ): { ok: true; value: ParsedSteerInput } | { ok: false; error: string } {
+  const normalizedTokens = tokens.map((token) => normalizeAcpOptionToken(token));
   let sessionToken: string | undefined;
   const instructionTokens: string[] = [];

-  for (let i = 0; i < tokens.length; ) {
+  for (let i = 0; i < normalizedTokens.length; ) {
     const sessionOption = readOptionValue({
-      tokens,
+      tokens: normalizedTokens,
       index: i,
       flag: "--session",
     });
@@ -292,7 +313,7 @@ export function parseSteerInput(
       continue;
     }

-    instructionTokens.push(tokens[i]);
+    instructionTokens.push(tokens[i] ?? "");
     i += 1;
   }

@@ -380,7 +401,7 @@ export function resolveAcpHelpText(): string {
   return [
     "ACP commands:",
     "-----",
-    "/acp spawn [agentId] [--mode persistent|oneshot] [--thread auto|here|off] [--cwd <path>] [--label <label>]",
+    "/acp spawn [harness-id] [--mode persistent|oneshot] [--thread auto|here|off] [--cwd <path>] [--label <label>]",
     "/acp cancel [session-key|session-id|session-label]",
     "/acp steer [--session <session-key|session-id|session-label>] <instruction>",
     "/acp close [session-key|session-id|session-label]",
@@ -397,6 +418,7 @@ export function resolveAcpHelpText(): string {
     "/acp sessions",
     "",
     "Notes:",
+    "- /acp spawn harness-id is an ACP runtime harness alias (for example codex), not an OpenClaw agents.list id.",
     "- /focus and /unfocus also work with ACP session keys.",
     "- ACP dispatch of normal thread messages is controlled by acp.dispatch.enabled.",
   ].join("\n");
```

---

## src/auto-reply/reply/commands-session-lifecycle.test.ts

**Lines changed:** +141 additions, 7 deletions

```diff
@@ -1,14 +1,21 @@
 import { beforeEach, describe, expect, it, vi } from "vitest";
 import type { OpenClawConfig } from "../../config/config.js";
 import type { SessionBindingRecord } from "../../infra/outbound/session-binding-service.js";

 const hoisted = vi.hoisted(() => {
   const getThreadBindingManagerMock = vi.fn();
   const setThreadBindingIdleTimeoutBySessionKeyMock = vi.fn();
   const setThreadBindingMaxAgeBySessionKeyMock = vi.fn();
+  const setTelegramThreadBindingIdleTimeoutBySessionKeyMock = vi.fn();
+  const setTelegramThreadBindingMaxAgeBySessionKeyMock = vi.fn();
   const sessionBindingResolveByConversationMock = vi.fn();
   return {
     getThreadBindingManagerMock,
     setThreadBindingIdleTimeoutBySessionKeyMock,
     setThreadBindingMaxAgeBySessionKeyMock,
+    setTelegramThreadBindingIdleTimeoutBySessionKeyMock,
+    setTelegramThreadBindingMaxAgeBySessionKeyMock,
     sessionBindingResolveByConversationMock,
   };
 });

@@ -22,6 +29,33 @@ vi.mock("../../discord/monitor/thread-bindings.js", async (importOriginal) => {
   };
 });

+vi.mock("../../telegram/thread-bindings.js", async (importOriginal) => {
+  const actual = await importOriginal<typeof import("../../telegram/thread-bindings.js")>();
+  return {
+    ...actual,
+    setTelegramThreadBindingIdleTimeoutBySessionKey:
+      hoisted.setTelegramThreadBindingIdleTimeoutBySessionKeyMock,
+    setTelegramThreadBindingMaxAgeBySessionKey:
+      hoisted.setTelegramThreadBindingMaxAgeBySessionKeyMock,
+  };
+});
 vi.mock("../../infra/outbound/session-binding-service.js", async (importOriginal) => {
   const actual =
     await importOriginal<typeof import("../../infra/outbound/session-binding-service.js")>();
   return {
     ...actual,
     getSessionBindingService: () => ({
       bind: vi.fn(),
       getCapabilities: vi.fn(),
       listBySession: vi.fn(),
       resolveByConversation: (ref: unknown) => hoisted.sessionBindingResolveByConversationMock(ref),
       touch: vi.fn(),
       unbind: vi.fn(),
     }),
   });
 });
 const { handleSessionCommand } = await import("./commands-session.js");
 const { buildCommandTestParams } = await import("./commands.test-harness.js");

@@ -55,6 +89,18 @@ function createDiscordCommandParams(commandBody: string, overrides?: Record<stri
   });
 }

+function createTelegramCommandParams(commandBody: string, overrides?: Record<string, unknown>) {
+  return buildCommandTestParams(commandBody, baseCfg, {
+    Provider: "telegram",
+    Surface: "telegram",
+    OriginatingChannel: "telegram",
+    OriginatingTo: "-100200300:topic:77",
+    AccountId: "default",
+    MessageThreadId: "77",
+    ...overrides,
+  });
+}
 function createFakeBinding(overrides: Partial<FakeBinding> = {}): FakeBinding {
   const now = Date.now();
   return {
@@ -71,6 +117,28 @@ function createFakeBinding(overrides: Partial<FakeBinding> = {}): FakeBinding {
   };
 }

+function createTelegramBinding(overrides?: Partial<SessionBindingRecord>): SessionBindingRecord {
+  return {
+    bindingId: "default:-100200300:topic:77",
+    targetSessionKey: "agent:main:subagent:child",
+    targetKind: "subagent",
+    conversation: {
+      channel: "telegram",
+      accountId: "default",
+      conversationId: "-100200300:topic:77",
+    },
+    status: "active",
+    boundAt: Date.now(),
+    metadata: {
+      boundBy: "user-1",
+      lastActivityAt: Date.now(),
+      idleTimeoutMs: 24 * 60 * 60 * 1000,
+      maxAgeMs: 0,
+    },
+    ...overrides,
+  };
+}
 function createFakeThreadBindingManager(binding: FakeBinding | null) {
   return {
     getByThreadId: vi.fn((_threadId: string) => binding),
@@ -81,13 +149,16 @@ function createFakeThreadBindingManager(binding: FakeBinding | null) {

 describe("/session idle and /session max-age", () => {
   beforeEach(() => {
     hoisted.getThreadBindingManagerMock.mockClear();
     hoisted.setThreadBindingIdleTimeoutBySessionKeyMock.mockClear();
     hoisted.setThreadBindingMaxAgeBySessionKeyMock.mockClear();
+    hoisted.getThreadBindingManagerMock.mockReset();
+    hoisted.setThreadBindingIdleTimeoutBySessionKeyMock.mockReset();
+    hoisted.setThreadBindingMaxAgeBySessionKeyMock.mockReset();
+    hoisted.setTelegramThreadBindingIdleTimeoutBySessionKeyMock.mockReset();
+    hoisted.setTelegramThreadBindingMaxAgeBySessionKeyMock.mockReset();
+    hoisted.sessionBindingResolveByConversationMock.mockReset().mockReturnValue(null);
     vi.useRealTimers();
   });

-  it("sets idle timeout for the focused session", async () => {
+  it("sets idle timeout for the focused Discord session", async () => {
     vi.useFakeTimers();
     vi.setSystemTime(new Date("2026-02-20T00:00:00.000Z"));

@@ -128,7 +199,7 @@ describe("/session idle and /session max-age", () => {
     expect(result?.reply?.text).toContain("2026-02-20T02:00:00.000Z");
   });

-  it("sets max age for the focused session", async () => {
+  it("sets max age for the focused Discord session", async () => {
     vi.useFakeTimers();
     vi.setSystemTime(new Date("2026-02-20T00:00:00.000Z"));

@@ -157,6 +228,67 @@ describe("/session idle and /session max-age", () => {
     expect(text).toContain("2026-02-20T03:00:00.000Z");
   });

+  it("sets idle timeout for focused Telegram conversations", async () => {
+    vi.useFakeTimers();
+    vi.setSystemTime(new Date("2026-02-20T00:00:00.000Z");
+    hoisted.sessionBindingResolveByConversationMock.mockReturnValue(createTelegramBinding());
+    hoisted.setTelegramThreadBindingIdleTimeoutBySessionKeyMock.mockReturnValue([
+      {
+        targetSessionKey: "agent:main:subagent:child",
+        boundAt: Date.now(),
+        lastActivityAt: Date.now(),
+        idleTimeoutMs: 2 * 60 * 60 * 1000,
+      },
+    ]);
+    const result = await handleSessionCommand(
+      createTelegramCommandParams("/session idle 2h"),
+      true,
+    );
+    const text = result?.reply?.text ?? "";
+    expect(hoisted.setTelegramThreadBindingIdleTimeoutBySessionKeyMock).toHaveBeenCalledWith({
+      targetSessionKey: "agent:main:subagent:child",
+      accountId: "default",
+      idleTimeoutMs: 2 * 60 * 60 * 1000,
+    });
+    expect(text).toContain("Idle timeout set to 2h");
+    expect(text).toContain("2026-02-20T02:00:00.000Z");
+  });
+  it("reports Telegram max-age expiry from the original bind time", async () => {
+    vi.useFakeTimers();
+    vi.setSystemTime(new Date("2026-02-20T00:00:00.000Z");
+    const boundAt = Date.parse("2026-02-19T22:00:00.000Z");
+    hoisted.sessionBindingResolveByConversationMock.mockReturnValue(
+      createTelegramBinding({ boundAt }),
+    );
+    hoisted.setTelegramThreadBindingMaxAgeBySessionKeyMock.mockReturnValue([
+      {
+        targetSessionKey: "agent:main:subagent:child",
+        boundAt,
+        lastActivityAt: Date.now(),
+        maxAgeMs: 3 * 60 * 60 * 1000,
+      },
+    ]);
+    const result = await handleSessionCommand(
+      createTelegramCommandParams("/session max-age 3h"),
+      true,
+    );
+    const text = result?.reply?.text ?? "";
+    expect(hoisted.setTelegramThreadBindingMaxAgeBySessionKeyMock).toHaveBeenCalledWith({
+      targetSessionKey: "agent:main:subagent:child",
+      accountId: "default",
+      maxAgeMs: 3 * 60 * 60 * 1000,
+    });
+    expect(text).toContain("Max age set to 3h");
+    expect(text).toContain("2026-02-20T01:00:00.000Z");
+  });
   it("disables max age when set to off", async () => {
     const binding = createFakeBinding({ maxAgeMs: 2 * 60 * 60 * 1000 });
     hoisted.getThreadBindingManagerMock.mockReturnValue(createFakeThreadBindingManager(binding));
@@ -175,10 +307,12 @@ describe("/session idle and /session max-age", () => {
     expect(result?.reply?.text).toContain("Max age disabled");
   });

-  it("is unavailable outside discord", async () => {
+  it("is unavailable outside discord and telegram", async () => {
     const params = buildCommandTestParams("/session idle 2h", baseCfg);
     const result = await handleSessionCommand(params, true);
     expect(result?.reply?.text).toContain("currently available for Discord thread-bound sessions");
     expect(result?.reply?.text).toContain(
       "currently available for Discord and Telegram bound sessions",
     );
   });

   it("requires binding owner for lifecycle updates", async () => {
```

---

## src/auto-reply/reply/commands-session.ts

**Lines changed:** +174 additions, 50 deletions

```diff
@@ -11,16 +11,23 @@ import {
   setThreadBindingMaxAgeBySessionKey,
 } from "../../discord/monitor/thread-bindings.js";
 import { logVerbose } from "../../globals.js";
 import { getSessionBindingService } from "../../infra/outbound/session-binding-service.js";
 import type { SessionBindingRecord } from "../../infra/outbound/session-binding-service.js";
 import { scheduleGatewaySigusr1Restart, triggerOpenClawRestart } from "../../infra/restart.js";
 import { loadCostUsageSummary, loadSessionCostSummary } from "../../infra/session-cost-usage.js";
+import {
+  setTelegramThreadBindingIdleTimeoutBySessionKey,
+  setTelegramThreadBindingMaxAgeBySessionKey,
+} from "../../telegram/thread-bindings.js";
 import { formatTokenCount, formatUsd } from "../../utils/usage-format.js";
 import { parseActivationCommand } from "../group-activation.js";
 import { parseSendPolicyCommand } from "../send-policy.js";
 import { normalizeUsageDisplay, resolveResponseUsageMode } from "../thinking.js";
+import { isDiscordSurface, isTelegramSurface, resolveChannelAccountId } from "./channel-context.js";
 import { handleAbortTrigger, handleStopCommand } from "./commands-session-abort.js";
 import { persistSessionEntry } from "./commands-session-store.js";
 import type { CommandHandler } from "./commands-types.js";
-import { isDiscordSurface, resolveDiscordAccountId } from "./discord-context.js";
-import { resolveTelegramConversationId } from "./telegram-context.js";

 const SESSION_COMMAND_PREFIX = "/session";
 const SESSION_DURATION_OFF_VALUES = new Set(["off", "disable", "disabled", "none", "0"]);
@@ -53,6 +60,72 @@ function formatSessionExpiry(expiresAt: number) {
   return new Date(expiresAt).toISOString();
 }

+function resolveTelegramBindingDurationMs(
+  binding: SessionBindingRecord,
+  key: "idleTimeoutMs" | "maxAgeMs",
+  fallbackMs: number,
+): number {
+  const raw = binding.metadata?.[key];
+  if (typeof raw !== "number" || !Number.isFinite(raw)) {
+    return fallbackMs;
+  }
+  return Math.max(0, Math.floor(raw));
+}
+function resolveTelegramBindingLastActivityAt(binding: SessionBindingRecord): number {
+  const raw = binding.metadata?.lastActivityAt;
+  if (typeof raw !== "number" || !Number.isFinite(raw)) {
+    return binding.boundAt;
+  }
+  return Math.max(Math.floor(raw), binding.boundAt);
+}
+function resolveTelegramBindingBoundBy(binding: SessionBindingRecord): string {
+  const raw = binding.metadata?.boundBy;
+  return typeof raw === "string" ? raw.trim() : "";
+}
+type UpdatedLifecycleBinding = {
+  boundAt: number;
+  lastActivityAt: number;
+  idleTimeoutMs?: number;
+  maxAgeMs?: number;
+};
+function resolveUpdatedBindingExpiry(params: {
+  action: typeof SESSION_ACTION_IDLE | typeof SESSION_ACTION_MAX_AGE;
+  bindings: UpdatedLifecycleBinding[];
+}): number | undefined {
+  const expiries = params.bindings
+    .map((binding) => {
+      if (params.action === SESSION_ACTION_IDLE) {
+        const idleTimeoutMs =
+          typeof binding.idleTimeoutMs === "number" && Number.isFinite(binding.idleTimeoutMs)
+            ? Math.max(0, Math.floor(binding.idleTimeoutMs))
+            : 0;
+        if (idleTimeoutMs <= 0) {
+          return undefined;
+        }
+        return Math.max(binding.lastActivityAt, binding.boundAt) + idleTimeoutMs;
+      }
+      const maxAgeMs =
+        typeof binding.maxAgeMs === "number" && Number.isFinite(binding.maxAgeMs)
+          ? Math.max(0, Math.floor(binding.maxAgeMs))
+          : 0;
+      if (maxAgeMs <= 0) {
+        return undefined;
+      }
+      return binding.boundAt + maxAgeMs;
+    })
+    .filter((expiresAt): expiresAt is number => typeof expiresAt === "number");
+  if (expiries.length === 0) {
+    return undefined;
+  }
+  return Math.min(...expiries);
+}
 export const handleActivationCommand: CommandHandler = async (params, allowTextCommands) => {
   if (!allowTextCommands) {
     return null;
@@ -243,59 +316,98 @@ export const handleSessionCommand: CommandHandler = async (params, allowTextComm
     };
   }

-  if (!isDiscordSurface(params)) {
+  const onDiscord = isDiscordSurface(params);
+  const onTelegram = isTelegramSurface(params);
+  if (!onDiscord && !onTelegram) {
     return {
       shouldContinue: false,
       reply: {
-        text: "⚠️ /session idle and /session max-age are currently available for Discord thread-bound sessions.",
+        text: "⚠️ /session idle and /session max-age are currently available for Discord and Telegram bound sessions.",
       },
     };
   }

-  const accountId = resolveDiscordAccountId(params);
-  const threadId =
-    params.ctx.MessageThreadId != null ? String(params.ctx.MessageThreadId).trim() : "";
-  if (!threadId) {
+  const accountId = resolveChannelAccountId(params);
+  const sessionBindingService = getSessionBindingService();
+  const threadId =
+    params.ctx.MessageThreadId != null ? String(params.ctx.MessageThreadId).trim() : "";
+  const telegramConversationId = onTelegram ? resolveTelegramConversationId(params) : undefined;
+  const discordManager = onDiscord ? getThreadBindingManager(accountId) : null;
+  if (onDiscord && !discordManager) {
+    return {
+      shouldContinue: false,
+      reply: { text: "⚠️ Discord thread bindings are unavailable for this account." },
+    };
+  }

-  const accountId = resolveChannelAccountId(params);
-  const sessionBindingService = getSessionBindingService();
-  const threadId =
-    params.ctx.MessageThreadId != null ? String(params.ctx.MessageThreadId).trim() : "";
-  if (!threadId) {
-    return {
-      shouldContinue: false,
-      reply: {
-        text: "⚠️ /session idle and /session max-age must be run inside a focused Discord thread.",
-      },
-    };
-  }
-
-  const accountId = resolveDiscordAccountId(params);
-  const threadBindings = getThreadBindingManager(accountId);
-  if (!threadBindings) {
+  const discordBinding =
+    onDiscord && threadId ? discordManager?.getByThreadId(threadId) : undefined;
+  const telegramBinding =
+    onTelegram && telegramConversationId
+      ? sessionBindingService.resolveByConversation({
+          channel: "telegram",
+          accountId,
+          conversationId: telegramConversationId,
+        })
+      : null;
+  if (onDiscord && !discordBinding) {
+    if (onDiscord && !threadId) {
       return {
         shouldContinue: false,
         reply: {
           text: "⚠️ /session idle and /session max-age must be run inside a focused Discord thread.",
         },
       };
     }
     return {
       shouldContinue: false,
-      reply: { text: "⚠️ Discord thread bindings are unavailable for this account." },
+      reply: { text: "ℹ️ This thread is not currently focused." },
     };
   }
-  const binding = threadBindings.getByThreadId(threadId);
-  if (!binding) {
+  if (onTelegram && !telegramBinding) {
+    if (!telegramConversationId) {
+      return {
+        shouldContinue: false,
+        reply: {
+          text: "⚠️ /session idle and /session max-age on Telegram require a topic context in groups, or a direct-message conversation.",
+        },
+      };
+    }
     return {
       shouldContinue: false,
-      reply: { text: "ℹ️ This thread is not currently focused." },
+      reply: { text: "ℹ️ This conversation is not currently focused." },
     };
   }

-  const idleTimeoutMs = resolveThreadBindingIdleTimeoutMs({
-    record: binding,
-    defaultIdleTimeoutMs: threadBindings.getIdleTimeoutMs(),
-  });
-  const idleExpiresAt = resolveThreadBindingInactivityExpiresAt({
-    record: binding,
-    defaultIdleTimeoutMs: threadBindings.getIdleTimeoutMs(),
-  });
-  const maxAgeMs = resolveThreadBindingMaxAgeMs({
-    record: binding,
-    defaultMaxAgeMs: threadBindings.getMaxAgeMs(),
-  });
-  const maxAgeExpiresAt = resolveThreadBindingMaxAgeExpiresAt({
-    record: binding,
-    defaultMaxAgeMs: threadBindings.getMaxAgeMs(),
-  });
+  const idleTimeoutMs = onDiscord
+    ? resolveThreadBindingIdleTimeoutMs({
+        record: discordBinding!,
+        defaultIdleTimeoutMs: discordManager!.getIdleTimeoutMs(),
+      })
+    : resolveTelegramBindingDurationMs(telegramBinding!, "idleTimeoutMs", 24 * 60 * 60 * 1000);
+  const idleExpiresAt = onDiscord
+    ? resolveThreadBindingInactivityExpiresAt({
+        record: discordBinding!,
+        defaultIdleTimeoutMs: discordManager!.getIdleTimeoutMs(),
+      })
+    : idleTimeoutMs > 0
+      ? resolveTelegramBindingLastActivityAt(telegramBinding!) + idleTimeoutMs
+      : undefined;
+  const maxAgeMs = onDiscord
+    ? resolveThreadBindingMaxAgeMs({
+        record: discordBinding!,
+        defaultMaxAgeMs: discordManager!.getMaxAgeMs(),
+      })
+    : resolveTelegramBindingDurationMs(telegramBinding!, "maxAgeMs", 0);
+  const maxAgeExpiresAt = onDiscord
+    ? resolveThreadBindingMaxAgeExpiresAt({
+        record: discordBinding!,
+        defaultMaxAgeMs: discordManager!.getMaxAgeMs(),
+      })
+    : maxAgeMs > 0
+      ? telegramBinding!.boundAt + maxAgeMs
+      : undefined;

   const durationArgRaw = tokens.slice(1).join("");
   if (!durationArgRaw) {
@@ -337,11 +449,16 @@ export const handleSessionCommand: CommandHandler = async (params, allowTextComm
   }

   const senderId = params.command.senderId?.trim() || "";
-  if (binding.boundBy && binding.boundBy !== "system" && senderId && senderId !== binding.boundBy) {
+  const boundBy = onDiscord
+    ? discordBinding!.boundBy
+    : resolveTelegramBindingBoundBy(telegramBinding!);
+  if (boundBy && boundBy !== "system" && senderId && senderId !== boundBy) {
     return {
       shouldContinue: false,
       reply: {
-        text: `⚠️ Only ${binding.boundBy} can update session lifecycle settings for this thread.`,
+        text: onDiscord
+          ? `⚠️ Only ${boundBy} can update session lifecycle settings for this thread.`
+          : `⚠️ Only ${boundBy} can update session lifecycle settings for this conversation.`,
       },
     };
   }
@@ -356,18 +473,32 @@ export const handleSessionCommand: CommandHandler = async (params, allowTextComm
     };
   }

-  const updatedBindings =
-    action === SESSION_ACTION_IDLE
-      ? setThreadBindingIdleTimeoutBySessionKey({
-          targetSessionKey: binding.targetSessionKey,
+  const updatedBindings = (() => {
+    if (onDiscord) {
+      return action === SESSION_ACTION_IDLE
+        ? setThreadBindingIdleTimeoutBySessionKey({
+            targetSessionKey: discordBinding!.targetSessionKey,
             accountId,
             idleTimeoutMs: durationMs,
           })
-        : setThreadBindingMaxAgeBySessionKey({
-            targetSessionKey: binding.targetSessionKey,
+        : setThreadBindingMaxAgeBySessionKey({
+            targetSessionKey: discordBinding!.targetSessionKey,
             accountId,
             maxAgeMs: durationMs,
           });
+    }
+    return action === SESSION_ACTION_IDLE
+      ? setTelegramThreadBindingIdleTimeoutBySessionKey({
+          targetSessionKey: telegramBinding!.targetSessionKey,
+          accountId,
+          idleTimeoutMs: durationMs,
+        })
+      : setThreadBindingMaxAgeBySessionKey({
+          targetSessionKey: binding.targetSessionKey,
+      : setTelegramThreadBindingMaxAgeBySessionKey({
+          targetSessionKey: telegramBinding!.targetSessionKey,
+          accountId,
+          maxAgeMs: durationMs,
+        });
+  })();
```

---

*Note: Output truncated at 50KB. Use offset 1290 to view remaining changes.*