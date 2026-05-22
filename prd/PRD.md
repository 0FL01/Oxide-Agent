# PRD: Async Parallel Tool Runtime для Oxide Agent

## 1. Summary

Oxide Agent должен заменить текущую логику tool calls на единый async parallel runtime. В v1 поддерживаются только `opencode go provider` и модель `DeepSeek V4 Flash`; GLM, MiniMax, Gemini, старые fallback-paths, approval gates и policy restrictions не входят в scope.

Новая модель выполнения:

```text
LLM response / stream
→ parse and validate tool calls
→ record assistant tool-call item in conversation/history
→ create ToolInvocation for every call
→ execute batch asynchronously and in parallel
→ enforce hard timeout / cancellation / hung detection / cleanup
→ normalize every terminal state into exactly one ToolOutput
→ record tool outputs in deterministic order
→ continue next LLM turn only after all outputs are recorded
```

Ключевой инвариант: на каждый `tool_call_id` текущего batch должен быть записан ровно один валидный tool output. Timeout, hung, cancellation, unknown tool, invalid args, join error, process cleanup failure и provider protocol mismatch не должны оставлять историю без paired output.

v1 работает в YOLO-режиме: runtime не блокирует destructive commands, не делает approval gates, не вводит per-tool allow/deny и не сериализует shell/SSH/DevOps-команды по умолчанию. YOLO относится только к policy layer. Технические предохранители обязательны: hard timeout, cancellation, process cleanup, hung detection, output truncation и artifact handling.

## 2. Goals

1. Полностью заменить текущие code paths выполнения tools в Oxide Agent на один runtime.
2. Поддержать opencode go provider + DeepSeek V4 Flash как единственную обязательную provider/model-комбинацию v1.
3. Выполнять все tool calls одного assistant response batch асинхронно и параллельно.
4. Гарантировать paired history: каждый assistant tool call получает ровно один tool output с тем же `tool_call_id`.
5. Применять per-tool hard timeout поверх любого executor-а.
6. Пропагировать user cancellation во все in-flight tools текущего turn.
7. Делать cleanup процессов после timeout/cancel/hung.
8. Превращать любой runtime failure в нормализованный `ToolOutput`, пригодный для отправки модели.
9. Ограничить stdout/stderr, попадающие в model context, и сохранять большие outputs как artifacts/logs.
10. Удалить legacy/fallback execution paths, а не держать новый runtime рядом со старым.
11. Оставить чистую extension point для будущего job/process API, не превращая v1 в background-job system.
12. Сохранить general-purpose применимость: web/search-like tools, filesystem, shell/linux, codebase analysis, package managers, git, diagnostics, DevOps, long-running commands.

## 3. Non-goals

В v1 не реализуются и не поддерживаются:

- GLM, MiniMax, Gemini и другие provider-specific форматы.
- Generic multi-provider compatibility layer.
- Fallback на старый `tool_bridge` или старый `execute_tools`.
- Два runtime-а под feature flag.
- Approval gates.
- Per-tool allow/deny.
- Safety classifier для команд.
- Resource-aware scheduler.
- Read/write locks для shell/SSH по умолчанию.
- DevOps-specific lock model.
- Запрет destructive commands.
- Background job system, где модель продолжает думать, пока tool всё ещё выполняется.
- “Model thinks while tools still running”.
- User policy-level max parallelism как safety feature.

Опциональный глобальный технический лимит in-flight tools допускается только как защита процесса Oxide Agent от исчерпания ресурсов. Дефолт v1 должен быть effectively unlimited или высоким. Такой лимит не является policy restriction.

## 4. Current Oxide Agent analysis

### 4.1 Найденные текущие code paths в Oxide Agent

Ниже перечислены конкретные файлы и code paths, которые нужно заменить, удалить или переподключить к новому runtime.

#### `crates/oxide-agent-core/src/agent/runner/mod.rs`

Текущие обязанности:

- `AgentRunner` хранит `LlmClient`, hooks, loop detection и route failover.
- `convert_memory_to_messages` конвертирует `AgentMessage` в `llm::Message`, включая `tool_call_id`, `tool_name`, `tool_calls`, `tool_call_correlations`.
- `run_with_timeout` оборачивает весь `runner.run(ctx)` в `tokio::time::timeout` и возвращает `TimedRunResult::TimedOut`.

Проблема:

- Agent-level timeout не является per-tool hard timeout.
- При timeout всего runner-а runtime не синтезирует missing tool outputs для уже записанных assistant tool calls.
- Новый runtime должен иметь собственные per-tool timeout/cancel/cleanup гарантии и не полагаться на agent-level timeout.

Действие:

- Оставить `AgentRunner` как orchestration shell.
- Убрать зависимость tool correctness от `run_with_timeout`.
- Встроить новый `ToolCallRuntime` в основную turn loop.

#### `crates/oxide-agent-core/src/agent/runner/execution.rs`

Текущие обязанности:

- Основная agent loop в `run`.
- Вызов `call_llm_with_tools`.
- Обработка LLM response в `handle_llm_response`.
- Обработка tool calls в `handle_tool_calls_response`.
- Запись assistant tool-call message через `record_assistant_tool_call`.
- Вызов `execute_tools`.
- Structured-output path, где JSON преобразуется в один synthetic tool call.
- Fallback parsing в `handle_unstructured_response`.
- Route failover через `call_llm_with_tools_with_failover` и legacy path через `call_llm_with_tools_legacy`.
- `repair_history_before_retry`, который вызывает provider-specific history repair.

Проблемы:

- `handle_tool_calls_response` передаёт batch в старый `execute_tools`, который не гарантирует per-tool hard timeout и cleanup.
- Structured/unstructured fallback logic создаёт дополнительные tool-call paths и может обходить строгий provider protocol.
- Route failover и legacy LLM path противоречат scope v1: только opencode go + DeepSeek V4 Flash.
- History repair не должен быть механизмом корректности для tool calls. В v1 история должна быть валидной до следующего provider request.

Действие:

- Заменить `handle_tool_calls_response` на вызов нового `ToolCallRuntime::execute_batch`.
- Удалить fallback parsing для tool calls из unstructured text.
- Удалить legacy/failover LLM paths из v1 execution flow.
- Перевести history repair для tool messages в assert/diagnostics mode или полностью удалить из активного path v1.

#### `crates/oxide-agent-core/src/agent/runner/tools.rs`

Текущие обязанности:

- `record_assistant_tool_call` записывает assistant tool-call message в память и provider-visible history.
- `execute_tools` применяет skill context, before-tool hooks, approval-like blocking, compression special case.
- `execute_approved_tools` запускает tools через `join_all`, сортирует результаты по index, затем пишет outputs.
- `record_tool_execution_result` добавляет `Message::tool_with_correlation` и `AgentMessage::tool_with_correlation`.

Проблемы:

- `execute_approved_tools` действительно запускает futures параллельно, но это не полноценный runtime.
- В основном path нет runtime-enforced per-tool hard timeout.
- `join_all` ждёт все futures; один hung provider/executor может повесить batch и оставить tool outputs незаписанными.
- Ошибка executor-а превращается в строку только после завершения future. Если future не завершается, output отсутствует.
- `execute_tools` содержит gates/hooks/blocking behavior, несовместимые с v1 YOLO.
- `TOOL_COMPRESS` выделен в отдельный sequential path.
- `record_tool_execution_result` содержит approval-pending detection через строки `APPROVAL_PENDING` / `Waiting for approval`, что нужно удалить.
- Tool output хранится как plain string, а не структурированный `ToolOutput` с timeout/cancel/cleanup/truncation metadata.

Действие:

- Переписать файл или заменить его новым `agent/tool_runtime/` module.
- Оставить только thin history facade, если нужно.
- Удалить before/after hook gating из tool execution path v1.
- Удалить approval string detection.
- Удалить sequential compression special path или переписать compression как обычный executor, если он нужен в v1.

#### `crates/oxide-agent-core/src/agent/tool_bridge.rs`

Текущие обязанности:

- Старый bridge `ToolExecutionContext`.
- `execute_tool_calls` выполняет tool calls последовательно.
- `execute_single_tool_call` парсит JSON, вызывает `execute_tool_with_timeout`, нормализует string result и append-ит tool output.
- `execute_tool_with_timeout` использует `AGENT_TOOL_TIMEOUT_SECS = 300` и cancellation select.
- Есть approval-related dead code: pending SSH approval, approval payload parsing, disabled approval paths.

Проблемы:

- Это отдельный legacy execution path.
- Timeout есть только в этом bridge, а не в основном `runner/tools.rs` path.
- Последовательное выполнение несовместимо с required batch parallelism.
- Cancellation может возвращать error до нормальной записи output в некоторых сценариях.
- Tool result — string, не structured `ToolOutput`.
- Наличие bridge создаёт риск fallback.

Действие:

- Удалить `tool_bridge.rs` из активного runtime.
- Не оставлять fallback на него.
- Перенести только полезную идею per-tool timeout в новый runtime wrapper.

#### `crates/oxide-agent-core/src/agent/executor/execution.rs`

Текущие обязанности:

- `run_execution` готовит execution, делает `replay_initial_tool_call`, затем вызывает `run_with_timeout`.
- `apply_execution_transition` обрабатывает `TimedOut` через `session.timeout()` и error.
- `resolve_execution_request` содержит `ResumeApproval` path.
- `replay_initial_tool_call` использует legacy `ToolExecutionContext` и `execute_single_tool_call` из `tool_bridge`.

Проблемы:

- `replay_initial_tool_call` — отдельный legacy tool path.
- Approval resume path противоречит v1.
- Agent timeout не создаёт paired tool outputs.

Действие:

- Удалить `replay_initial_tool_call` как bridge path.
- Удалить `ResumeApproval` execution path из v1.
- Перевести initial tool call replay, если он нужен, через новый `ToolCallRuntime::execute_batch` с batch size 1.

#### `crates/oxide-agent-core/src/agent/registry.rs`

Текущие обязанности:

- `ToolRegistry` хранит `Vec<Box<dyn ToolProvider>>`.
- `all_tools()` flatten-ит provider tools.
- `execute()` линейно сканирует providers через `can_handle` и возвращает `Err(anyhow!("Unknown tool"))`.

Проблемы:

- Linear scan и `can_handle` дают менее deterministic dispatch, чем прямой lookup.
- Unknown tool возвращается как runtime error, а не как paired tool output.
- `ToolProvider` возвращает `Result<String>`, что не содержит structured status/cleanup/truncation.

Действие:

- Заменить registry на deterministic `BTreeMap<ToolName, Arc<dyn ToolExecutor>>` или `IndexMap` с explicit conflict detection.
- Unknown tool должен превращаться в `ToolOutput { status: unknown_tool }`.

#### `crates/oxide-agent-core/src/agent/provider.rs`

Текущие обязанности:

- `ToolProvider` trait: `tools`, `can_handle`, `execute(...) -> Result<String>`.

Проблемы:

- `String` result не позволяет runtime корректно отражать status, exit code, cleanup, truncation, artifact refs.
- Timeout/cancel awareness делегируется provider-ам, а должна быть enforced runtime wrapper-ом.

Действие:

- Ввести `ToolExecutor` trait, возвращающий `ToolOutput`.
- Старые providers портировать на executors без compatibility bridge.

#### `crates/oxide-agent-core/src/agent/executor/registry.rs`

