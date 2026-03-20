# Context Window Tracking System

## Overview

A context window tracking system monitors the token usage of LLM conversations to prevent exceeding model limits. This document provides a universal specification for implementing such a system in any AI chat application.

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Data Structures](#data-structures)
3. [Token Collection Flow](#token-collection-flow)
4. [Real-time Synchronization](#real-time-synchronization)
5. [UI Components](#ui-components)
6. [API Specification](#api-specification)
7. [Implementation Examples](#implementation-examples)
8. [Best Practices](#best-practices)

---

## Architecture Overview

The system operates on five layers:

```
┌─────────────────────────────────────────────────────────────┐
│ Layer 5: UI                  │  Progress indicators, stats   │
├─────────────────────────────────────────────────────────────┤
│ Layer 4: Synchronization     │  WebSocket/SSE, Event Bus     │
├─────────────────────────────────────────────────────────────┤
│ Layer 3: Backend Events      │  Event publishing system      │
├─────────────────────────────────────────────────────────────┤
│ Layer 2: Token Processing    │  Normalization, aggregation   │
├─────────────────────────────────────────────────────────────┤
│ Layer 1: LLM Response        │  Provider API usage data      │
└─────────────────────────────────────────────────────────────┘
```

### Layer Responsibilities

**Layer 1 - LLM Response**

- Receives raw usage data from LLM providers
- Handles provider-specific formats (OpenAI, Anthropic, Google, etc.)

**Layer 2 - Token Processing**

- Normalizes token data to unified format
- Calculates totals and costs
- Handles provider quirks (cache token inclusion/exclusion)

**Layer 3 - Backend Events**

- Publishes updates when messages complete
- Maintains event history for new connections

**Layer 4 - Synchronization**

- Delivers real-time updates to connected clients
- Handles reconnection and state synchronization

**Layer 5 - UI**

- Displays current context usage
- Visual indicators for approaching limits
- Detailed statistics breakdown

---

## Data Structures

### Token Structure

```typescript
interface TokenUsage {
  /** Total tokens (optional, can be calculated) */
  total?: number

  /** Input/prompt tokens */
  input: number

  /** Output/completion tokens */
  output: number

  /** Reasoning tokens (for thinking models like o1) */
  reasoning: number

  /** Cache tokens */
  cache: {
    /** Tokens read from cache */
    read: number
    /** Tokens written to cache */
    write: number
  }
}

/** Example usage */
const usage: TokenUsage = {
  input: 1500,
  output: 800,
  reasoning: 200,
  cache: {
    read: 500,
    write: 0,
  },
}

// Calculate total
total = input + output + reasoning + cache.read + cache.write
// = 1500 + 800 + 200 + 500 + 0 = 3000
```

### Message Structure

```typescript
interface AssistantMessage {
  id: string
  sessionId: string
  role: "assistant"
  providerId: string
  modelId: string
  content: string

  /** Token usage for this message */
  tokens: TokenUsage

  /** Cost in USD (calculated from model pricing) */
  cost: number

  /** When the message was created */
  createdAt: Date

  /** Reason for finishing (stop, length, tool_calls, etc.) */
  finishReason?: string
}

interface UserMessage {
  id: string
  sessionId: string
  role: "user"
  content: string
  createdAt: Date
}

type Message = UserMessage | AssistantMessage
```

### Model Limits

```typescript
interface ModelLimits {
  /** Total context window size */
  context: number

  /** Maximum input tokens (if different from context) */
  input?: number

  /** Maximum output tokens */
  output: number
}

interface Model {
  id: string
  name: string
  providerId: string
  limits: ModelLimits

  /** Pricing per 1M tokens */
  pricing: {
    input: number
    output: number
    cacheRead?: number
    cacheWrite?: number
  }
}
```

### Context Metrics

```typescript
interface ContextMetrics {
  /** Total tokens across all messages */
  totalTokens: number

  /** Context window limit */
  limit: number

  /** Percentage used (0-100+) */
  usage: number

  /** Breakdown by category */
  breakdown: {
    system: number
    user: number
    assistant: number
    tool: number
  }

  /** Total cost of the session */
  totalCost: number

  /** Reserved buffer for compaction */
  reserved: number

  /** Available tokens before limit */
  available: number
}
```

---

## Token Collection Flow

### 1. Receiving Usage from Provider

Different providers return usage in different formats:

**OpenAI Format:**

```json
{
  "usage": {
    "prompt_tokens": 1500,
    "completion_tokens": 800,
    "total_tokens": 2300
  }
}
```

**Anthropic Format:**

```json
{
  "usage": {
    "input_tokens": 1500,
    "output_tokens": 800,
    "cache_creation_input_tokens": 200,
    "cache_read_input_tokens": 500
  }
}
```

**Google Format:**

```json
{
  "usageMetadata": {
    "promptTokenCount": 1500,
    "candidatesTokenCount": 800,
    "totalTokenCount": 2300
  }
}
```

### 2. Normalization Function

```typescript
interface RawUsage {
  inputTokens: number
  outputTokens: number
  reasoningTokens?: number
  cachedInputTokens?: number
}

interface ProviderMetadata {
  provider: string
  cacheCreationInputTokens?: number
  // Provider-specific fields
}

function normalizeTokenUsage(raw: RawUsage, metadata: ProviderMetadata, model: Model): TokenUsage {
  const inputTokens = raw.inputTokens || 0
  const outputTokens = raw.outputTokens || 0
  const reasoningTokens = raw.reasoningTokens || 0

  // Handle cache tokens based on provider
  const cacheReadTokens = raw.cachedInputTokens || 0
  const cacheWriteTokens = metadata.cacheCreationInputTokens || 0

  // Anthropic includes cache tokens in input, others don't
  const excludesCachedTokens = metadata.provider === "anthropic"

  const adjustedInputTokens = excludesCachedTokens ? inputTokens : inputTokens - cacheReadTokens - cacheWriteTokens

  // Calculate total
  const total = adjustedInputTokens + outputTokens + reasoningTokens + cacheReadTokens + cacheWriteTokens

  return {
    total,
    input: adjustedInputTokens,
    output: outputTokens,
    reasoning: reasoningTokens,
    cache: {
      read: cacheReadTokens,
      write: cacheWriteTokens,
    },
  }
}
```

### 3. Calculating Cost

```typescript
function calculateCost(usage: TokenUsage, model: Model): number {
  const inputCost = (usage.input / 1_000_000) * model.pricing.input
  const outputCost = (usage.output / 1_000_000) * model.pricing.output
  const cacheReadCost = (usage.cache.read / 1_000_000) * (model.pricing.cacheRead || model.pricing.input * 0.1)
  const cacheWriteCost = (usage.cache.write / 1_000_000) * (model.pricing.cacheWrite || model.pricing.input * 1.25)

  return inputCost + outputCost + cacheReadCost + cacheWriteCost
}
```

### 4. Complete Processing Flow

```typescript
async function processLLMResponse(response: LLMResponse, session: Session, model: Model): Promise<AssistantMessage> {
  // Step 1: Extract raw usage
  const rawUsage = extractUsage(response)

  // Step 2: Normalize tokens
  const tokens = normalizeTokenUsage(rawUsage, response.metadata, model)

  // Step 3: Calculate cost
  const cost = calculateCost(tokens, model)

  // Step 4: Create message
  const message: AssistantMessage = {
    id: generateId(),
    sessionId: session.id,
    role: "assistant",
    providerId: model.providerId,
    modelId: model.id,
    content: response.content,
    tokens,
    cost,
    createdAt: new Date(),
    finishReason: response.finishReason,
  }

  // Step 5: Save to database
  await saveMessage(message)

  // Step 6: Publish event
  await publishEvent({
    type: "message.created",
    payload: { message },
  })

  return message
}
```

---

## Real-time Synchronization

### Event Types

```typescript
type EventType = "message.created" | "message.updated" | "message.part.delta" | "session.updated"

interface Event {
  type: EventType
  timestamp: Date
  sessionId: string
  payload: unknown
}

interface MessageCreatedEvent extends Event {
  type: "message.created"
  payload: {
    message: Message
  }
}

interface MessageUpdatedEvent extends Event {
  type: "message.updated"
  payload: {
    messageId: string
    updates: Partial<Message>
  }
}
```

### Server-Sent Events (SSE) Implementation

```typescript
// Server-side (Node.js/Express example)
import { EventEmitter } from "events"

const eventBus = new EventEmitter()

// Publish function
function publishEvent(event: Event): void {
  eventBus.emit(event.sessionId, event)
}

// SSE endpoint
app.get("/api/sessions/:sessionId/events", (req, res) => {
  const { sessionId } = req.params

  // Set headers for SSE
  res.setHeader("Content-Type", "text/event-stream")
  res.setHeader("Cache-Control", "no-cache")
  res.setHeader("Connection", "keep-alive")

  // Send initial state
  const messages = getMessages(sessionId)
  res.write(`data: ${JSON.stringify({ type: "init", messages })}\n\n`)

  // Listen for events
  const listener = (event: Event) => {
    res.write(`data: ${JSON.stringify(event)}\n\n`)
  }

  eventBus.on(sessionId, listener)

  // Cleanup on disconnect
  req.on("close", () => {
    eventBus.off(sessionId, listener)
  })
})
```

### Client-side Event Handling

```typescript
class ContextTrackingClient {
  private eventSource: EventSource | null = null
  private listeners: Map<string, Set<(event: Event) => void>> = new Map()

  connect(sessionId: string): void {
    this.eventSource = new EventSource(`/api/sessions/${sessionId}/events`)

    this.eventSource.onmessage = (event) => {
      const data = JSON.parse(event.data)
      this.handleEvent(data)
    }

    this.eventSource.onerror = (error) => {
      console.error("SSE error:", error)
      // Reconnect logic here
    }
  }

  private handleEvent(event: Event): void {
    const listeners = this.listeners.get(event.type)
    if (listeners) {
      listeners.forEach((listener) => listener(event))
    }
  }

  on(eventType: string, callback: (event: Event) => void): () => void {
    if (!this.listeners.has(eventType)) {
      this.listeners.set(eventType, new Set())
    }
    this.listeners.get(eventType)!.add(callback)

    // Return unsubscribe function
    return () => {
      this.listeners.get(eventType)?.delete(callback)
    }
  }

  disconnect(): void {
    this.eventSource?.close()
    this.eventSource = null
  }
}

// Usage
const client = new ContextTrackingClient()
client.connect("session-123")

client.on("message.created", (event) => {
  console.log("New message:", event.payload.message)
  updateUI(event.payload.message)
})
```

### WebSocket Alternative

```typescript
// Using WebSocket for bidirectional communication
class ContextTrackingWebSocket {
  private ws: WebSocket | null = null

  connect(sessionId: string): void {
    this.ws = new WebSocket(`wss://api.example.com/sessions/${sessionId}`)

    this.ws.onmessage = (event) => {
      const data = JSON.parse(event.data)
      this.handleEvent(data)
    }
  }

  // Can also send messages to server
  send(message: unknown): void {
    this.ws?.send(JSON.stringify(message))
  }
}
```

---

## UI Components

### 1. Context Usage Indicator

A compact component showing current context usage:

```typescript
interface ContextUsageProps {
  metrics: ContextMetrics;
  size?: 'sm' | 'md' | 'lg';
}

function ContextUsageIndicator({
  metrics,
  size = 'md'
}: ContextUsageProps) {
  const percentage = Math.min(metrics.usage, 100);
  const isWarning = metrics.usage > 80;
  const isCritical = metrics.usage > 95;

  const sizeClasses = {
    sm: 'w-4 h-4',
    md: 'w-6 h-6',
    lg: 'w-8 h-8'
  };

  const colorClass = isCritical
    ? 'text-red-500'
    : isWarning
      ? 'text-yellow-500'
      : 'text-green-500';

  return (
    <div className="relative inline-flex items-center">
      <svg
        className={`${sizeClasses[size]} ${colorClass} transform -rotate-90`}
        viewBox="0 0 36 36"
      >
        {/* Background circle */}
        <path
          className="text-gray-200"
          d="M18 2.0845 a 15.9155 15.9155 0 0 1 0 31.831 a 15.9155 15.9155 0 0 1 0 -31.831"
          fill="none"
          stroke="currentColor"
          strokeWidth="3"
        />
        {/* Progress circle */}
        <path
          className="transition-all duration-500 ease-out"
          d="M18 2.0845 a 15.9155 15.9155 0 0 1 0 31.831 a 15.9155 15.9155 0 0 1 0 -31.831"
          fill="none"
          stroke="currentColor"
          strokeWidth="3"
          strokeDasharray={`${percentage}, 100`}
        />
      </svg>

      {/* Tooltip */}
      <div className="absolute bottom-full mb-2 hidden group-hover:block">
        <div className="bg-gray-900 text-white text-xs rounded px-2 py-1 whitespace-nowrap">
          {metrics.totalTokens.toLocaleString()} / {metrics.limit.toLocaleString()} tokens
          <br />
          ({metrics.usage.toFixed(1)}%)
        </div>
      </div>
    </div>
  );
}
```

### 2. Context Breakdown Bar

Shows distribution of tokens across categories:

```typescript
interface ContextBreakdownProps {
  breakdown: ContextMetrics['breakdown'];
}

function ContextBreakdownBar({ breakdown }: ContextBreakdownProps) {
  const total = Object.values(breakdown).reduce((a, b) => a + b, 0);

  const categories = [
    { key: 'system', label: 'System', color: 'bg-blue-500' },
    { key: 'user', label: 'User', color: 'bg-green-500' },
    { key: 'assistant', label: 'Assistant', color: 'bg-purple-500' },
    { key: 'tool', label: 'Tool', color: 'bg-orange-500' }
  ] as const;

  return (
    <div className="w-full">
      <div className="flex h-2 rounded-full overflow-hidden">
        {categories.map(({ key, color }) => {
          const value = breakdown[key];
          const percentage = total > 0 ? (value / total) * 100 : 0;

          return percentage > 0 ? (
            <div
              key={key}
              className={`${color} transition-all duration-300`}
              style={{ width: `${percentage}%` }}
              title={`${key}: ${value.toLocaleString()} tokens`}
            />
          ) : null;
        })}
      </div>

      {/* Legend */}
      <div className="flex gap-4 mt-2 text-xs">
        {categories.map(({ key, label, color }) => (
          <div key={key} className="flex items-center gap-1">
            <div className={`w-2 h-2 rounded-full ${color}`} />
            <span>{label}: {breakdown[key].toLocaleString()}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
```

### 3. Detailed Context Statistics

Full panel with comprehensive metrics:

```typescript
interface ContextStatsPanelProps {
  session: Session;
  messages: Message[];
  model: Model;
}

function ContextStatsPanel({
  session,
  messages,
  model
}: ContextStatsPanelProps) {
  const metrics = calculateContextMetrics(messages, model);
  const assistantMessages = messages.filter(m => m.role === 'assistant');
  const lastMessage = assistantMessages[assistantMessages.length - 1];

  return (
    <div className="p-4 bg-white rounded-lg shadow">
      <h3 className="text-lg font-semibold mb-4">Context Statistics</h3>

      {/* Overview Grid */}
      <div className="grid grid-cols-2 gap-4 mb-6">
        <StatCard
          label="Session"
          value={session.name}
        />
        <StatCard
          label="Messages"
          value={messages.length.toString()}
        />
        <StatCard
          label="Model"
          value={model.name}
        />
        <StatCard
          label="Context Limit"
          value={model.limits.context.toLocaleString()}
        />
      </div>

      {/* Token Breakdown */}
      <div className="mb-6">
        <h4 className="text-sm font-medium text-gray-600 mb-2">
          Token Usage
        </h4>
        <div className="space-y-2">
          <TokenRow label="Total" value={metrics.totalTokens} />
          <TokenRow label="Input" value={lastMessage?.tokens.input || 0} />
          <TokenRow label="Output" value={lastMessage?.tokens.output || 0} />
          <TokenRow label="Reasoning" value={lastMessage?.tokens.reasoning || 0} />
          <TokenRow label="Cache Read" value={lastMessage?.tokens.cache.read || 0} />
          <TokenRow label="Cache Write" value={lastMessage?.tokens.cache.write || 0} />
        </div>
      </div>

      {/* Usage Progress */}
      <div className="mb-6">
        <div className="flex justify-between text-sm mb-1">
          <span>Context Usage</span>
          <span className={metrics.usage > 90 ? 'text-red-600' : ''}>
            {metrics.usage.toFixed(1)}%
          </span>
        </div>
        <div className="h-2 bg-gray-200 rounded-full overflow-hidden">
          <div
            className={`h-full transition-all duration-500 ${
              metrics.usage > 90 ? 'bg-red-500' : 'bg-blue-500'
            }`}
            style={{ width: `${Math.min(metrics.usage, 100)}%` }}
          />
        </div>
      </div>

      {/* Cost */}
      <div className="pt-4 border-t">
        <div className="flex justify-between">
          <span className="text-gray-600">Total Cost</span>
          <span className="font-medium">
            ${metrics.totalCost.toFixed(4)} USD
          </span>
        </div>
      </div>
    </div>
  );
}

// Helper components
function StatCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="bg-gray-50 p-3 rounded">
      <div className="text-xs text-gray-500">{label}</div>
      <div className="font-medium truncate">{value}</div>
    </div>
  );
}

function TokenRow({ label, value }: { label: string; value: number }) {
  return (
    <div className="flex justify-between text-sm">
      <span className="text-gray-600">{label}</span>
      <span>{value.toLocaleString()}</span>
    </div>
  );
}
```

---

## API Specification

### REST Endpoints

#### Get Session Messages

```http
GET /api/sessions/{sessionId}/messages

Response:
{
  "messages": [
    {
      "id": "msg_123",
      "sessionId": "session_456",
      "role": "assistant",
      "content": "Hello!",
      "tokens": {
        "input": 10,
        "output": 5,
        "reasoning": 0,
        "cache": { "read": 0, "write": 0 }
      },
      "cost": 0.0001,
      "createdAt": "2024-01-15T10:00:00Z"
    }
  ]
}
```

#### Get Context Metrics

```http
GET /api/sessions/{sessionId}/metrics

Response:
{
  "sessionId": "session_456",
  "totalTokens": 5000,
  "limit": 128000,
  "usage": 3.9,
  "breakdown": {
    "system": 500,
    "user": 2000,
    "assistant": 2000,
    "tool": 500
  },
  "totalCost": 0.015,
  "reserved": 20000,
  "available": 123000
}
```

#### Get Available Models

```http
GET /api/models

Response:
{
  "models": [
    {
      "id": "gpt-4",
      "name": "GPT-4",
      "providerId": "openai",
      "limits": {
        "context": 128000,
        "output": 4096
      },
      "pricing": {
        "input": 10.00,
        "output": 30.00
      }
    }
  ]
}
```

### WebSocket/SSE Events

#### Client → Server

```typescript
// Subscribe to session updates
{
  "type": "subscribe",
  "sessionId": "session_456"
}

// Request metrics refresh
{
  "type": "refresh_metrics",
  "sessionId": "session_456"
}
```

#### Server → Client

```typescript
// Message created/updated
{
  "type": "message.created",
  "timestamp": "2024-01-15T10:00:00Z",
  "sessionId": "session_456",
  "payload": {
    "message": { /* ... */ }
  }
}

// Metrics update
{
  "type": "metrics.updated",
  "timestamp": "2024-01-15T10:00:00Z",
  "sessionId": "session_456",
  "payload": {
    "metrics": { /* ... */ }
  }
}

// Context warning
{
  "type": "context.warning",
  "timestamp": "2024-01-15T10:00:00Z",
  "sessionId": "session_456",
  "payload": {
    "usage": 85.5,
    "message": "Context usage is above 80%"
  }
}
```

---

## Implementation Examples

### Complete Backend Example (Node.js)

```typescript
// context-tracking.ts
import { EventEmitter } from "events"

export class ContextTrackingService {
  private eventBus = new EventEmitter()
  private sessions = new Map<string, Session>()
  private messages = new Map<string, Message[]>()

  // Add message and calculate metrics
  async addMessage(message: Message): Promise<void> {
    const sessionMessages = this.messages.get(message.sessionId) || []
    sessionMessages.push(message)
    this.messages.set(message.sessionId, sessionMessages)

    // Calculate and broadcast metrics
    const session = this.sessions.get(message.sessionId)
    if (session) {
      const metrics = this.calculateMetrics(session, sessionMessages)
      this.broadcast(message.sessionId, {
        type: "metrics.updated",
        timestamp: new Date(),
        sessionId: message.sessionId,
        payload: { metrics },
      })

      // Check for warnings
      if (metrics.usage > 80) {
        this.broadcast(message.sessionId, {
          type: "context.warning",
          timestamp: new Date(),
          sessionId: message.sessionId,
          payload: {
            usage: metrics.usage,
            message: `Context usage at ${metrics.usage.toFixed(1)}%`,
          },
        })
      }
    }

    this.broadcast(message.sessionId, {
      type: "message.created",
      timestamp: new Date(),
      sessionId: message.sessionId,
      payload: { message },
    })
  }

  // Calculate context metrics
  calculateMetrics(session: Session, messages: Message[]): ContextMetrics {
    const model = getModel(session.modelId)
    const totalTokens = this.estimateTotalTokens(messages)

    const breakdown = {
      system: 0, // Calculate from system prompts
      user: 0,
      assistant: 0,
      tool: 0,
    }

    messages.forEach((msg) => {
      if (msg.role === "user") {
        breakdown.user += this.estimateTokens(msg.content)
      } else if (msg.role === "assistant") {
        breakdown.assistant += (msg as AssistantMessage).tokens.total || 0
      }
    })

    const reserved = 20000 // Configurable buffer
    const limit = model.limits.context

    return {
      totalTokens,
      limit,
      usage: (totalTokens / limit) * 100,
      breakdown,
      totalCost: messages
        .filter((m) => m.role === "assistant")
        .reduce((sum, m) => sum + (m as AssistantMessage).cost, 0),
      reserved,
      available: limit - totalTokens - reserved,
    }
  }

  // Simple token estimation (fallback)
  estimateTokens(text: string): number {
    // Rough estimate: 4 characters ≈ 1 token
    return Math.ceil(text.length / 4)
  }

  estimateTotalTokens(messages: Message[]): number {
    return messages.reduce((total, msg) => {
      if (msg.role === "assistant") {
        return total + ((msg as AssistantMessage).tokens.total || 0)
      }
      return total + this.estimateTokens(msg.content)
    }, 0)
  }

  // Event subscription
  subscribe(sessionId: string, callback: (event: Event) => void): () => void {
    this.eventBus.on(sessionId, callback)
    return () => this.eventBus.off(sessionId, callback)
  }

  private broadcast(sessionId: string, event: Event): void {
    this.eventBus.emit(sessionId, event)
  }
}
```

### React Hook Example

```typescript
// useContextTracking.ts
import { useState, useEffect, useCallback } from 'react';

export function useContextTracking(sessionId: string) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [metrics, setMetrics] = useState<ContextMetrics | null>(null);
  const [isConnected, setIsConnected] = useState(false);

  useEffect(() => {
    const eventSource = new EventSource(
      `/api/sessions/${sessionId}/events`
    );

    eventSource.onopen = () => setIsConnected(true);

    eventSource.onmessage = (event) => {
      const data = JSON.parse(event.data);

      switch (data.type) {
        case 'init':
          setMessages(data.messages);
          break;

        case 'message.created':
          setMessages(prev => [...prev, data.payload.message]);
          break;

        case 'metrics.updated':
          setMetrics(data.payload.metrics);
          break;

        case 'context.warning':
          console.warn('Context warning:', data.payload.message);
          break;
      }
    };

    eventSource.onerror = () => {
      setIsConnected(false);
      // Reconnect logic here
    };

    return () => eventSource.close();
  }, [sessionId]);

  const refreshMetrics = useCallback(async () => {
    const response = await fetch(`/api/sessions/${sessionId}/metrics`);
    const data = await response.json();
    setMetrics(data);
  }, [sessionId]);

  return { messages, metrics, isConnected, refreshMetrics };
}

// Usage
function ChatSession({ sessionId }: { sessionId: string }) {
  const { messages, metrics, isConnected } = useContextTracking(sessionId);

  return (
    <div>
      <div className="flex items-center gap-2">
        <ConnectionStatus connected={isConnected} />
        {metrics && (
          <ContextUsageIndicator metrics={metrics} />
        )}
      </div>

      <MessageList messages={messages} />

      {metrics && (
        <ContextStatsPanel metrics={metrics} />
      )}
    </div>
  );
}
```

---

## Best Practices

### 1. Token Estimation Fallback

Always implement a fallback token estimation for cases where the LLM provider doesn't return usage data:

```typescript
function estimateTokens(text: string): number {
  // This is a rough estimate
  // For production, consider using a proper tokenizer
  return Math.ceil(text.length / 4)
}
```

### 2. Buffer Management

Maintain a configurable buffer for compaction operations:

```typescript
const DEFAULT_BUFFER = 20000 // tokens

function calculateUsableContext(limit: number, buffer: number = DEFAULT_BUFFER): number {
  return limit - buffer
}
```

### 3. Warning Thresholds

Implement progressive warnings:

```typescript
const WARNING_THRESHOLDS = {
  INFO: 70, // Blue indicator
  WARNING: 80, // Yellow indicator + console warning
  CRITICAL: 95, // Red indicator + alert
}

function getWarningLevel(usage: number): keyof typeof WARNING_THRESHOLDS | null {
  if (usage >= WARNING_THRESHOLDS.CRITICAL) return "CRITICAL"
  if (usage >= WARNING_THRESHOLDS.WARNING) return "WARNING"
  if (usage >= WARNING_THRESHOLDS.INFO) return "INFO"
  return null
}
```

### 4. Historical Data

Track token usage over time for analytics:

```typescript
interface UsageHistory {
  timestamp: Date
  totalTokens: number
  messageCount: number
  cost: number
}

// Store usage snapshots every N messages or every M minutes
```

### 5. Graceful Degradation

Handle cases where model limits are unknown:

```typescript
function calculateUsage(totalTokens: number, limit?: number): { percentage: number | null; status: string } {
  if (!limit || limit === 0) {
    return {
      percentage: null,
      status: "unknown",
    }
  }

  const percentage = (totalTokens / limit) * 100
  return {
    percentage,
    status: percentage > 90 ? "critical" : percentage > 70 ? "warning" : "ok",
  }
}
```

### 6. Provider-Specific Handling

Normalize differences between providers:

```typescript
const PROVIDER_HANDLERS = {
  openai: {
    extractUsage: (response: any) => ({
      input: response.usage.prompt_tokens,
      output: response.usage.completion_tokens,
    }),
  },
  anthropic: {
    extractUsage: (response: any) => ({
      input: response.usage.input_tokens,
      output: response.usage.output_tokens,
      cacheRead: response.usage.cache_read_input_tokens,
      cacheWrite: response.usage.cache_creation_input_tokens,
    }),
  },
  // Add more providers
}
```

### 7. Testing

Test your tracking system with edge cases:

```typescript
// Test cases
describe("ContextTracking", () => {
  it("calculates total correctly", () => {
    const usage: TokenUsage = {
      input: 1000,
      output: 500,
      reasoning: 200,
      cache: { read: 300, write: 100 },
    }

    expect(calculateTotal(usage)).toBe(2100)
  })

  it("handles unknown limits", () => {
    const metrics = calculateMetrics(messages, { context: 0 })
    expect(metrics.usage).toBeNull()
  })

  it("warns at threshold", () => {
    const metrics = { usage: 85 }
    expect(getWarningLevel(metrics.usage)).toBe("WARNING")
  })
})
```

### 8. Performance Considerations

- Use efficient data structures for large message histories
- Implement pagination for message loading
- Cache metrics calculations
- Use binary search for sorted message arrays

```typescript
// Efficient message lookup
function findMessage(messages: Message[], id: string): Message | undefined {
  // Assuming messages are sorted by id or timestamp
  const index = binarySearch(messages, id, (m) => m.id)
  return index >= 0 ? messages[index] : undefined
}
```

---

## Integration Checklist

- [ ] Define your data structures (TokenUsage, Message, Model)
- [ ] Implement token normalization for your LLM providers
- [ ] Set up event publishing system
- [ ] Implement SSE or WebSocket endpoints
- [ ] Create UI components (indicator, breakdown, stats panel)
- [ ] Add warning thresholds and alerts
- [ ] Implement token estimation fallback
- [ ] Test with multiple providers
- [ ] Add error handling for edge cases
- [ ] Document your API endpoints

---

## References

- [OpenAI API - Usage](https://platform.openai.com/docs/api-reference/chat/object#chat/object-usage)
- [Anthropic API - Cache](https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching)
- [Google Gemini API - UsageMetadata](https://ai.google.dev/api/rest/v1beta/GenerateContentResponse#usagemetadata)
- [Tiktoken - Tokenizer](https://github.com/openai/tiktoken)
