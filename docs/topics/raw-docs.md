35 files changed
+2395
-451
lines changed
 
‎CHANGELOG.md‎
+1Lines changed: 1 addition & 0 deletions

Original file line number	Diff line number	Diff line change
@@ -21,6 +21,7 @@ Docs: https://docs.openclaw.ai
- Gateway: add SecretRef support for gateway.auth.token with auth-mode guardrails. (#35094) Thanks @joshavant.
- Plugins/hook policy: add `plugins.entries.<id>.hooks.allowPromptInjection`, validate unknown typed hook names at runtime, and preserve legacy `before_agent_start` model/provider overrides while stripping prompt-mutating fields when prompt injection is disabled. (#36567) thanks @gumadeiras.
- Tools/Diffs guidance: restore a short system-prompt hint for enabled diffs while keeping the detailed instructions in the companion skill, so diffs usage guidance stays out of user-prompt space. (#36904) thanks @gumadeiras.
- Telegram/ACP topic bindings: accept Telegram Mac Unicode dash option prefixes in `/acp spawn`, support Telegram topic thread binding (`--thread here|auto`), route bound-topic follow-ups to ACP sessions, add actionable Telegram approval buttons with prefixed approval-id resolution, and pin successful bind confirmations in-topic. (#36683) Thanks @huntharo.

### Breaking

‎docs/channels/telegram.md‎
+7Lines changed: 7 additions & 0 deletions

Original file line number	Diff line number	Diff line change
@@ -524,6 +524,13 @@ curl "https://api.telegram.org/bot<bot_token>/getUpdates"

    This is currently scoped to forum topics in groups and supergroups.

    **Thread-bound ACP spawn from chat**:
    - `/acp spawn <agent> --thread here|auto` can bind the current Telegram topic to a new ACP session.
    - Follow-up topic messages route to the bound ACP session directly (no `/acp steer` required).
    - OpenClaw pins the spawn confirmation message in-topic after a successful bind.
    - Requires `channels.telegram.threadBindings.spawnAcpSessions=true`.
    Template context includes:

    - `MessageThreadId`
‎docs/tools/acp-agents.md‎
+7-2Lines changed: 7 additions & 2 deletions

Original file line number	Diff line number	Diff line change
@@ -79,11 +79,14 @@ Required feature flags for thread-bound ACP:
- `acp.dispatch.enabled` is on by default (set `false` to pause ACP dispatch)
- Channel-adapter ACP thread-spawn flag enabled (adapter-specific)
  - Discord: `channels.discord.threadBindings.spawnAcpSessions=true`
  - Telegram: `channels.telegram.threadBindings.spawnAcpSessions=true`

### Thread supporting channels

- Any channel adapter that exposes session/thread binding capability.
- Current built-in support: Discord.
- Current built-in support:
  - Discord threads/channels
  - Telegram topics (forum topics in groups/supergroups and DM topics)
- Plugin channels can add support through the same binding interface.

## Channel specific settings
@@ -303,7 +306,9 @@ If no target resolves, OpenClaw returns a clear error (`Unable to resolve sessio
Notes:

- On non-thread binding surfaces, default behavior is effectively `off`.
- Thread-bound spawn requires channel policy support (for Discord: `channels.discord.threadBindings.spawnAcpSessions=true`).
- Thread-bound spawn requires channel policy support:
  - Discord: `channels.discord.threadBindings.spawnAcpSessions=true`
  - Telegram: `channels.telegram.threadBindings.spawnAcpSessions=true`

## ACP controls

‎extensions/acpx/src/runtime-internals/test-fixtures.ts‎
+4Lines changed: 4 additions & 0 deletions
Original file line number	Diff line number	Diff line change
@@ -223,6 +223,10 @@ if (command === "prompt") {
    process.exit(1);
  }
  if (stdinText.includes("permission-denied")) {
    process.exit(5);
  }
  if (stdinText.includes("split-spacing")) {
    emitUpdate(sessionFromOption, {
      sessionUpdate: "agent_message_chunk",
‎extensions/acpx/src/runtime.test.ts‎
+36Lines changed: 36 additions & 0 deletions
Original file line number	Diff line number	Diff line change
@@ -224,6 +224,42 @@ describe("AcpxRuntime", () => {
    });
  });

  it("maps acpx permission-denied exits to actionable guidance", async () => {
    const runtime = sharedFixture?.runtime;
    expect(runtime).toBeDefined();
    if (!runtime) {
      throw new Error("shared runtime fixture missing");
    }
    const handle = await runtime.ensureSession({
      sessionKey: "agent:codex:acp:permission-denied",
      agent: "codex",
      mode: "persistent",
    });
    const events = [];
    for await (const event of runtime.runTurn({
      handle,
      text: "permission-denied",
      mode: "prompt",
      requestId: "req-perm",
    })) {
      events.push(event);
    }
    expect(events).toContainEqual(
      expect.objectContaining({
        type: "error",
        message: expect.stringContaining("Permission denied by ACP runtime (acpx)."),
      }),
    );
    expect(events).toContainEqual(
      expect.objectContaining({
        type: "error",
        message: expect.stringContaining("approve-reads, approve-all, deny-all"),
      }),
    );
  });
  it("supports cancel and close using encoded runtime handle state", async () => {
    const { runtime, logPath, config } = await createMockRuntimeFixture();
    const handle = await runtime.ensureSession({
‎extensions/acpx/src/runtime.ts‎
+28-2Lines changed: 28 additions & 2 deletions
Original file line number	Diff line number	Diff line change
@@ -42,10 +42,30 @@ export const ACPX_BACKEND_ID = "acpx";

const ACPX_RUNTIME_HANDLE_PREFIX = "acpx:v1:";
const DEFAULT_AGENT_FALLBACK = "codex";
const ACPX_EXIT_CODE_PERMISSION_DENIED = 5;
const ACPX_CAPABILITIES: AcpRuntimeCapabilities = {
  controls: ["session/set_mode", "session/set_config_option", "session/status"],
};

function formatPermissionModeGuidance(): string {
  return "Configure plugins.entries.acpx.config.permissionMode to one of: approve-reads, approve-all, deny-all.";
}
function formatAcpxExitMessage(params: {
  stderr: string;
  exitCode: number | null | undefined;
}): string {
  const stderr = params.stderr.trim();
  if (params.exitCode === ACPX_EXIT_CODE_PERMISSION_DENIED) {
    return [
      stderr || "Permission denied by ACP runtime (acpx).",
      "ACPX blocked a write/exec permission request in a non-interactive session.",
      formatPermissionModeGuidance(),
    ].join(" ");
  }
  return stderr || `acpx exited with code ${params.exitCode ?? "unknown"}`;
}
export function encodeAcpxRuntimeHandleState(state: AcpxHandleState): string {
  const payload = Buffer.from(JSON.stringify(state), "utf8").toString("base64url");
  return `${ACPX_RUNTIME_HANDLE_PREFIX}${payload}`;
@@ -333,7 +353,10 @@ export class AcpxRuntime implements AcpRuntime {
      if ((exit.code ?? 0) !== 0 && !sawError) {
        yield {
          type: "error",
          message: stderr.trim() || `acpx exited with code ${exit.code ?? "unknown"}`,
          message: formatAcpxExitMessage({
            stderr,
            exitCode: exit.code,
          }),
        };
        return;
      }
@@ -639,7 +662,10 @@ export class AcpxRuntime implements AcpRuntime {
    if ((result.code ?? 0) !== 0) {
      throw new AcpRuntimeError(
        params.fallbackCode,
        result.stderr.trim() || `acpx exited with code ${result.code ?? "unknown"}`,
        formatAcpxExitMessage({
          stderr: result.stderr,
          exitCode: result.code,
        }),
      );
    }
    return events;
‎src/auto-reply/commands-registry.data.ts‎
+3-2Lines changed: 3 additions & 2 deletions
Original file line number	Diff line number	Diff line change
@@ -354,7 +354,8 @@ function buildChatCommands(): ChatCommandDefinition[] {
    defineChatCommand({
      key: "focus",
      nativeName: "focus",
      description: "Bind this Discord thread (or a new one) to a session target.",
      description:
        "Bind this thread (Discord) or topic/conversation (Telegram) to a session target.",
      textAlias: "/focus",
      category: "management",
      args: [
@@ -369,7 +370,7 @@ function buildChatCommands(): ChatCommandDefinition[] {
    defineChatCommand({
      key: "unfocus",
      nativeName: "unfocus",
      description: "Remove the current Discord thread binding.",
      description: "Remove the current thread (Discord) or topic/conversation (Telegram) binding.",
      textAlias: "/unfocus",
      category: "management",
    }),
‎src/auto-reply/reply/discord-context.ts‎ ‎src/auto-reply/reply/channel-context.ts‎src/auto-reply/reply/discord-context.ts renamed to src/auto-reply/reply/channel-context.ts
+15-5Lines changed: 15 additions & 5 deletions
Original file line number	Diff line number	Diff line change
@@ -17,19 +17,29 @@ type DiscordAccountParams = {
};

export function isDiscordSurface(params: DiscordSurfaceParams): boolean {
  return resolveCommandSurfaceChannel(params) === "discord";
}
export function isTelegramSurface(params: DiscordSurfaceParams): boolean {
  return resolveCommandSurfaceChannel(params) === "telegram";
}
export function resolveCommandSurfaceChannel(params: DiscordSurfaceParams): string {
  const channel =
    params.ctx.OriginatingChannel ??
    params.command.channel ??
    params.ctx.Surface ??
    params.ctx.Provider;
  return (
    String(channel ?? "")
      .trim()
      .toLowerCase() === "discord"
  );
  return String(channel ?? "")
    .trim()
    .toLowerCase();
}

export function resolveDiscordAccountId(params: DiscordAccountParams): string {
  return resolveChannelAccountId(params);
}
export function resolveChannelAccountId(params: DiscordAccountParams): string {
  const accountId = typeof params.ctx.AccountId === "string" ? params.ctx.AccountId.trim() : "";
  return accountId || "default";
}
‎src/auto-reply/reply/commands-acp.test.ts‎
+150-9Lines changed: 150 additions & 9 deletions
Original file line number	Diff line number	Diff line change
@@ -118,7 +118,7 @@ type FakeBinding = {
  targetSessionKey: string;
  targetKind: "subagent" | "session";
  conversation: {
    channel: "discord";
    channel: "discord" | "telegram";
    accountId: string;
    conversationId: string;
    parentConversationId?: string;
@@ -242,7 +242,11 @@ function createSessionBindingCapabilities() {

type AcpBindInput = {
  targetSessionKey: string;
  conversation: { accountId: string; conversationId: string };
  conversation: {
    channel?: "discord" | "telegram";
    accountId: string;
    conversationId: string;
  };
  placement: "current" | "child";
  metadata?: Record<string, unknown>;
};
@@ -251,14 +255,22 @@ function createAcpThreadBinding(input: AcpBindInput): FakeBinding {
  const nextConversationId =
    input.placement === "child" ? "thread-created" : input.conversation.conversationId;
  const boundBy = typeof input.metadata?.boundBy === "string" ? input.metadata.boundBy : "user-1";
  const channel = input.conversation.channel ?? "discord";
  return createSessionBinding({
    targetSessionKey: input.targetSessionKey,
    conversation: {
      channel: "discord",
      accountId: input.conversation.accountId,
      conversationId: nextConversationId,
      parentConversationId: "parent-1",
    },
    conversation:
      channel === "discord"
        ? {
            channel: "discord",
            accountId: input.conversation.accountId,
            conversationId: nextConversationId,
            parentConversationId: "parent-1",
          }
        : {
            channel: "telegram",
            accountId: input.conversation.accountId,
            conversationId: nextConversationId,
          },
    metadata: { boundBy, webhookId: "wh-1" },
  });
}
@@ -297,6 +309,31 @@ function createThreadParams(commandBody: string, cfg: OpenClawConfig = baseCfg)
  return params;
}

function createTelegramTopicParams(commandBody: string, cfg: OpenClawConfig = baseCfg) {
  const params = buildCommandTestParams(commandBody, cfg, {
    Provider: "telegram",
    Surface: "telegram",
    OriginatingChannel: "telegram",
    OriginatingTo: "telegram:-1003841603622",
    AccountId: "default",
    MessageThreadId: "498",
  });
  params.command.senderId = "user-1";
  return params;
}
function createTelegramDmParams(commandBody: string, cfg: OpenClawConfig = baseCfg) {
  const params = buildCommandTestParams(commandBody, cfg, {
    Provider: "telegram",
    Surface: "telegram",
    OriginatingChannel: "telegram",
    OriginatingTo: "telegram:123456789",
    AccountId: "default",
  });
  params.command.senderId = "user-1";
  return params;
}
async function runDiscordAcpCommand(commandBody: string, cfg: OpenClawConfig = baseCfg) {
  return handleAcpCommand(createDiscordParams(commandBody, cfg), true);
}
@@ -305,6 +342,14 @@ async function runThreadAcpCommand(commandBody: string, cfg: OpenClawConfig = ba
  return handleAcpCommand(createThreadParams(commandBody, cfg), true);
}

async function runTelegramAcpCommand(commandBody: string, cfg: OpenClawConfig = baseCfg) {
  return handleAcpCommand(createTelegramTopicParams(commandBody, cfg), true);
}
async function runTelegramDmAcpCommand(commandBody: string, cfg: OpenClawConfig = baseCfg) {
  return handleAcpCommand(createTelegramDmParams(commandBody, cfg), true);
}
describe("/acp command", () => {
  beforeEach(() => {
    acpManagerTesting.resetAcpSessionManagerForTests();
@@ -448,10 +493,70 @@ describe("/acp command", () => {
    expect(seededWithoutEntry?.runtimeSessionName).toContain(":runtime");
  });

  it("accepts unicode dash option prefixes in /acp spawn args", async () => {
    const result = await runThreadAcpCommand(
      "/acp spawn codex \u2014mode oneshot \u2014thread here \u2014cwd /home/bob/clawd \u2014label jeerreview",
    );
    expect(result?.reply?.text).toContain("Spawned ACP session agent:codex:acp:");
    expect(result?.reply?.text).toContain("Bound this thread to");
    expect(hoisted.ensureSessionMock).toHaveBeenCalledWith(
      expect.objectContaining({
        agent: "codex",
        mode: "oneshot",
        cwd: "/home/bob/clawd",
      }),
    );
    expect(hoisted.sessionBindingBindMock).toHaveBeenCalledWith(
      expect.objectContaining({
        placement: "current",
        metadata: expect.objectContaining({
          label: "jeerreview",
        }),
      }),
    );
  });
  it("binds Telegram topic ACP spawns to full conversation ids", async () => {
    const result = await runTelegramAcpCommand("/acp spawn codex --thread here");
    expect(result?.reply?.text).toContain("Spawned ACP session agent:codex:acp:");
    expect(result?.reply?.text).toContain("Bound this conversation to");
    expect(result?.reply?.channelData).toEqual({ telegram: { pin: true } });
    expect(hoisted.sessionBindingBindMock).toHaveBeenCalledWith(
      expect.objectContaining({
        placement: "current",
        conversation: expect.objectContaining({
          channel: "telegram",
          accountId: "default",
          conversationId: "-1003841603622:topic:498",
        }),
      }),
    );
  });
  it("binds Telegram DM ACP spawns to the DM conversation id", async () => {
    const result = await runTelegramDmAcpCommand("/acp spawn codex --thread here");
    expect(result?.reply?.text).toContain("Spawned ACP session agent:codex:acp:");
    expect(result?.reply?.text).toContain("Bound this conversation to");
    expect(result?.reply?.channelData).toBeUndefined();
    expect(hoisted.sessionBindingBindMock).toHaveBeenCalledWith(
      expect.objectContaining({
        placement: "current",
        conversation: expect.objectContaining({
          channel: "telegram",
          accountId: "default",
          conversationId: "123456789",
        }),
      }),
    );
  });
  it("requires explicit ACP target when acp.defaultAgent is not configured", async () => {
    const result = await runDiscordAcpCommand("/acp spawn");

    expect(result?.reply?.text).toContain("ACP target agent is required");
    expect(result?.reply?.text).toContain("ACP target harness id is required");
    expect(hoisted.ensureSessionMock).not.toHaveBeenCalled();
  });

@@ -528,6 +633,42 @@ describe("/acp command", () => {
    expect(result?.reply?.text).toContain("Applied steering.");
  });

  it("resolves bound Telegram topic ACP sessions for /acp steer without explicit target", async () => {
    hoisted.sessionBindingResolveByConversationMock.mockImplementation(
      (ref: { channel?: string; accountId?: string; conversationId?: string }) =>
        ref.channel === "telegram" &&
        ref.accountId === "default" &&
        ref.conversationId === "-1003841603622:topic:498"
          ? createSessionBinding({
              targetSessionKey: defaultAcpSessionKey,
              conversation: {
                channel: "telegram",
                accountId: "default",
                conversationId: "-1003841603622:topic:498",
              },
            })
          : null,
    );
    hoisted.readAcpSessionEntryMock.mockReturnValue(createAcpSessionEntry());
    hoisted.runTurnMock.mockImplementation(async function* () {
      yield { type: "text_delta", text: "Viewed diver package." };
      yield { type: "done" };
    });
    const result = await runTelegramAcpCommand("/acp steer use npm to view package diver");
    expect(hoisted.runTurnMock).toHaveBeenCalledWith(
      expect.objectContaining({
        handle: expect.objectContaining({
          sessionKey: defaultAcpSessionKey,
        }),
        mode: "steer",
        text: "use npm to view package diver",
      }),
    );
    expect(result?.reply?.text).toContain("Viewed diver package.");
  });
  it("blocks /acp steer when ACP dispatch is disabled by policy", async () => {
    const cfg = {
      ...baseCfg,
‎src/auto-reply/reply/commands-acp/context.test.ts‎
+18Lines changed: 18 additions & 0 deletions
Original file line number	Diff line number	Diff line change
@@ -108,4 +108,22 @@ describe("commands-acp context", () => {
    });
    expect(resolveAcpCommandConversationId(params)).toBe("-1001234567890:topic:42");
  });
  it("resolves Telegram DM conversation ids from telegram targets", () => {
    const params = buildCommandTestParams("/acp status", baseCfg, {
      Provider: "telegram",
      Surface: "telegram",
      OriginatingChannel: "telegram",
      OriginatingTo: "telegram:123456789",
    });
    expect(resolveAcpCommandBindingContext(params)).toEqual({
      channel: "telegram",
      accountId: "default",
      threadId: undefined,
      conversationId: "123456789",
      parentConversationId: "123456789",
    });
    expect(resolveAcpCommandConversationId(params)).toBe("123456789");
  });
});
‎src/auto-reply/reply/commands-acp/context.ts‎
+20-10Lines changed: 20 additions & 10 deletions
Original file line number	Diff line number	Diff line change
@@ -6,6 +6,7 @@ import { DISCORD_THREAD_BINDING_CHANNEL } from "../../../channels/thread-binding
import { resolveConversationIdFromTargets } from "../../../infra/outbound/conversation-id.js";
import { parseAgentSessionKey } from "../../../routing/session-key.js";
import type { HandleCommandsParams } from "../commands-types.js";
import { resolveTelegramConversationId } from "../telegram-context.js";

function normalizeString(value: unknown): string {
  if (typeof value === "string") {
@@ -40,19 +41,28 @@ export function resolveAcpCommandThreadId(params: HandleCommandsParams): string
export function resolveAcpCommandConversationId(params: HandleCommandsParams): string | undefined {
  const channel = resolveAcpCommandChannel(params);
  if (channel === "telegram") {
    const telegramConversationId = resolveTelegramConversationId({
      ctx: {
        MessageThreadId: params.ctx.MessageThreadId,
        OriginatingTo: params.ctx.OriginatingTo,
        To: params.ctx.To,
      },
      command: {
        to: params.command.to,
      },
    });
    if (telegramConversationId) {
      return telegramConversationId;
    }
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
      return threadId;
      return (
        buildTelegramTopicConversationId({
          chatId: parentConversationId,
          topicId: threadId,
        }) ?? threadId
      );
    }
  }
  return resolveConversationIdFromTargets({
‎src/auto-reply/reply/commands-acp/lifecycle.ts‎
+32-22Lines changed: 32 additions & 22 deletions
Original file line number	Diff line number	Diff line change
@@ -37,7 +37,7 @@ import type { CommandHandlerResult, HandleCommandsParams } from "../commands-typ
import {
  resolveAcpCommandAccountId,
  resolveAcpCommandBindingContext,
  resolveAcpCommandThreadId,
  resolveAcpCommandConversationId,
} from "./context.js";
import {
  ACP_STEER_OUTPUT_LIMIT,
@@ -123,37 +123,39 @@ async function bindSpawnedAcpSessionToThread(params: {
  }

  const currentThreadId = bindingContext.threadId ?? "";
  if (threadMode === "here" && !currentThreadId) {
  const currentConversationId = bindingContext.conversationId?.trim() || "";
  const requiresThreadIdForHere = channel !== "telegram";
  if (
    threadMode === "here" &&
    ((requiresThreadIdForHere && !currentThreadId) ||
      (!requiresThreadIdForHere && !currentConversationId))
  ) {
    return {
      ok: false,
      error: `--thread here requires running /acp spawn inside an active ${channel} thread/conversation.`,
    };
  }

  const threadId = currentThreadId || undefined;
  const placement = threadId ? "current" : "child";
  const placement = channel === "telegram" ? "current" : currentThreadId ? "current" : "child";
  if (!capabilities.placements.includes(placement)) {
    return {
      ok: false,
      error: `Thread bindings do not support ${placement} placement for ${channel}.`,
    };
  }
  const channelId = placement === "child" ? bindingContext.conversationId : undefined;
  if (placement === "child" && !channelId) {
  if (!currentConversationId) {
    return {
      ok: false,
      error: `Could not resolve a ${channel} conversation for ACP thread spawn.`,
    };
  }

  const senderId = commandParams.command.senderId?.trim() || "";
  if (threadId) {
  if (placement === "current") {
    const existingBinding = bindingService.resolveByConversation({
      channel: spawnPolicy.channel,
      accountId: spawnPolicy.accountId,
      conversationId: threadId,
      conversationId: currentConversationId,
    });
    const boundBy =
      typeof existingBinding?.metadata?.boundBy === "string"
@@ -162,19 +164,13 @@ async function bindSpawnedAcpSessionToThread(params: {
    if (existingBinding && boundBy && boundBy !== "system" && senderId && senderId !== boundBy) {
      return {
        ok: false,
        error: `Only ${boundBy} can rebind this thread.`,
        error: `Only ${boundBy} can rebind this ${channel === "telegram" ? "conversation" : "thread"}.`,
      };
    }
  }

  const label = params.label || params.agentId;
  const conversationId = threadId || channelId;
  if (!conversationId) {
    return {
      ok: false,
      error: `Could not resolve a ${channel} conversation for ACP thread spawn.`,
    };
  }
  const conversationId = currentConversationId;

  try {
    const binding = await bindingService.bind({
@@ -344,12 +340,13 @@ export async function handleAcpSpawnAction(
    `✅ Spawned ACP session ${sessionKey} (${spawn.mode}, backend ${initializedBackend}).`,
  ];
  if (binding) {
    const currentThreadId = resolveAcpCommandThreadId(params) ?? "";
    const currentConversationId = resolveAcpCommandConversationId(params)?.trim() || "";
    const boundConversationId = binding.conversation.conversationId.trim();
    if (currentThreadId && boundConversationId === currentThreadId) {
      parts.push(`Bound this thread to ${sessionKey}.`);
    const placementLabel = binding.conversation.channel === "telegram" ? "conversation" : "thread";
    if (currentConversationId && boundConversationId === currentConversationId) {
      parts.push(`Bound this ${placementLabel} to ${sessionKey}.`);
    } else {
      parts.push(`Created thread ${boundConversationId} and bound it to ${sessionKey}.`);
      parts.push(`Created ${placementLabel} ${boundConversationId} and bound it to ${sessionKey}.`);
    }
  } else {
    parts.push("Session is unbound (use /focus <session-key> to bind this thread/conversation).");
@@ -360,6 +357,19 @@ export async function handleAcpSpawnAction(
    parts.push(`ℹ️ ${dispatchNote}`);
  }

  const shouldPinBindingNotice =
    binding?.conversation.channel === "telegram" &&
    binding.conversation.conversationId.includes(":topic:");
  if (shouldPinBindingNotice) {
    return {
      shouldContinue: false,
      reply: {
        text: parts.join(" "),
        channelData: { telegram: { pin: true } },
      },
    };
  }
  return stopWithText(parts.join(" "));
}

‎src/auto-reply/reply/commands-acp/shared.test.ts‎
+22Lines changed: 22 additions & 0 deletions
Original file line number	Diff line number	Diff line change
@@ -0,0 +1,22 @@
import { describe, expect, it } from "vitest";
import { parseSteerInput } from "./shared.js";
describe("parseSteerInput", () => {
  it("preserves non-option instruction tokens while normalizing unicode-dash flags", () => {
    const parsed = parseSteerInput([
      "\u2014session",
      "agent:codex:acp:s1",
      "\u2014briefly",
      "summarize",
      "this",
    ]);
    expect(parsed).toEqual({
      ok: true,
      value: {
        sessionToken: "agent:codex:acp:s1",
        instruction: "\u2014briefly summarize this",
      },
    });
  });
});
‎src/auto-reply/reply/commands-acp/shared.ts‎
+36-14Lines changed: 36 additions & 14 deletions
Original file line number	Diff line number	Diff line change
@@ -11,7 +11,7 @@ export { resolveAcpInstallCommandHint, resolveConfiguredAcpBackendId } from "./i

export const COMMAND = "/acp";
export const ACP_SPAWN_USAGE =
  "Usage: /acp spawn [agentId] [--mode persistent|oneshot] [--thread auto|here|off] [--cwd <path>] [--label <label>].";
  "Usage: /acp spawn [harness-id] [--mode persistent|oneshot] [--thread auto|here|off] [--cwd <path>] [--label <label>].";
export const ACP_STEER_USAGE =
  "Usage: /acp steer [--session <session-key|session-id|session-label>] <instruction>";
export const ACP_SET_MODE_USAGE =
@@ -77,6 +77,9 @@ export type ParsedSetCommandInput = {
  sessionToken?: string;
};

const ACP_UNICODE_DASH_PREFIX_RE =
  /^[\u2010\u2011\u2012\u2013\u2014\u2015\u2212\uFE58\uFE63\uFF0D]+/;
export function stopWithText(text: string): CommandHandlerResult {
  return {
    shouldContinue: false,
@@ -118,9 +121,9 @@ function readOptionValue(params: { tokens: string[]; index: number; flag: string
      error?: string;
    }
  | { matched: false } {
  const token = params.tokens[params.index] ?? "";
  const token = normalizeAcpOptionToken(params.tokens[params.index] ?? "");
  if (token === params.flag) {
    const nextValue = params.tokens[params.index + 1]?.trim() ?? "";
    const nextValue = normalizeAcpOptionToken(params.tokens[params.index + 1] ?? "");
    if (!nextValue || nextValue.startsWith("--")) {
      return {
        matched: true,
@@ -152,6 +155,18 @@ function readOptionValue(params: { tokens: string[]; index: number; flag: string
  return { matched: false };
}

function normalizeAcpOptionToken(raw: string): string {
  const token = raw.trim();
  if (!token || token.startsWith("--")) {
    return token;
  }
  const dashPrefix = token.match(ACP_UNICODE_DASH_PREFIX_RE)?.[0];
  if (!dashPrefix) {
    return token;
  }
  return `--${token.slice(dashPrefix.length)}`;
}
function resolveDefaultSpawnThreadMode(params: HandleCommandsParams): AcpSpawnThreadMode {
  if (resolveAcpCommandChannel(params) !== DISCORD_THREAD_BINDING_CHANNEL) {
    return "off";
@@ -164,16 +179,17 @@ export function parseSpawnInput(
  params: HandleCommandsParams,
  tokens: string[],
): { ok: true; value: ParsedSpawnInput } | { ok: false; error: string } {
  const normalizedTokens = tokens.map((token) => normalizeAcpOptionToken(token));
  let mode: AcpRuntimeSessionMode = "persistent";
  let thread = resolveDefaultSpawnThreadMode(params);
  let cwd: string | undefined;
  let label: string | undefined;
  let rawAgentId: string | undefined;

  for (let i = 0; i < tokens.length; ) {
    const token = tokens[i] ?? "";
  for (let i = 0; i < normalizedTokens.length; ) {
    const token = normalizedTokens[i] ?? "";

    const modeOption = readOptionValue({ tokens, index: i, flag: "--mode" });
    const modeOption = readOptionValue({ tokens: normalizedTokens, index: i, flag: "--mode" });
    if (modeOption.matched) {
      if (modeOption.error) {
        return { ok: false, error: `${modeOption.error}. ${ACP_SPAWN_USAGE}` };
@@ -190,7 +206,11 @@ export function parseSpawnInput(
      continue;
    }

    const threadOption = readOptionValue({ tokens, index: i, flag: "--thread" });
    const threadOption = readOptionValue({
      tokens: normalizedTokens,
      index: i,
      flag: "--thread",
    });
    if (threadOption.matched) {
      if (threadOption.error) {
        return { ok: false, error: `${threadOption.error}. ${ACP_SPAWN_USAGE}` };
@@ -207,7 +227,7 @@ export function parseSpawnInput(
      continue;
    }

    const cwdOption = readOptionValue({ tokens, index: i, flag: "--cwd" });
    const cwdOption = readOptionValue({ tokens: normalizedTokens, index: i, flag: "--cwd" });
    if (cwdOption.matched) {
      if (cwdOption.error) {
        return { ok: false, error: `${cwdOption.error}. ${ACP_SPAWN_USAGE}` };
@@ -217,7 +237,7 @@ export function parseSpawnInput(
      continue;
    }

    const labelOption = readOptionValue({ tokens, index: i, flag: "--label" });
    const labelOption = readOptionValue({ tokens: normalizedTokens, index: i, flag: "--label" });
    if (labelOption.matched) {
      if (labelOption.error) {
        return { ok: false, error: `${labelOption.error}. ${ACP_SPAWN_USAGE}` };
@@ -251,7 +271,7 @@ export function parseSpawnInput(
  if (!selectedAgent) {
    return {
      ok: false,
      error: `ACP target agent is required. Pass an agent id or configure acp.defaultAgent. ${ACP_SPAWN_USAGE}`,
      error: `ACP target harness id is required. Pass an ACP harness id (for example codex) or configure acp.defaultAgent. ${ACP_SPAWN_USAGE}`,
    };
  }
  const normalizedAgentId = normalizeAgentId(selectedAgent);
@@ -271,12 +291,13 @@ export function parseSpawnInput(
export function parseSteerInput(
  tokens: string[],
): { ok: true; value: ParsedSteerInput } | { ok: false; error: string } {
  const normalizedTokens = tokens.map((token) => normalizeAcpOptionToken(token));
  let sessionToken: string | undefined;
  const instructionTokens: string[] = [];

  for (let i = 0; i < tokens.length; ) {
  for (let i = 0; i < normalizedTokens.length; ) {
    const sessionOption = readOptionValue({
      tokens,
      tokens: normalizedTokens,
      index: i,
      flag: "--session",
    });
@@ -292,7 +313,7 @@ export function parseSteerInput(
      continue;
    }

    instructionTokens.push(tokens[i]);
    instructionTokens.push(tokens[i] ?? "");
    i += 1;
  }

@@ -380,7 +401,7 @@ export function resolveAcpHelpText(): string {
  return [
    "ACP commands:",
    "-----",
    "/acp spawn [agentId] [--mode persistent|oneshot] [--thread auto|here|off] [--cwd <path>] [--label <label>]",
    "/acp spawn [harness-id] [--mode persistent|oneshot] [--thread auto|here|off] [--cwd <path>] [--label <label>]",
    "/acp cancel [session-key|session-id|session-label]",
    "/acp steer [--session <session-key|session-id|session-label>] <instruction>",
    "/acp close [session-key|session-id|session-label]",
@@ -397,6 +418,7 @@ export function resolveAcpHelpText(): string {
    "/acp sessions",
    "",
    "Notes:",
    "- /acp spawn harness-id is an ACP runtime harness alias (for example codex), not an OpenClaw agents.list id.",
    "- /focus and /unfocus also work with ACP session keys.",
    "- ACP dispatch of normal thread messages is controlled by acp.dispatch.enabled.",
  ].join("\n");
‎src/auto-reply/reply/commands-session-lifecycle.test.ts‎
+141-7Lines changed: 141 additions & 7 deletions
Original file line number	Diff line number	Diff line change
@@ -1,14 +1,21 @@
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { OpenClawConfig } from "../../config/config.js";
import type { SessionBindingRecord } from "../../infra/outbound/session-binding-service.js";

const hoisted = vi.hoisted(() => {
  const getThreadBindingManagerMock = vi.fn();
  const setThreadBindingIdleTimeoutBySessionKeyMock = vi.fn();
  const setThreadBindingMaxAgeBySessionKeyMock = vi.fn();
  const setTelegramThreadBindingIdleTimeoutBySessionKeyMock = vi.fn();
  const setTelegramThreadBindingMaxAgeBySessionKeyMock = vi.fn();
  const sessionBindingResolveByConversationMock = vi.fn();
  return {
    getThreadBindingManagerMock,
    setThreadBindingIdleTimeoutBySessionKeyMock,
    setThreadBindingMaxAgeBySessionKeyMock,
    setTelegramThreadBindingIdleTimeoutBySessionKeyMock,
    setTelegramThreadBindingMaxAgeBySessionKeyMock,
    sessionBindingResolveByConversationMock,
  };
});

@@ -22,6 +29,33 @@ vi.mock("../../discord/monitor/thread-bindings.js", async (importOriginal) => {
  };
});

vi.mock("../../telegram/thread-bindings.js", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../telegram/thread-bindings.js")>();
  return {
    ...actual,
    setTelegramThreadBindingIdleTimeoutBySessionKey:
      hoisted.setTelegramThreadBindingIdleTimeoutBySessionKeyMock,
    setTelegramThreadBindingMaxAgeBySessionKey:
      hoisted.setTelegramThreadBindingMaxAgeBySessionKeyMock,
  };
});
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
  };
});
const { handleSessionCommand } = await import("./commands-session.js");
const { buildCommandTestParams } = await import("./commands.test-harness.js");

@@ -55,6 +89,18 @@ function createDiscordCommandParams(commandBody: string, overrides?: Record<stri
  });
}

function createTelegramCommandParams(commandBody: string, overrides?: Record<string, unknown>) {
  return buildCommandTestParams(commandBody, baseCfg, {
    Provider: "telegram",
    Surface: "telegram",
    OriginatingChannel: "telegram",
    OriginatingTo: "-100200300:topic:77",
    AccountId: "default",
    MessageThreadId: "77",
    ...overrides,
  });
}
function createFakeBinding(overrides: Partial<FakeBinding> = {}): FakeBinding {
  const now = Date.now();
  return {
@@ -71,6 +117,28 @@ function createFakeBinding(overrides: Partial<FakeBinding> = {}): FakeBinding {
  };
}

function createTelegramBinding(overrides?: Partial<SessionBindingRecord>): SessionBindingRecord {
  return {
    bindingId: "default:-100200300:topic:77",
    targetSessionKey: "agent:main:subagent:child",
    targetKind: "subagent",
    conversation: {
      channel: "telegram",
      accountId: "default",
      conversationId: "-100200300:topic:77",
    },
    status: "active",
    boundAt: Date.now(),
    metadata: {
      boundBy: "user-1",
      lastActivityAt: Date.now(),
      idleTimeoutMs: 24 * 60 * 60 * 1000,
      maxAgeMs: 0,
    },
    ...overrides,
  };
}
function createFakeThreadBindingManager(binding: FakeBinding | null) {
  return {
    getByThreadId: vi.fn((_threadId: string) => binding),
@@ -81,13 +149,16 @@ function createFakeThreadBindingManager(binding: FakeBinding | null) {

describe("/session idle and /session max-age", () => {
  beforeEach(() => {
    hoisted.getThreadBindingManagerMock.mockClear();
    hoisted.setThreadBindingIdleTimeoutBySessionKeyMock.mockClear();
    hoisted.setThreadBindingMaxAgeBySessionKeyMock.mockClear();
    hoisted.getThreadBindingManagerMock.mockReset();
    hoisted.setThreadBindingIdleTimeoutBySessionKeyMock.mockReset();
    hoisted.setThreadBindingMaxAgeBySessionKeyMock.mockReset();
    hoisted.setTelegramThreadBindingIdleTimeoutBySessionKeyMock.mockReset();
    hoisted.setTelegramThreadBindingMaxAgeBySessionKeyMock.mockReset();
    hoisted.sessionBindingResolveByConversationMock.mockReset().mockReturnValue(null);
    vi.useRealTimers();
  });

  it("sets idle timeout for the focused session", async () => {
  it("sets idle timeout for the focused Discord session", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-02-20T00:00:00.000Z"));

@@ -128,7 +199,7 @@ describe("/session idle and /session max-age", () => {
    expect(result?.reply?.text).toContain("2026-02-20T02:00:00.000Z");
  });

  it("sets max age for the focused session", async () => {
  it("sets max age for the focused Discord session", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-02-20T00:00:00.000Z"));

@@ -157,6 +228,67 @@ describe("/session idle and /session max-age", () => {
    expect(text).toContain("2026-02-20T03:00:00.000Z");
  });

  it("sets idle timeout for focused Telegram conversations", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-02-20T00:00:00.000Z"));
    hoisted.sessionBindingResolveByConversationMock.mockReturnValue(createTelegramBinding());
    hoisted.setTelegramThreadBindingIdleTimeoutBySessionKeyMock.mockReturnValue([
      {
        targetSessionKey: "agent:main:subagent:child",
        boundAt: Date.now(),
        lastActivityAt: Date.now(),
        idleTimeoutMs: 2 * 60 * 60 * 1000,
      },
    ]);
    const result = await handleSessionCommand(
      createTelegramCommandParams("/session idle 2h"),
      true,
    );
    const text = result?.reply?.text ?? "";
    expect(hoisted.setTelegramThreadBindingIdleTimeoutBySessionKeyMock).toHaveBeenCalledWith({
      targetSessionKey: "agent:main:subagent:child",
      accountId: "default",
      idleTimeoutMs: 2 * 60 * 60 * 1000,
    });
    expect(text).toContain("Idle timeout set to 2h");
    expect(text).toContain("2026-02-20T02:00:00.000Z");
  });
  it("reports Telegram max-age expiry from the original bind time", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-02-20T00:00:00.000Z"));
    const boundAt = Date.parse("2026-02-19T22:00:00.000Z");
    hoisted.sessionBindingResolveByConversationMock.mockReturnValue(
      createTelegramBinding({ boundAt }),
    );
    hoisted.setTelegramThreadBindingMaxAgeBySessionKeyMock.mockReturnValue([
      {
        targetSessionKey: "agent:main:subagent:child",
        boundAt,
        lastActivityAt: Date.now(),
        maxAgeMs: 3 * 60 * 60 * 1000,
      },
    ]);
    const result = await handleSessionCommand(
      createTelegramCommandParams("/session max-age 3h"),
      true,
    );
    const text = result?.reply?.text ?? "";
    expect(hoisted.setTelegramThreadBindingMaxAgeBySessionKeyMock).toHaveBeenCalledWith({
      targetSessionKey: "agent:main:subagent:child",
      accountId: "default",
      maxAgeMs: 3 * 60 * 60 * 1000,
    });
    expect(text).toContain("Max age set to 3h");
    expect(text).toContain("2026-02-20T01:00:00.000Z");
  });
  it("disables max age when set to off", async () => {
    const binding = createFakeBinding({ maxAgeMs: 2 * 60 * 60 * 1000 });
    hoisted.getThreadBindingManagerMock.mockReturnValue(createFakeThreadBindingManager(binding));
@@ -175,10 +307,12 @@ describe("/session idle and /session max-age", () => {
    expect(result?.reply?.text).toContain("Max age disabled");
  });

  it("is unavailable outside discord", async () => {
  it("is unavailable outside discord and telegram", async () => {
    const params = buildCommandTestParams("/session idle 2h", baseCfg);
    const result = await handleSessionCommand(params, true);
    expect(result?.reply?.text).toContain("currently available for Discord thread-bound sessions");
    expect(result?.reply?.text).toContain(
      "currently available for Discord and Telegram bound sessions",
    );
  });

  it("requires binding owner for lifecycle updates", async () => {
‎src/auto-reply/reply/commands-session.ts‎
+174-50Lines changed: 174 additions & 50 deletions
Original file line number	Diff line number	Diff line change
@@ -11,16 +11,23 @@ import {
  setThreadBindingMaxAgeBySessionKey,
} from "../../discord/monitor/thread-bindings.js";
import { logVerbose } from "../../globals.js";
import { getSessionBindingService } from "../../infra/outbound/session-binding-service.js";
import type { SessionBindingRecord } from "../../infra/outbound/session-binding-service.js";
import { scheduleGatewaySigusr1Restart, triggerOpenClawRestart } from "../../infra/restart.js";
import { loadCostUsageSummary, loadSessionCostSummary } from "../../infra/session-cost-usage.js";
import {
  setTelegramThreadBindingIdleTimeoutBySessionKey,
  setTelegramThreadBindingMaxAgeBySessionKey,
} from "../../telegram/thread-bindings.js";
import { formatTokenCount, formatUsd } from "../../utils/usage-format.js";
import { parseActivationCommand } from "../group-activation.js";
import { parseSendPolicyCommand } from "../send-policy.js";
import { normalizeUsageDisplay, resolveResponseUsageMode } from "../thinking.js";
import { isDiscordSurface, isTelegramSurface, resolveChannelAccountId } from "./channel-context.js";
import { handleAbortTrigger, handleStopCommand } from "./commands-session-abort.js";
import { persistSessionEntry } from "./commands-session-store.js";
import type { CommandHandler } from "./commands-types.js";
import { isDiscordSurface, resolveDiscordAccountId } from "./discord-context.js";
import { resolveTelegramConversationId } from "./telegram-context.js";

const SESSION_COMMAND_PREFIX = "/session";
const SESSION_DURATION_OFF_VALUES = new Set(["off", "disable", "disabled", "none", "0"]);
@@ -53,6 +60,72 @@ function formatSessionExpiry(expiresAt: number) {
  return new Date(expiresAt).toISOString();
}

function resolveTelegramBindingDurationMs(
  binding: SessionBindingRecord,
  key: "idleTimeoutMs" | "maxAgeMs",
  fallbackMs: number,
): number {
  const raw = binding.metadata?.[key];
  if (typeof raw !== "number" || !Number.isFinite(raw)) {
    return fallbackMs;
  }
  return Math.max(0, Math.floor(raw));
}
function resolveTelegramBindingLastActivityAt(binding: SessionBindingRecord): number {
  const raw = binding.metadata?.lastActivityAt;
  if (typeof raw !== "number" || !Number.isFinite(raw)) {
    return binding.boundAt;
  }
  return Math.max(Math.floor(raw), binding.boundAt);
}
function resolveTelegramBindingBoundBy(binding: SessionBindingRecord): string {
  const raw = binding.metadata?.boundBy;
  return typeof raw === "string" ? raw.trim() : "";
}
type UpdatedLifecycleBinding = {
  boundAt: number;
  lastActivityAt: number;
  idleTimeoutMs?: number;
  maxAgeMs?: number;
};
function resolveUpdatedBindingExpiry(params: {
  action: typeof SESSION_ACTION_IDLE | typeof SESSION_ACTION_MAX_AGE;
  bindings: UpdatedLifecycleBinding[];
}): number | undefined {
  const expiries = params.bindings
    .map((binding) => {
      if (params.action === SESSION_ACTION_IDLE) {
        const idleTimeoutMs =
          typeof binding.idleTimeoutMs === "number" && Number.isFinite(binding.idleTimeoutMs)
            ? Math.max(0, Math.floor(binding.idleTimeoutMs))
            : 0;
        if (idleTimeoutMs <= 0) {
          return undefined;
        }
        return Math.max(binding.lastActivityAt, binding.boundAt) + idleTimeoutMs;
      }
      const maxAgeMs =
        typeof binding.maxAgeMs === "number" && Number.isFinite(binding.maxAgeMs)
          ? Math.max(0, Math.floor(binding.maxAgeMs))
          : 0;
      if (maxAgeMs <= 0) {
        return undefined;
      }
      return binding.boundAt + maxAgeMs;
    })
    .filter((expiresAt): expiresAt is number => typeof expiresAt === "number");
  if (expiries.length === 0) {
    return undefined;
  }
  return Math.min(...expiries);
}
export const handleActivationCommand: CommandHandler = async (params, allowTextCommands) => {
  if (!allowTextCommands) {
    return null;
@@ -243,59 +316,98 @@ export const handleSessionCommand: CommandHandler = async (params, allowTextComm
    };
  }

  if (!isDiscordSurface(params)) {
  const onDiscord = isDiscordSurface(params);
  const onTelegram = isTelegramSurface(params);
  if (!onDiscord && !onTelegram) {
    return {
      shouldContinue: false,
      reply: {
        text: "⚠️ /session idle and /session max-age are currently available for Discord thread-bound sessions.",
        text: "⚠️ /session idle and /session max-age are currently available for Discord and Telegram bound sessions.",
      },
    };
  }

  const accountId = resolveChannelAccountId(params);
  const sessionBindingService = getSessionBindingService();
  const threadId =
    params.ctx.MessageThreadId != null ? String(params.ctx.MessageThreadId).trim() : "";
  if (!threadId) {
  const telegramConversationId = onTelegram ? resolveTelegramConversationId(params) : undefined;
  const discordManager = onDiscord ? getThreadBindingManager(accountId) : null;
  if (onDiscord && !discordManager) {
    return {
      shouldContinue: false,
      reply: {
        text: "⚠️ /session idle and /session max-age must be run inside a focused Discord thread.",
      },
      reply: { text: "⚠️ Discord thread bindings are unavailable for this account." },
    };
  }

  const accountId = resolveDiscordAccountId(params);
  const threadBindings = getThreadBindingManager(accountId);
  if (!threadBindings) {
  const discordBinding =
    onDiscord && threadId ? discordManager?.getByThreadId(threadId) : undefined;
  const telegramBinding =
    onTelegram && telegramConversationId
      ? sessionBindingService.resolveByConversation({
          channel: "telegram",
          accountId,
          conversationId: telegramConversationId,
        })
      : null;
  if (onDiscord && !discordBinding) {
    if (onDiscord && !threadId) {
      return {
        shouldContinue: false,
        reply: {
          text: "⚠️ /session idle and /session max-age must be run inside a focused Discord thread.",
        },
      };
    }
    return {
      shouldContinue: false,
      reply: { text: "⚠️ Discord thread bindings are unavailable for this account." },
      reply: { text: "ℹ️ This thread is not currently focused." },
    };
  }
  const binding = threadBindings.getByThreadId(threadId);
  if (!binding) {
  if (onTelegram && !telegramBinding) {
    if (!telegramConversationId) {
      return {
        shouldContinue: false,
        reply: {
          text: "⚠️ /session idle and /session max-age on Telegram require a topic context in groups, or a direct-message conversation.",
        },
      };
    }
    return {
      shouldContinue: false,
      reply: { text: "ℹ️ This thread is not currently focused." },
      reply: { text: "ℹ️ This conversation is not currently focused." },
    };
  }

  const idleTimeoutMs = resolveThreadBindingIdleTimeoutMs({
    record: binding,
    defaultIdleTimeoutMs: threadBindings.getIdleTimeoutMs(),
  });
  const idleExpiresAt = resolveThreadBindingInactivityExpiresAt({
    record: binding,
    defaultIdleTimeoutMs: threadBindings.getIdleTimeoutMs(),
  });
  const maxAgeMs = resolveThreadBindingMaxAgeMs({
    record: binding,
    defaultMaxAgeMs: threadBindings.getMaxAgeMs(),
  });
  const maxAgeExpiresAt = resolveThreadBindingMaxAgeExpiresAt({
    record: binding,
    defaultMaxAgeMs: threadBindings.getMaxAgeMs(),
  });
  const idleTimeoutMs = onDiscord
    ? resolveThreadBindingIdleTimeoutMs({
        record: discordBinding!,
        defaultIdleTimeoutMs: discordManager!.getIdleTimeoutMs(),
      })
    : resolveTelegramBindingDurationMs(telegramBinding!, "idleTimeoutMs", 24 * 60 * 60 * 1000);
  const idleExpiresAt = onDiscord
    ? resolveThreadBindingInactivityExpiresAt({
        record: discordBinding!,
        defaultIdleTimeoutMs: discordManager!.getIdleTimeoutMs(),
      })
    : idleTimeoutMs > 0
      ? resolveTelegramBindingLastActivityAt(telegramBinding!) + idleTimeoutMs
      : undefined;
  const maxAgeMs = onDiscord
    ? resolveThreadBindingMaxAgeMs({
        record: discordBinding!,
        defaultMaxAgeMs: discordManager!.getMaxAgeMs(),
      })
    : resolveTelegramBindingDurationMs(telegramBinding!, "maxAgeMs", 0);
  const maxAgeExpiresAt = onDiscord
    ? resolveThreadBindingMaxAgeExpiresAt({
        record: discordBinding!,
        defaultMaxAgeMs: discordManager!.getMaxAgeMs(),
      })
    : maxAgeMs > 0
      ? telegramBinding!.boundAt + maxAgeMs
      : undefined;

  const durationArgRaw = tokens.slice(1).join("");
  if (!durationArgRaw) {
@@ -337,11 +449,16 @@ export const handleSessionCommand: CommandHandler = async (params, allowTextComm
  }

  const senderId = params.command.senderId?.trim() || "";
  if (binding.boundBy && binding.boundBy !== "system" && senderId && senderId !== binding.boundBy) {
  const boundBy = onDiscord
    ? discordBinding!.boundBy
    : resolveTelegramBindingBoundBy(telegramBinding!);
  if (boundBy && boundBy !== "system" && senderId && senderId !== boundBy) {
    return {
      shouldContinue: false,
      reply: {
        text: `⚠️ Only ${binding.boundBy} can update session lifecycle settings for this thread.`,
        text: onDiscord
          ? `⚠️ Only ${boundBy} can update session lifecycle settings for this thread.`
          : `⚠️ Only ${boundBy} can update session lifecycle settings for this conversation.`,
      },
    };
  }
@@ -356,18 +473,32 @@ export const handleSessionCommand: CommandHandler = async (params, allowTextComm
    };
  }

  const updatedBindings =
    action === SESSION_ACTION_IDLE
      ? setThreadBindingIdleTimeoutBySessionKey({
          targetSessionKey: binding.targetSessionKey,
  const updatedBindings = (() => {
    if (onDiscord) {
      return action === SESSION_ACTION_IDLE
        ? setThreadBindingIdleTimeoutBySessionKey({
            targetSessionKey: discordBinding!.targetSessionKey,
            accountId,
            idleTimeoutMs: durationMs,
          })
        : setThreadBindingMaxAgeBySessionKey({
            targetSessionKey: discordBinding!.targetSessionKey,
            accountId,
            maxAgeMs: durationMs,
          });
    }
    return action === SESSION_ACTION_IDLE
      ? setTelegramThreadBindingIdleTimeoutBySessionKey({
          targetSessionKey: telegramBinding!.targetSessionKey,
          accountId,
          idleTimeoutMs: durationMs,
        })
      : setThreadBindingMaxAgeBySessionKey({
          targetSessionKey: binding.targetSessionKey,
      : setTelegramThreadBindingMaxAgeBySessionKey({
          targetSessionKey: telegramBinding!.targetSessionKey,
          accountId,
          maxAgeMs: durationMs,
        });
  })();
  if (updatedBindings.length === 0) {
    return {
      shouldContinue: false,
@@ -392,17 +523,10 @@ export const handleSessionCommand: CommandHandler = async (params, allowTextComm
    };
  }

  const nextBinding = updatedBindings[0];
  const nextExpiry =
    action === SESSION_ACTION_IDLE
      ? resolveThreadBindingInactivityExpiresAt({
          record: nextBinding,
          defaultIdleTimeoutMs: threadBindings.getIdleTimeoutMs(),
        })
      : resolveThreadBindingMaxAgeExpiresAt({
          record: nextBinding,
          defaultMaxAgeMs: threadBindings.getMaxAgeMs(),
        });
  const nextExpiry = resolveUpdatedBindingExpiry({
    action,
    bindings: updatedBindings,
  });
  const expiryLabel =
    typeof nextExpiry === "number" && Number.isFinite(nextExpiry)
      ? formatSessionExpiry(nextExpiry)
‎src/auto-reply/reply/commands-subagents-focus.test.ts‎
+129-211Lines changed: 129 additions & 211 deletions
Large diffs are not rendered by default.
‎src/auto-reply/reply/commands-subagents.ts‎
+1-1Lines changed: 1 addition & 1 deletion
Original file line number	Diff line number	Diff line change
@@ -70,7 +70,7 @@ export const handleSubagentsCommand: CommandHandler = async (params, allowTextCo
    case "focus":
      return await handleSubagentsFocusAction(ctx);
    case "unfocus":
      return handleSubagentsUnfocusAction(ctx);
      return await handleSubagentsUnfocusAction(ctx);
    case "list":
      return handleSubagentsListAction(ctx);
    case "kill":
‎src/auto-reply/reply/commands-subagents/action-agents.ts‎
+63-23Lines changed: 63 additions & 23 deletions
Original file line number	Diff line number	Diff line change
@@ -1,23 +1,55 @@
import { getThreadBindingManager } from "../../../discord/monitor/thread-bindings.js";
import { getSessionBindingService } from "../../../infra/outbound/session-binding-service.js";
import type { CommandHandlerResult } from "../commands-types.js";
import { formatRunLabel, sortSubagentRuns } from "../subagents-utils.js";
import {
  type SubagentsCommandContext,
  isDiscordSurface,
  resolveDiscordAccountId,
  resolveChannelAccountId,
  resolveCommandSurfaceChannel,
  stopWithText,
} from "./shared.js";

function formatConversationBindingText(params: {
  channel: string;
  conversationId: string;
}): string {
  if (params.channel === "discord") {
    return `thread:${params.conversationId}`;
  }
  if (params.channel === "telegram") {
    return `conversation:${params.conversationId}`;
  }
  return `binding:${params.conversationId}`;
}
export function handleSubagentsAgentsAction(ctx: SubagentsCommandContext): CommandHandlerResult {
  const { params, requesterKey, runs } = ctx;
  const isDiscord = isDiscordSurface(params);
  const accountId = isDiscord ? resolveDiscordAccountId(params) : undefined;
  const threadBindings = accountId ? getThreadBindingManager(accountId) : null;
  const channel = resolveCommandSurfaceChannel(params);
  const accountId = resolveChannelAccountId(params);
  const bindingService = getSessionBindingService();
  const bindingsBySession = new Map<string, ReturnType<typeof bindingService.listBySession>>();
  const resolveSessionBindings = (sessionKey: string) => {
    const cached = bindingsBySession.get(sessionKey);
    if (cached) {
      return cached;
    }
    const resolved = bindingService
      .listBySession(sessionKey)
      .filter(
        (entry) =>
          entry.status === "active" &&
          entry.conversation.channel === channel &&
          entry.conversation.accountId === accountId,
      );
    bindingsBySession.set(sessionKey, resolved);
    return resolved;
  };
  const visibleRuns = sortSubagentRuns(runs).filter((entry) => {
    if (!entry.endedAt) {
      return true;
    }
    return Boolean(threadBindings?.listBySessionKey(entry.childSessionKey)[0]);
    return resolveSessionBindings(entry.childSessionKey).length > 0;
  });

  const lines = ["agents:", "-----"];
@@ -26,28 +58,36 @@ export function handleSubagentsAgentsAction(ctx: SubagentsCommandContext): Comma
  } else {
    let index = 1;
    for (const entry of visibleRuns) {
      const threadBinding = threadBindings?.listBySessionKey(entry.childSessionKey)[0];
      const bindingText = threadBinding
        ? `thread:${threadBinding.threadId}`
        : isDiscord
      const binding = resolveSessionBindings(entry.childSessionKey)[0];
      const bindingText = binding
        ? formatConversationBindingText({
            channel,
            conversationId: binding.conversation.conversationId,
          })
        : channel === "discord" || channel === "telegram"
          ? "unbound"
          : "bindings available on discord";
          : "bindings available on discord/telegram";
      lines.push(`${index}. ${formatRunLabel(entry)} (${bindingText})`);
      index += 1;
    }
  }

  if (threadBindings) {
    const acpBindings = threadBindings
      .listBindings()
      .filter((entry) => entry.targetKind === "acp" && entry.targetSessionKey === requesterKey);
    if (acpBindings.length > 0) {
      lines.push("", "acp/session bindings:", "-----");
      for (const binding of acpBindings) {
        lines.push(
          `- ${binding.label ?? binding.targetSessionKey} (thread:${binding.threadId}, session:${binding.targetSessionKey})`,
        );
      }
  const requesterBindings = resolveSessionBindings(requesterKey).filter(
    (entry) => entry.targetKind === "session",
  );
  if (requesterBindings.length > 0) {
    lines.push("", "acp/session bindings:", "-----");
    for (const binding of requesterBindings) {
      const label =
        typeof binding.metadata?.label === "string" && binding.metadata.label.trim()
          ? binding.metadata.label.trim()
          : binding.targetSessionKey;
      lines.push(
        `- ${label} (${formatConversationBindingText({
          channel,
          conversationId: binding.conversation.conversationId,
        })}, session:${binding.targetSessionKey})`,
      );
    }
  }

‎src/auto-reply/reply/commands-subagents/action-focus.ts‎
+94-43Lines changed: 94 additions & 43 deletions
Original file line number	Diff line number	Diff line change
@@ -4,71 +4,122 @@ import {
} from "../../../acp/runtime/session-identifiers.js";
import { readAcpSessionEntry } from "../../../acp/runtime/session-meta.js";
import {
  resolveDiscordThreadBindingIdleTimeoutMs,
  resolveDiscordThreadBindingMaxAgeMs,
  resolveThreadBindingIntroText,
  resolveThreadBindingThreadName,
} from "../../../discord/monitor/thread-bindings.js";
} from "../../../channels/thread-bindings-messages.js";
import {
  resolveThreadBindingIdleTimeoutMsForChannel,
  resolveThreadBindingMaxAgeMsForChannel,
} from "../../../channels/thread-bindings-policy.js";
import { getSessionBindingService } from "../../../infra/outbound/session-binding-service.js";
import type { CommandHandlerResult } from "../commands-types.js";
import {
  type SubagentsCommandContext,
  isDiscordSurface,
  resolveDiscordAccountId,
  isTelegramSurface,
  resolveChannelAccountId,
  resolveCommandSurfaceChannel,
  resolveDiscordChannelIdForFocus,
  resolveFocusTargetSession,
  resolveTelegramConversationId,
  stopWithText,
} from "./shared.js";

type FocusBindingContext = {
  channel: "discord" | "telegram";
  accountId: string;
  conversationId: string;
  placement: "current" | "child";
  labelNoun: "thread" | "conversation";
};
function resolveFocusBindingContext(
  params: SubagentsCommandContext["params"],
): FocusBindingContext | null {
  if (isDiscordSurface(params)) {
    const currentThreadId =
      params.ctx.MessageThreadId != null ? String(params.ctx.MessageThreadId).trim() : "";
    const parentChannelId = currentThreadId ? undefined : resolveDiscordChannelIdForFocus(params);
    const conversationId = currentThreadId || parentChannelId;
    if (!conversationId) {
      return null;
    }
    return {
      channel: "discord",
      accountId: resolveChannelAccountId(params),
      conversationId,
      placement: currentThreadId ? "current" : "child",
      labelNoun: "thread",
    };
  }
  if (isTelegramSurface(params)) {
    const conversationId = resolveTelegramConversationId(params);
    if (!conversationId) {
      return null;
    }
    return {
      channel: "telegram",
      accountId: resolveChannelAccountId(params),
      conversationId,
      placement: "current",
      labelNoun: "conversation",
    };
  }
  return null;
}
export async function handleSubagentsFocusAction(
  ctx: SubagentsCommandContext,
): Promise<CommandHandlerResult> {
  const { params, runs, restTokens } = ctx;
  if (!isDiscordSurface(params)) {
    return stopWithText("⚠️ /focus is only available on Discord.");
  const channel = resolveCommandSurfaceChannel(params);
  if (channel !== "discord" && channel !== "telegram") {
    return stopWithText("⚠️ /focus is only available on Discord and Telegram.");
  }

  const token = restTokens.join(" ").trim();
  if (!token) {
    return stopWithText("Usage: /focus <subagent-label|session-key|session-id|session-label>");
  }

  const accountId = resolveDiscordAccountId(params);
  const accountId = resolveChannelAccountId(params);
  const bindingService = getSessionBindingService();
  const capabilities = bindingService.getCapabilities({
    channel: "discord",
    channel,
    accountId,
  });
  if (!capabilities.adapterAvailable || !capabilities.bindSupported) {
    return stopWithText("⚠️ Discord thread bindings are unavailable for this account.");
    const label = channel === "discord" ? "Discord thread" : "Telegram conversation";
    return stopWithText(`⚠️ ${label} bindings are unavailable for this account.`);
  }

  const focusTarget = await resolveFocusTargetSession({ runs, token });
  if (!focusTarget) {
    return stopWithText(`⚠️ Unable to resolve focus target: ${token}`);
  }

  const currentThreadId =
    params.ctx.MessageThreadId != null ? String(params.ctx.MessageThreadId).trim() : "";
  const parentChannelId = currentThreadId ? undefined : resolveDiscordChannelIdForFocus(params);
  if (!currentThreadId && !parentChannelId) {
  const bindingContext = resolveFocusBindingContext(params);
  if (!bindingContext) {
    if (channel === "telegram") {
      return stopWithText(
        "⚠️ /focus on Telegram requires a topic context in groups, or a direct-message conversation.",
      );
    }
    return stopWithText("⚠️ Could not resolve a Discord channel for /focus.");
  }

  const senderId = params.command.senderId?.trim() || "";
  if (currentThreadId) {
    const existingBinding = bindingService.resolveByConversation({
      channel: "discord",
      accountId,
      conversationId: currentThreadId,
    });
    const boundBy =
      typeof existingBinding?.metadata?.boundBy === "string"
        ? existingBinding.metadata.boundBy.trim()
        : "";
    if (existingBinding && boundBy && boundBy !== "system" && senderId && senderId !== boundBy) {
      return stopWithText(`⚠️ Only ${boundBy} can refocus this thread.`);
    }
  const existingBinding = bindingService.resolveByConversation({
    channel: bindingContext.channel,
    accountId: bindingContext.accountId,
    conversationId: bindingContext.conversationId,
  });
  const boundBy =
    typeof existingBinding?.metadata?.boundBy === "string"
      ? existingBinding.metadata.boundBy.trim()
      : "";
  if (existingBinding && boundBy && boundBy !== "system" && senderId && senderId !== boundBy) {
    return stopWithText(`⚠️ Only ${boundBy} can refocus this ${bindingContext.labelNoun}.`);
  }

  const label = focusTarget.label || token;
@@ -79,13 +130,8 @@ export async function handleSubagentsFocusAction(
          sessionKey: focusTarget.targetSessionKey,
        })?.acp
      : undefined;
  const placement = currentThreadId ? "current" : "child";
  if (!capabilities.placements.includes(placement)) {
    return stopWithText("⚠️ Discord thread bindings are unavailable for this account.");
  }
  const conversationId = currentThreadId || parentChannelId;
  if (!conversationId) {
    return stopWithText("⚠️ Could not resolve a Discord channel for /focus.");
  if (!capabilities.placements.includes(bindingContext.placement)) {
    return stopWithText(`⚠️ ${channel} bindings are unavailable for this account.`);
  }

  let binding;
@@ -94,11 +140,11 @@ export async function handleSubagentsFocusAction(
      targetSessionKey: focusTarget.targetSessionKey,
      targetKind: focusTarget.targetKind === "acp" ? "session" : "subagent",
      conversation: {
        channel: "discord",
        accountId,
        conversationId,
        channel: bindingContext.channel,
        accountId: bindingContext.accountId,
        conversationId: bindingContext.conversationId,
      },
      placement,
      placement: bindingContext.placement,
      metadata: {
        threadName: resolveThreadBindingThreadName({
          agentId: focusTarget.agentId,
@@ -110,12 +156,14 @@ export async function handleSubagentsFocusAction(
        introText: resolveThreadBindingIntroText({
          agentId: focusTarget.agentId,
          label,
          idleTimeoutMs: resolveDiscordThreadBindingIdleTimeoutMs({
          idleTimeoutMs: resolveThreadBindingIdleTimeoutMsForChannel({
            cfg: params.cfg,
            channel: bindingContext.channel,
            accountId,
          }),
          maxAgeMs: resolveDiscordThreadBindingMaxAgeMs({
          maxAgeMs: resolveThreadBindingMaxAgeMsForChannel({
            cfg: params.cfg,
            channel: bindingContext.channel,
            accountId,
          }),
          sessionCwd: focusTarget.targetKind === "acp" ? resolveAcpSessionCwd(acpMeta) : undefined,
@@ -130,11 +178,14 @@ export async function handleSubagentsFocusAction(
      },
    });
  } catch {
    return stopWithText("⚠️ Failed to bind a Discord thread to the target session.");
    return stopWithText(
      `⚠️ Failed to bind this ${bindingContext.labelNoun} to the target session.`,
    );
  }

  const actionText = currentThreadId
    ? `bound this thread to ${binding.targetSessionKey}`
    : `created thread ${binding.conversation.conversationId} and bound it to ${binding.targetSessionKey}`;
  const actionText =
    bindingContext.placement === "child"
      ? `created thread ${binding.conversation.conversationId} and bound it to ${binding.targetSessionKey}`
      : `bound this ${bindingContext.labelNoun} to ${binding.targetSessionKey}`;
  return stopWithText(`✅ ${actionText} (${focusTarget.targetKind}).`);
}
‎src/auto-reply/reply/commands-subagents/action-unfocus.ts‎
+54-20Lines changed: 54 additions & 20 deletions
Original file line number	Diff line number	Diff line change
@@ -1,42 +1,76 @@
import { getThreadBindingManager } from "../../../discord/monitor/thread-bindings.js";
import { getSessionBindingService } from "../../../infra/outbound/session-binding-service.js";
import type { CommandHandlerResult } from "../commands-types.js";
import {
  type SubagentsCommandContext,
  isDiscordSurface,
  resolveDiscordAccountId,
  isTelegramSurface,
  resolveChannelAccountId,
  resolveCommandSurfaceChannel,
  resolveTelegramConversationId,
  stopWithText,
} from "./shared.js";

export function handleSubagentsUnfocusAction(ctx: SubagentsCommandContext): CommandHandlerResult {
export async function handleSubagentsUnfocusAction(
  ctx: SubagentsCommandContext,
): Promise<CommandHandlerResult> {
  const { params } = ctx;
  if (!isDiscordSurface(params)) {
    return stopWithText("⚠️ /unfocus is only available on Discord.");
  const channel = resolveCommandSurfaceChannel(params);
  if (channel !== "discord" && channel !== "telegram") {
    return stopWithText("⚠️ /unfocus is only available on Discord and Telegram.");
  }

  const threadId = params.ctx.MessageThreadId != null ? String(params.ctx.MessageThreadId) : "";
  if (!threadId.trim()) {
    return stopWithText("⚠️ /unfocus must be run inside a Discord thread.");
  }
  const accountId = resolveChannelAccountId(params);
  const bindingService = getSessionBindingService();
  const conversationId = (() => {
    if (isDiscordSurface(params)) {
      const threadId = params.ctx.MessageThreadId != null ? String(params.ctx.MessageThreadId) : "";
      return threadId.trim() || undefined;
    }
    if (isTelegramSurface(params)) {
      return resolveTelegramConversationId(params);
    }
    return undefined;
  })();

  const threadBindings = getThreadBindingManager(resolveDiscordAccountId(params));
  if (!threadBindings) {
    return stopWithText("⚠️ Discord thread bindings are unavailable for this account.");
  if (!conversationId) {
    if (channel === "discord") {
      return stopWithText("⚠️ /unfocus must be run inside a Discord thread.");
    }
    return stopWithText(
      "⚠️ /unfocus on Telegram requires a topic context in groups, or a direct-message conversation.",
    );
  }

  const binding = threadBindings.getByThreadId(threadId);
  const binding = bindingService.resolveByConversation({
    channel,
    accountId,
    conversationId,
  });
  if (!binding) {
    return stopWithText("ℹ️ This thread is not currently focused.");
    return stopWithText(
      channel === "discord"
        ? "ℹ️ This thread is not currently focused."
        : "ℹ️ This conversation is not currently focused.",
    );
  }

  const senderId = params.command.senderId?.trim() || "";
  if (binding.boundBy && binding.boundBy !== "system" && senderId && senderId !== binding.boundBy) {
    return stopWithText(`⚠️ Only ${binding.boundBy} can unfocus this thread.`);
  const boundBy =
    typeof binding.metadata?.boundBy === "string" ? binding.metadata.boundBy.trim() : "";
  if (boundBy && boundBy !== "system" && senderId && senderId !== boundBy) {
    return stopWithText(
      channel === "discord"
        ? `⚠️ Only ${boundBy} can unfocus this thread.`
        : `⚠️ Only ${boundBy} can unfocus this conversation.`,
    );
  }

  threadBindings.unbindThread({
    threadId,
  await bindingService.unbind({
    bindingId: binding.bindingId,
    reason: "manual",
    sendFarewell: true,
  });
  return stopWithText("✅ Thread unfocused.");
  return stopWithText(
    channel === "discord" ? "✅ Thread unfocused." : "✅ Conversation unfocused.",
  );
}
‎src/auto-reply/reply/commands-subagents/shared.ts‎
+16-2Lines changed: 16 additions & 2 deletions
Original file line number	Diff line number	Diff line change
@@ -21,17 +21,31 @@ import {
  formatTokenUsageDisplay,
  truncateLine,
} from "../../../shared/subagents-format.js";
import {
  isDiscordSurface,
  isTelegramSurface,
  resolveCommandSurfaceChannel,
  resolveDiscordAccountId,
  resolveChannelAccountId,
} from "../channel-context.js";
import type { CommandHandler, CommandHandlerResult } from "../commands-types.js";
import { isDiscordSurface, resolveDiscordAccountId } from "../discord-context.js";
import {
  formatRunLabel,
  formatRunStatus,
  resolveSubagentTargetFromRuns,
  type SubagentTargetResolution,
} from "../subagents-utils.js";
import { resolveTelegramConversationId } from "../telegram-context.js";

export { extractAssistantText, stripToolMessages };
export { isDiscordSurface, resolveDiscordAccountId };
export {
  isDiscordSurface,
  isTelegramSurface,
  resolveCommandSurfaceChannel,
  resolveDiscordAccountId,
  resolveChannelAccountId,
  resolveTelegramConversationId,
};

export const COMMAND = "/subagents";
export const COMMAND_KILL = "/kill";
‎src/auto-reply/reply/telegram-context.test.ts‎
+47Lines changed: 47 additions & 0 deletions
Original file line number	Diff line number	Diff line change
@@ -0,0 +1,47 @@
import { describe, expect, it } from "vitest";
import { resolveTelegramConversationId } from "./telegram-context.js";
describe("resolveTelegramConversationId", () => {
  it("builds canonical topic ids from chat target and message thread id", () => {
    const conversationId = resolveTelegramConversationId({
      ctx: {
        OriginatingTo: "-100200300",
        MessageThreadId: "77",
      },
      command: {},
    });
    expect(conversationId).toBe("-100200300:topic:77");
  });
  it("returns the direct-message chat id when no topic id is present", () => {
    const conversationId = resolveTelegramConversationId({
      ctx: {
        OriginatingTo: "123456",
      },
      command: {},
    });
    expect(conversationId).toBe("123456");
  });
  it("does not treat non-topic groups as globally bindable conversations", () => {
    const conversationId = resolveTelegramConversationId({
      ctx: {
        OriginatingTo: "-100200300",
      },
      command: {},
    });
    expect(conversationId).toBeUndefined();
  });
  it("falls back to command target when originating target is missing", () => {
    const conversationId = resolveTelegramConversationId({
      ctx: {
        To: "123456",
      },
      command: {
        to: "78910",
      },
    });
    expect(conversationId).toBe("78910");
  });
});
‎src/auto-reply/reply/telegram-context.ts‎
+41Lines changed: 41 additions & 0 deletions
Original file line number	Diff line number	Diff line change
@@ -0,0 +1,41 @@
import { parseTelegramTarget } from "../../telegram/targets.js";
type TelegramConversationParams = {
  ctx: {
    MessageThreadId?: string | number | null;
    OriginatingTo?: string;
    To?: string;
  };
  command: {
    to?: string;
  };
};
export function resolveTelegramConversationId(
  params: TelegramConversationParams,
): string | undefined {
  const rawThreadId =
    params.ctx.MessageThreadId != null ? String(params.ctx.MessageThreadId).trim() : "";
  const threadId = rawThreadId || undefined;
  const toCandidates = [
    typeof params.ctx.OriginatingTo === "string" ? params.ctx.OriginatingTo : "",
    typeof params.command.to === "string" ? params.command.to : "",
    typeof params.ctx.To === "string" ? params.ctx.To : "",
  ]
    .map((value) => value.trim())
    .filter(Boolean);
  const chatId = toCandidates
    .map((candidate) => parseTelegramTarget(candidate).chatId.trim())
    .find((candidate) => candidate.length > 0);
  if (!chatId) {
    return undefined;
  }
  if (threadId) {
    return `${chatId}:topic:${threadId}`;
  }
  // Non-topic groups should not become globally focused conversations.
  if (chatId.startsWith("-")) {
    return undefined;
  }
  return chatId;
}
‎src/config/schema.help.ts‎
+10Lines changed: 10 additions & 0 deletions
Original file line number	Diff line number	Diff line change
@@ -1438,6 +1438,16 @@ export const FIELD_HELP: Record<string, string> = {
    "Override Node autoSelectFamily for Telegram (true=enable, false=disable).",
  "channels.telegram.timeoutSeconds":
    "Max seconds before Telegram API requests are aborted (default: 500 per grammY).",
  "channels.telegram.threadBindings.enabled":
    "Enable Telegram conversation binding features (/focus, /unfocus, /agents, and /session idle|max-age). Overrides session.threadBindings.enabled when set.",
  "channels.telegram.threadBindings.idleHours":
    "Inactivity window in hours for Telegram bound sessions. Set 0 to disable idle auto-unfocus (default: 24). Overrides session.threadBindings.idleHours when set.",
  "channels.telegram.threadBindings.maxAgeHours":
    "Optional hard max age in hours for Telegram bound sessions. Set 0 to disable hard cap (default: 0). Overrides session.threadBindings.maxAgeHours when set.",
  "channels.telegram.threadBindings.spawnSubagentSessions":
    "Allow subagent spawns with thread=true to auto-bind Telegram current conversations when supported.",
  "channels.telegram.threadBindings.spawnAcpSessions":
    "Allow ACP spawns with thread=true to auto-bind Telegram current conversations when supported.",
  "channels.whatsapp.dmPolicy":
    'Direct message access control ("pairing" recommended). "open" requires channels.whatsapp.allowFrom=["*"].',
  "channels.whatsapp.selfChatMode": "Same-phone setup (bot uses your personal WhatsApp number).",
‎src/config/schema.labels.ts‎
+5Lines changed: 5 additions & 0 deletions
Original file line number	Diff line number	Diff line change
@@ -695,6 +695,11 @@ export const FIELD_LABELS: Record<string, string> = {
  "channels.telegram.network.autoSelectFamily": "Telegram autoSelectFamily",
  "channels.telegram.timeoutSeconds": "Telegram API Timeout (seconds)",
  "channels.telegram.capabilities.inlineButtons": "Telegram Inline Buttons",
  "channels.telegram.threadBindings.enabled": "Telegram Thread Binding Enabled",
  "channels.telegram.threadBindings.idleHours": "Telegram Thread Binding Idle Timeout (hours)",
  "channels.telegram.threadBindings.maxAgeHours": "Telegram Thread Binding Max Age (hours)",
  "channels.telegram.threadBindings.spawnSubagentSessions": "Telegram Thread-Bound Subagent Spawn",
  "channels.telegram.threadBindings.spawnAcpSessions": "Telegram Thread-Bound ACP Spawn",
  "channels.whatsapp.dmPolicy": "WhatsApp DM Policy",
  "channels.whatsapp.selfChatMode": "WhatsApp Self-Phone Mode",
  "channels.whatsapp.debounceMs": "WhatsApp Message Debounce (ms)",
‎src/config/types.telegram.ts‎
+3Lines changed: 3 additions & 0 deletions
Original file line number	Diff line number	Diff line change
@@ -6,6 +6,7 @@ import type {
  MarkdownConfig,
  OutboundRetryConfig,
  ReplyToMode,
  SessionThreadBindingsConfig,
} from "./types.base.js";
import type { ChannelHeartbeatVisibilityConfig } from "./types.channels.js";
import type { DmConfig, ProviderCommandsConfig } from "./types.messages.js";
@@ -141,6 +142,8 @@ export type TelegramAccountConfig = {
  webhookPort?: number;
  /** Per-action tool gating (default: true for all). */
  actions?: TelegramActionConfig;
  /** Telegram thread/conversation binding overrides. */
  threadBindings?: SessionThreadBindingsConfig;
  /**
   * Controls which user reactions trigger notifications:
   * - "off" (default): ignore all reactions
‎src/config/zod-schema.providers-core.ts‎
+10Lines changed: 10 additions & 0 deletions
Original file line number	Diff line number	Diff line change
@@ -231,6 +231,16 @@ export const TelegramAccountSchemaBase = z
      })
      .strict()
      .optional(),
    threadBindings: z
      .object({
        enabled: z.boolean().optional(),
        idleHours: z.number().nonnegative().optional(),
        maxAgeHours: z.number().nonnegative().optional(),
        spawnSubagentSessions: z.boolean().optional(),
        spawnAcpSessions: z.boolean().optional(),
      })
      .strict()
      .optional(),
    reactionNotifications: z.enum(["off", "own", "all"]).optional(),
    reactionLevel: z.enum(["off", "ack", "minimal", "extensive"]).optional(),
    heartbeat: ChannelHeartbeatVisibilitySchema,
‎src/telegram/bot-message-context.thread-binding.test.ts‎
+116Lines changed: 116 additions & 0 deletions
Original file line number	Diff line number	Diff line change
@@ -0,0 +1,116 @@
import { beforeEach, describe, expect, it, vi } from "vitest";
const hoisted = vi.hoisted(() => {
  const resolveByConversationMock = vi.fn();
  const touchMock = vi.fn();
  return {
    resolveByConversationMock,
    touchMock,
  };
});
vi.mock("../infra/outbound/session-binding-service.js", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("../infra/outbound/session-binding-service.js")>();
  return {
    ...actual,
    getSessionBindingService: () => ({
      bind: vi.fn(),
      getCapabilities: vi.fn(),
      listBySession: vi.fn(),
      resolveByConversation: (ref: unknown) => hoisted.resolveByConversationMock(ref),
      touch: (bindingId: string, at?: number) => hoisted.touchMock(bindingId, at),
      unbind: vi.fn(),
    }),
  };
});
const { buildTelegramMessageContextForTest } =
  await import("./bot-message-context.test-harness.js");
describe("buildTelegramMessageContext bound conversation override", () => {
  beforeEach(() => {
    hoisted.resolveByConversationMock.mockReset().mockReturnValue(null);
    hoisted.touchMock.mockReset();
  });
  it("routes forum topic messages to the bound session", async () => {
    hoisted.resolveByConversationMock.mockReturnValue({
      bindingId: "default:-100200300:topic:77",
      targetSessionKey: "agent:codex-acp:session-1",
    });
    const ctx = await buildTelegramMessageContextForTest({
      message: {
        message_id: 1,
        chat: { id: -100200300, type: "supergroup", is_forum: true },
        message_thread_id: 77,
        date: 1_700_000_000,
        text: "hello",
        from: { id: 42, first_name: "Alice" },
      },
      options: { forceWasMentioned: true },
      resolveGroupActivation: () => true,
    });
    expect(hoisted.resolveByConversationMock).toHaveBeenCalledWith({
      channel: "telegram",
      accountId: "default",
      conversationId: "-100200300:topic:77",
    });
    expect(ctx?.ctxPayload?.SessionKey).toBe("agent:codex-acp:session-1");
    expect(hoisted.touchMock).toHaveBeenCalledWith("default:-100200300:topic:77", undefined);
  });
  it("treats named-account bound conversations as explicit route matches", async () => {
    hoisted.resolveByConversationMock.mockReturnValue({
      bindingId: "work:-100200300:topic:77",
      targetSessionKey: "agent:codex-acp:session-2",
    });
    const ctx = await buildTelegramMessageContextForTest({
      accountId: "work",
      message: {
        message_id: 1,
        chat: { id: -100200300, type: "supergroup", is_forum: true },
        message_thread_id: 77,
        date: 1_700_000_000,
        text: "hello",
        from: { id: 42, first_name: "Alice" },
      },
      options: { forceWasMentioned: true },
      resolveGroupActivation: () => true,
    });
    expect(ctx).not.toBeNull();
    expect(ctx?.route.accountId).toBe("work");
    expect(ctx?.route.matchedBy).toBe("binding.channel");
    expect(ctx?.ctxPayload?.SessionKey).toBe("agent:codex-acp:session-2");
    expect(hoisted.touchMock).toHaveBeenCalledWith("work:-100200300:topic:77", undefined);
  });
  it("routes dm messages to the bound session", async () => {
    hoisted.resolveByConversationMock.mockReturnValue({
      bindingId: "default:1234",
      targetSessionKey: "agent:codex-acp:session-dm",
    });
    const ctx = await buildTelegramMessageContextForTest({
      message: {
        message_id: 1,
        chat: { id: 1234, type: "private" },
        date: 1_700_000_000,
        text: "hello",
        from: { id: 42, first_name: "Alice" },
      },
    });
    expect(hoisted.resolveByConversationMock).toHaveBeenCalledWith({
      channel: "telegram",
      accountId: "default",
      conversationId: "1234",
    });
    expect(ctx?.ctxPayload?.SessionKey).toBe("agent:codex-acp:session-dm");
    expect(hoisted.touchMock).toHaveBeenCalledWith("default:1234", undefined);
  });
});
‎src/telegram/bot-message-context.ts‎
+32-2Lines changed: 32 additions & 2 deletions
Original file line number	Diff line number	Diff line change
@@ -42,6 +42,7 @@ import type {
} from "../config/types.js";
import { logVerbose, shouldLogVerbose } from "../globals.js";
import { recordChannelActivity } from "../infra/channel-activity.js";
import { getSessionBindingService } from "../infra/outbound/session-binding-service.js";
import {
  buildAgentSessionKey,
  pickFirstExistingAgentId,
@@ -51,6 +52,7 @@ import {
import {
  DEFAULT_ACCOUNT_ID,
  buildAgentMainSessionKey,
  resolveAgentIdFromSessionKey,
  resolveThreadSessionKeys,
} from "../routing/session-key.js";
import { resolvePinnedMainDmOwnerFromAllowlist } from "../security/dm-policy-shared.js";
@@ -257,9 +259,37 @@ export const buildTelegramMessageContext = async ({
    conversationId: peerId,
    parentConversationId: isGroup ? String(chatId) : undefined,
  });
  const configuredBinding = configuredRoute.configuredBinding;
  const configuredBindingSessionKey = configuredRoute.boundSessionKey ?? "";
  let configuredBinding = configuredRoute.configuredBinding;
  let configuredBindingSessionKey = configuredRoute.boundSessionKey ?? "";
  route = configuredRoute.route;
  const threadBindingConversationId =
    replyThreadId != null
      ? `${chatId}:topic:${replyThreadId}`
      : !isGroup
        ? String(chatId)
        : undefined;
  if (threadBindingConversationId) {
    const threadBinding = getSessionBindingService().resolveByConversation({
      channel: "telegram",
      accountId: account.accountId,
      conversationId: threadBindingConversationId,
    });
    const boundSessionKey = threadBinding?.targetSessionKey?.trim();
    if (threadBinding && boundSessionKey) {
      route = {
        ...route,
        sessionKey: boundSessionKey,
        agentId: resolveAgentIdFromSessionKey(boundSessionKey),
        matchedBy: "binding.channel",
      };
      configuredBinding = null;
      configuredBindingSessionKey = "";
      getSessionBindingService().touch(threadBinding.bindingId);
      logVerbose(
        `telegram: routed via bound conversation ${threadBindingConversationId} -> ${boundSessionKey}`,
      );
    }
  }
  const requiresExplicitAccountBinding = (candidate: ResolvedAgentRoute): boolean =>
    candidate.accountId !== DEFAULT_ACCOUNT_ID && candidate.matchedBy === "default";
  // Fail closed for named Telegram accounts when route resolution falls back to
‎src/telegram/bot.ts‎
+33Lines changed: 33 additions & 0 deletions
Original file line number	Diff line number	Diff line change
@@ -5,6 +5,11 @@ import { Bot } from "grammy";
import { resolveDefaultAgentId } from "../agents/agent-scope.js";
import { resolveTextChunkLimit } from "../auto-reply/chunk.js";
import { DEFAULT_GROUP_HISTORY_LIMIT, type HistoryEntry } from "../auto-reply/reply/history.js";
import {
  resolveThreadBindingIdleTimeoutMsForChannel,
  resolveThreadBindingMaxAgeMsForChannel,
  resolveThreadBindingSpawnPolicy,
} from "../channels/thread-bindings-policy.js";
import {
  isNativeCommandsExplicitlyDisabled,
  resolveNativeCommandsEnabled,
@@ -36,6 +41,7 @@ import { buildTelegramGroupPeerId, resolveTelegramStreamMode } from "./bot/helpe
import { resolveTelegramFetch } from "./fetch.js";
import { createTelegramSendChatActionHandler } from "./sendchataction-401-backoff.js";
import { getTelegramSequentialKey } from "./sequential-key.js";
import { createTelegramThreadBindingManager } from "./thread-bindings.js";

export type TelegramBotOptions = {
  token: string;
@@ -67,6 +73,27 @@ export function createTelegramBot(opts: TelegramBotOptions) {
    cfg,
    accountId: opts.accountId,
  });
  const threadBindingPolicy = resolveThreadBindingSpawnPolicy({
    cfg,
    channel: "telegram",
    accountId: account.accountId,
    kind: "subagent",
  });
  const threadBindingManager = threadBindingPolicy.enabled
    ? createTelegramThreadBindingManager({
        accountId: account.accountId,
        idleTimeoutMs: resolveThreadBindingIdleTimeoutMsForChannel({
          cfg,
          channel: "telegram",
          accountId: account.accountId,
        }),
        maxAgeMs: resolveThreadBindingMaxAgeMsForChannel({
          cfg,
          channel: "telegram",
          accountId: account.accountId,
        }),
      })
    : null;
  const telegramCfg = account.config;

  const fetchImpl = resolveTelegramFetch(opts.proxyFetch, {
@@ -379,5 +406,11 @@ export function createTelegramBot(opts: TelegramBotOptions) {
    logger,
  });

  const originalStop = bot.stop.bind(bot);
  bot.stop = ((...args: Parameters<typeof originalStop>) => {
    threadBindingManager?.stop();
    return originalStop(...args);
  }) as typeof bot.stop;
  return bot;
}
‎src/telegram/bot/delivery.replies.ts‎
+101-26Lines changed: 101 additions & 26 deletions
Original file line number	Diff line number	Diff line change
@@ -36,6 +36,11 @@ type DeliveryProgress = {
  deliveredCount: number;
};

type TelegramReplyChannelData = {
  buttons?: TelegramInlineButtons;
  pin?: boolean;
};
type ChunkTextFn = (markdown: string) => ReturnType<typeof markdownToTelegramChunks>;

function buildChunkTextResolver(params: {
@@ -102,7 +107,8 @@ async function deliverTextReply(params: {
  replyToId?: number;
  replyToMode: ReplyToMode;
  progress: DeliveryProgress;
}): Promise<void> {
}): Promise<number | undefined> {
  let firstDeliveredMessageId: number | undefined;
  const chunks = params.chunkText(params.replyText);
  for (let i = 0; i < chunks.length; i += 1) {
    const chunk = chunks[i];
@@ -115,18 +121,28 @@ async function deliverTextReply(params: {
      replyToMode: params.replyToMode,
      progress: params.progress,
    });
    await sendTelegramText(params.bot, params.chatId, chunk.html, params.runtime, {
      replyToMessageId: replyToForChunk,
      replyQuoteText: params.replyQuoteText,
      thread: params.thread,
      textMode: "html",
      plainText: chunk.text,
      linkPreview: params.linkPreview,
      replyMarkup: shouldAttachButtons ? params.replyMarkup : undefined,
    });
    const messageId = await sendTelegramText(
      params.bot,
      params.chatId,
      chunk.html,
      params.runtime,
      {
        replyToMessageId: replyToForChunk,
        replyQuoteText: params.replyQuoteText,
        thread: params.thread,
        textMode: "html",
        plainText: chunk.text,
        linkPreview: params.linkPreview,
        replyMarkup: shouldAttachButtons ? params.replyMarkup : undefined,
      },
    );
    if (firstDeliveredMessageId == null) {
      firstDeliveredMessageId = messageId;
    }
    markReplyApplied(params.progress, replyToForChunk);
    markDelivered(params.progress);
  }
  return firstDeliveredMessageId;
}

async function sendPendingFollowUpText(params: {
@@ -188,14 +204,15 @@ async function sendTelegramVoiceFallbackText(opts: {
  linkPreview?: boolean;
  replyMarkup?: ReturnType<typeof buildInlineKeyboard>;
  replyQuoteText?: string;
}): Promise<void> {
}): Promise<number | undefined> {
  let firstDeliveredMessageId: number | undefined;
  const chunks = opts.chunkText(opts.text);
  let appliedReplyTo = false;
  for (let i = 0; i < chunks.length; i += 1) {
    const chunk = chunks[i];
    // Only apply reply reference, quote text, and buttons to the first chunk.
    const replyToForChunk = !appliedReplyTo ? opts.replyToId : undefined;
    await sendTelegramText(opts.bot, opts.chatId, chunk.html, opts.runtime, {
    const messageId = await sendTelegramText(opts.bot, opts.chatId, chunk.html, opts.runtime, {
      replyToMessageId: replyToForChunk,
      replyQuoteText: !appliedReplyTo ? opts.replyQuoteText : undefined,
      thread: opts.thread,
@@ -204,10 +221,14 @@ async function sendTelegramVoiceFallbackText(opts: {
      linkPreview: opts.linkPreview,
      replyMarkup: !appliedReplyTo ? opts.replyMarkup : undefined,
    });
    if (firstDeliveredMessageId == null) {
      firstDeliveredMessageId = messageId;
    }
    if (replyToForChunk) {
      appliedReplyTo = true;
    }
  }
  return firstDeliveredMessageId;
}

async function deliverMediaReply(params: {
@@ -227,7 +248,8 @@ async function deliverMediaReply(params: {
  replyToId?: number;
  replyToMode: ReplyToMode;
  progress: DeliveryProgress;
}): Promise<void> {
}): Promise<number | undefined> {
  let firstDeliveredMessageId: number | undefined;
  let first = true;
  let pendingFollowUpText: string | undefined;
  for (const mediaUrl of params.mediaList) {
@@ -269,34 +291,43 @@ async function deliverMediaReply(params: {
      }),
    };
    if (isGif) {
      await sendTelegramWithThreadFallback({
      const result = await sendTelegramWithThreadFallback({
        operation: "sendAnimation",
        runtime: params.runtime,
        thread: params.thread,
        requestParams: mediaParams,
        send: (effectiveParams) =>
          params.bot.api.sendAnimation(params.chatId, file, { ...effectiveParams }),
      });
      if (firstDeliveredMessageId == null) {
        firstDeliveredMessageId = result.message_id;
      }
      markDelivered(params.progress);
    } else if (kind === "image") {
      await sendTelegramWithThreadFallback({
      const result = await sendTelegramWithThreadFallback({
        operation: "sendPhoto",
        runtime: params.runtime,
        thread: params.thread,
        requestParams: mediaParams,
        send: (effectiveParams) =>
          params.bot.api.sendPhoto(params.chatId, file, { ...effectiveParams }),
      });
      if (firstDeliveredMessageId == null) {
        firstDeliveredMessageId = result.message_id;
      }
      markDelivered(params.progress);
    } else if (kind === "video") {
      await sendTelegramWithThreadFallback({
      const result = await sendTelegramWithThreadFallback({
        operation: "sendVideo",
        runtime: params.runtime,
        thread: params.thread,
        requestParams: mediaParams,
        send: (effectiveParams) =>
          params.bot.api.sendVideo(params.chatId, file, { ...effectiveParams }),
      });
      if (firstDeliveredMessageId == null) {
        firstDeliveredMessageId = result.message_id;
      }
      markDelivered(params.progress);
    } else if (kind === "audio") {
      const { useVoice } = resolveTelegramVoiceSend({
@@ -308,7 +339,7 @@ async function deliverMediaReply(params: {
      if (useVoice) {
        await params.onVoiceRecording?.();
        try {
          await sendTelegramWithThreadFallback({
          const result = await sendTelegramWithThreadFallback({
            operation: "sendVoice",
            runtime: params.runtime,
            thread: params.thread,
@@ -317,6 +348,9 @@ async function deliverMediaReply(params: {
            send: (effectiveParams) =>
              params.bot.api.sendVoice(params.chatId, file, { ...effectiveParams }),
          });
          if (firstDeliveredMessageId == null) {
            firstDeliveredMessageId = result.message_id;
          }
          markDelivered(params.progress);
        } catch (voiceErr) {
          if (isVoiceMessagesForbidden(voiceErr)) {
@@ -332,7 +366,7 @@ async function deliverMediaReply(params: {
              replyToMode: params.replyToMode,
              progress: params.progress,
            });
            await sendTelegramVoiceFallbackText({
            const fallbackMessageId = await sendTelegramVoiceFallbackText({
              bot: params.bot,
              chatId: params.chatId,
              runtime: params.runtime,
@@ -344,6 +378,9 @@ async function deliverMediaReply(params: {
              replyMarkup: params.replyMarkup,
              replyQuoteText: params.replyQuoteText,
            });
            if (firstDeliveredMessageId == null) {
              firstDeliveredMessageId = fallbackMessageId;
            }
            markReplyApplied(params.progress, voiceFallbackReplyTo);
            markDelivered(params.progress);
            continue;
@@ -355,14 +392,17 @@ async function deliverMediaReply(params: {
            const noCaptionParams = { ...mediaParams };
            delete noCaptionParams.caption;
            delete noCaptionParams.parse_mode;
            await sendTelegramWithThreadFallback({
            const result = await sendTelegramWithThreadFallback({
              operation: "sendVoice",
              runtime: params.runtime,
              thread: params.thread,
              requestParams: noCaptionParams,
              send: (effectiveParams) =>
                params.bot.api.sendVoice(params.chatId, file, { ...effectiveParams }),
            });
            if (firstDeliveredMessageId == null) {
              firstDeliveredMessageId = result.message_id;
            }
            markDelivered(params.progress);
            const fallbackText = params.reply.text;
            if (fallbackText?.trim()) {
@@ -384,25 +424,31 @@ async function deliverMediaReply(params: {
          throw voiceErr;
        }
      } else {
        await sendTelegramWithThreadFallback({
        const result = await sendTelegramWithThreadFallback({
          operation: "sendAudio",
          runtime: params.runtime,
          thread: params.thread,
          requestParams: mediaParams,
          send: (effectiveParams) =>
            params.bot.api.sendAudio(params.chatId, file, { ...effectiveParams }),
        });
        if (firstDeliveredMessageId == null) {
          firstDeliveredMessageId = result.message_id;
        }
        markDelivered(params.progress);
      }
    } else {
      await sendTelegramWithThreadFallback({
      const result = await sendTelegramWithThreadFallback({
        operation: "sendDocument",
        runtime: params.runtime,
        thread: params.thread,
        requestParams: mediaParams,
        send: (effectiveParams) =>
          params.bot.api.sendDocument(params.chatId, file, { ...effectiveParams }),
      });
      if (firstDeliveredMessageId == null) {
        firstDeliveredMessageId = result.message_id;
      }
      markDelivered(params.progress);
    }
    markReplyApplied(params.progress, replyToMessageId);
@@ -423,6 +469,28 @@ async function deliverMediaReply(params: {
      pendingFollowUpText = undefined;
    }
  }
  return firstDeliveredMessageId;
}
async function maybePinFirstDeliveredMessage(params: {
  shouldPin: boolean;
  bot: Bot;
  chatId: string;
  runtime: RuntimeEnv;
  firstDeliveredMessageId?: number;
}): Promise<void> {
  if (!params.shouldPin || typeof params.firstDeliveredMessageId !== "number") {
    return;
  }
  try {
    await params.bot.api.pinChatMessage(params.chatId, params.firstDeliveredMessageId, {
      disable_notification: true,
    });
  } catch (err) {
    logVerbose(
      `telegram pinChatMessage failed chat=${params.chatId} message=${params.firstDeliveredMessageId}: ${formatErrorMessage(err)}`,
    );
  }
}

export async function deliverReplies(params: {
@@ -507,12 +575,12 @@ export async function deliverReplies(params: {
      const deliveredCountBeforeReply = progress.deliveredCount;
      const replyToId =
        params.replyToMode === "off" ? undefined : resolveTelegramReplyId(reply.replyToId);
      const telegramData = reply.channelData?.telegram as
        | { buttons?: TelegramInlineButtons }
        | undefined;
      const telegramData = reply.channelData?.telegram as TelegramReplyChannelData | undefined;
      const shouldPinFirstMessage = telegramData?.pin === true;
      const replyMarkup = buildInlineKeyboard(telegramData?.buttons);
      let firstDeliveredMessageId: number | undefined;
      if (mediaList.length === 0) {
        await deliverTextReply({
        firstDeliveredMessageId = await deliverTextReply({
          bot: params.bot,
          chatId: params.chatId,
          runtime: params.runtime,
@@ -527,7 +595,7 @@ export async function deliverReplies(params: {
          progress,
        });
      } else {
        await deliverMediaReply({
        firstDeliveredMessageId = await deliverMediaReply({
          reply,
          mediaList,
          bot: params.bot,
@@ -546,6 +614,13 @@ export async function deliverReplies(params: {
          progress,
        });
      }
      await maybePinFirstDeliveredMessage({
        shouldPin: shouldPinFirstMessage,
        bot: params.bot,
        chatId: params.chatId,
        runtime: params.runtime,
        firstDeliveredMessageId,
      });

      if (hasMessageSentHooks) {
        const deliveredThisReply = progress.deliveredCount > deliveredCountBeforeReply;
‎src/telegram/bot/delivery.test.ts‎
+39Lines changed: 39 additions & 0 deletions
Original file line number	Diff line number	Diff line change
@@ -708,6 +708,45 @@ describe("deliverReplies", () => {
    expect(sendPhoto.mock.calls[1][2]).not.toHaveProperty("reply_to_message_id");
  });

  it("pins the first delivered text message when telegram pin is requested", async () => {
    const runtime = createRuntime();
    const sendMessage = vi
      .fn()
      .mockResolvedValueOnce({ message_id: 101, chat: { id: "123" } })
      .mockResolvedValueOnce({ message_id: 102, chat: { id: "123" } });
    const pinChatMessage = vi.fn().mockResolvedValue(true);
    const bot = createBot({ sendMessage, pinChatMessage });
    await deliverReplies({
      replies: [{ text: "chunk-one\n\nchunk-two", channelData: { telegram: { pin: true } } }],
      chatId: "123",
      token: "tok",
      runtime,
      bot,
      replyToMode: "off",
      textLimit: 12,
    });
    expect(pinChatMessage).toHaveBeenCalledTimes(1);
    expect(pinChatMessage).toHaveBeenCalledWith("123", 101, { disable_notification: true });
  });
  it("continues when pinning fails", async () => {
    const runtime = createRuntime();
    const sendMessage = vi.fn().mockResolvedValue({ message_id: 201, chat: { id: "123" } });
    const pinChatMessage = vi.fn().mockRejectedValue(new Error("pin failed"));
    const bot = createBot({ sendMessage, pinChatMessage });
    await deliverWith({
      replies: [{ text: "hello", channelData: { telegram: { pin: true } } }],
      runtime,
      bot,
    });
    expect(sendMessage).toHaveBeenCalledTimes(1);
    expect(pinChatMessage).toHaveBeenCalledTimes(1);
  });
  it("rethrows VOICE_MESSAGES_FORBIDDEN when no text fallback is available", async () => {
    const { runtime, sendVoice, sendMessage, bot } = createVoiceFailureHarness({
      voiceError: createVoiceMessagesForbiddenError(),
‎src/telegram/thread-bindings.test.ts‎
+166Lines changed: 166 additions & 0 deletions
Original file line number	Diff line number	Diff line change
@@ -0,0 +1,166 @@
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resolveStateDir } from "../config/paths.js";
import { getSessionBindingService } from "../infra/outbound/session-binding-service.js";
import {
  __testing,
  createTelegramThreadBindingManager,
  setTelegramThreadBindingIdleTimeoutBySessionKey,
  setTelegramThreadBindingMaxAgeBySessionKey,
} from "./thread-bindings.js";
describe("telegram thread bindings", () => {
  let stateDirOverride: string | undefined;
  beforeEach(() => {
    __testing.resetTelegramThreadBindingsForTests();
  });
  afterEach(() => {
    vi.useRealTimers();
    if (stateDirOverride) {
      delete process.env.OPENCLAW_STATE_DIR;
      fs.rmSync(stateDirOverride, { recursive: true, force: true });
      stateDirOverride = undefined;
    }
  });
  it("registers a telegram binding adapter and binds current conversations", async () => {
    const manager = createTelegramThreadBindingManager({
      accountId: "work",
      persist: false,
      enableSweeper: false,
      idleTimeoutMs: 30_000,
      maxAgeMs: 0,
    });
    const bound = await getSessionBindingService().bind({
      targetSessionKey: "agent:main:subagent:child-1",
      targetKind: "subagent",
      conversation: {
        channel: "telegram",
        accountId: "work",
        conversationId: "-100200300:topic:77",
      },
      placement: "current",
      metadata: {
        boundBy: "user-1",
      },
    });
    expect(bound.conversation.channel).toBe("telegram");
    expect(bound.conversation.accountId).toBe("work");
    expect(bound.conversation.conversationId).toBe("-100200300:topic:77");
    expect(bound.targetSessionKey).toBe("agent:main:subagent:child-1");
    expect(manager.getByConversationId("-100200300:topic:77")?.boundBy).toBe("user-1");
  });
  it("does not support child placement", async () => {
    createTelegramThreadBindingManager({
      accountId: "default",
      persist: false,
      enableSweeper: false,
    });
    await expect(
      getSessionBindingService().bind({
        targetSessionKey: "agent:main:subagent:child-1",
        targetKind: "subagent",
        conversation: {
          channel: "telegram",
          accountId: "default",
          conversationId: "-100200300:topic:77",
        },
        placement: "child",
      }),
    ).rejects.toMatchObject({
      code: "BINDING_CAPABILITY_UNSUPPORTED",
    });
  });
  it("updates lifecycle windows by session key", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-03-06T10:00:00.000Z"));
    const manager = createTelegramThreadBindingManager({
      accountId: "work",
      persist: false,
      enableSweeper: false,
    });
    await getSessionBindingService().bind({
      targetSessionKey: "agent:main:subagent:child-1",
      targetKind: "subagent",
      conversation: {
        channel: "telegram",
        accountId: "work",
        conversationId: "1234",
      },
    });
    const original = manager.listBySessionKey("agent:main:subagent:child-1")[0];
    expect(original).toBeDefined();
    const idleUpdated = setTelegramThreadBindingIdleTimeoutBySessionKey({
      accountId: "work",
      targetSessionKey: "agent:main:subagent:child-1",
      idleTimeoutMs: 2 * 60 * 60 * 1000,
    });
    vi.setSystemTime(new Date("2026-03-06T12:00:00.000Z"));
    const maxAgeUpdated = setTelegramThreadBindingMaxAgeBySessionKey({
      accountId: "work",
      targetSessionKey: "agent:main:subagent:child-1",
      maxAgeMs: 6 * 60 * 60 * 1000,
    });
    expect(idleUpdated).toHaveLength(1);
    expect(idleUpdated[0]?.idleTimeoutMs).toBe(2 * 60 * 60 * 1000);
    expect(maxAgeUpdated).toHaveLength(1);
    expect(maxAgeUpdated[0]?.maxAgeMs).toBe(6 * 60 * 60 * 1000);
    expect(maxAgeUpdated[0]?.boundAt).toBe(original?.boundAt);
    expect(maxAgeUpdated[0]?.lastActivityAt).toBe(Date.parse("2026-03-06T12:00:00.000Z"));
    expect(manager.listBySessionKey("agent:main:subagent:child-1")[0]?.maxAgeMs).toBe(
      6 * 60 * 60 * 1000,
    );
  });
  it("does not persist lifecycle updates when manager persistence is disabled", async () => {
    stateDirOverride = fs.mkdtempSync(path.join(os.tmpdir(), "openclaw-telegram-bindings-"));
    process.env.OPENCLAW_STATE_DIR = stateDirOverride;
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-03-06T10:00:00.000Z"));
    createTelegramThreadBindingManager({
      accountId: "no-persist",
      persist: false,
      enableSweeper: false,
    });
    await getSessionBindingService().bind({
      targetSessionKey: "agent:main:subagent:child-2",
      targetKind: "subagent",
      conversation: {
        channel: "telegram",
        accountId: "no-persist",
        conversationId: "-100200300:topic:88",
      },
    });
    setTelegramThreadBindingIdleTimeoutBySessionKey({
      accountId: "no-persist",
      targetSessionKey: "agent:main:subagent:child-2",
      idleTimeoutMs: 60 * 60 * 1000,
    });
    setTelegramThreadBindingMaxAgeBySessionKey({
      accountId: "no-persist",
      targetSessionKey: "agent:main:subagent:child-2",
      maxAgeMs: 2 * 60 * 60 * 1000,
    });
    const statePath = path.join(
      resolveStateDir(process.env, os.homedir),
      "telegram",
      "thread-bindings-no-persist.json",
    );
    expect(fs.existsSync(statePath)).toBe(false);
  });
});