Текущие обязанности:

- `build_tool_registry` регистрирует Todos, Sandbox, Compression, StackLogs, FileHoster, MediaFile, Ytdlp, Delegation, topic providers, WikiMemory, MCP, search/browser, TTS и другие tools.
- `current_tool_definitions()` фильтрует tools через `execution_profile.tool_policy().filter_definitions(...)`.

Проблемы:

- Tool policy filtering противоречит v1 YOLO без policy restrictions.
- Registry строится вокруг `ToolProvider`/string output.
- Много providers могут использовать свои собственные timeout/cancel assumptions.

Действие:

- Сформировать v1 registry только из tools, реально доступных в Oxide Agent v1.
- Убрать policy filtering из v1 tool spec exposure.
- Портировать providers в `ToolExecutor`.

#### `crates/oxide-agent-core/src/agent/executor.rs`

Текущие обязанности:

- `AgentExecutor` содержит `execution_profile`, `tool_policy_state`, `hook_policy_state` и другие policy/hook states.

Проблемы:

- Runtime v1 не должен принимать решения через policy gates.
- Hook/policy state не должен влиять на возможность execute tool call.

Действие:

- Удалить policy/hook coupling из tool execution path v1.
- Если hook events нужны для observability, они должны быть passive telemetry, не gate.

#### `crates/oxide-agent-core/src/agent/providers/sandbox.rs`

Текущие обязанности:

- `SandboxProvider` реализует `execute_command`, `read_file`, `write_file`, `send_file_to_user`, `list_files`, `recreate_sandbox`.
- `execution_gate: Arc<RwLock<()>>`: `recreate_sandbox` берёт write lock, остальные sandbox tools read lock.
- `handle_execute_command` вызывает `sandbox.exec_command(&args.command, cancellation_token)` и возвращает JSON с `stdout`, `stderr`, `exit_code`.

Проблемы:

- `execute_command` возвращает полный stdout/stderr в JSON без truncation.
- `read_file` возвращает полный file content без binary/output budget guard.
- Locking вокруг sandbox может ограничить parallelism. В v1 shell/linux operations не должны сериализоваться по умолчанию.

Действие:

- Портировать sandbox tools в executors.
- `execute_command` должен идти через `ProcessManager`/container process manager с hard timeout, cleanup и output cap.
- `read_file` должен применять output budget и binary detection.

#### `crates/oxide-agent-core/src/sandbox/manager.rs`

Текущие обязанности:

- `SandboxManager::exec_command` dispatch-ит Docker/Broker execution.
- Docker path запускает command через Docker exec и собирает stdout/stderr.
- `SANDBOX_EXEC_TIMEOUT_SECS = 60`.
- На cancellation вызывается `kill_processes`.
- На timeout возвращается error.
- `run_exec` аккумулирует stdout/stderr в `String` без лимита.
- `kill_processes` вызывает `killall5 -9` через docker exec.

Проблемы:

- На timeout Docker path не вызывает `kill_processes`; command/process может жить дальше.
- stdout/stderr собираются целиком в память.
- `killall5 -9` слишком грубый и не связан с конкретной process group invocation.
- Container exec не даёт reliable paired process lifecycle metadata.

Действие:

- Переписать command execution через `ProcessManager` semantics.
- Если Docker exec остаётся, добавить invocation-scoped process group/session внутри container и cleanup конкретного process tree.
- На timeout/cancel/hung всегда делать cleanup.
- Всегда возвращать `ToolOutput`, не только error.

#### `crates/oxide-agent-core/src/agent/providers/ssh_mcp.rs`

Текущие обязанности:

- SSH provider tools: `ssh_exec`, `ssh_sudo_exec`, `ssh_read_file`, `ssh_apply_file_edit`, `ssh_check_process`, `ssh_send_file_to_user`.
- `UpstreamSshMcpBackend` использует `call_lock: Arc<Mutex<()>>`; upstream calls сериализуются.
- Timeout/cancel в `call_tool` приводят к `reset_session().await` и error.
- Approval registry и approval token paths присутствуют; `requires_approval` сейчас фактически отключён.
- `spawn_session` запускает upstream binary с `--maxChars=none` и `--max-output-tokens=UPSTREAM_MAX_OUTPUT_TOKENS`.

Проблемы:

- `call_lock` прямо противоречит v1 YOLO parallel execution.
- Timeout/cancel становятся error, а не structured tool output.
- SSH approval remnants создают forbidden gate/fallback surface.
- Remote stdout/stderr возвращаются JSON payload-ом без общей runtime-нормализации.

Действие:

- Удалить global SSH serialization lock из v1 path.
- Если upstream MCP не поддерживает concurrent calls in one session, создать отдельную upstream session per invocation или pool без shared serialization.
- Удалить approval registry/path из v1.
- Timeout/cancel/hung/cleanup нормализовать в `ToolOutput`.

#### `crates/oxide-agent-core/src/agent/tool_runtime.rs`

Текущие обязанности:

- Task-local model route metadata.

Проблемы:

- Название похоже на runtime, но фактически это не tool runtime.

Действие:

- Переименовать текущий файл, если он нужен, например в `tool_model_route.rs`.
- Освободить namespace `agent/tool_runtime/` для нового runtime.

#### `crates/oxide-agent-core/src/llm/types.rs`

Текущие обязанности:

- `Message` содержит role/content/reasoning/tool fields.
- `ToolCall` содержит `id`, optional `tool_call_correlation`, `function.name`, `function.arguments`, `is_recovered`.
- `ToolCall::invocation_id()` возвращает internal invocation id.
- `ToolCall::wire_tool_call_id()` возвращает provider wire id, если он есть.
- `Message::tool_with_correlation` пишет role `tool` с correlation.

Вывод:

- Correlation model полезна и должна сохраниться.
- Новый runtime должен явно различать internal `invocation_id` и provider-visible `wire_tool_call_id`.
- Для opencode go tool output должен использовать provider wire id, если он был в assistant tool call.

Действие:

- Расширить/адаптировать `ToolCall` mapping в `ToolInvocation`.
- Не терять `tool_call_correlation`.

#### `crates/oxide-agent-core/src/llm/providers/opencode_go.rs`

Текущие обязанности:

- OpenAI-compatible chat completions provider для opencode go.
- Request body с tools включает `tools`, `tool_choice: "auto"`, `parallel_tool_calls: true`, `stream: false`.
- JSON mode не включается, если есть tools.
- Tool calls приходят как chat-like `tool_calls[]` с `id`, `type: "function"`, `function.name`, `function.arguments`.
- Tool role message кодируется как `{ role: "tool", tool_call_id, content }`.
- `parse_tool_calls` нормализует `function.arguments` в string.
- При наличии provider `id` создаётся internal invocation id, а provider wire id сохраняется в correlation.
- Если `id` отсутствует, текущий код создаёт uncorrelated internal id.
- Если `function` или `name` отсутствуют, текущий код может silently skip элемент.

Проблемы:

- Silent skip malformed tool calls может сломать paired output invariant.
- Missing provider id нельзя silently превращать в нормальный successful call без явной protocol handling.
- Для DeepSeek V4 Flash через opencode go strict history должен быть стабильным: tool output обязан ссылаться на тот же `tool_call_id`, который есть в assistant tool call message.

Действие:

- Сохранить request format: `parallel_tool_calls: true`, `tool_choice: "auto"`, chat-like function tools.
- Убрать silent skip malformed tool calls.
- Missing/duplicate/invalid `tool_call_id` обрабатывать как provider protocol error с synthetic repaired local id только внутри записанной assistant message; см. раздел 8.

#### `crates/oxide-agent-core/src/llm/providers/protocol_profiles.rs`, `tool_call_adapter.rs`, `tool_result_encoder.rs`

Текущие обязанности:

- Chat-like tool protocol.
- `ProviderToolCallAdapter` создаёт internal `InvocationId` и сохраняет provider tool call id.
- `ProviderToolResultEncoder` для ChatLike использует `message.resolved_tool_call_correlation().wire_tool_call_id()` и `message.content`.

Вывод:

- Эти abstractions полезны, но v1 не должен становиться multi-provider project.
- Для opencode go нужно оставить strict ChatLike pairing.

Действие:

- Использовать только chat-like profile для opencode go.
- Не добавлять compatibility branches для GLM/MiniMax/Gemini.

#### `crates/oxide-agent-core/src/llm/capabilities.rs`

Текущие обязанности:

- Для `opencode-go` указаны tool calling, structured output и strict tool history.
- `deepseek-v4-flash` распознаётся как model-specific structured output support.

Действие:

- В v1 hardcode/validate active provider/model: opencode go + DeepSeek V4 Flash.
- Если конфиг указывает другую provider/model combo, fail fast до turn execution.

#### `crates/oxide-agent-core/src/agent/memory.rs` и `crates/oxide-agent-core/src/agent/recovery.rs`

Текущие обязанности:

- `AgentMemory::add_message` вызывает history repair после mutation.
- `recovery.rs` может drop-ать orphan tool results, trim incomplete parallel batch, drop duplicate tool results и т.д.

Проблемы:

- Runtime не должен создавать историю, которую потом надо чинить.
- Repair может скрыть bugs в paired tool-call invariant.

Действие:

- Для v1 сделать repair assert-only для tool-message pairing в dev/test.
- В production не полагаться на repair для нормального пути.
- Если history write fail, runtime обязан логировать fatal и не продолжать следующий LLM request.

### 4.2 Главные текущие риски

1. Есть несколько execution paths: `runner/tools.rs`, `tool_bridge.rs`, `executor/execution.rs` replay path, structured/unstructured fallback paths.
2. Timeout существует в legacy bridge и отдельных provider-ах, но не как единая runtime гарантия основного parallel path.
3. Timeout Docker sandbox не делает cleanup процесса.
4. Cancellation Rust task не гарантирует OS process cleanup во всех providers.
5. SSH MCP serializes calls через `call_lock`.
6. stdout/stderr/file contents могут попадать в prompt целиком.
7. Unknown tool / invalid args / join error не всегда превращаются в paired tool output.
8. History repair может скрывать missing/duplicate tool outputs.
9. opencode go / DeepSeek V4 Flash требует strict chat-like pairing; silent skip malformed tool call опасен.
10. `parallel_tool_calls: true` уже выставляется provider-ом, но runtime semantics не имеют hard timeout/cleanup guarantees.

## 5. Codex CLI architecture findings

Codex CLI публично расположен в `openai/codex` и содержит Rust workspace `codex-rs`. Для Oxide Agent важны не UI-решения Codex, а runtime patterns: turn loop, tool router/registry/executor, cancellation tokens, output normalization и process lifecycle handling.

### 5.1 Turn loop

В Codex Rust core turn loop концептуально устроен как repeated sampling loop:

- построить prompt из history и visible tool specs;
- отправить sampling request;
- обработать streamed/non-streamed response events;
- если model возвращает function/tool calls, превратить их в internal tool calls;
- выполнить tool call(s);
- записать tool output response items;
- повторить sampling request;
- если assistant message без tool calls, завершить turn.

Для Oxide Agent нужно перенести именно barrier semantics для v1: один assistant response может содержать несколько tool calls; runtime выполняет batch параллельно, но следующий LLM request стартует только после записи всех outputs.

### 5.2 Tool router / registry / executor

В Codex есть разделение:

