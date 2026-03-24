Да. Если делать рефакторинг “по-взрослому”, я бы закладывал не “универсальный формат `tool_call_id`”, а **универсальную модель корреляции**.

Фактически у вас есть две разные сущности:

1. **внутренний id вызова** — нужен вам для БД, очередей, трассировки, retry, dedup;
2. **provider tool call id** — нужен тол([AI SDK][1])йдер**.

Это ближе всего к тому, как сами провайдеры и SDK уже устроены:
AI SDK везде держит `toolCallId` просто как `string`; OpenAI требует возвращать `call_id`/`tool_call_id`; Mistral в примере тоже возвращает результат по `tool_call_id = tool_call.id`; в Rust-обертках вроде `async-openai` это тоже просто `String`. ([AI SDK][1])ay” я бы сделал

Не так:

```rust
struct ToolCall {
    id: String, // тут все подряд: и internal, и provider, и correlation
}
```

А так:

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ulid::Ulid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InvocationId(pub Ulid);

impl InvocationId {
    pub fn new() -> Self {
        Self(Ulid::new())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderToolCallId(String);

impl ProviderToolCallId {
    pub fn new(raw: String) -> Result<Self, ToolCallIdError> {
        if raw.is_empty() {
            return Err(ToolCallIdError::Empty);
        }
        if raw.len() > 256 {
            return Err(ToolCallIdError::TooLong(raw.len()));
        }
        Ok(Self(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ToolCallIdError {
    #[error("provider tool call id is empty")]
    Empty,
    #[error("provider tool call id too long: {0}")]
    TooLong(usize),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderKind {
    OpenAi,
    Mistral,
    Zhipu,
    MiniMax,
    Other(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub invocation_id: InvocationId,              // ваш stable internal id
    pub provider: ProviderKind,                   // откуда пришло
    pub provider_tool_call_id: ProviderToolCallId,// что надо будет эхо-вернуть
    pub provider_item_id: Option<String>,         // отдельный provider item id, если есть
    pub run_id: String,                           // ваш run / conversation / trace id
    pub tool_name: String,
    pub args: Value,
    pub status: ToolInvocationStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolInvocationStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
}
```

### Почему это лучше

Потому что у некоторых API есть **только один id**, а у некоторых — **два разных**:

* в OpenAI Chat Completions обычно используете `tool_call.id` → потом отправляете его как `tool_call_id`;
* в OpenAI Responses есть **и** `id`, **и** `call_id`, причем именно `call_id` используется для `function_call_output`;
* `llm-sdk-rs` тоже прямо разделяет `id` и `tool_call_id`. ([OpenAI Developers][2])ider_item_id: Option<String>` — это не “перестраховка”, а нормальный задел под реальные API. ([OpenAI Developers][2])инцип

**Внутри системы живет `InvocationId`.
На wire-границе живет `ProviderToolCallId`.**

Бизнес-логика, executor, очередь задач, retry worker — все должны работать по `invocation_id`.
Только provider-adapter должен знать про `provider_tool_call_id`.

Вот это и убирает будущий техдолг.

---

## Правильное разделение слоев

### 1. Provider wire layer

Парсит JSON конкретного провайдера.

### 2. Domain layer

Превращает вход в `ToolInvocation`.

### 3. Tool executor layer

Выполняет инструмент по `invocation_id`.

### 4. Provider response layer

Берет `ToolInvocation + ToolResult` и сериализует обратно в формат провайдера, используя **исходный** `provider_tool_call_id`.

---

## Пример адаптера

### Вход от Mistral / OpenAI Chat-style

У Mistral в примере tool call выглядит так:

```json
{
  "function": {
    "name": "retrieve_payment_status",
    "arguments": "{\"transaction_id\": \"T1001\"}"
  },
  "id": "D681PevKs",
  "type": "function"
}
```

А результат потом отправляется с `"tool_call_id": tool_call.id`. ([Mistral AI][3])#[derive(Debug, Deserialize)]
pub struct ChatStyleToolCall {
pub id: String,
#[serde(rename = "type")]
pub kind: String,
pub function: ChatStyleFunction,
}

#[derive(Debug, Deserialize)]
pub struct ChatStyleFunction {
pub name: String,
pub arguments: String, // JSON string
}

impl ChatStyleToolCall {
pub fn into_domain(
self,
provider: ProviderKind,
run_id: String,
) -> Result<ToolInvocation, anyhow::Error> {
Ok(ToolInvocation {
invocation_id: InvocationId::new(),
provider,
provider_tool_call_id: ProviderToolCallId::new(self.id)?,
provider_item_id: None,
run_id,
tool_name: self.function.name,
args: serde_json::from_str(&self.function.arguments)?,
status: ToolInvocationStatus::Pending,
})
}
}

```

### Вход от OpenAI Responses-style

В Responses API у function call есть и `id`, и `call_id`, а для обратной отправки нужен именно `call_id`. :contentReference[oaicite:10]{index=10}ebug, Deserialize)]
pub struct ResponsesFunctionCall {
    pub id: String,        // provider item id
    pub call_id: String,   // correlation id for function_call_output
    pub name: String,
    pub arguments: String,
    #[serde(rename = "type")]
    pub kind: String,
}

impl ResponsesFunctionCall {
    pub fn into_domain(
        self,
        provider: ProviderKind,
        run_id: String,
    ) -> Result<ToolInvocation, anyhow::Error> {
        Ok(ToolInvocation {
            invocation_id: InvocationId::new(),
            provider,
            provider_tool_call_id: ProviderToolCallId::new(self.call_id)?,
            provider_item_id: Some(self.id),
            run_id,
            tool_name: self.name,
            args: serde_json::from_str(&self.arguments)?,
            status: ToolInvocationStatus::Pending,
        })
    }
}
```

---

## Как должен выглядеть executor API

Не так:

```rust
async fn execute_tool(provider_tool_call_id: String, ...)
```

А так:

```rust
#[derive(Clone, Debug)]
pub struct ExecuteToolRequest {
    pub invocation_id: InvocationId,
    pub tool_name: String,
    pub args: Value,
}

#[derive(Clone, Debug)]
pub struct ExecuteToolResult {
    pub invocation_id: InvocationId,
    pub output: Value,
    pub is_error: bool,
}
```

Идея простая:
executor вообще **не должен знать**, какой там был `call_xxx`, `D681PevKs` или `a1b2c3d4e`.

Он знает только ваш `invocation_id`.

---

## Как собирать ответ обратно провайдеру

### Mistral / OpenAI Chat-style

И OpenAI Chat-style, и Mistral используют возврат результата через `tool_call_id`, который должен ссылаться на исходный tool call id. ([OpenAI Developers][2])json::json;

pub fn make_chat_style_tool_result(
invocation: &ToolInvocation,
output: &Value,
) -> Value {
json!({
"role": "tool",
"name": invocation.tool_name,
"content": output.to_string(),
"tool_call_id": invocation.provider_tool_call_id.as_str()
})
}

```

### OpenAI Responses-style

Там нужен `call_id`. :contentReference[oaicite:14]{index=14}e_responses_tool_result(
    invocation: &ToolInvocation,
    output: &Value,
) -> Value {
    json!({
        "type": "function_call_output",
        "call_id": invocation.provider_tool_call_id.as_str(),
        "output": output
    })
}
```

---

## Самая важная инженерная мысль

У вас не должно быть такого кода:

```rust
match provider {
    ProviderKind::Mistral => assert_eq!(tool_call_id.len(), 9),
    ProviderKind::OpenAi => assert!(tool_call_id.starts_with("call_")),
    ProviderKind::Zhipu => ...
}
```

Это и есть техдолг.

Должно быть так:

```rust
let provider_tool_call_id = ProviderToolCallId::new(raw_id)?;
```

Все.
Максимум — базовая санитарная валидация: не пусто, не безумно длинно.

---

## Что хранить в БД

Я бы сделал примерно такую схему:

```sql
create table tool_invocations (
    invocation_id           char(26) primary key,  -- ULID
    run_id                  text not null,
    provider                text not null,
    provider_tool_call_id   varchar(256) not null,
    provider_item_id        varchar(256),
    tool_name               varchar(128) not null,
    args_json               jsonb not null,
    status                  varchar(32) not null,
    output_json             jsonb,
    error_text              text,
    created_at              timestamptz not null default now(),
    updated_at              timestamptz not null default now()
);

create unique index ux_tool_invocations_provider_corr
    on tool_invocations(run_id, provider, provider_tool_call_id);
```

### Почему не делать `provider_tool_call_id` primary key

Потому что это **не ваш id**.
Он может:

* быть коротким;
* быть длинным;
* иметь provider-specific формат;
* потенциально коллидировать между разными runs / providers.

Поэтому primary key — только ваш `invocation_id`.

---

## Что я бы сделал в рефакторинге по шагам

### Шаг 1

Добавил бы новый internal id:

* `invocation_id`
* `provider_tool_call_id`
* `provider_item_id nullable`

### Шаг 2

Переименовал бы старое `tool_call_id`:

* если сейчас там лежит provider id — в `provider_tool_call_id`
* если там лежит ваш id — в `invocation_id`

Главное: убрать двусмысленность из названия.

### Шаг 3

Вынес бы все provider-specific struct в `providers/*/wire.rs`

Например:

```rust
providers/
  openai/
    wire.rs
    adapter.rs
  mistral/
    wire.rs
    adapter.rs
domain/
  tool_invocation.rs
application/
  execute_tool.rs
```

### Шаг 4

Сделал бы единый domain constructor:

```rust
pub trait ProviderAdapter {
    type InboundToolCall;
    type OutboundToolResult;

    fn parse_tool_call(
        &self,
        run_id: String,
        payload: Self::InboundToolCall,
    ) -> anyhow::Result<ToolInvocation>;

    fn build_tool_result(
        &self,
        invocation: &ToolInvocation,
        output: &Value,
        is_error: bool,
    ) -> anyhow::Result<Self::OutboundToolResult>;
}
```

### Шаг 5

Все очереди / retries / tracing перевел бы на `invocation_id`.

---

## Хороший компромисс по типам

Если не хотите сразу большой рефакторинг, можно начать с такого минимального улучшения:

```rust
pub struct ToolCallRef {
    pub invocation_id: InvocationId,
    pub provider_tool_call_id: String,
}
```

Но это только промежуточный этап.
Лучше все же newtype, иначе через 2 месяца снова начнете случайно смешивать ids.

---

## Что делать со streaming

Тут тонкость: в stream не всегда сразу приходит финально собранный tool call.

Правило:

* пока провайдер шлет дельты аргументов — буферизуйте по временному stream-key;
* как только появляется финальный provider tool call id — создавайте `ToolInvocation`;
* после этого весь pipeline живет по `invocation_id`.

AI SDK и в обычном, и в streaming tool flow также завязан на строковый `toolCallId`, без требования к формату. ([AI SDK][1])читал “готовым дизайном без техдолга”

Вот эти правила:

1. `provider_tool_call_id` — opaque string.
2. `invocation_id` — ваш внутренний stable id.
3. `provider_item_id` — optional, если у API есть второй id.
4. business logic не знает про provider ids.
5. provider adapter всегда эхом возвращает исходный provider correlation id.
6. БД и JSON не предполагают UUID-формат для external ids.
7. Никаких regex/length-веток по провайдерам, кроме мягкой санитарной валидации.

---

## Мой практический совет именно для вашего кейса

Если вы уже готовы к рефакторингу, я бы делал **не “поддержку коротких и длинных id”**, а вот это переименование модели:

**было**

```rust
ToolCall { id: String }
```

**стало**

```rust
ToolInvocation {
    invocation_id: InvocationId,
    provider_tool_call_id: ProviderToolCallId,
    provider_item_id: Option<String>,
    ...
}
```

Это самый полезный refactor на дистанции.

Дальше уже не важно, пришло:

* `call_a1b2c3d4-e5f6-7890-abcd-ef1234567890`
* `D681PevKs`
* `a1b2c3d4e`

Ваш runtime остается одинаковым.

[1]: https://ai-sdk.dev/docs/reference/ai-sdk-core/generate-text "AI SDK Core: generateText"
[2]: https://developers.openai.com/api/docs/guides/function-calling/ "Function calling | OpenAI API"
[3]: https://docs.mistral.ai/capabilities/function_calling "Function Calling | Mistral Docs"












---

Да — **для Anthropic client tools твой дизайн совместим очень хорошо**.

Там просто другой wire-контракт:

* Claude возвращает блок `tool_use` с полем `id`
* ты выполняешь инструмент
* потом отправляешь `tool_result` с полем `tool_use_id`, которое должно быть равно исходному `tool_use.id` ([Claude API Docs][1])

То есть твоя абстракция:

```rust
invocation_id: InvocationId
provider_tool_call_id: ProviderToolCallId
```

ложится на Anthropic напрямую:

* `invocation_id` — твой внутренний id
* `provider_tool_call_id` — это `tool_use.id` от Claude
* при ответе обратно в Anthropic ты кладешь его в `tool_use_id`

## Как это маппится

### Domain-модель

```rust
pub struct ToolInvocation {
    pub invocation_id: InvocationId,
    pub provider: ProviderKind,
    pub provider_tool_call_id: ProviderToolCallId, // для Anthropic = content.id
    pub provider_item_id: Option<String>,          // можно оставить None
    pub tool_name: String,
    pub args: serde_json::Value,
}
```

### Что приходит от Anthropic

По docs, если `stop_reason == "tool_use"`, в `response.content` будут блоки `tool_use`; у такого блока есть `id`, `name`, `input`. ([Claude API Docs][2])

Схематично:

```json
{
  "type": "tool_use",
  "id": "toolu_123",
  "name": "calculator",
  "input": { "operation": "add", "a": 1234, "b": 5678 }
}
```

### Что ты отправляешь обратно

Anthropic ожидает новый `user` message с блоком:

```json
{
  "type": "tool_result",
  "tool_use_id": "toolu_123",
  "content": "6912"
}
```

Это официальный паттерн из docs. ([Claude API Docs][1])

---

## Практически: адаптер для Anthropic

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct AnthropicToolUseBlock {
    #[serde(rename = "type")]
    pub kind: String, // "tool_use"
    pub id: String,
    pub name: String,
    pub input: Value,
}

impl AnthropicToolUseBlock {
    pub fn into_domain(self) -> Result<ToolInvocation, ToolCallIdError> {
        Ok(ToolInvocation {
            invocation_id: InvocationId::new(),
            provider: ProviderKind::Anthropic,
            provider_tool_call_id: ProviderToolCallId::new(self.id)?,
            provider_item_id: None,
            tool_name: self.name,
            args: self.input,
        })
    }
}
```

И обратный маппинг:

```rust
use serde_json::json;

pub fn anthropic_tool_result(inv: &ToolInvocation, output: Value) -> Value {
    json!({
        "role": "user",
        "content": [{
            "type": "tool_result",
            "tool_use_id": inv.provider_tool_call_id.as_str(),
            "content": output
        }]
    })
}
```

---

## Где именно отличие от OpenAI/Mistral

Архитектурно отличие **не в id-модели**, а в форме wire-message:

### OpenAI / Mistral-style

Обычно ты возвращаешь что-то вроде:

```json
{
  "role": "tool",
  "tool_call_id": "..."
}
```

### Anthropic-style

Ты возвращаешь:

```json
{
  "role": "user",
  "content": [
    {
      "type": "tool_result",
      "tool_use_id": "..."
    }
  ]
}
```

То есть:

* **id-корреляция та же самая**
* **transport envelope другой** ([Claude API Docs][1])

Поэтому твой refactor с `provider_tool_call_id: String` — правильный и для Anthropic тоже.

---

## Но есть 3 важных нюанса для Anthropic

### 1. `tool_result` должен идти как новый `user` message

Anthropic прямо показывает, что результат инструмента возвращается в новом сообщении роли `user`, а не `assistant`. ([Claude API Docs][1])

### 2. Не надо добавлять текст рядом с `tool_result`

В docs есть отдельное предупреждение: не добавлять text block сразу после `tool_result`, иначе можно сломать цикл tool use. ([Claude API Docs][1])

То есть это корректно:

```json
{
  "role": "user",
  "content": [
    { "type": "tool_result", "tool_use_id": "toolu_123", "content": "6912" }
  ]
}
```

А это уже плохой паттерн для Anthropic:

```json
{
  "role": "user",
  "content": [
    { "type": "tool_result", "tool_use_id": "toolu_123", "content": "6912" },
    { "type": "text", "text": "Вот результат" }
  ]
}
```

### 3. У Anthropic есть еще server tools

Для client tools твоя схема подходит идеально.
Но у Anthropic есть отдельный flow для server tools (`web_search`, `web_fetch` и т.п.), где сервер сам выполняет инструмент, а ты иногда должен обрабатывать `pause_turn` и продолжать диалог, пересылая ответ обратно как есть. Это уже другой operational path. ([Claude API Docs][3])

То есть я бы сделал так:

```rust
pub enum ToolTransport {
    ClientExecuted,   // OpenAI, Mistral, Anthropic client tools
    ServerExecuted,   // Anthropic server tools
}
```

---

## Какой вариант я бы считал “правильно совместимым”

Я бы добавил в доменную модель не только provider, но и protocol flavor:

```rust
pub enum ToolProtocol {
    OpenAiChatLike,
    OpenAiResponses,
    AnthropicClientTools,
    AnthropicServerTools,
}
```

Тогда сериализация результата станет честно разной:

```rust
pub trait ToolResultEncoder {
    fn encode(&self, inv: &ToolInvocation, output: &serde_json::Value) -> serde_json::Value;
}
```

### Реализация для Anthropic client tools

```rust
pub struct AnthropicClientEncoder;

impl ToolResultEncoder for AnthropicClientEncoder {
    fn encode(&self, inv: &ToolInvocation, output: &serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": inv.provider_tool_call_id.as_str(),
                "content": output
            }]
        })
    }
}
```

### Реализация для OpenAI/Mistral

```rust
pub struct ChatLikeEncoder;

impl ToolResultEncoder for ChatLikeEncoder {
    fn encode(&self, inv: &ToolInvocation, output: &serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "role": "tool",
            "tool_call_id": inv.provider_tool_call_id.as_str(),
            "content": output.to_string()
        })
    }
}
```

Вот это уже почти без техдолга.

---

## Итог

**Да, совместим.**
Причем именно твой proposed-подход — `internal id + provider opaque id` — для Anthropic правильнее, чем попытка сделать единый “универсальный `tool_call_id` формат”.

Для Anthropic надо просто помнить:

* входящий id у них называется `tool_use.id`
* исходящий correlation-id у них называется `tool_use_id`
* `tool_result` отправляется как `user` message
* не смешивать `tool_result` с произвольным текстом в том же сообщении
* server tools лучше выделить в отдельный flow ([Claude API Docs][1])

Если хочешь, я могу сразу набросать тебе **единый Rust trait `ProviderAdapter` с реализациями для OpenAI / Mistral / Anthropic**, чтобы было видно финальную форму API после рефакторинга.

[1]: https://docs.anthropic.com/en/api/handling-stop-reasons "Handling stop reasons - Claude API Docs"
[2]: https://docs.anthropic.com/en/api/handling-stop-reasons?utm_source=chatgpt.com "Handling stop reasons - Claude API Docs"
[3]: https://docs.anthropic.com/en/docs/build-with-claude/tool-use?utm_source=chatgpt.com "Tool use with Claude - Claude API Docs"