- `ToolRouter` строит `ToolCall` из provider response item и dispatch-ит через registry.
- `ToolRegistry` хранит tools по canonical name, проверяет kind/payload compatibility и возвращает output item.
- `ToolExecutor` реализуется конкретными tools.
- `ToolInvocation` несёт session/turn/cancellation/call id/tool name/payload.
- `ToolOutput` умеет превращаться в provider response item.

Для Oxide Agent это правильное направление, но v1 должен быть проще:

- нет dynamic multi-provider scope;
- нет approval/sandbox policy gates;
- registry deterministic;
- runtime wrapper, а не executor, отвечает за timeout/cancel/hung/cleanup guarantees;
- error не должен оставаться `Err`, если provider ожидает tool output.

### 5.3 Parallel runtime

Codex `ToolCallRuntime` использует `tokio::spawn`, cancellation token и `AbortOnDropHandle`. Он различает tools, поддерживающие parallel calls, и tools, которые требуют exclusive guard. При cancellation он либо сохраняет уже завершённый lifecycle/output, либо abort-ит task и создаёт aborted output.

Для Oxide Agent v1 нужно взять:

- per-call spawned task;
- cancellation token per invocation;
- terminal outcome detection;
- fallback-to-output при tool task failure;
- deterministic conversion в model response item.

Нужно не переносить:

- per-tool `supports_parallel_tool_calls` как serializing mechanism;
- approval hooks;
- policy/sandbox restrictions;
- hidden compatibility branches.

В v1 все calls batch-а запускаются параллельно. Исключения запрещены, кроме технического global max in-flight tools, если он задан config-ом.

### 5.4 Timeout and process execution

Codex exec layer имеет `ExecExpiration` с timeout/cancellation variants и default exec timeout. Также есть formatting output for model с wall time, exit code и truncation. Для Oxide Agent нужно взять:

- timeout как explicit expiration object;
- cancellation as first-class signal;
- model-facing output formatting with duration/exit metadata;
- output truncation before model context.

Нужно усилить относительно текущего Oxide:

- hard timeout должен приводить к cleanup;
- cleanup result должен быть частью `ToolOutput`;
- timeout/hung/cancel должны быть output statuses, а не только errors;
- executor panic/join error должен нормализоваться.

### 5.5 Output normalization

Codex tool outputs имеют typed conversion в response item. Для Oxide Agent v1 нужен более строгий нормализатор:

- `ToolOutputStatus` обязателен;
- stdout/stderr previews/head/tail separate;
- artifact refs separate;
- cleanup status separate;
- provider wire id preserved;
- model-facing content is JSON string or compact text generated by one encoder.

### 5.6 Что концептуально переносить из Codex

Переносить:

- `ToolInvocation` as first-class object.
- `ToolExecutor` trait with typed output.
- `ToolRegistry` as deterministic dispatcher.
- `ToolCallRuntime` as spawned async batch runner.
- Cancellation token per tool call.
- Provider response item generation from typed output.
- Output truncation before model context.
- Tracing spans per tool call.

Не переносить в v1:

- Approval policy.
- Sandbox policy gates.
- Per-tool parallel-support locks.
- Multi-provider compatibility scope.
- Background process/job API as mandatory behavior.

## 6. Target architecture

### 6.1 New module layout

Рекомендуемая структура:

```text
crates/oxide-agent-core/src/agent/tool_runtime/
  mod.rs
  invocation.rs
  output.rs
  executor.rs
  registry.rs
  runtime.rs
  process.rs
  hung.rs
  normalizer.rs
  history.rs
  artifacts.rs
  config.rs
  provider_opencode_go.rs
```

Текущий `agent/tool_runtime.rs`, который хранит task-local model route metadata, нужно переименовать или переместить, чтобы не конфликтовать с новым module namespace.

### 6.2 Component responsibilities

`ToolInvocation`:

- canonical input для executor-а;
- содержит provider payload, normalized args, execution context, timeout config, cancellation token, metadata и timestamps.

`ToolOutput`:

- canonical terminal state;
- используется для history write, provider encoding, telemetry и artifacts.

`ToolExecutor`:

- tool-specific business logic;
- не отвечает за final guarantee “output exactly once”; это делает runtime wrapper.

`ToolRegistry`:

- deterministic lookup by tool name;
- exposes tool specs to opencode go;
- unknown/mismatched calls превращает в `ToolOutput`, не panic/error.

`ToolCallRuntime`:

- принимает batch tool calls;
- записывает assistant tool call item;
- запускает tasks;
- enforce-ит timeout/cancel/hung;
- вызывает cleanup;
- normalizes every terminal state;
- записывает outputs;
- возвращает deterministic Vec<ToolOutput>.

`ProcessManager`:

- отвечает за local/container/SSH-like process lifecycle;
- process group/session;
- stdout/stderr draining;
- terminate/kill/reap;
- cleanup result.

`HungDetector`:

- soft detector above hard timeout;
- no-output/no-progress/process-liveness tracking;
- initiates controlled cancellation/termination;
- never replaces hard timeout.

`OutputNormalizer`:

- converts `Result<ToolOutput, ToolRuntimeError>`, `JoinError`, panic, timeout, cancel, cleanup failure, invalid args and protocol error into valid `ToolOutput`.

`HistoryWriter`:

- enforces history invariants;
- writes assistant tool call before execution;
- writes exactly one tool output for every call;
- fails turn if history write fails after logging complete diagnostic state.

`ArtifactStore`:

- writes full stdout/stderr/large payloads to files when inline budget exceeded;
- returns artifact refs for `ToolOutput`.

### 6.3 Runtime flow

```text
handle_llm_response(response):
  if response.tool_calls.is_empty():
      record assistant text and finish/continue existing non-tool flow
  else:
      normalized_batch = ProviderToolCallParser::parse_opencode_go(response.tool_calls)
      outputs = ToolCallRuntime::execute_batch(normalized_batch, turn_ctx).await
      continue next LLM request with updated history
```

`ToolCallRuntime::execute_batch` owns all tool-call correctness guarantees. No other module should execute tools directly.

## 7. Runtime flow

### 7.1 Batch lifecycle

1. Receive LLM response containing `tool_calls`.
2. Validate provider/tool-call payloads for opencode go chat-like format.
3. Create `ToolCallBatch` with stable batch id and per-call index.
4. Repair only local missing/duplicate ids before writing assistant message, if allowed by section 8 rules.
5. Record assistant tool-call message in history.
6. Build `ToolInvocation` for each call.
7. Spawn one task per invocation.
8. Each task runs through runtime wrapper:
   - parse args;
   - registry dispatch;
   - hard timeout;
   - cancellation select;
   - hung detector;
   - process cleanup if applicable;
   - output normalization.
9. Collect outputs from all tasks.
10. Normalize any task-level failure.
11. Sort outputs by original batch index.
12. Verify `output.tool_call_id == call.tool_call_id` for every call.
13. Write tool outputs in deterministic order.
14. Only then continue to next LLM request.

### 7.2 Pseudo-code: batch handling

```rust
pub async fn execute_tool_batch(
    &self,
    raw_calls: Vec<ProviderToolCall>,
    ctx: TurnContext,
) -> Result<Vec<ToolOutput>, ToolRuntimeFatal> {
    let batch = self
        .provider_parser
        .parse_opencode_go_batch(raw_calls, &ctx)
        .map_or_repair_protocol_errors()?;

    self.history
        .record_assistant_tool_calls(&batch.assistant_message)
        .await
        .map_err(ToolRuntimeFatal::history_write_failed)?;

    let mut handles = Vec::with_capacity(batch.calls.len());

    for call in batch.calls.iter().cloned() {
        let invocation = ToolInvocation::from_call(call, ctx.child_for_tool());
        let runtime = self.clone();

        handles.push(ToolTaskHandle {
            batch_index: invocation.batch_index,
            tool_call_id: invocation.tool_call_id.clone(),
            handle: tokio::spawn(async move {
                runtime.run_one_tool(invocation).await
            }),
        });
    }

    let mut outputs = Vec::with_capacity(handles.len());

    for task in handles {
        let output = match task.handle.await {
            Ok(output) => output,
            Err(join_error) => self.normalizer.from_join_error(
                task.tool_call_id,
                task.batch_index,
                join_error,
            ),
        };
        outputs.push(output);
    }

    outputs.sort_by_key(|o| o.batch_index);
    self.invariants.verify_exactly_one_output_per_call(&batch, &outputs)?;

    for output in &outputs {
        self.history
            .record_tool_output(output)
            .await
            .map_err(ToolRuntimeFatal::history_write_failed)?;
    }

    Ok(outputs)
}
```

### 7.3 Pseudo-code: one tool call with timeout/cancellation

```rust
async fn run_one_tool(&self, invocation: ToolInvocation) -> ToolOutput {
    let started_at = Instant::now();
    let tool_call_id = invocation.tool_call_id.clone();
    let tool_name = invocation.tool_name.clone();
    let timeout = invocation.timeout.per_tool;
    let cancel = invocation.cancellation_token.clone();

    let parsed_args = match self.argument_parser.parse(&invocation) {
        Ok(args) => args,
        Err(err) => {
            return self.normalizer.invalid_arguments(
                invocation,
                err,
                started_at.elapsed(),
            );
        }
    };

    let executor = match self.registry.get(&tool_name) {
        Some(executor) => executor,
        None => {
            return self.normalizer.unknown_tool(
                invocation,
                started_at.elapsed(),
            );
        }
    };

    let child_cancel = cancel.child_token();
    let hung_signal = self.hung_detector.start(&invocation);

    let execute_future = executor.execute(invocation.with_args(parsed_args));

    let result = tokio::select! {
        biased;

        _ = cancel.cancelled() => {
            ToolTerminal::Cancelled { reason: CancellationReason::User }
        }

        hung = hung_signal.wait() => {
            ToolTerminal::Hung { detail: hung }
        }

        _ = tokio::time::sleep(timeout) => {
            ToolTerminal::Timeout { timeout }
        }

        res = execute_future => {
            ToolTerminal::ExecutorResult(res)
        }
    };

    child_cancel.cancel();

    let cleanup = if result.requires_cleanup() {
        self.process_manager.cleanup_for_invocation(&tool_call_id).await
    } else {
        CleanupResult::not_needed()
    };

    self.normalizer.normalize_terminal(
        tool_call_id,
        tool_name,
        result,
        cleanup,
        started_at.elapsed(),
    )
}
```

### 7.4 Barrier semantics

v1 uses batch barrier semantics:

- All tool calls from one assistant response start in parallel.
- Runtime waits until every call reaches a terminal output state.
- Next LLM request is blocked until all outputs are written.
- Long-running commands are allowed, but bounded by timeout/cancel/hung/cleanup.
- Future job/process API can be added behind `ProcessManager` and `ToolOutput.artifact_refs`, not by changing history invariants.

## 8. Provider compatibility: opencode go + DeepSeek V4 Flash

### 8.1 Provider/model scope

v1 supports exactly:

- provider: `opencode go` / `opencode-go`;
- model: `deepseek-v4-flash` / normalized Oxide model id for DeepSeek V4 Flash;
- tool protocol: chat-like function calling;
- tool history mode: strict paired assistant tool calls and tool outputs.

At session start or first turn, runtime must validate provider/model. If another provider/model is selected, fail fast with explicit config error. Do not fallback to GLM/MiniMax/Gemini or legacy provider paths.

### 8.2 Request format

Current opencode go provider already uses the correct shape. v1 must preserve:

```json
{
  "model": "deepseek-v4-flash",
  "messages": [...],
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "tool_name",
        "description": "...",
        "parameters": { "type": "object", "properties": {}, "required": [] }
      }
    }
  ],
  "tool_choice": "auto",
  "parallel_tool_calls": true,
  "stream": false
}
```

Rules:

- `parallel_tool_calls: true` must be set whenever tools are sent.
- `response_format: { "type": "json_object" }` must not be set for tool-call turns.
- Tool specs come from new deterministic `ToolRegistry`.
- Tool names must match exactly the executor registry names.

### 8.3 Incoming tool call format

Expected chat-like response item:

```json
{
  "id": "call_abc123",
  "type": "function",
  "function": {
    "name": "execute_command",
    "arguments": "{\"command\":\"kubectl get pods\"}"
  }
}
```

Provider parser must accept `function.arguments` as:

- JSON string containing object JSON;
- object value, serialized to canonical JSON string before arg parsing.

Provider parser must reject or repair with protocol output:

- missing `function`;
- missing `function.name`;
- empty tool name;
- malformed `arguments`;
- missing/empty `id`;
- duplicate `id` in one batch;
- non-array `tool_calls`.

### 8.4 Tool call id handling

Use two IDs:

- `invocation_id`: internal Oxide id, unique even when provider id is missing or duplicated.
- `wire_tool_call_id`: provider-visible id used in assistant tool-call history and tool output message.

Rules:

1. If provider sends a valid non-empty id, use it as `wire_tool_call_id`.
2. If provider id is missing/empty, create a deterministic local wire id before recording assistant message:
   ```text
   oxide_missing_tool_call_id_{turn_id}_{batch_index}
   ```
   The output for this call must have status `provider_protocol_error` unless the call is otherwise executable and integration tests prove opencode accepts synthetic ids.
3. If provider sends duplicate id, keep the first occurrence and rewrite later duplicates to:
   ```text
   oxide_duplicate_tool_call_id_{turn_id}_{batch_index}
   ```
   Later duplicate outputs must be `provider_protocol_error` unless explicitly allowed by integration tests.
4. Never silently skip malformed provider tool calls after the assistant response has been accepted.
5. Before next LLM request, the stored assistant message and tool messages must pair on the same `wire_tool_call_id` values.

This approach is stricter than current code, where missing id can become an uncorrelated internal invocation and malformed calls can be skipped. In v1, the provider-visible history must remain pairable.

### 8.5 Tool output format

Current opencode go encoder sends tool output as chat-like message:

```json
{
  "role": "tool",
  "tool_call_id": "call_abc123",
  "content": "..."
}
```

v1 must keep that shape. `content` should be a compact JSON string generated by one encoder, not arbitrary provider strings. Recommended content shape:

```json
{
  "tool_call_id": "call_abc123",
  "tool_name": "execute_command",
  "status": "success",
  "success": true,
  "duration_ms": 1234,
  "exit_code": 0,
  "stdout": {
    "text": "...",
    "truncated": false
  },
  "stderr": {
    "text": "...",
    "truncated": false
  },
  "artifacts": []
}
```

For failure states:

```json
{
  "tool_call_id": "call_abc123",
  "tool_name": "execute_command",
  "status": "timeout",
  "success": false,
  "duration_ms": 300000,
  "error_message": "tool exceeded hard timeout of 300s",
  "timeout_reason": "per_tool_hard_timeout",
  "cleanup_status": "killed_process_group",
  "stdout": { "head": "...", "tail": "...", "truncated": true },
  "stderr": { "head": "...", "tail": "...", "truncated": true },
  "artifacts": ["artifact://session/turn/call/stdout.log"]
}
```

### 8.6 Multiple tool calls in one response

If `tool_calls` contains N items:

- parse all N;
- record one assistant message containing all N calls;
- execute all N in parallel;
- write N tool output messages in original batch order;
- continue next LLM turn after all N are recorded.

If provider returns calls sequentially across multiple assistant responses, each response is its own batch. Runtime still applies same invariants to each batch.

### 8.7 Invalid/malformed arguments

Invalid JSON arguments are not provider errors if call id/name are present. They are tool invocation errors:

- no executor is called;
- output status: `invalid_arguments`;
- output references original `tool_call_id`;
- content includes concise parse error and expected schema summary if available;
- next LLM turn continues after this output is recorded.

### 8.8 Provider protocol mismatch

Provider protocol mismatch examples:

- missing call id;
- duplicate call id;
- missing function object;
- missing function name;
- non-array `tool_calls`;
- tool output rejected by provider in integration tests.

Behavior:

- Convert to `ToolOutputStatus::provider_protocol_error` when a paired output can be generated.
- If no pairable assistant message can be safely recorded, fail the turn before execution and log fatal provider protocol diagnostic. Do not continue to LLM with broken history.

## 9. ToolInvocation / ToolOutput model

### 9.1 ToolInvocation

`ToolInvocation` is the only object passed to executors.

Required fields:

```rust
pub struct ToolInvocation {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub batch_id: ToolBatchId,
    pub batch_index: usize,

    pub invocation_id: InvocationId,
    pub tool_call_id: ToolCallId,
    pub provider_tool_call_id: Option<String>,

    pub tool_name: ToolName,
    pub raw_provider_payload: serde_json::Value,
    pub raw_arguments: String,
    pub normalized_arguments: serde_json::Value,

    pub cancellation_token: CancellationToken,
    pub timeout: ToolTimeoutConfig,
    pub execution_context: ToolExecutionContext,

    pub provider_metadata: ProviderMetadata,
    pub model_metadata: ModelMetadata,

    pub working_directory: Option<PathBuf>,
    pub environment_metadata: Option<EnvironmentMetadata>,

    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
}
```

`ExecutionContext` must include:

- current cwd;
- sandbox/container/session reference;
- environment variables allowed for execution;
- artifact directory;
- progress/event sender;
- tracing span metadata;
- optional process manager handle;
- provider/tool correlation data.

### 9.2 ToolOutput

`ToolOutput` is the only object written as tool output.

Required fields:

```rust
pub struct ToolOutput {
    pub tool_call_id: ToolCallId,
    pub provider_tool_call_id: Option<String>,
    pub invocation_id: InvocationId,
    pub tool_name: ToolName,
    pub batch_index: usize,

    pub status: ToolOutputStatus,
    pub success: bool,

    pub exit_code: Option<i32>,
    pub stdout: OutputPreview,
    pub stderr: OutputPreview,
    pub structured_payload: Option<serde_json::Value>,

    pub error_message: Option<String>,
    pub timeout_reason: Option<TimeoutReason>,
    pub cancellation_reason: Option<CancellationReason>,
    pub cleanup_status: CleanupStatus,

    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub duration: Duration,

    pub truncation: OutputTruncationMetadata,
    pub artifacts: Vec<ArtifactRef>,
}
```

### 9.3 ToolOutputStatus

Required enum:

```rust
pub enum ToolOutputStatus {
    Success,
    Failure,
    Timeout,
    Cancelled,
    HungTimeout,
    ProcessCleanupFailed,
    InvalidArguments,
    UnknownTool,
    ProviderProtocolError,
    InternalRuntimeError,
}
```

Rules:

- Non-zero exit code is `Failure`, not `InternalRuntimeError`.
- Tool-specific application errors are `Failure`.
- Spawn failure is `Failure` with `error_message` and `cleanup_status = not_started`.
- Join error/panic is `InternalRuntimeError`.
- Cleanup failure after timeout can be encoded as `ProcessCleanupFailed` with `timeout_reason` preserved, or `Timeout` with cleanup error metadata. For model clarity, prefer `ProcessCleanupFailed` only when cleanup failure is the most important terminal state; otherwise keep `Timeout` and set `cleanup_status = failed`.
- Hung detected before hard timeout is `HungTimeout`.

### 9.4 OutputPreview

```rust
pub struct OutputPreview {
    pub text: Option<String>,
    pub head: Option<String>,
    pub tail: Option<String>,
    pub bytes_captured: usize,
    pub bytes_total_known: Option<usize>,
    pub truncated: bool,
    pub binary: bool,
    pub artifact: Option<ArtifactRef>,
}
```

If output is binary:

- do not inline raw bytes;
- write artifact;
- set `binary = true`;
- include type/size/checksum if available.

## 10. ToolRegistry / ToolExecutor

### 10.1 ToolExecutor trait

```rust
#[async_trait::async_trait]
pub trait ToolExecutor: Send + Sync {
    fn name(&self) -> ToolName;

    fn spec(&self) -> ToolSpec;

    async fn execute(
        &self,
        invocation: ToolInvocation,
    ) -> Result<ToolOutput, ToolRuntimeError>;
}
```

Executor contract:

- Must not write to conversation/history.
- Must not start next LLM turn.
- Must not swallow cancellation forever.
- Must accept cancellation token from `ToolInvocation`.
- Should return `ToolOutput` on normal success/failure.
- May return `ToolRuntimeError`; runtime must normalize it.
- Should use `ProcessManager` for shell/linux/process commands.
- Must not implement approval/policy restrictions in v1.

### 10.2 ToolRegistry

`ToolRegistry` requirements:

- Store tools in deterministic map by canonical `ToolName`.
- Detect duplicate registrations at startup and fail fast.
- Expose specs for opencode go function tools.
- Dispatch by exact name.
- Unknown name returns `ToolOutputStatus::UnknownTool` through normalizer.
- No `can_handle` linear scan in active v1 path.
- No fallback bridge.
- No provider-specific compatibility branching beyond opencode go tool spec encoding.

Recommended structure:

```rust
pub struct ToolRegistry {
    tools: BTreeMap<ToolName, Arc<dyn ToolExecutor>>,
}

impl ToolRegistry {
    pub fn get(&self, name: &ToolName) -> Option<Arc<dyn ToolExecutor>>;
    pub fn specs_for_opencode_go(&self) -> Vec<OpenCodeGoToolSpec>;
    pub fn register(&mut self, executor: Arc<dyn ToolExecutor>) -> Result<(), RegistryError>;
}
```

### 10.3 Tool specs

Tool specs must be generated from executor `spec()` and encoded as OpenAI-compatible function definitions for opencode go.

Rules:

- Names must be stable.
- JSON schemas must be explicit.
- No hidden policy filtering.
- No dynamic removal of “dangerous” tools.
- No approval annotations in v1 model-visible specs.

## 11. ToolCallRuntime

### 11.1 Responsibilities

`ToolCallRuntime` is the only active execution path for tools.

It must:

- receive a parsed `ToolCallBatch`;
- record assistant tool calls;
- build `ToolInvocation`s;
- spawn one task per call;
- execute tasks in parallel;
- enforce hard timeout;
- listen for user cancellation;
- run hung detector;
- trigger process cleanup;
- normalize every terminal state;
- return outputs in deterministic order;
- write exactly one output per call;
- not leave orphan tasks or orphan processes.

### 11.2 Internal state

```rust
pub struct ToolCallRuntime {
    registry: Arc<ToolRegistry>,
    process_manager: Arc<ProcessManager>,
    hung_detector: Arc<HungDetector>,
    normalizer: Arc<OutputNormalizer>,
    history: Arc<ToolHistoryWriter>,
    artifacts: Arc<ArtifactStore>,
    config: ToolRuntimeConfig,
}
```

### 11.3 No hidden fallback

Forbidden:

- `if new_runtime_failed { old_execute_tools(...) }`
- `feature_flag_use_legacy_tools`
- `tool_bridge` fallback
- fallback parsing unstructured assistant text into tool calls
- provider failover to non-opencode providers
- route failover for tools or model provider in v1

If runtime fails fatally before assistant tool calls are recorded, return turn error. If assistant tool calls are already recorded, runtime must produce outputs or fail the turn before next LLM call with a clear history-fatal error.

### 11.4 Deterministic order

Outputs must be written in original assistant `tool_calls[]` order, not completion order.

Rationale:

- deterministic history;
- easier debugging;
- stable tests;
- strict provider pairing.

### 11.5 Partial batch failure

Partial failure is normal:

- Some tools can succeed.
- Some can timeout.
- Some can fail args.
- Some can be unknown.

The batch completes when every call has a `ToolOutput`. Then all outputs are written. Next LLM turn continues unless history write or provider protocol fatal prevents valid continuation.

## 12. ProcessManager

### 12.1 Scope

`ProcessManager` is mandatory for shell/linux/CLI tools and any executor that spawns local/container commands.

It should also provide a contract for SSH/MCP process-like tools, even if actual remote cleanup is delegated to upstream service.

### 12.2 Required behavior

`ProcessManager` must:

- start processes in a controllable process group/session where possible;
- store process id and process group id;
- stream/drain stdout and stderr concurrently;
- cap model-facing output;
- write full output artifacts if needed;
- track last output/activity timestamp;
- terminate gracefully;
- force kill if terminate fails;
- cleanup process tree;
- wait/reap child processes;
- return `CleanupResult`;
- never leave processes after timeout/cancel/hung in tests.

### 12.3 Unix/Linux implementation

For local process execution:

1. Spawn command via `tokio::process::Command`.
2. Use `pre_exec` to create a new process group/session:
   - preferred: `setsid()`;
   - fallback: `setpgid(0, 0)`.
3. Capture `pid` and `pgid`.
4. Spawn stdout/stderr drain tasks immediately.
5. On normal exit, wait/reap and drain final output.
6. On timeout/cancel/hung:
   - mark terminal reason;
   - send SIGTERM to process group: `kill(-pgid, SIGTERM)`;
   - wait `terminate_grace_period`;
   - if still alive, send SIGKILL to process group: `kill(-pgid, SIGKILL)`;
   - wait `kill_grace_period`;
   - reap child;
   - collect final stdout/stderr tail;
   - return cleanup status.
7. If group kill fails, try individual pid kill.
8. If cleanup still fails, return `CleanupStatus::Failed` and include details in `ToolOutput`.

### 12.4 Container execution

For Docker/container sandbox execution:

- Do not rely on container-global `killall5 -9` as the primary cleanup mechanism.
- Wrap the command inside an invocation-scoped process group in the container.
- Store the wrapper pid/pgid.
- On timeout/cancel/hung, kill that group only.
- If Docker exec API cannot expose process group, run a small managed wrapper script that writes pid/pgid metadata to a file in a known runtime directory.
- As last resort, container reset can be used, but output status must reflect cleanup degradation.

If the current sandbox broker is kept in v1, extend the broker protocol with a managed exec request/response instead of adding a background job API.

Recommended broker contract:

```rust
ExecCommandManaged {
    scope,
    image_name,
    invocation_id,
    command,
    timeout_ms,
    stdout_cap_bytes,
    stderr_cap_bytes,
}

ExecResultManaged {
    invocation_id,
    status,
    exit_code,
    stdout_preview,
    stderr_preview,
    stdout_artifact,
    stderr_artifact,
    timed_out,
    cancelled,
    hung_detected,
    pid,
    pgid,
    cleanup_status,
}
```

The broker-side implementation must run the command through the same invocation-scoped wrapper and acknowledge cleanup status. The client must not infer cleanup success from socket disconnect or timeout.

### 12.5 SSH/MCP execution

For SSH commands:

- The runtime still creates `ToolInvocation` and timeout/cancel/hung guarantees.
- Upstream session reset is not enough unless it guarantees remote process termination.
- If upstream MCP cannot kill specific remote process trees, output must include `cleanup_status = best_effort_remote_cleanup` or `cleanup_status = failed_remote_cleanup`.
- For v1 YOLO parallelism, do not serialize all SSH calls with one mutex.
- Default v1 implementation: create one upstream SSH MCP session per invocation.
- A bounded session pool is allowed later as an optimization, but not required for v1.
- Reusing one shared upstream session is allowed only after an integration test proves true concurrent calls through that session.

### 12.6 Pseudo-code: process cleanup

```rust
async fn cleanup_process_tree(
    handle: ProcessHandle,
    reason: CleanupReason,
    cfg: CleanupConfig,
) -> CleanupResult {
    if handle.state().await == ProcessState::Exited {
        return CleanupResult::already_exited(handle.final_status().await);
    }

    let mut result = CleanupResult::started(reason, handle.pid, handle.pgid);

    match handle.pgid {
        Some(pgid) => {
            result.sigterm_sent = send_signal_to_group(pgid, Signal::Term).ok();
        }
        None => {
            result.sigterm_sent = send_signal_to_pid(handle.pid, Signal::Term).ok();
        }
    }

    if wait_for_exit(&handle, cfg.terminate_grace_period).await.is_ok() {
        result.outcome = CleanupOutcome::TerminatedGracefully;
        result.reaped = reap_child(&handle).await.ok();
        return result;
    }

    match handle.pgid {
        Some(pgid) => {
            result.sigkill_sent = send_signal_to_group(pgid, Signal::Kill).ok();
        }
        None => {
            result.sigkill_sent = send_signal_to_pid(handle.pid, Signal::Kill).ok();
        }
    }

    if wait_for_exit(&handle, cfg.kill_grace_period).await.is_ok() {
        result.outcome = CleanupOutcome::Killed;
        result.reaped = reap_child(&handle).await.ok();
        return result;
    }

    result.outcome = CleanupOutcome::Failed;
    result.error_message = Some("process did not exit after SIGKILL grace period".into());
    result
}
```

## 13. Timeout / Cancellation / Cleanup

### 13.1 Hard timeout

Hard timeout is per tool call and enforced by `ToolCallRuntime`, not by executor.

Default v1:

```text
per_tool_hard_timeout_secs = 300
```

Rationale: current `AGENT_TOOL_TIMEOUT_SECS` already uses 300 seconds, so v1 keeps the same external expectation while making it universal and cleanup-safe.

Rules:

- Hard timeout applies to shell/linux process tools.
- Hard timeout applies to async non-process tools.
- Hard timeout starts when invocation execution starts, not when LLM response arrives.
- On timeout, runtime cancels invocation token and triggers cleanup.
- Output status: `timeout` unless cleanup failure dominates.
- Timeout must never produce missing output.
- Agent-level timeout does not replace per-tool timeout.

### 13.2 Batch timeout

v1 does not require a separate batch timeout by default.

Reason:

- With per-tool timeout, a batch of N parallel tools should finish after roughly max(per-tool timeout + cleanup), not sum.
- Adding batch timeout can create ambiguous partial cleanup races.

Optional config:

```text
batch_timeout_secs = none
```

If configured, batch timeout must:

- cancel all incomplete tool invocations;
- cleanup their processes;
- return `cancelled` or `timeout` outputs for every incomplete call;
- not drop already completed outputs.

### 13.3 Total turn timeout

Existing agent-level timeout may remain as a final guard, but it must not be the primary tool timeout.

Conflict rules:

- Earliest timeout/cancellation signal wins for each invocation.
- If per-tool timeout fires first: output status `timeout`.
- If user cancellation fires first: output status `cancelled`.
- If hung detector fires first: output status `hung_timeout`.
- If agent-level timeout fires while tool outputs are pending, runtime must enter emergency normalization and write outputs if history already has assistant tool calls. If this cannot be completed safely, fail the turn before next provider request and log history-fatal diagnostic.

### 13.4 Cancellation

User cancellation must cancel all in-flight tool calls of the current turn.

Required behavior:

- Runtime owns a turn-level cancellation token.
- Each invocation gets child token.
- User cancellation cancels turn token.
- Each in-flight invocation receives cancellation.
- Process invocations trigger cleanup.
- Async non-process invocations are awaited for a short cancellation grace period, then their task is aborted.
- Every unfinished invocation returns `ToolOutputStatus::Cancelled`.
- Completed outputs are preserved.

Cancellation cases:

- During spawn: if process id is not available yet, output `cancelled` with `cleanup_status = not_started` or `spawn_interrupted`. If pid becomes available concurrently, cleanup it.
- During active process: cancel token, SIGTERM group, grace, SIGKILL, wait/reap, output `cancelled`.
- After process exits but before history write: preserve successful/failure output; cancellation must not replace a completed output.
- During cleanup: continue cleanup until bounded by kill grace. If cleanup fails, output `process_cleanup_failed` with cancellation reason.
- During join error: output `internal_runtime_error`; if process handle exists, cleanup first.

### 13.5 Cleanup

Cleanup is mandatory after timeout/cancel/hung.

Required statuses:

```rust
pub enum CleanupStatus {
    NotNeeded,
    NotStarted,
    AlreadyExited,
    TerminatedGracefully,
    KilledProcessGroup,
    KilledProcess,
    BestEffortRemoteCleanup,
    Failed,
}
```

If cleanup fails:

- still return tool output;
- include cleanup error message;
- include pid/pgid if known;
- log structured error;
- increment metric `tool.cleanup.failure`.

## 14. Hung detection

### 14.1 Purpose

Hung detection is a soft layer above hard timeout. It detects likely stuck invocations earlier than hard timeout, but hard timeout remains final protection.

Hung detection must never be the only termination mechanism.

### 14.2 Signals

Detector may use:

- no stdout/stderr for threshold duration;
- no progress events for threshold duration;
- process still alive but no output;
- no heartbeat from remote/broker transport;
- executor-specific heartbeat if available.

Absence of output alone is not always hung. Long commands like `pg_dump`, `terraform plan`, package builds, `grep`, backups and indexers can be quiet. Therefore detector must support tool/process metadata:

- `quiet_ok: bool`;
- `expected_no_output_secs`;
- `heartbeat_required: bool`;
- command started vs command made progress.

### 14.3 Defaults

Recommended v1 defaults:

```text
hung_detection_enabled = true
hung_startup_grace_secs = 30
hung_no_output_threshold_secs = 120
hung_no_progress_threshold_secs = 180
hung_cleanup_on_detection = true
```

Rules:

- If detector disabled, hard timeout still works.
- If detector fires, runtime cancels invocation and starts cleanup.
- Output status: `hung_timeout`.
- `timeout_reason` should include `hung_no_output` or `hung_no_progress`.
- If hard timeout fires during hung cleanup, output remains `hung_timeout` but cleanup metadata notes hard-timeout overlap.

### 14.4 Pseudo-code: hung detector

```rust
async fn wait_for_hung(&self, invocation: &ToolInvocation) -> HungSignal {
    let grace = invocation.timeout.hung_startup_grace;
    tokio::time::sleep(grace).await;

    loop {
        let last_activity = self.activity_tracker.last_activity(&invocation.invocation_id).await;
        let idle_for = Instant::now().saturating_duration_since(last_activity);

        if idle_for >= invocation.timeout.hung_no_output_threshold
            && self.process_manager.is_alive(&invocation.invocation_id).await
            && !invocation.execution_context.quiet_ok
        {
            return HungSignal::NoOutput { idle_for };
        }

        if self.heartbeat_required(invocation)
            && self.heartbeat_missing(invocation).await
        {
            return HungSignal::HeartbeatMissing;
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
```

## 15. Parallel execution model

### 15.1 Scope v1

- All tool calls from one batch are scheduled immediately.
- No per-tool restrictions.
- No per-tool `supports_parallel` gates.
- No shell/SSH serialization by default.
- No read/write locks for resources as policy.
- No DevOps-specific locks.
- No approval gates.

### 15.2 Optional global technical limit

Config may include:

```text
max_in_flight_tools = none
```

If set, it is a technical backpressure limit only. It must not be described as safety. If unset, runtime schedules the whole batch.

Recommended implementation:

- use optional `Semaphore` only when config is present;
- output ordering remains original batch order;
- waiting for semaphore does not count against per-tool execution timeout until invocation starts;
- cancellation while waiting returns `cancelled` output.

### 15.3 No resource-aware scheduling v1

Do not implement:

- “kubectl calls serialize by cluster”; 
- “git commands serialize by repository”; 
- “filesystem writes lock paths”; 
- “SSH commands lock host”; 
- “terraform locks workspace”; 
- “dangerous commands run alone”.

These can be future scheduler features, but v1 must not include them.

### 15.4 General-purpose examples

Runtime must handle:

- `kubectl get pods`, `kubectl get events`, `helm status` in parallel;
- `journalctl` and `systemctl status` in parallel;
- `terraform plan` while `kubectl` diagnostics run;
- parallel HTTP/search queries;
- parallel filesystem reads;
- parallel `rg`/`grep` over codebase;
- parallel package-manager metadata commands;
- parallel git status/log/diff commands;
- long-running build/test/indexing/export commands bounded by timeout.

## 16. Output handling and artifacts

### 16.1 Context budget

stdout/stderr must not be placed into model context without limits.

Default v1:

```text
max_captured_stdout_bytes = 65536
max_captured_stderr_bytes = 65536
output_head_bytes = 16384
output_tail_bytes = 32768
max_tool_output_content_bytes = 131072
```

Rules:

- If output fits budget, inline as `text`.
- If output exceeds budget, include head/tail and `truncated = true`.
- Full output should be stored as artifact/log.
- Binary output should not be inlined.
- Tool output must state truncation explicitly.
- stdout and stderr budgets are independent.
- Structured payloads are also subject to max content bytes.

### 16.2 ArtifactStore

If current Oxide artifact mechanism is insufficient, implement minimal store:

```text
.oxide/tool-artifacts/{session_id}/{turn_id}/{tool_call_id}/stdout.log
.oxide/tool-artifacts/{session_id}/{turn_id}/{tool_call_id}/stderr.log
.oxide/tool-artifacts/{session_id}/{turn_id}/{tool_call_id}/payload.json
```

ArtifactRef fields:

```rust
pub struct ArtifactRef {
    pub uri: String,
    pub local_path: PathBuf,
    pub user_download_uri: Option<String>,
    pub kind: ArtifactKind,
    pub bytes: u64,
    pub sha256: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}
```

Artifact reference rules:

- Model-facing output uses internal refs such as `artifact://session/{session_id}/turn/{turn_id}/{tool_call_id}/stdout.log`.
- Internal refs must be resolvable by Oxide Agent for follow-up tool calls and diagnostics.
- User-downloadable links/files are optional and must be created only through an explicit delivery/upload path.
- Do not make every artifact public by default.
- When a user-downloadable URI exists, include it separately as `user_download_uri`; keep the internal `uri` stable for agent use.

Retention defaults for v1:

- Artifacts: keep for 7 days.
- Tool logs: keep for 30 days.
- Soft storage cap: configurable, default 1 GiB per deployment or runtime data root.
- Cleanup runs best-effort on startup and after turns that create artifacts.
- Retention is a technical storage policy, not a tool safety/policy restriction.

If artifact write fails:

- output status stays as primary status if inline preview is available;
- `artifact_write_failed` is recorded in metadata;
- if artifact was required for binary/huge output and preview unavailable, output status `failure` or `internal_runtime_error` depending on source;
- log structured error.

### 16.3 Formatting for model

Model-facing `content` should be concise and structured. Recommended:

- JSON string for all tools.
- Include `status`, `success`, `duration_ms`, `exit_code`, `stdout`, `stderr`, `error_message`, `cleanup_status`, `artifacts`.
- Avoid dumping raw multiline text without metadata.

### 16.4 Binary output

Binary detection:

- inspect at least the first 64 KiB when available;
- binary if a NUL byte is present;
- binary if invalid UTF-8 bytes exceed 5% of inspected bytes;
- binary if non-whitespace control bytes exceed 2% of inspected bytes;
- file magic for archive/image/database dump when available.

Binary behavior:

- do not inline raw bytes;
- store artifact;
- include mime/type guess;
- include size/hash;
- include short note in output.
- For mixed logs below the binary threshold, a lossy UTF-8 preview is allowed, but raw undecoded bytes must never be inlined.

### 16.5 `pg_dump` scenario

`pg_dump` must not return dump data through stdout tool output by default.

Correct command pattern:

```bash
pg_dump "$DATABASE_URL" --format=custom --file /workspace/artifacts/pg_dump_2026-05-22.dump
```

Why stdout is wrong:

- database dumps can be huge;
- dump can be binary/custom format;
- dump can contain sensitive application data;
- model context cannot handle it;
- it blocks useful summary and can exhaust memory.

Correct success output:

```json
{
  "tool_call_id": "call_pg_dump_1",
  "tool_name": "execute_command",
  "status": "success",
  "success": true,
  "exit_code": 0,
  "duration_ms": 84231,
  "stdout": { "text": "", "truncated": false },
  "stderr": { "tail": "pg_dump: dumping contents of table ...", "truncated": true },
  "artifacts": [
    {
      "uri": "artifact://session/turn/call_pg_dump_1/pg_dump_2026-05-22.dump",
      "kind": "file",
      "bytes": 734003200
    }
  ]
}
```

Timeout behavior:

- per-tool hard timeout fires;
- invocation token cancelled;
- process group gets SIGTERM;
- after grace, SIGKILL if still alive;
- partial dump file remains artifact only if safe to reference with `partial = true`;
- output status: `timeout`;
- cleanup status included.

Hung behavior:

- no-output/no-progress detector fires before hard timeout;
- cleanup starts;
- output status: `hung_timeout`;
- include last stderr tail and artifact refs for partial files if present.

Cancellation behavior:

- user cancellation cancels invocation;
- process group cleanup runs;
- output status: `cancelled`;
- partial artifact marked partial.

Cleanup failure behavior:

```json
{
  "status": "process_cleanup_failed",
  "success": false,
  "timeout_reason": "per_tool_hard_timeout",
  "cleanup_status": "failed",
  "error_message": "process group 12345 did not exit after SIGKILL grace period"
}
```

Even cleanup failure must produce a paired tool output.

## 17. Conversation/history invariants

### 17.1 Required invariants

1. Every assistant tool call item is recorded before execution starts.
2. Every recorded assistant tool call gets exactly one tool output.
3. Tool output references the same provider-visible `tool_call_id` as the assistant tool call.
4. Output order is deterministic by original batch index.
5. Runtime errors never create missing tool outputs.
6. Unknown tool gets a tool output.
7. Invalid arguments get a tool output.
8. Timeout/hung/cancel get a tool output.
9. Duplicate/missing provider ids are handled before history write or become fatal before next provider request.
10. Next LLM turn never starts until all batch outputs are recorded.
11. History repair is not used as normal correctness mechanism.
12. No tool executor writes history directly.

### 17.2 History write sequence

```text
assistant(tool_calls=[call_1, call_2, call_3])
tool(tool_call_id=call_1, content=output_1)
tool(tool_call_id=call_2, content=output_2)
tool(tool_call_id=call_3, content=output_3)
```

### 17.3 Pseudo-code: history write invariant

```rust
async fn record_history_pair(
    &self,
    batch: &ToolCallBatch,
    outputs: &[ToolOutput],
) -> Result<(), HistoryError> {
    self.invariants.verify_batch_ids_unique(batch)?;
    self.invariants.verify_outputs_match_calls(batch, outputs)?;

    self.memory.add_message(batch.assistant_message.clone()).await?;

    for (call, output) in batch.calls.iter().zip(outputs.iter()) {
        debug_assert_eq!(call.wire_tool_call_id, output.tool_call_id);

        let content = self.provider_encoder
            .encode_tool_output_for_opencode_go(output)?;

        self.memory.add_message(Message::tool_with_correlation(
            call.wire_tool_call_id.clone(),
            call.tool_call_correlation.clone(),
            content,
        )).await?;
    }

    self.invariants.verify_history_pairing_after_write(&self.memory).await?;
    Ok(())
}
```

If `record_tool_output` fails after assistant tool calls are already written:

- do not call next LLM request;
- emit fatal runtime error;
- persist diagnostic snapshot if possible;
- do not let recovery silently drop the incomplete batch.

## 18. Failure modes

### 18.1 Unknown tool

Runtime behavior:

- Registry lookup fails.
- Executor is not called.
- Output status: `unknown_tool`.
- History: tool output with original `tool_call_id` and message listing available tool names or concise “unknown tool”.
- Agent turn continues after output is recorded.
- Log: warning with provider/model/session/turn/tool name.

### 18.2 Invalid JSON arguments

Runtime behavior:

- Argument parser fails before executor.
- Output status: `invalid_arguments`.
- History: output includes parse error and expected schema summary.
- Agent turn continues.
- Log: warning with raw args preview.

### 18.3 Provider returned malformed tool call

Runtime behavior:

- Provider parser detects malformed item.
- If item can be represented with repaired id, create protocol-error output.
- If item cannot be represented safely, fail before assistant tool-call history write.
- Output status: `provider_protocol_error` when pairable.
- Agent turn continues only with valid pairable history.
- Log: error.

### 18.4 Missing tool_call_id

Runtime behavior:

- Create deterministic synthetic id before assistant message write.
- Output status: `provider_protocol_error` unless integration proves executable synthetic ids are accepted.
- History: assistant call and tool output use same synthetic id.
- Agent turn continues with caution after output recorded.
- Log: error and metric.

### 18.5 Duplicate tool_call_id

Runtime behavior:

- First call keeps id.
- Later duplicates get synthetic ids before assistant message write.
- Duplicate calls output `provider_protocol_error` unless explicitly repaired by parser rules.
- History remains pairable.
- Log: error.

### 18.6 Executor panic

Runtime behavior:

- Spawned task join returns panic/join error.
- Runtime normalizes.
- Cleanup process handle if registered.
- Output status: `internal_runtime_error`.
- History: output contains concise panic/join message, not full backtrace in model context.
- Agent turn continues after output recorded.
- Log: error with backtrace/diagnostics.

### 18.7 Tokio join error

Same as executor panic:

- status `internal_runtime_error`;
- cleanup if process exists;
- paired output required.

### 18.8 Process spawn failed

Runtime behavior:

- ProcessManager returns spawn error.
- Output status: `failure`.
- Cleanup status: `not_started`.
- History: error message includes command and spawn error preview.
- Agent turn continues.
- Log: error.

### 18.9 Process exited non-zero

Runtime behavior:

- Normal terminal state.
- Output status: `failure`.
- Success false.
- Include exit code, stdout/stderr previews.
- Agent turn continues.
- Log: info/warn depending exit code.

### 18.10 stdout too large

Runtime behavior:

- Drain continues; model preview truncated.
- Artifact written for full stdout.
- Primary status remains success/failure/timeout/etc.
- Truncation metadata set.
- Agent turn continues.
- Log: metric `tool.output.stdout_truncated`.

### 18.11 stderr too large

Same as stdout too large.

### 18.12 Binary output

Runtime behavior:

- Binary not inlined.
- Artifact written.
- Output preview says binary output omitted.
- Primary status preserved.
- Agent turn continues.
- Log: metric.

### 18.13 Timeout

Runtime behavior:

- Per-tool timeout fires.
- Cancellation token cancelled.
- Process cleanup starts.
- Output status: `timeout`.
- History: paired output with cleanup status and output tails.
- Agent turn continues after all batch outputs recorded.
- Log: warning and metric.

### 18.14 Hung detected

Runtime behavior:

- HungDetector fires before hard timeout.
- Runtime cancels/cleans up invocation.
- Output status: `hung_timeout`.
- History: paired output with hung reason.
- Agent turn continues.
- Log: warning and metric.

### 18.15 Cancellation

Runtime behavior:

- Turn cancellation cancels all in-flight tools.
- Completed outputs are preserved.
- Incomplete outputs become `cancelled` after cleanup.
- History: every call gets one output.
- Agent turn normally stops after recording outputs, depending outer cancellation semantics. It must not send next LLM request after user cancellation unless explicitly resumed.
- Log: info.

### 18.16 Cleanup failed

Runtime behavior:

- Cleanup failure captured.
- Output status: `process_cleanup_failed` or primary status with failed cleanup metadata.
- History: paired output.
- Agent turn continues only after logging severe warning; if process is still running and dangerous to continue, outer session may be marked degraded, but no provider request should happen with missing output.
- Log: error and metric.

### 18.17 Artifact write failed

Runtime behavior:

- If inline preview is enough, primary status preserved and artifact failure metadata included.
- If output cannot be represented without artifact, status `internal_runtime_error` or `failure` depending source.
- History: paired output.
- Agent turn continues if output content remains protocol-valid.
- Log: error.

### 18.18 History write failed

Runtime behavior:

- This is fatal for the turn.
- Do not send next LLM request.
- If assistant call already recorded and output write failed, persist diagnostic snapshot.
- Output status cannot help if history write failed; runtime must stop.
- Log: fatal error.

### 18.19 Model/provider rejected tool output

Runtime behavior:

- Provider error on next LLM request.
- Do not route to other provider.
- Do not use history repair fallback silently.
- Log full provider error and encoded message previews.
- Integration tests should catch this for opencode go + DeepSeek V4 Flash.

### 18.20 Partial batch failure

Runtime behavior:

- All calls receive outputs.
- Success/failure mixed statuses allowed.
- Next LLM turn continues with complete batch history.
- Log per-call metrics and batch summary.

## 19. Legacy removal plan

### 19.1 Delete or remove from active path

Remove these from active v1 execution:

- `crates/oxide-agent-core/src/agent/tool_bridge.rs`
- `execute_tool_calls`
- `execute_single_tool_call`
- `execute_tool_with_timeout`
- `ToolExecutionContext`
- `ToolExecutionResult` bridge type
- `replay_initial_tool_call` in `agent/executor/execution.rs`
- `ResumeApproval` execution path for tool replay
- approval-pending string detection in `runner/tools.rs`
- SSH approval registry and pending approval replay in v1 path
- before-tool blocking and approval gates in `execute_tools`
- route failover paths not targeting opencode go + DeepSeek V4 Flash
- `call_llm_with_tools_legacy` active path
- structured/unstructured fallback parser that creates tool calls from assistant text outside provider tool_call protocol

### 19.2 Replace

Replace:

- `runner/tools.rs::execute_tools` → `ToolCallRuntime::execute_batch`
- `runner/tools.rs::execute_approved_tools` → runtime batch scheduler
- `runner/tools.rs::record_tool_execution_result` → `ToolHistoryWriter::record_tool_output`
- `agent/registry.rs::ToolRegistry` → deterministic executor registry
- `agent/provider.rs::ToolProvider` → `ToolExecutor`
- `sandbox/manager.rs::exec_command` process behavior → `ProcessManager`/container process manager with cleanup
- `providers/sandbox.rs::handle_execute_command` → executor returning structured `ToolOutput`
- `providers/ssh_mcp.rs` execution/cancel path → executor returning structured `ToolOutput`, no global serialization lock

### 19.3 Config flags to remove or ignore in v1

Remove from v1 active behavior:

- approval-related config for tool execution;
- per-tool policy allow/deny;
- provider compatibility switches for GLM/MiniMax/Gemini;
- legacy bridge feature flags;
- route failover config for this runtime;
- hooks that block or rewrite tool invocation.

Keep only passive observability hooks if they cannot change execution behavior.

### 19.4 Tests to rewrite

Rewrite tests that assume:

- sequential tool execution;
- approval pending strings;
- old bridge timeout behavior;
- provider fallback;
- history repair as normal operation;
- full stdout/stderr in model context;
- SSH global serialization.

Add tests listed in section 22.

### 19.5 Old assumptions no longer valid

- Tool output is not arbitrary string; it is encoded `ToolOutput`.
- Timeout is not provider-specific; runtime enforces it.
- Cancellation cannot just drop a future; process cleanup is required.
- Unknown tool is not fatal missing output; it is a tool output status.
- Large output is not safe to send to model.
- Approval/policy gates are not part of v1.
- History repair is not a substitute for runtime invariants.

## 20. Configuration

### 20.1 Minimal v1 config

```rust
pub struct ToolRuntimeConfig {
    pub per_tool_hard_timeout: Duration,          // default 300s
    pub cancellation_grace_period: Duration,      // default 2s
    pub terminate_grace_period: Duration,         // default 5s
    pub kill_grace_period: Duration,              // default 2s

    pub hung_detection_enabled: bool,             // default true
    pub hung_startup_grace: Duration,             // default 30s
    pub hung_no_output_threshold: Duration,       // default 120s
    pub hung_no_progress_threshold: Duration,     // default 180s

    pub max_captured_stdout_bytes: usize,         // default 65536
    pub max_captured_stderr_bytes: usize,         // default 65536
    pub output_head_bytes: usize,                 // default 16384
    pub output_tail_bytes: usize,                 // default 32768
    pub max_tool_output_content_bytes: usize,     // default 131072

    pub artifact_dir: PathBuf,                    // default .oxide/tool-artifacts
    pub log_dir: PathBuf,                         // default .oxide/tool-logs
    pub artifact_retention: Duration,             // default 7d
    pub log_retention: Duration,                  // default 30d
    pub storage_soft_cap_bytes: Option<u64>,      // default Some(1 GiB)

    pub max_in_flight_tools: Option<usize>,       // default None
}
```

### 20.2 Not allowed in v1 config

Do not add:

- GLM/MiniMax/Gemini provider compatibility config.
- Per-tool safety config.
- Approval config.
- Dangerous command classifier config.
- Resource-aware lock config.
- Shell/SSH serialization config as default behavior.

### 20.3 Timeout conflict config

If multiple timeout values exist:

- per-tool hard timeout is primary for tools;
- existing agent timeout remains outer guard;
- batch timeout defaults to none;
- when conflict occurs, earliest terminal signal wins.

## 21. Implementation plan

### Phase 1: Runtime types and config

Deliver:

- `agent/tool_runtime/config.rs`
- `invocation.rs`
- `output.rs`
- `normalizer.rs`
- `artifacts.rs`
- unit tests for status encoding, truncation metadata and provider content JSON.

Acceptance for phase:

- `ToolOutput` can encode all required statuses.
- `OutputNormalizer` can create valid output from every required failure mode.

### Phase 2: Provider parser/encoder for opencode go

Deliver:

- strict parser for chat-like opencode go `tool_calls[]`;
- parser handling string/object arguments;
- duplicate/missing id handling;
- output encoder for `{ role: "tool", tool_call_id, content }`;
- integration fixture for DeepSeek V4 Flash format.

Acceptance:

- `parallel_tool_calls: true` remains in request with tools.
- Strict paired history fixture passes.

### Phase 3: Registry and executor interface

Deliver:

- `ToolExecutor` trait;
- deterministic `ToolRegistry`;
- adapter-free port of key existing tools;
- startup duplicate-name detection.

Acceptance:

- Unknown tool returns `ToolOutputStatus::UnknownTool`.
- No active dependency on `ToolProvider::execute -> String` for v1 path.

### Phase 4: ToolCallRuntime batch scheduler

Deliver:

- `execute_batch`;
- task spawn/join handling;
- deterministic output ordering;
- cancellation propagation;
- invariant verification;
- history writer.

Acceptance:

- Batch of 10+ test tools runs in parallel.
- Hung task cannot block batch forever.

### Phase 5: ProcessManager

Deliver:

- local Unix process group/session implementation;
- stdout/stderr drain with caps;
- SIGTERM/SIGKILL cleanup;
- wait/reap;
- process lifecycle tests.

Acceptance:

- Infinite loop killed on timeout.
- Child process killed on timeout.
- Command ignoring SIGTERM killed via SIGKILL.
- No orphan process in tests.

### Phase 6: Sandbox and SSH tool ports

Deliver:

- sandbox `execute_command` through process/container manager;
- `read_file` output budget/binary detection;
- SSH executor with no global serialization lock;
- remote cleanup status metadata.

Acceptance:

- sandbox timeout kills process;
- SSH parallel test does not serialize by default unless upstream protocol physically cannot support it, in which case separate sessions are used.

### Phase 7: Remove legacy paths

Deliver:

- delete/disable `tool_bridge.rs` active usage;
- remove `replay_initial_tool_call` bridge path;
- remove approval gating from tool runtime;
- remove provider failover from v1 path;
- remove unstructured fallback tool parser;
- update tests.

Acceptance:

- Static grep confirms no active call to legacy bridge.
- No feature flag can re-enable old runtime.

### Phase 8: Integration tests

Deliver:

- opencode go + DeepSeek V4 Flash protocol tests;
- full turn tests with multiple parallel tool calls;
- timeout/cancel/hung/cleanup/history tests;
- artifact tests.

Acceptance:

- v1 acceptance criteria pass.

## 22. Test plan

### 22.1 Unit tests

Required:

- One successful tool call produces one success output.
- Multiple successful tool calls execute concurrently and output order is deterministic.
- Mixed success/failure batch produces N outputs for N calls.
- Unknown tool returns `unknown_tool` output.
- Invalid args returns `invalid_arguments` output.
- Timeout returns `timeout` output.
- Hung returns `hung_timeout` output.
- Cancellation returns `cancelled` output.
- Executor panic returns `internal_runtime_error` output.
- Tokio join error returns `internal_runtime_error` output.
- Process spawn failed returns `failure` output.
- Process exited non-zero returns `failure` output with exit code.
- stdout truncation metadata is correct.
- stderr truncation metadata is correct.
- Binary output is not inlined.
- Large artifact output writes artifact ref.
- Deterministic output ordering by batch index.
- Duplicate `tool_call_id` handling before history write.
- Missing `tool_call_id` handling before history write.
- Provider malformed tool call returns protocol output when pairable.
- History writer rejects missing output.
- History writer rejects duplicate output.
- Next LLM turn only starts after all tool outputs are recorded.
- No legacy fallback path is invoked.

### 22.2 Process integration tests

Required:

- `sleep 600` times out and returns output.
- `while true; do sleep 1; done` times out and is killed.
- Command producing huge stdout is truncated and artifacted.
- Command producing huge stderr is truncated and artifacted.
- Command spawning child process then timeout leaves no child alive.
- Command ignoring SIGTERM receives SIGKILL.
- Command exits non-zero and still returns output.
- Cancellation during active process kills process group.
- Cancellation during spawn returns paired output.
- Cleanup failure is represented in output.

### 22.3 Sandbox tests

Required:

- Docker/container command timeout kills invocation-specific process group.
- Container command huge output does not exhaust memory.
- `read_file` on large file returns head/tail/artifact.
- `read_file` on binary file does not inline bytes.
- Sandbox recreate does not create hidden serialization for normal commands in v1. If recreate remains special, document it as environment lifecycle operation outside normal tool batch execution.

### 22.4 SSH/MCP tests

Required:

- Parallel `ssh_exec` calls do not serialize through one mutex.
- SSH timeout returns paired output.
- SSH cancellation returns paired output.
- Remote cleanup success/failure is reflected.
- Upstream session crash returns `internal_runtime_error` output.

### 22.5 Provider integration tests

Required:

- opencode go request includes `parallel_tool_calls: true` when tools exist.
- Tool specs encode as chat-like function tools.
- DeepSeek V4 Flash response with multiple tool calls parses correctly.
- Tool output messages use role `tool`, exact `tool_call_id`, content string.
- Strict history with assistant tool call + tool output is accepted by provider fixture.
- Invalid/missing/duplicate tool ids handled as defined.
- No JSON response format is sent in tool-call request.

### 22.6 End-to-end turn tests

Required:

- One successful tool call.
- Batch with 10+ parallel tools.
- Batch with long-running and short-running tools together.
- Mixed success/failure/timeout/unknown tool batch.
- User cancellation while batch is running.
- Hung long-running command.
- `pg_dump`-like command writing to file/artifact.
- Parallel `grep`/`ripgrep` commands.
- Parallel Linux CLI commands.
- DevOps diagnostics: `kubectl get pods`, `kubectl get events`, `helm status` in one batch.

## 23. Acceptance criteria

v1 is accepted only when all criteria pass:

1. Old tool execution logic is removed from active path.
2. New runtime is the only tool execution path.
3. No fallback to `tool_bridge` exists.
4. No feature flag can route back to old runtime.
5. opencode go + DeepSeek V4 Flash is the only supported provider/model scope.
6. GLM/MiniMax/Gemini are not part of v1 tool runtime.
7. Batch tool calls execute in parallel.
8. Every tool call receives exactly one tool output.
9. Tool outputs preserve provider-visible `tool_call_id`.
10. Unknown tool returns paired output.
11. Invalid arguments return paired output.
12. Timeout returns paired output.
13. Cancellation returns paired output.
14. Hung detection returns paired output.
15. Process cleanup runs after timeout/cancel/hung.
16. No orphan processes remain in timeout/cancel tests.
17. stdout/stderr are bounded before model context.
18. Large outputs are truncated and artifacted.
19. Binary outputs are not inlined.
20. Output order is deterministic.
21. Next LLM turn never starts before all batch outputs are recorded.
22. History repair is not required for normal paired tool-call correctness.
23. Provider integration tests for opencode go + DeepSeek V4 Flash pass.
24. `pg_dump` scenario returns artifact refs, not dump stdout.
25. Observability emits per-tool spans, durations, statuses, timeout/cancel/hung/cleanup/truncation metrics.

## 24. Risks and mitigations

### 24.1 opencode go missing tool_call_id behavior

Risk:

- Provider may omit `tool_call_id`, and synthetic ids may or may not be accepted by opencode go / DeepSeek V4 Flash on next request.

Mitigation:

- Integration test exact behavior.
- Default v1 behavior: treat missing/duplicate provider ids as `provider_protocol_error`, not as successful tool execution.
- Insert a deterministic synthetic id into the local assistant message only when needed to keep local history pairable.
- If provider rejects synthetic id history, fail turn before next request and log protocol error.
- Executing the underlying tool after synthetic id repair is allowed only if live integration tests prove opencode go + DeepSeek V4 Flash accepts that history shape reliably.

### 24.2 SSH upstream concurrency

Risk:

- Current SSH MCP upstream may not support concurrent calls through one session.

Mitigation:

- Use one independent upstream SSH MCP session per invocation in v1.
- Do not use one global mutex in v1 path.
- Add a bounded session pool later only after basic correctness is proven.
- If upstream hard-limits concurrency, represent it as transport limitation with tests, not as policy serialization.

### 24.3 Docker exec cleanup limitations

Risk:

- Docker exec may not expose enough process metadata to kill one invocation-specific process tree.

Mitigation:

- Run commands through a managed wrapper that creates process group and writes metadata.
- Use invocation-scoped group kill.
- Container reset only as last resort with cleanup status metadata.

### 24.4 History write failure after assistant call recorded

Risk:

- Assistant tool call is recorded, but output write fails.

Mitigation:

- Treat as fatal turn error.
- Do not call provider again.
- Persist diagnostic snapshot.
- Add tests that simulate memory write failure.

### 24.5 Output artifact storage growth

Risk:

- Large outputs can fill disk.

Mitigation:

- Use per-session artifact directories.
- Default retention: 7 days for artifacts and 30 days for tool logs.
- Default soft cap: 1 GiB per deployment/runtime data root, configurable.
- Run best-effort cleanup on startup and after turns that create artifacts.
- Metrics and warnings for artifact bytes.
- Future cleanup service can make retention stricter, but v1 does not need a separate daemon.

### 24.6 Hidden legacy path remains

Risk:

- Some request type still calls `tool_bridge` or old provider path.

Mitigation:

- Static grep tests.
- Runtime assertion that all tool execution goes through `ToolCallRuntime`.
- Remove imports to old bridge.
- Delete old tests or rewrite them.

### 24.7 Cancellation aborts Rust task but not process

Risk:

- Aborted async task drops process handle without killing process tree.

Mitigation:

- ProcessManager registers handles before await points.
- Runtime cleanup by invocation id is available even if executor task aborts.
- Tests verify no orphan child processes.

### 24.8 Large output still reaches model

Risk:

- Executor returns structured payload with huge nested strings bypassing stdout/stderr cap.

Mitigation:

- OutputNormalizer applies global `max_tool_output_content_bytes` to final provider content.
- ArtifactStore handles large structured payloads too.
- Provider encoder refuses oversized content.

## 25. Resolved v1 decisions

The following decisions close the previously open questions for v1. Some still require validation tests, but the runtime behavior is defined now.

### 25.1 Missing or duplicate opencode go `tool_call_id`

Decision:

- Do not rely on locally synthesized `tool_call_id` as a normal successful path.
- Missing/empty provider id produces a deterministic synthetic local wire id before history write:
  ```text
  oxide_missing_tool_call_id_{turn_id}_{batch_index}
  ```
- Duplicate provider ids keep the first occurrence and rewrite later duplicates to:
  ```text
  oxide_duplicate_tool_call_id_{turn_id}_{batch_index}
  ```
- Repaired calls default to `ToolOutputStatus::ProviderProtocolError`.
- The underlying tool is not executed for repaired ids unless live integration tests prove opencode go + DeepSeek V4 Flash accepts synthetic-id paired history reliably.
- If the provider rejects repaired history, fail the turn before the next LLM request and persist a protocol diagnostic snapshot.

Validation:

- Add a live/provider fixture test for synthetic id history acceptance.
- Add unit tests for missing id, duplicate id and malformed call pairing.

### 25.2 SSH MCP concurrency

Decision:

- v1 uses one upstream SSH MCP session per invocation.
- Remove the one-global-mutex execution model from the active v1 path.
- Do not build a scheduler, lock model or resource-aware SSH pool in v1.
- A session pool can be added later as an optimization after correctness tests pass.
- Reusing one session concurrently is allowed only after an integration test proves the upstream MCP transport supports concurrent calls through one session.

Validation:

- Add a test with parallel `ssh_exec` calls proving they do not serialize through one shared mutex.
- Add timeout/cancel tests showing paired outputs and remote cleanup metadata.

### 25.3 Artifact and log retention

Decision:

- Store artifacts under per-session/per-turn/per-tool-call directories.
- Default retention:
  - `.oxide/tool-artifacts`: 7 days;
  - `.oxide/tool-logs`: 30 days.
- Default soft cap: 1 GiB per deployment/runtime data root, configurable.
- Cleanup is best-effort on startup and after turns that create artifacts.
- Retention is technical storage management and is not a tool safety/policy restriction.

Validation:

- Add tests for artifact path layout, expiry metadata and cleanup eligibility.
- Emit metrics/warnings for artifact bytes written and soft-cap pressure.

### 25.4 Artifact references and user downloads

Decision:

- Tool outputs always include internal artifact refs for agent/model follow-up:
  ```text
  artifact://session/{session_id}/turn/{turn_id}/{tool_call_id}/{name}
  ```
- Internal refs must resolve to local/storage paths in Oxide Agent.
- User-downloadable links/files are optional and must be created only through explicit delivery/upload paths such as file delivery or filehoster tools.
- Do not make all tool artifacts public by default.
- If a user-downloadable URI exists, include it separately from the internal artifact URI.

Validation:

- Add tests that large stdout/stderr create internal artifact refs.
- Add tests that user-downloadable refs are absent unless an explicit delivery/upload path created them.

### 25.5 Binary-output detection threshold

Decision:

- Inspect at least the first 64 KiB when available.
- Treat output as binary if any NUL byte is present.
- Treat output as binary if invalid UTF-8 bytes exceed 5% of inspected bytes.
- Treat output as binary if non-whitespace control bytes exceed 2% of inspected bytes.
- Use file magic for common archive/image/database dump formats when available.
- Mixed logs below those thresholds may use a lossy UTF-8 preview, but raw undecoded bytes are never inlined.

Validation:

- Add fixtures for UTF-8 logs, mixed-encoding logs, archives, database dumps, image bytes and logs containing isolated control characters.

### 25.6 Sandbox broker managed exec protocol

Decision:

- If the broker backend remains enabled in v1, add a managed exec request/response instead of adding a background job system.
- The request must include `invocation_id`, command, timeout and output caps.
- The response must include the same `invocation_id`, terminal status, exit code, output previews/artifact refs, pid/pgid when available and cleanup status.
- Broker-side execution must use an invocation-scoped process group/wrapper.
- Timeout/cancel/hung cleanup success must be explicitly acknowledged by the broker response.
- Socket disconnect, client-side timeout or broker error must not be interpreted as process cleanup success.

Validation:

- Add broker roundtrip tests for managed exec.
- Add timeout/cancel tests proving invocation-scoped process cleanup and no orphan processes.
