# PRD: Codex-style Compaction для Oxide-Agent

## 1. Background

Oxide-Agent сейчас имеет активную compaction pipeline, которая решает слишком много задач одновременно: считает бюджет, классифицирует hot memory, схлопывает retry/errors, дедуплицирует tool outputs, externalize/prune артефакты, пишет archive/payload ссылки, вызывает отдельный LLM summarizer, парсит структурированный JSON summary и затем rebuild/truncate hot context. Эта логика находится в `crates/oxide-agent-core/src/agent/compaction/*` и подключена из runner/executor слоя.

Главная проблема не в наличии summary как такового, а в том, что compaction реализована как отдельный многоступенчатый flow внутри agent execution. Это создаёт дублирование состояния, сложный blast radius и риск, что старый flow и новый flow будут одновременно менять одну и ту же историю.

Из Codex CLI нужно перенести не код один-в-один, а архитектурный принцип:

- compaction является runtime/session-level операцией, а не отдельным agent/sidecar flow;
- runtime смотрит на текущую историю и token budget;
- при необходимости запускается compact task;
- task получает compact summary;
- новая history строится детерминированно;
- история заменяется атомарно только после успешного результата;
- turn продолжается без потери состояния;
- old summary messages не попадают повторно в hot context как обычная история.

Для Oxide-Agent default backend должен быть provider-agnostic `LocalLlmSummary`: обычный LLM request с compact prompt и plain text summary. OpenAI `/responses/compact` не должен быть частью core architecture и не должен требоваться в первой версии.

## 2. Current State в Oxide-Agent

### Проверенные зоны репозитория

Проверены следующие зоны Oxide-Agent ветки `dev`:

- `crates/oxide-agent-core/src/agent/compaction/mod.rs`
- `crates/oxide-agent-core/src/agent/compaction/service.rs`
- `crates/oxide-agent-core/src/agent/compaction/types.rs`
- `crates/oxide-agent-core/src/agent/compaction/budget.rs`
- `crates/oxide-agent-core/src/agent/compaction/classifier.rs`
- `crates/oxide-agent-core/src/agent/compaction/externalize.rs`
- `crates/oxide-agent-core/src/agent/compaction/prune.rs`
- `crates/oxide-agent-core/src/agent/compaction/rebuild.rs`
- `crates/oxide-agent-core/src/agent/compaction/archive.rs`
- `crates/oxide-agent-core/src/agent/compaction/summarizer.rs`
- `crates/oxide-agent-core/src/agent/compaction/summary.rs`
- `crates/oxide-agent-core/src/agent/compaction/prompt.rs`
- `crates/oxide-agent-core/src/agent/runner/execution.rs`
- `crates/oxide-agent-core/src/agent/runner/types.rs`
- `crates/oxide-agent-core/src/agent/executor.rs`
- `crates/oxide-agent-core/src/agent/executor/config.rs`
- `crates/oxide-agent-core/src/agent/executor/compaction.rs`
- `crates/oxide-agent-core/src/agent/executor/execution.rs`
- `crates/oxide-agent-core/src/agent/context.rs`
- `crates/oxide-agent-core/src/agent/session.rs`
- `crates/oxide-agent-core/src/agent/memory.rs`
- `crates/oxide-agent-core/src/agent/recovery.rs`
- `crates/oxide-agent-core/src/agent/progress.rs`
- `crates/oxide-agent-core/src/config.rs`
- `crates/oxide-agent-core/src/storage/compaction.rs`
- `crates/oxide-agent-core/src/storage/r2_memory.rs`
- `crates/oxide-agent-transport-telegram/src/bot/progress_render.rs`
- `crates/oxide-agent-transport-telegram/src/bot/agent_transport.rs`
- `crates/oxide-agent-transport-web/src/web_transport.rs`
- `crates/oxide-agent-transport-web/src/session.rs`
- `.env.example`
- `README.md`

### Текущий compaction flow

Основной facade сейчас называется `CompactionService` и находится в `crates/oxide-agent-core/src/agent/compaction/service.rs`.

Ключевая функция старого flow:

- `CompactionService::prepare_for_run(&self, request: &CompactionRequest, agent: &mut dyn AgentContext) -> Result<CompactionOutcome>`

Она делает следующее:

- вызывает `estimate_request_budget` из `budget.rs`;
- решает, нужны ли deterministic stages через `should_apply_deterministic_stages`;
- вызывает `apply_deterministic_stages`;
- вызывает `summarize_and_rebuild`;
- повторно считает token budget;
- возвращает `CompactionOutcome`.

`apply_deterministic_stages` сейчас запускает цепочку:

- `classify_hot_memory` из `classifier.rs`;
- `collapse_error_retries` из `error_retry_collapse.rs`;
- `dedup_superseded_tool_results` из `dedup_superseded.rs`;
- `externalize_hot_memory` из `externalize.rs`;
- `prune_hot_memory` из `prune.rs`;
- `agent.memory_mut().replace_messages(...)`.

`summarize_and_rebuild` запускает:

- `CompactionSummarizer::summarize_if_needed(...)`, если summarizer установлен;
- deterministic fallback summary, если LLM summary недоступен;
- `persist_compacted_history_chunk(...)` из `archive.rs`;
- `truncate_to_working_set(...)` для `PostRun`;
- `rebuild_hot_context(...)` для остальных triggers.

Это означает, что текущая pipeline не просто summarization. Она одновременно:

- меняет retention классы;
- externalize большие payloads;
- prune старые сообщения;
- создаёт archive references;
- вставляет breadcrumb cards;
- вставляет structured summary;
- перестраивает active hot context;
- ремонтирует tool history косвенно через `AgentMemory::replace_messages`.

### Sidecar compaction agent / summarizer

Sidecar summarizer находится в `crates/oxide-agent-core/src/agent/compaction/summarizer.rs`.

Ключевые элементы:

- `CompactionSummarizer`
- `CompactionSummarizerConfig`
- `CompactionSummarizer::summarize_if_needed(...)`
- `CompactionSummarizer::call_llm(...)`
- `parse_summary_response(...)`
- `build_compaction_user_message(...)` из `prompt.rs`
- `compaction_system_prompt()` из `prompt.rs`

Сейчас summarizer просит LLM вернуть только JSON со структурой:

- `goal`
- `constraints`
- `decisions`
- `discoveries`
- `relevant_files_entities`
- `remaining_work`
- `risks`

Это старый формат. Для Codex-style compaction он не должен оставаться активным backend. Новый default path должен получать plain text handoff summary, а Oxide сам должен строить replacement history.

### Где compaction подключена к runner/execution

В `crates/oxide-agent-core/src/agent/runner/execution.rs` старый flow подключён через:

- `run_pre_llm_maintenance(...)`
- `run_iteration_compaction(...)`
- `run_compaction_checkpoint(...)`
- ветку retry при context overflow внутри `handle_llm_attempt_result(...)`
- `refresh_messages_from_memory(...)`

Текущий pre-LLM flow:

- `run_pre_llm_maintenance` сначала применяет hooks;
- затем, если есть manual request, вызывает `run_compaction_checkpoint(..., CompactionTrigger::Manual)`;
- иначе вызывает `run_iteration_compaction(ctx, state, iteration)`;
- `run_iteration_compaction` выбирает `PreRun` для первой итерации и `PreIteration` для последующих;
- при fresh session может пропустить compaction;
- при необходимости вызывает `run_compaction_checkpoint`.

Context overflow retry сейчас использует старую semantic label `Manual`:

- `handle_llm_attempt_result(...)` проверяет `llm_error_suggests_context_overflow(&error) && attempt == 1`;
- затем вызывает `run_compaction_checkpoint(ctx, state, CompactionTrigger::Manual)`;
- если outcome applied, retry продолжается.

Это нужно заменить: context overflow должен стать reason `ContextLimit` или phase `MidTurn`, а не manual compaction.

### Где compaction подключена к executor

В `crates/oxide-agent-core/src/agent/executor.rs` поле executor:

- `compaction_service: CompactionService`

В `crates/oxide-agent-core/src/agent/executor/config.rs` `AgentExecutor::new(...)` создаёт сервис так:

- берёт `settings.get_configured_compaction_model()`;
- берёт inherited agent routes и dedicated compaction routes;
- создаёт `CompactionService::default().with_summarizer(CompactionSummarizer::new(...))`.

В `crates/oxide-agent-core/src/agent/executor/types.rs` `RunnerContextServices` содержит:

- `compaction_service: &'a CompactionService`

В `PreparedExecution::build_runner_context(...)` сервис передаётся в `AgentRunnerContext::new_base(..., Some(services.compaction_service), ...)`.

В `crates/oxide-agent-core/src/agent/executor/compaction.rs` manual compaction реализована через:

- `AgentExecutor::compact_current_context(...)`
- `CompactionRequest::new(CompactionTrigger::Manual, ...)`
- `self.compaction_service.prepare_for_run(...)`
- events `CompactionStarted`, `PruningApplied`, `CompactionCompleted`, `CompactionFailed`

Новая архитектура должна заменить это на `CompactionController::manual_compact(...)` или аналогичный single entrypoint. Старый `CompactionService` не должен оставаться активным fallback.

### Current data model

`crates/oxide-agent-core/src/agent/compaction/types.rs` содержит current compaction data model:

- `AgentMessageKind`
- `RetentionClass`
- `CompactionTrigger`
- `CompactionPolicy`
- `HotContextLimits`
- `CompactionRequest`
- `BudgetState`
- `CompactionSummary`
- `BreadcrumbCard`
- `SummaryGenerationOutcome`
- `RebuildOutcome`
- `CompactionSnapshot`
- `CompactionOutcome`

Current `CompactionTrigger`:

- `PreRun`
- `PreIteration`
- `Manual`
- `PostRun`

Для новой архитектуры этого недостаточно. Нужно заменить или расширить reason/phase model:

- `PreTurn`
- `MidTurn`
- `Manual`
- `ContextLimit`
- `ModelDownshift`

Лучше разделить `reason` и `phase`, чтобы не смешивать “почему compact” и “где в runtime произошло”.

### Current memory and persistence

`crates/oxide-agent-core/src/agent/memory.rs` содержит `AgentMessage` с полями:

- `kind`
- `role`
- `content`
- `reasoning`
- `tool_call_id`
- `tool_call_correlation`
- `tool_calls`
- `tool_call_correlations`
- `externalized_payload`
- `pruned_artifact`
- `structured_summary`
- `archive_ref`
- `breadcrumb_card`

Существующие summary helpers:

- `AgentMessage::summary(...)`
- `AgentMessage::from_compaction_summary(...)`
- `AgentMessage::from_breadcrumb_card(...)`
- `AgentMessage::archive_reference_with_ref(...)`
- `format_compaction_summary(...)`, который использует prefix `[COMPACTION_SUMMARY]`
- `format_breadcrumb_card(...)`, который использует prefix `[BREADCRUMB_CARD]`

`AgentMemory::replace_messages(...)` пересчитывает token count и вызывает `repair_history_after_mutation("replace_messages")`. Это важная мина: новая `replace_compacted_history` должна либо использовать этот repair осознанно, либо валидировать replacement до вызова, чтобы не получить silent mutation после compaction.

`crates/oxide-agent-core/src/agent/session.rs` хранит `AgentMemory`, `AgentMemoryScope`, runtime context inbox, pending approvals/user input и persistence checkpoint. `AgentSession::persist_memory_checkpoint(...)` и `persist_memory_checkpoint_background(...)` сохраняют memory snapshots. Новая compaction должна заменить именно session memory и затем синхронизировать `ctx.messages` через runner.

`crates/oxide-agent-core/src/storage/r2_memory.rs` сохраняет и загружает `AgentMemory` JSON. Значит migration должна быть backward-compatible для старых `structured_summary`, `archive_ref`, `externalized_payload`, `pruned_artifact`.

### R2/object storage integration

Старая pipeline имеет два вида R2-related state:

- history archives через `ArchiveSink`, `ArchiveRef`, `persist_compacted_history_chunk(...)` в `compaction/archive.rs`;
- externalized tool payloads через `PayloadSink` и `ExternalizedPayloadRecord` в `compaction/externalize.rs`.

`crates/oxide-agent-core/src/storage/compaction.rs` реализует:

- `CompactionBlobBackend`
- `R2ArchiveSink`
- `R2PayloadSink`

В первой версии Codex-style compaction новый path не должен писать новые R2 archive/payload objects. Старые R2 references должны оставаться читаемыми и не должны теряться при migration.

### Events and progress system

`crates/oxide-agent-core/src/agent/progress.rs` содержит events:

- `AgentEvent::CompactionStarted { trigger }`
- `AgentEvent::PruningApplied { pruned_count, reclaimed_tokens }`
- `AgentEvent::CompactionCompleted { trigger, applied, externalized_count, pruned_count, reclaimed_tokens, archived_chunk_count, summary_updated }`
- `AgentEvent::CompactionFailed { trigger, error }`
- `AgentEvent::RepeatedCompactionWarning { kind, count }`
- `AgentEvent::HistoryRepairApplied { ... }`

`ProgressState` хранит `last_compaction_status`, `repeated_compaction_warning`, token snapshots и history repair status.

Telegram rendering находится в `crates/oxide-agent-transport-telegram/src/bot/progress_render.rs`. Оно показывает секцию `Context:` и использует `last_compaction_status`; тест `renders_compaction_status_and_warning` ожидает текст `Compaction: refreshed summary and rebuilt active context` и учитывает `PruningApplied`/old completed payload.

Web transport находится в `crates/oxide-agent-transport-web/src/web_transport.rs`. Функция `event_variant_name(...)` мапит current events в names:

- `compaction_started`
- `pruning_applied`
- `compaction_completed`
- `compaction_failed`
- `repeated_compaction_warning`
- `history_repair_applied`

После миграции `pruning_applied` должен быть удалён из active compaction story или оставлен только для backward event compatibility tests. Новый runtime должен emit `compaction_skipped` и richer metadata.

### Current config/env/docs

`crates/oxide-agent-core/src/config.rs` содержит настройки:

- `compaction_model_id`
- `compaction_model_provider`
- `compaction_model_max_output_tokens`, alias `compaction_model_max_tokens`
- `compaction_model_timeout_secs`
- `soft_warning_tokens`
- `hard_compaction_tokens`
- `get_configured_compaction_model()`
- `get_configured_compaction_model_routes(...)`
- `get_hot_context_limits()`
- `get_agent_internal_context_budget_tokens()`

`.env.example` описывает compaction model как “context compression” и говорит, что staged compaction pipeline fallback still works. Это нужно переписать: staged pipeline больше не должна быть active fallback.

`README.md` упоминает automatic compression, R2 context management и history repair for `tool_call_id` relationships. Эти docs нужно обновить под новую runtime-level compaction.

## 3. Codex CLI Reference

### Проверенные зоны Codex CLI

Проверены следующие зоны Codex CLI:

- `codex-rs/core/src/compact.rs`
- `codex-rs/core/src/tasks/compact.rs`
- `codex-rs/core/src/compact_remote.rs`
- `codex-rs/core/src/compact_remote_v2.rs`
- `codex-rs/core/src/session/turn.rs`
- `codex-rs/core/src/session/turn_context.rs`
- `codex-rs/core/src/client.rs`
- `codex-rs/core/templates/compact/prompt.md`
- `codex-rs/core/templates/compact/summary_prefix.md`
- `codex-rs/protocol/src/protocol.rs`
- `codex-rs/protocol/src/items.rs`
- compaction tests inside `compact_tests.rs` и inline tests in `compact_remote_v2.rs`

### Когда запускается compaction в Codex

Codex имеет несколько triggers:

- manual compact через `Op::Compact` и `CompactTask`;
- pre-sampling auto compaction до model request;
- mid-turn auto compaction после context limit, если нужен follow-up sampling;
- model downshift compaction при переключении на модель с меньшим context window.

В `codex-rs/core/src/session/turn.rs` pre-sampling compaction выполняется до skills/plugins injection и до записи нового user input. Это важно: compaction работает на session history, затем runtime добавляет свежий turn context.

Ключевые функции Codex:

- `run_pre_sampling_compact(...)`
- `auto_compact_token_status(...)`
- `maybe_run_previous_model_inline_compact(...)`
- `run_auto_compact(...)`
- `run_inline_auto_compact_task(...)` в `compact.rs`
- `run_compact_task(...)` для manual compact

### Как определяется token threshold

Codex считает active context token usage через session-level token accounting:

- `sess.get_total_token_usage().await`

Затем `auto_compact_token_status(...)` сравнивает usage с:

- model-specific auto compact token limit;
- configured `model_auto_compact_token_limit`;
- full model context window, если scope `BodyAfterPrefix`.

Codex поддерживает scope:

- `AutoCompactTokenLimitScope::Total`
- `AutoCompactTokenLimitScope::BodyAfterPrefix`

Для Oxide не нужно копировать этот механизм буквально. Нужно взять принцип: threshold logic lives in runtime/session layer, консервативно считает projected prompt size и запускает compaction до model request.

### Local compaction в Codex

Local compaction находится в `codex-rs/core/src/compact.rs`.

Ключевые элементы:

- `SUMMARIZATION_PROMPT = include_str!("../templates/compact/prompt.md")`
- `SUMMARY_PREFIX = include_str!("../templates/compact/summary_prefix.md")`
- `COMPACT_USER_MESSAGE_MAX_TOKENS: usize = 20_000`
- `run_inline_auto_compact_task(...)`
- `run_compact_task(...)`
- `run_compact_task_inner(...)`
- `run_compact_task_inner_impl(...)`
- `collect_user_messages(...)`
- `is_summary_message(...)`
- `build_compacted_history(...)`
- `insert_initial_context_before_last_real_user_or_summary(...)`
- `drain_to_completed(...)`

Local path строит compaction prompt, отправляет обычный model request, получает assistant summary, добавляет stable prefix и затем строит replacement history.

Важные принципы:

- source history клонируется до compaction;
- compaction task записывает marker `ContextCompactionItem::new()`;
- prompt добавляется как user input только для compact turn;
- если compact generation завершилась успешно, берётся последний assistant message compact turn;
- итоговый summary текст получает stable prefix;
- replacement history строится заново;
- old summary messages фильтруются по prefix;
- recent real user messages сохраняются с bounded token budget;
- история заменяется через `sess.replace_compacted_history(...)`;
- token usage пересчитывается через `sess.recompute_token_usage(...)`;
- при ошибке исходная история не должна быть повреждена.

Codex local compaction intentionally не переносит всю старую историю и не пытается сохранять arbitrary function-call/tool-call chains. Для Oxide это означает: replacement history должен быть явно построен так, чтобы provider chat invariants не сломались. Если active turn требует tool pair, Oxide должен сохранить валидную пару или отложить compaction до safe boundary.

### Remote compaction в Codex

Codex выбирает local vs remote через `should_use_remote_compact_task(provider: &ModelProviderInfo) -> bool`, который возвращает provider capability `supports_remote_compaction()`.

`codex-rs/core/src/tasks/compact.rs` выбирает:

- `compact_remote_v2`, если provider поддерживает remote и включена feature `RemoteCompactionV2`;
- `compact_remote`, если provider поддерживает remote;
- local `crate::compact::run_compact_task`, если remote не поддерживается.

`codex-rs/core/src/client.rs` содержит `RESPONSES_COMPACT_ENDPOINT = "/responses/compact"` и `compact_conversation_history(...)`. Это OpenAI Responses API-specific endpoint.

Для Oxide это reference-only. В первой версии нельзя включать `/responses/compact` в core architecture, потому что Oxide-Agent multi-provider:

- Anthropic;
- Gemini;
- OpenRouter;
- Ollama;
- Bedrock;
- Mistral;
- Groq;
- ZAI;
- MiniMax;
- обычные Chat Completions-compatible providers.

OpenAI remote compaction может быть только future optional optimization:

- не default path;
- не fallback для остальных провайдеров;
- включается только при explicit capability `supports_responses_compact = true`;
- доступно только для OpenAI Responses API route;
- opaque/encrypted remote compaction items нельзя хранить как provider-neutral `AgentMessage`;
- результат нельзя передавать Anthropic/Gemini/OpenRouter/Ollama/Bedrock;
- unsupported provider всегда использует `LocalLlmSummary`;
- первая версия PRD не требует реализации remote compaction.

### Как Codex строит compact prompt

`codex-rs/core/templates/compact/prompt.md` содержит handoff prompt: summary должен описать current progress, key decisions, context, constraints, preferences, remaining work, critical data. Это не JSON-contract; это compact handoff summary.

`codex-rs/core/templates/compact/summary_prefix.md` содержит stable prefix, который говорит следующей модели, что другой model instance произвёл summary предыдущего context. Codex использует этот prefix как machine-detectable marker.

Oxide должен использовать похожий принцип, но собственный tag, например:

```text
[OXIDE_COMPACTED_SUMMARY_V1]
```

Этот tag нужен не для красоты, а как invariant:

- находить compacted summary messages;
- не summary-of-summary;
- отличать runtime summary от обычных user/assistant messages;
- мигрировать старые `[COMPACTION_SUMMARY]` и `[BREADCRUMB_CARD]`;
- не допускать duplicate summaries в hot context.

### Как Codex сохраняет/заменяет history

Codex не мутирует историю по стадиям. Он строит replacement history и затем вызывает one-shot replacement:

- `sess.replace_compacted_history(new_history, reference_context_item, compacted_item).await`

Replacement history в local path состоит из:

- optional initial/canonical context;
- selected recent real user messages;
- one prefixed summary message.

После replacement Codex пересчитывает token usage и emits completed event/warning.

Для Oxide нужен аналог:

- `replace_compacted_history(...)` как единственная атомарная точка замены session memory;
- никакой параллельной `externalize/prune/rebuild` мутации;
- no-op при failure;
- refresh runner `ctx.messages` только после successful replacement.

### Как Codex избегает summary-of-summary

В local path Codex использует `is_summary_message(...)`, который проверяет stable `SUMMARY_PREFIX`. `collect_user_messages(...)` сохраняет только real user messages и фильтрует summary messages.

Для Oxide нужно фильтровать:

- новый `[OXIDE_COMPACTED_SUMMARY_V1]`;
- старый `[COMPACTION_SUMMARY]`;
- старый `[BREADCRUMB_CARD]`;
- `AgentMessageKind::Summary` со старым `structured_summary`;
- `AgentMessageKind::Breadcrumb`;
- `AgentMessageKind::ArchiveReference`, если он не нужен как active memory pointer.

Старый summary можно использовать как input signal для нового compaction prompt, но нельзя держать как обычную историю рядом с новым summary.

### Как Codex handles mid-turn context limit

В `codex-rs/core/src/session/turn.rs` после sampling Codex проверяет, достигнут ли token limit и нужен ли follow-up. Если `token_limit_reached && needs_follow_up`, он запускает auto compact с:

- `InitialContextInjection::BeforeLastUserMessage`
- `CompactionReason::ContextLimit`
- `CompactionPhase::MidTurn`

Это важно: mid-turn compact должен учитывать, что turn ещё не завершён. Для Oxide аналог нужен в tool loop/continuation path: если модель вернула tool calls или требуется follow-up sampling, но context уже unsafe, runtime делает compact и только потом продолжает.

### Tool calls / function calls

Codex remote paths фильтруют retained history и не сохраняют произвольные tool outputs как ordinary retained history. Oxide работает с chat-completion providers, где `tool_call_id` invariants очень строгие. Поэтому перенос принципа Codex должен быть адаптирован:

- нельзя оставлять tool result без соответствующего assistant tool call;
- нельзя удалять assistant tool call, если следующий provider request всё ещё содержит matching tool result;
- нельзя compact active open tool batch, если model ждёт results;
- mid-turn compact должен либо сохранить полные valid pairs, либо дождаться safe boundary;
- `agent/recovery.rs` должен быть использован как validator, но не как скрытая замена архитектурным гарантиям.

## 4. Problem Statement

Сейчас Oxide-Agent имеет слишком много стадий compaction, и каждая стадия может менять hot context независимо:

- budget estimation;
- classifier;
- error retry collapse;
- dedup superseded tool results;
- externalize;
- prune;
- archive;
- structured JSON summarizer;
- rebuild;
- truncate working set;
- runtime history repair.

Такой design создаёт несколько проблем.

Первая проблема: compaction не является single runtime operation. Она выглядит как отдельный mini-agent flow с собственным prompt, JSON parser, archive/payload side effects и rebuild policy.

Вторая проблема: высокий риск дублирования summary. Старые `Summary`, `Breadcrumb`, `ArchiveReference` и future compacted summary могут попасть в hot context одновременно. Это ведёт к summary-of-summary и progressive drift.

Третья проблема: externalize/prune/archive усложняют state model. Если новая compaction будет добавлена рядом, один turn сможет пройти через два разных механизма context reduction.

Четвёртая проблема: context overflow сейчас вызывает `CompactionTrigger::Manual`, хотя это не manual trigger. Это смешивает UX/manual intent и runtime recovery.

Пятая проблема: sidecar summarizer возвращает JSON schema, а новая цель требует plain text handoff summary и deterministic replacement history, построенную Oxide runtime.

Шестая проблема: provider-agnostic режим Oxide несовместим с vendor-specific `/responses/compact` как core mechanism.

Нужна единая runtime-level операция compaction, которая заменяет историю атомарно и выключает старую active pipeline.

## 5. Goals

Обязательные цели:

- удалить или вывести из active flow старую multi-stage compaction pipeline;
- заменить sidecar compaction agent/summarizer на Codex-style compact task;
- сделать один основной compaction path;
- запретить два active compaction implementations в одном turn;
- не дублировать compaction state;
- сохранить multi-provider модель Oxide-Agent;
- сделать `LocalLlmSummary` default и required backend;
- обеспечить работу без OpenAI `/responses/compact`;
- строить replacement history детерминированно;
- заменять session history атомарно;
- не ломать tool calls, tool results и `tool_call_id` invariants;
- не терять pinned/system/developer/tool/runtime context;
- не терять long-term memory/wiki memory/todos/active tool state;
- не ломать Telegram/Web progress UX;
- не ломать memory persistence/R2 compatibility;
- сделать compaction observable через events/logs/metrics;
- обеспечить safe rollback на уровне deployment/code version и persisted data compatibility.

## 6. Non-goals

Не цели первой версии:

- не строить второй memory subsystem;
- не сохранять старую multi-stage pipeline как активный fallback;
- не держать `budget/classifier/externalize/prune/rebuild/summarizer` как параллельный active path;
- не делать OpenAI `/responses/compact` обязательным для всех providers;
- не делать Oxide-Agent зависимым от OpenAI Responses API;
- не хранить OpenAI encrypted/opaque compaction items в общей provider-neutral истории;
- не передавать OpenAI remote compaction artifacts другим providers;
- не пытаться эмулировать `/responses/compact` для Anthropic/Gemini/OpenRouter/Ollama/Bedrock;
- не менять unrelated agent planning/tool execution behavior;
- не переписывать весь runner без необходимости;
- не заменять durable Wiki memory subsystem;
- не удалять старые R2 objects автоматически;
- не делать archive/payload externalization новым default behavior;
- не требовать JSON summary от compact LLM.

OpenAI `/responses/compact`, если когда-либо добавляется, должен быть отдельным optional adapter с explicit capability `supports_responses_compact = true`. Он не входит в first-version acceptance criteria.

## 7. Proposed Architecture

### Target principle

Compaction становится системной операцией runtime/session layer.

Runtime делает:

- смотрит на текущую `AgentMemory` и текущую prepared `ctx.messages`;
- считает token budget для selected route/model;
- решает, нужна ли compaction;
- запускает `CompactTask` через default `LocalLlmSummary` backend;
- получает plain text summary;
- строит replacement history;
- валидирует tool call/tool result invariants;
- атомарно заменяет session history;
- refresh runner messages;
- emits observable events;
- продолжает turn.

### Новые компоненты

Создать новую simplified compaction layer в `crates/oxide-agent-core/src/agent/compaction/`.

Предлагаемые новые файлы:

- `controller.rs`
- `task.rs`
- `history.rs`
- `metadata.rs`
- `local_llm_summary.rs`
- `prompt.rs`, заменив старый JSON prompt content
- `budget.rs`, упростив текущую budget logic вместо старого multi-stage budget policy
- `types.rs`, обновив types под reason/phase/result model

Не обязательно использовать эти имена буквально, но coding agent должен сохранить один active path и не оставлять старый service flow рядом.

### CompactionController

`CompactionController` должен заменить `CompactionService` как runtime entrypoint.

Предлагаемые методы:

- `maybe_compact_before_sampling(...)`
- `maybe_compact_mid_turn(...)`
- `manual_compact(...)`
- `model_downshift_compact(...)`
- `replace_compacted_history(...)`

`CompactionController` должен быть единственной точкой входа для auto/manual compaction. Runner/executor не должны напрямую вызывать classifier, prune, rebuild, externalize или summarizer.

### CompactTask

`CompactTask` должен представлять одну попытку compact.

Inputs:

- current session memory snapshot;
- canonical system prompt/context;
- current tools metadata, если это нужно для summary prompt;
- current provider/model route;
- reason;
- phase;
- token snapshot before;
- previous compacted summary, извлечённая из old messages, если есть;
- recent raw messages;
- active tool state, если turn mid-flight.

Output:

- plain text compact summary;
- `CompactedHistoryPlan` или equivalent intermediate struct;
- final replacement `Vec<AgentMessage>`;
- metrics: token_before, token_after, items_before, items_after, duration.

### LocalLlmSummary backend

Default backend: `LocalLlmSummary`.

Behavior:

- вызывает обычный provider-agnostic LLM request;
- использует compact prompt;
- не использует tools;
- не требует structured output;
- получает plain text summary;
- trims/validates output;
- возвращает summary text;
- при failure возвращает error и не мутирует history.

Backend selection:

- если configured `COMPACTION_MODEL_*` есть, использовать configured compaction route как model для summary generation;
- если dedicated compaction route отсутствует, использовать active agent route или first available configured route;
- если route не поддерживает обычную text generation, compaction skipped/failed с observable error;
- не использовать `/responses/compact` в first version.

Важно: выделенный compaction model сам по себе не является “sidecar compaction agent”. Это просто model route для `CompactTask`, без multi-stage pipeline, JSON schema, archive/prune/rebuild side effects.

### Token threshold logic

Threshold logic должен жить в controller/budget layer, а не в classifier/prune pipeline.

Источник данных:

- `AgentMemory::token_count()`;
- system prompt length;
- tool schema tokens;
- loaded skill tokens;
- reserved output tokens;
- hard reserve;
- current/selected route context window.

Существующий `estimate_request_budget(...)` можно временно использовать как reference, но его output должен быть упрощён до вопроса: “нужно ли compact до следующего request”. Не должно быть states `ShouldPrune` и `ShouldCompact` как разные actions. Новый action один: compact or skip.

Рекомендуемая политика:

- conservative projected total = hot memory + system prompt + tools + skills + reserved output + hard reserve;
- compact threshold default: 85% от effective context window;
- hard preflight threshold: 95% от context window;
- model downshift threshold: если текущая history fit old route, но projected total не fit new route, compact before sampling;
- context overflow from provider: force `ContextLimit` compact if turn can continue.

Config compatibility:

- `soft_warning_tokens` может остаться только для warning/observability;
- `hard_compaction_tokens` должен быть mapped to new compact threshold или deprecated;
- лучше добавить explicit percent/window-based config позднее, но first version может использовать existing values as shim.

### Compact prompt location

`crates/oxide-agent-core/src/agent/compaction/prompt.rs` должен перестать быть JSON-only prompt builder.

Новый prompt должен быть ближе к Codex:

- “Ты создаёшь compact handoff summary для продолжения той же agent session.”
- “Не отвечай пользователю.”
- “Сохрани цель, текущий прогресс, решения, constraints, user preferences, files/entities, pending actions, active tool context, remaining work, risks.”
- “Не суммаризируй старую summary как отдельный факт; используй previous compacted summary только как source signal.”
- “Не выдумывай состояние.”
- “Вывод plain text, concise but complete.”

Prompt builder должен принимать:

- previous compacted summary text, если есть;
- compactable historical messages;
- recent raw messages, если они нужны для context;
- pinned context/todos/active tool state;
- current task.

### build_compacted_history

Создать `build_compacted_history(...)` в `compaction/history.rs` или аналогичном модуле.

Он должен строить replacement history из:

- canonical pinned context, который должен остаться model-visible;
- one compacted summary message with stable tag;
- latest real user messages within token budget;
- latest assistant messages only if required for continuity;
- valid tool call/tool result pairs, если они обязательны для продолжения turn;
- active open tool state only if safe and valid;
- todos/pinned memory references, если они model-visible through `AgentMemory`;
- old archive/payload references only if still needed to explain existing placeholders.

Он должен фильтровать:

- old compacted summary messages;
- old `[COMPACTION_SUMMARY]` messages;
- old `[BREADCRUMB_CARD]` messages;
- old `AgentMessageKind::Breadcrumb`;
- old duplicate `AgentMessageKind::ArchiveReference`, если они не active references;
- orphaned tool results;
- tool results whose tool calls are dropped;
- summary-of-summary content as normal history.

Function contract:

- input history не мутируется;
- output содержит не больше одного current compacted summary;
- output проходит tool pairing validator;
- output token estimate ниже target budget;
- output deterministic for same inputs;
- errors return plan failure without modifying session.

### replace_compacted_history

Создать atomic replacement method.

Варианты placement:

- `AgentMemory::replace_compacted_history(...)` в `memory.rs`;
- или `CompactionController::replace_compacted_history(...)`, который вызывает `AgentMemory::replace_messages(...)` и затем проверяет repair outcome.

Обязательные semantics:

- исходная history clone/snapshot до compaction;
- replacement применяется только после successful local summary и successful history build validation;
- при failure source history remains untouched;
- после replacement token count пересчитан;
- если `repair_history_after_mutation` что-то меняет, это логируется и отражается в compaction result;
- runner `ctx.messages` refresh только после success;
- memory checkpoint persistence запускается после success, не до.

### Runner integration

`crates/oxide-agent-core/src/agent/runner/execution.rs` должен перестать вызывать `run_iteration_compaction` старого типа.

Новый wiring:

- `run_pre_llm_maintenance(...)` вызывает hooks, затем `maybe_compact_before_sampling(...)`;
- manual flag из `RunState` вызывает `manual_compact(...)`, но не старый service;
- context overflow branch вызывает `maybe_compact_mid_turn(... reason ContextLimit ...)` или force compact method;
- after model response branch вызывает `maybe_compact_mid_turn(...)` только если turn требует continuation/follow-up sampling;
- route failover/model downshift branch вызывает `model_downshift_compact(...)`, если selected next route has smaller context window and current prompt no longer fits;
- после successful compaction вызывается `refresh_messages_from_memory(ctx)`.

### Executor integration

В `crates/oxide-agent-core/src/agent/executor.rs` заменить поле:

- old: `compaction_service: CompactionService`
- new: `compaction_controller: CompactionController`

В `crates/oxide-agent-core/src/agent/executor/config.rs` заменить создание `CompactionService::default().with_summarizer(...)` на создание controller с `LocalLlmSummary` backend.

В `crates/oxide-agent-core/src/agent/executor/types.rs` заменить:

- old `RunnerContextServices { compaction_service: &'a CompactionService }`
- new `RunnerContextServices { compaction_controller: &'a CompactionController }`

В `crates/oxide-agent-core/src/agent/executor/compaction.rs` заменить `compact_current_context(...)` на manual controller call. События должны быть new-style, без `PruningApplied`.

## 8. Data Model

### Summary message format

New compacted summary message должен иметь stable machine-detectable prefix.

Recommended visible content:

```text
[OXIDE_COMPACTED_SUMMARY_V1]
generation: <number>
reason: <PreTurn | MidTurn | Manual | ContextLimit | ModelDownshift>
phase: <PreSampling | MidTurn | Manual | ModelSwitch>
provider: <provider>
route: <model>
token_before: <number>
token_after: <number>
created_at: <RFC3339>

<plain text handoff summary>
```

`[OXIDE_COMPACTED_SUMMARY_V1]` должен быть the only authoritative new summary prefix.

### Metadata

Добавить structured metadata к `AgentMessage` или к new summary wrapper. Recommended minimal struct:

- `generation`
- `reason`
- `phase`
- `token_before`
- `token_after`
- `history_items_before`
- `history_items_after`
- `provider`
- `route`
- `backend`, например `local_llm_summary`
- `timestamp`
- `source_summary_prefix_version`, например `OXIDE_COMPACTED_SUMMARY_V1`
- `previous_summary_detected`
- `old_archive_refs_preserved`
- `repair_applied`

Если менять `AgentMessage` schema, использовать `#[serde(default)]` для backward compatibility.

### AgentMessage kind/role

Существующий `AgentMessageKind::Summary` можно сохранить как message kind для нового compacted summary. Но content должен использовать новый prefix/tag.

Роль summary message нужно выбрать с учётом existing conversion:

- existing `AgentRunner::convert_memory_to_messages(...)` мапит `MessageRole::System` в `system`;
- current `AgentMessage::from_compaction_summary(...)` уже создаёт summary-like system message;
- если providers плохо переносят mid-history `system`, conversion/provider layer должен нормализовать это безопасно.

Open Question: нужно проверить provider-specific message normalization перед финальным выбором role. Если mid-history system messages вызывают ошибки у некоторых providers, использовать role `user` для compacted summary или provider conversion shim. Это должно быть решено coding agent до implementation.

### Old summaries

Старые summary formats должны быть migration shims, не active logic:

- `[COMPACTION_SUMMARY]`
- `[BREADCRUMB_CARD]`
- `AgentMessageKind::Summary` with `structured_summary`
- `AgentMessageKind::Breadcrumb`
- `AgentMessageKind::ArchiveReference`
- `structured_summary` JSON fields from `summary.rs`

Migration behavior:

- при новом compact run old summaries are detected;
- old summary text can be fed into prompt as previous context;
- old summary messages are not copied as regular hot history;
- replacement history contains exactly one `[OXIDE_COMPACTED_SUMMARY_V1]` message;
- old persisted sessions are migrated lazily on next successful compaction;
- no destructive migration is required for first version.

### Old archived summaries and R2 objects

Old R2 archives/payloads must not be silently deleted.

Rules:

- new default compaction does not create new archive chunks;
- existing `ArchiveRef` and `ExternalizedPayload` fields remain deserializable;
- if a hot message references an externalized payload, builder must preserve the reference or preserve a readable placeholder;
- no dangling references should be introduced;
- object retention/deletion policy remains out of scope for first version;
- docs must explain that old archive objects remain historical compatibility data.

### Pinned memory, todos, active tool state

Pinned/critical state must not be encoded only in the summary.

Preserve explicitly:

- `AgentMessageKind::TopicAgentsMd`
- `AgentMessageKind::UserTask`, especially latest task/current task;
- `AgentMessageKind::RuntimeContext`, if pending/current;
- `AgentMessageKind::SkillContext`, if still active;
- `AgentMessageKind::ApprovalReplay`, if pending SSH approval requires it;
- `AgentMessageKind::InfraStatus`, if current topic infra state is active;
- `AgentMemory.todos` and `todos_arc` state;
- pending user input / pending approval state from `AgentSession`;
- memory behavior runtime drafts, if they affect ongoing turn;
- Wiki memory durable context is not replaced by compaction.

## 9. Runtime Flow

### Pre-turn / before sampling

Target call site: `crates/oxide-agent-core/src/agent/runner/execution.rs`.

Replace old behavior:

- remove `run_iteration_compaction(...)` as active flow;
- remove old `PreRun`/`PreIteration` staging as compaction architecture;
- keep hooks before sampling if they are unrelated to compaction.

New flow:

- apply hooks;
- compute token snapshot for active route;
- if manual compaction request exists, call `manual_compact(...)`;
- else call `maybe_compact_before_sampling(...)`;
- if skipped, emit `compaction_skipped` only when observability requires it, not every healthy turn unless configured;
- if completed, refresh `ctx.messages` from memory;
- continue normal LLM sampling.

### Mid-turn context limit

Target call sites:

- `handle_llm_attempt_result(...)` in `runner/execution.rs`;
- post-response continuation/tool-loop decision point in `runner/execution.rs`.

Old behavior:

- context overflow can call `run_compaction_checkpoint(..., CompactionTrigger::Manual)`.

New behavior:

- context overflow uses reason `ContextLimit`, phase `MidTurn`;
- compaction is allowed only if source history is in a safe state;
- if there is an active tool batch with unmatched calls/results, controller must preserve full valid pairs or return not-safe-to-compact;
- if compact succeeds, refresh messages and retry once;
- if compact fails, original history remains untouched and the original LLM error is surfaced or failover continues according to existing policy.

### Continuation/tool loop compaction

After model response, compact mid-turn only if:

- token threshold/context limit reached;
- and turn needs continuation;
- examples: tool loop, pending action, continuation response, follow-up sampling.

Do not compact mid-turn after final answer if no follow-up sampling is needed. That can be deferred to next pre-turn compact.

### Manual compaction

Target call site:

- `AgentExecutor::compact_current_context(...)` in `executor/compaction.rs`.

New behavior:

- create current task/system/tools context as before;
- call `CompactionController::manual_compact(...)`;
- emit `compaction_started` with reason `Manual`;
- if success, emit completed and persist memory checkpoint;
- if failure, no history mutation;
- no prune/archive/externalization side effects;
- no `PruningApplied` active event.

### Model downshift compaction

Target call site:

- route selection/failover logic inside `call_llm_with_tools_with_failover(...)` in `runner/execution.rs`;
- helper `current_model_route(...)` in `executor/types.rs` may be useful for route metadata.

New behavior:

- when selected next route has smaller `context_window_tokens` than previous active route;
- and projected prompt/history does not fit new route;
- run `model_downshift_compact(...)` before sampling on the smaller route;
- reason `ModelDownshift`;
- phase `PreSampling` or `ModelSwitch`;
- summary generation may use old/previous route if current route cannot fit compaction prompt, otherwise active route.

Do not treat model downshift as ordinary context overflow. It is a distinct reason and should be observable.

### Compaction failure

Failure cases:

- LLM summary request fails;
- LLM summary times out;
- summary text empty/invalid;
- replacement history cannot fit target budget;
- tool pair validation fails;
- cancellation occurs;
- provider route unsupported;
- persistence checkpoint fails after replacement.

Required behavior:

- if failure occurs before replacement, source history remains untouched;
- emit `compaction_failed` with reason, phase, provider, route, failure reason;
- do not run old pipeline fallback;
- for pre-turn auto compact, continue only if prompt still fits safe budget; otherwise surface context error;
- for manual compact, return error to caller;
- for mid-turn context overflow, do not hide the provider context error unless another failover path safely handles it.

## 10. Migration Plan

### Step 1: Audit and freeze old active flow

Create an inventory issue or migration note listing all old compaction entrypoints:

- `CompactionService::prepare_for_run`
- `apply_deterministic_stages`
- `summarize_and_rebuild`
- `CompactionSummarizer::summarize_if_needed`
- `run_iteration_compaction`
- `run_compaction_checkpoint`
- `AgentExecutor::compact_current_context`
- context overflow branch that uses `CompactionTrigger::Manual`
- progress events with pruning/archive/externalized counts
- `.env.example` staged pipeline docs

Add explicit comments or guards that only one compaction implementation may be active.

### Step 2: Introduce new abstraction behind a switch

Introduce `CompactionController` and `LocalLlmSummary` behind a temporary migration flag.

Rules for flag:

- the flag chooses old or new path, never both;
- while flag is off, old tests still pass;
- while flag is on, old active flow must not run;
- flag is temporary and must be removed after rollout.

Possible flag name:

- `OXIDE_CODEX_STYLE_COMPACTION=1`

This flag is for migration only. It must not become a permanent fallback story.

### Step 3: Implement history builder and metadata

Add:

- `is_compacted_summary_message(...)`
- `extract_previous_compacted_summary(...)`
- `filter_old_summaries(...)`
- `build_compacted_history(...)`
- `validate_tool_pairs(...)`
- `replace_compacted_history(...)`

Add tests before wiring into runner.

### Step 4: Wire pre-turn path

Replace `run_iteration_compaction(...)` calls with `maybe_compact_before_sampling(...)`.

Keep old path disabled when new flag is on.

### Step 5: Wire manual path

Replace `AgentExecutor::compact_current_context(...)` internals to call controller.

Update manual progress events and tests.

### Step 6: Wire context overflow and mid-turn path

Replace context overflow `CompactionTrigger::Manual` retry with:

- `reason = ContextLimit`
- `phase = MidTurn`
- force compact if safe
- retry only after successful replacement

Add post-response continuation check:

- if follow-up sampling is needed and threshold exceeded, compact before next sampling.

### Step 7: Wire model downshift path

Add route context window comparison during failover/model switch.

Run `model_downshift_compact(...)` before smaller-route sampling when projected prompt exceeds smaller route.

### Step 8: Disable old active flow

When new path passes unit tests and runner regression tests:

- stop constructing `CompactionService` in executor;
- stop passing `CompactionService` to runner context;
- stop using old `CompactionSummarizer` as active backend;
- stop emitting `PruningApplied` from compaction;
- stop writing new compaction archive/payload objects.

### Step 9: Persistence migration

Add lazy migration behavior:

- old summary messages are detected on next compaction;
- old summaries are not copied to replacement history;
- old archive/payload references are preserved if referenced by retained messages;
- old fields remain serde-compatible;
- no immediate R2 deletion.

### Step 10: Update tests

Rewrite tests that assert old architecture, not behavior.

Keep behavior tests:

- context overflow retry;
- history repair;
- tool call pairing;
- long conversation continuation;
- progress rendering.

Remove or rewrite tests that assert:

- `externalized_count`;
- `pruned_count`;
- archive chunk creation as part of compaction;
- JSON summary parser behavior;
- rebuild/truncate working set internals.

### Step 11: Update docs

Update:

- `.env.example`
- `README.md`
- any docs mentioning staged compaction, sidecar summarizer, archive/payload pipeline, automatic compression.

Docs must say:

- default compaction is provider-agnostic local LLM summary;
- no OpenAI Responses API dependency;
- old archived payloads are historical compatibility artifacts;
- only one active compaction path exists.

### Step 12: Default on and remove migration switch

After regression pass:

- enable new path by default;
- remove old active wiring;
- remove temporary migration flag;
- keep only serde/data compatibility shims.

## 11. Deletion Plan

### Must remove from active flow

The following components must not remain active once Codex-style compaction is enabled by default.

`crates/oxide-agent-core/src/agent/compaction/service.rs`:

- remove or replace `CompactionService` as active facade;
- remove active use of `prepare_for_run(...)`;
- remove active use of `apply_deterministic_stages(...)`;
- remove active use of `summarize_and_rebuild(...)`;
- if file remains temporarily, it must be clearly marked compatibility-only and not wired into runner.

`crates/oxide-agent-core/src/agent/compaction/classifier.rs`:

- remove `classify_hot_memory` from active compaction path;
- do not classify retention as a precondition for compaction;
- if any constants are useful for recent window selection, move them into new history builder with new tests.

`crates/oxide-agent-core/src/agent/compaction/externalize.rs`:

- remove `externalize_hot_memory` from active compaction path;
- remove active `PayloadSink` writes during compaction;
- keep deserialization/read compatibility for old `ExternalizedPayload` references.

`crates/oxide-agent-core/src/agent/compaction/prune.rs`:

- remove `prune_hot_memory` from active compaction path;
- remove active `PrunedArtifact` creation during compaction;
- do not require latest summary boundary to prune because pruning is no longer a stage.

`crates/oxide-agent-core/src/agent/compaction/rebuild.rs`:

- remove active `rebuild_hot_context(...)`;
- remove active `truncate_to_working_set(...)`;
- move only useful tool-pair validation/removal logic to new history validation or `agent/recovery.rs` if needed;
- do not keep rebuild as fallback.

`crates/oxide-agent-core/src/agent/compaction/summarizer.rs`:

- remove `CompactionSummarizer` as active backend;
- remove JSON-only LLM summary contract;
- replace with `LocalLlmSummary` compact task backend;
- do not keep old summarizer as fallback.

`crates/oxide-agent-core/src/agent/compaction/summary.rs`:

- remove active structured summary parser/formatter;
- keep only migration extraction helpers if old sessions still contain `structured_summary`;
- do not produce new structured summary JSON in compaction.

`crates/oxide-agent-core/src/agent/compaction/archive.rs`:

- remove active `persist_compacted_history_chunk(...)` calls from compaction;
- keep `ArchiveRef` compatibility only if old persisted messages use it;
- no new archive chunk should be created by default compaction.

`crates/oxide-agent-core/src/agent/compaction/dedup_superseded.rs` and `error_retry_collapse.rs`:

- remove from active compaction path;
- if retry collapse/dedup is still valuable, it should be a separate explicit cleanup feature, not part of compaction;
- do not run it automatically during compact.

`crates/oxide-agent-core/src/agent/runner/execution.rs`:

- remove active `run_iteration_compaction(...)`;
- replace `run_compaction_checkpoint(...)` with controller calls or remove entirely;
- replace context overflow `Manual` trigger with `ContextLimit`/`MidTurn` controller path.

`crates/oxide-agent-core/src/agent/executor/config.rs`:

- remove construction of `CompactionService::default().with_summarizer(...)`;
- construct `CompactionController` with `LocalLlmSummary` backend.

`crates/oxide-agent-core/src/agent/executor/types.rs`:

- replace `RunnerContextServices { compaction_service }` with controller.

`crates/oxide-agent-core/src/agent/executor/compaction.rs`:

- replace manual compaction body;
- remove manual pruning event.

`crates/oxide-agent-core/src/agent/progress.rs`:

- remove active dependence on `PruningApplied` for compaction status;
- update `CompactionCompleted` payload to new fields;
- add `CompactionSkipped` or equivalent.

Telegram/Web tests:

- update tests that assert old status text and old counts.

### Temporary compatibility shims

Keep temporarily for persisted data compatibility, not active flow:

- `AgentMessageKind::Summary`, because new compacted summary can reuse this kind;
- `AgentMessageKind::Breadcrumb`, only for old sessions;
- `AgentMessageKind::ArchiveReference`, only for old sessions;
- `structured_summary` field in `AgentMessage`, serde default, old sessions only;
- `archive_ref` field in `AgentMessage`, old sessions only;
- `externalized_payload` and `pruned_artifact`, old sessions only;
- `[COMPACTION_SUMMARY]` parser/detector, migration only;
- `[BREADCRUMB_CARD]` parser/detector, migration only;
- `R2ArchiveSink`/`R2PayloadSink` types if other code still references old objects;
- config parsing for old compaction envs until docs/users migrate.

Compatibility shims must have a removal condition:

- after one release with lazy migration;
- after persisted sessions have been compacted to `[OXIDE_COMPACTED_SUMMARY_V1]` or old fields are proven unused;
- after R2 old object retention policy is documented.

### Config/env deprecation

Deprecate old semantics, not necessarily all variable names.

Keep or repurpose:

- `COMPACTION_MODEL_ID`
- `COMPACTION_MODEL_PROVIDER`
- `COMPACTION_MODEL_MAX_OUTPUT_TOKENS`
- `COMPACTION_MODEL_TIMEOUT_SECS`

These can configure `LocalLlmSummary` route. Docs must say they no longer configure a structured JSON sidecar summarizer.

Deprecate or remap:

- `SOFT_WARNING_TOKENS`, if used only for old staged budget status;
- `HARD_COMPACTION_TOKENS`, if it implies old hard trigger rather than model-window threshold;
- any docs that mention deterministic staged compaction fallback.

Open Question: exact env variable names in deployment docs beyond `.env.example` must be searched in docs/CI manifests during implementation.

## 12. Blast Radius

### Runner/execution loop

Files impacted:

- `crates/oxide-agent-core/src/agent/runner/execution.rs`
- `crates/oxide-agent-core/src/agent/runner/types.rs`

Impact:

- pre-LLM maintenance changes;
- manual compaction trigger changes;
- context overflow retry changes;
- mid-turn continuation changes;
- route failover/model downshift logic changes;
- `ctx.messages` refresh timing changes.

Mitigation:

- one controller call before sampling;
- one forced controller call for context limit;
- no old checkpoint calls;
- tests for long session, overflow retry and tool-heavy turns.

### Model routing

Files impacted:

- `runner/execution.rs`
- `executor/types.rs`
- `config.rs`

Impact:

- compaction must know selected route context window;
- model downshift compaction must run before smaller route request;
- dedicated compaction model route may differ from active route.

Mitigation:

- carry provider/model/context_window into compaction request;
- conservative threshold;
- use active route fallback only for normal text generation;
- remote OpenAI compact not part of default route logic.

### Provider abstraction

Files impacted:

- `llm` provider call sites through `LlmClient`
- `compaction/local_llm_summary.rs`

Impact:

- compaction backend must use ordinary text generation;
- some providers may not support system messages mid-history;
- tool schemas must not be sent to compact backend unless intentionally included as text context.

Mitigation:

- no tools in compact LLM request;
- plain text output;
- provider-neutral `Message` shape;
- tests for at least mock chat provider and route failover provider.

### Token counting

Files impacted:

- `compaction/budget.rs`
- `agent/memory.rs`
- `agent/progress.rs`

Impact:

- existing `cl100k_base` approximation may mismatch Anthropic/Gemini/etc.;
- compaction may trigger too late if estimates are optimistic;
- model-specific context window may be missing or zero.

Mitigation:

- conservative reserve;
- lower default threshold;
- fall back to configured internal context budget;
- emit token_before/token_after and skipped reasons;
- tests for downshift and threshold.

### History persistence

Files impacted:

- `agent/memory.rs`
- `agent/session.rs`
- `storage/r2_memory.rs`

Impact:

- new summary metadata changes serialized memory;
- old sessions contain old summary/archive/payload fields;
- replacement history may trigger runtime repair.

Mitigation:

- serde defaults;
- lazy migration;
- no destructive migration;
- repair outcome logged;
- persistence tests with old JSON fixture.

### R2/object storage

Files impacted:

- `storage/compaction.rs`
- `compaction/archive.rs`
- `compaction/externalize.rs`
- `storage/r2_memory.rs`

Impact:

- old pipeline created archive and payload objects;
- new pipeline should not create new ones;
- old references may still appear in memory messages.

Mitigation:

- keep old refs deserializable;
- preserve referenced placeholders;
- no auto-delete;
- docs explain old R2 objects are compatibility data.

### Long-term memory / Wiki memory

Files impacted:

- `agent/executor/execution.rs`
- `agent/wiki_memory/*`
- `agent/memory_behavior.rs`

Impact:

- background wiki memory writer uses recent transcript from `AgentMemory`;
- compaction may reduce transcript detail;
- explicit remember behavior must not be lost.

Mitigation:

- compaction does not replace Wiki memory;
- preserve current task, recent real user messages and final response path;
- keep memory behavior runtime outside compaction history mutation;
- test completed run still flushes wiki memory drafts.

### Pinned context

Files impacted:

- `agent/memory.rs`
- `agent/session.rs`
- `agent/prompt/*`
- `agent/skills/*`

Impact:

- AGENTS.md, runtime context, skill context, approval replay and infra status could be dropped accidentally.

Mitigation:

- `build_compacted_history` has explicit pinned context preservation;
- canonical system prompt remains outside history and is passed through runner as today;
- tests for pinned live context and todos.

### Tool call/tool result pairing

Files impacted:

- `agent/recovery.rs`
- `agent/memory.rs`
- `runner/execution.rs`
- `runner/tools.rs`
- `llm::Message`

Impact:

- invalid tool pairs cause provider errors;
- active tool loop can be broken by compaction;
- `AgentMemory::replace_messages` can repair silently.

Mitigation:

- validate before replacement;
- preserve active valid pairs;
- compact only at safe boundary or with full pairs;
- emit history repair metrics;
- tests for orphan tool results and active tool batch.

### History repair

Files impacted:

- `agent/recovery.rs`
- `agent/memory.rs`
- `agent/progress.rs`

Impact:

- current repair may hide builder mistakes.

Mitigation:

- builder should pass validation before replacement;
- repair should be final safety net;
- if repair changes replacement, emit `HistoryRepairApplied` and compaction metric;
- test replacement does not rely on repair for normal cases.

### Telegram UX

Files impacted:

- `crates/oxide-agent-transport-telegram/src/bot/progress_render.rs`
- `crates/oxide-agent-transport-telegram/src/bot/agent_transport.rs`

Impact:

- current UI expects old compaction status text and repeated compaction warnings;
- pruning/externalized/archive counts no longer exist.

Mitigation:

- update context section to show “Compaction: compacted history” with token_before/token_after or reclaimed estimate;
- keep repeated warning if repeated compactions happen;
- update tests such as `renders_compaction_status_and_warning`.

### Web UX

Files impacted:

- `crates/oxide-agent-transport-web/src/web_transport.rs`
- `crates/oxide-agent-transport-web/src/session.rs`

Impact:

- event timeline currently maps old event variants;
- SSE clients may expect `pruning_applied`.

Mitigation:

- update event_variant_name for new `compaction_skipped` and new payloads;
- remove active `pruning_applied` from new compaction story;
- keep old event name only if enum remains for compatibility, not emitted by new compaction.

### Progress events

Files impacted:

- `agent/progress.rs`

Impact:

- event payloads change;
- `ProgressState::update(...)` must render new status.

Mitigation:

- introduce `CompactionReason` and `CompactionPhase`;
- include token and item counts;
- keep status concise;
- tests for completed/skipped/failed/manual.

### Tests/e2e

Impact:

- old tests around pruned/externalized/archive counts will fail;
- tests expecting old summary JSON parser will fail;
- overflow retry test currently asserts `summary_payload().is_some()`.

Mitigation:

- rewrite around externally visible behavior: session continues, summary tag exists once, tool pairs valid, no duplicate summary;
- add old-session migration fixtures.

### Observability/logging

Impact:

- current logs mention prune/externalize/archive;
- new logs need reason/phase/backend/provider.

Mitigation:

- structured logs at start/complete/fail/skip;
- metrics counters by reason/backend/provider;
- no sensitive summary content in metrics.

### Config/env

Impact:

- existing users may have compaction model env vars;
- docs mention old staged pipeline.

Mitigation:

- keep route variables but change semantics;
- deprecate staged pipeline docs;
- add migration note.

### Old production sessions

Impact:

- old sessions contain old summary, archive refs, externalized/pruned artifacts;
- new builder must not treat them as ordinary messages.

Mitigation:

- lazy migration on next compact;
- compatibility shims;
- fixture tests;
- no silent deletion of R2 objects.

## 13. Mines / Risk Register

### Мина 1: OpenAI remote compaction нельзя сделать mandatory

Риск: Oxide-Agent станет зависимым от OpenAI Responses API и потеряет multi-provider behavior.

Разминирование:

- first version implements only `LocalLlmSummary`;
- `/responses/compact` не входит в core architecture;
- future remote adapter only with `supports_responses_compact = true`;
- remote result не хранится в provider-neutral history;
- unsupported providers always use local compaction.

### Мина 2: summary-of-summary

Риск: old summary messages попадут в prompt/history как обычные messages и будут суммаризироваться снова.

Разминирование:

- stable new prefix `[OXIDE_COMPACTED_SUMMARY_V1]`;
- detector for old `[COMPACTION_SUMMARY]`;
- detector for old `[BREADCRUMB_CARD]`;
- metadata-based summary detection;
- replacement history contains exactly one current compacted summary;
- repeated compaction tests.

### Мина 3: tool_call_id invariants

Риск: provider rejects request due orphaned tool result or missing tool call.

Разминирование:

- `build_compacted_history` validates pairs before replacement;
- preserve active full pairs if turn requires continuation;
- do not compact open tool batch unless all required results are present or safely represented;
- use `agent/recovery.rs` as validator/safety net;
- test orphan result removal and active tool pair preservation.

### Мина 4: mid-turn context limit

Риск: compaction after partial turn loses current action, breaks follow-up sampling or replays wrong context.

Разминирование:

- mid-turn compaction only when follow-up sampling required;
- inject canonical context and current task;
- preserve active tool pairs/pending state;
- refresh `ctx.messages` immediately after replacement;
- retry only after successful replacement.

### Мина 5: потеря pinned/system/developer/tool context

Риск: AGENTS.md, runtime context, tool policy or approvals disappear from compacted history.

Разминирование:

- define canonical context source: `system_prompt`, tools, skill registry, `AgentSession`, pinned `AgentMessageKind`s;
- builder explicitly preserves pinned kinds;
- summary is not the only carrier of active state;
- tests for pinned live context/todos/approval replay.

### Мина 6: old R2 archived payloads

Риск: old externalized/archive references become dangling or invisible.

Разминирование:

- do not delete old R2 objects;
- keep old fields deserializable;
- preserve references in retained messages;
- if old archive refs are not retained, ensure summary carries needed context;
- document retention policy.

### Мина 7: token counting mismatch

Риск: Oxide underestimates tokens for providers other than OpenAI tokenizer.

Разминирование:

- conservative threshold;
- hard reserve;
- provider context window from route config;
- compact earlier, not later;
- metrics compare token_before/token_after and provider-reported usage when available.

### Мина 8: compaction failure

Риск: failed compact corrupts history or loses messages.

Разминирование:

- source history snapshot before summary call;
- replacement only after success;
- no old fallback mutation;
- failure emits event;
- manual failure returns error;
- auto failure no-ops unless context is already impossible.

### Мина 9: regression drift

Риск: repeated compaction progressively degrades memory.

Разминирование:

- previous summary used as source signal, not retained as normal message;
- recent real user messages preserved;
- one summary only;
- repeated compaction tests with stable facts;
- metrics for generation count.

### Мина 10: дублирование старого и нового flow

Риск: old `CompactionService` and new `CompactionController` both fire in one turn.

Разминирование:

- one selected path by wiring;
- migration flag chooses exactly one path;
- old path removed after rollout;
- tests assert no old archive/prune/summarizer events under new path;
- code review checklist rejects dual-path fallback.

## 14. Testing Plan

### Unit tests for history builder

Add tests for `build_compacted_history(...)`:

- inserts exactly one `[OXIDE_COMPACTED_SUMMARY_V1]`;
- filters previous `[OXIDE_COMPACTED_SUMMARY_V1]`;
- filters old `[COMPACTION_SUMMARY]`;
- filters old `[BREADCRUMB_CARD]`;
- preserves latest real user messages within token budget;
- truncates or drops oldest real user messages deterministically;
- preserves pinned `TopicAgentsMd`, latest `UserTask`, current `RuntimeContext`, active `SkillContext`;
- preserves valid tool call/tool result pairs required for continuation;
- removes orphan tool result or fails validation before replacement;
- does not include duplicate summaries after repeated compaction;
- handles empty summary by failing or using explicit safe placeholder, not silent garbage.

### Unit tests for local compaction backend

Add tests for `LocalLlmSummary`:

- success returns plain text summary;
- timeout/failure returns error;
- empty LLM output rejected or normalized explicitly;
- no tools are sent to compact LLM request;
- configured compaction route used when present;
- active route used when no dedicated route;
- output is not parsed as JSON.

### Unit tests for replacement

Add tests for `replace_compacted_history(...)`:

- success replaces memory and recalculates token count;
- failure before replacement leaves memory unchanged;
- validation failure leaves memory unchanged;
- repair outcome is surfaced if `AgentMemory::replace_messages` changes anything;
- memory checkpoint is requested only after success.

### Threshold tests

Add tests for:

- pre-sampling trigger when projected tokens exceed threshold;
- no trigger under healthy budget;
- conservative threshold with missing context window;
- context overflow forces compact attempt;
- model downshift triggers when new route window is smaller;
- model downshift skipped when history already fits smaller route.

### Runner tests

Rewrite existing tests in `runner/execution.rs`:

- `run_retries_after_context_overflow_with_manual_compaction` should become context-limit compaction test;
- assert reason `ContextLimit`, phase `MidTurn`;
- assert new summary tag exists;
- assert old manual trigger is not used for overflow;
- assert no old archive/prune/externalization counts are emitted;
- assert `refresh_messages_from_memory` happens after replacement;
- assert long conversation continues.

Add tool-heavy runner tests:

- model returns tool calls, tools execute, context near limit, follow-up sampling works after mid-turn compaction;
- unmatched tool result is not sent;
- active approval replay is preserved;
- history repair event only fires when genuinely needed.

### Migration tests

Add persisted memory fixtures with:

- old `structured_summary`;
- old `[COMPACTION_SUMMARY]` content;
- old `[BREADCRUMB_CARD]` content;
- old `ArchiveReference`;
- old `externalized_payload`;
- old `pruned_artifact`;
- valid tool pairs.

Expected:

- lazy compaction produces one new summary;
- old summary messages not retained as normal history;
- old refs remain deserializable;
- no panic on load;
- no R2 object deletion.

### Transport/progress tests

Update Telegram tests in `progress_render.rs`:

- render new compaction status;
- no requirement for `PruningApplied`;
- show token reduction or compacted item count;
- repeated compaction warning still renders if event exists.

Update Web tests:

- event names include `compaction_started`, `compaction_completed`, `compaction_failed`, `compaction_skipped`;
- old `pruning_applied` not emitted by new compaction path;
- SSE/event log still records progress.

### E2E tests

Add or update e2e/regression tests:

- long conversation over threshold continues;
- repeated compaction does not duplicate summary;
- tool-heavy conversation continues;
- context overflow retry succeeds after compact;
- multi-provider mock route works without OpenAI remote endpoint;
- route downshift to smaller window compacts before sampling;
- old session JSON loads and compacts;
- R2/persistence compatibility with old archive refs;
- manual compaction produces new summary and no old prune/archive side effects.

## 15. Observability

### Events

Update `AgentEvent` in `crates/oxide-agent-core/src/agent/progress.rs` to support:

- `compaction_started`
- `compaction_completed`
- `compaction_failed`
- `compaction_skipped`

Recommended event fields:

- `reason`: `PreTurn`, `MidTurn`, `Manual`, `ContextLimit`, `ModelDownshift`
- `phase`: `PreSampling`, `MidTurn`, `Manual`, `ModelSwitch`
- `backend`: `LocalLlmSummary`
- `provider`
- `route`
- `token_before`
- `token_after`
- `history_items_before`
- `history_items_after`
- `duration_ms`
- `generation`
- `failure_reason`
- `skipped_reason`
- `repair_applied`

### Logs

Structured logs:

- start: reason, phase, provider, route, token_before, items_before;
- compact LLM call: backend, timeout, route;
- completed: token_after, items_after, reduction, duration, generation;
- failed: failure category, no source history mutation;
- skipped: threshold not reached, unsafe active tool state, no provider route, cancelled;
- migration: old summary detected, old archive refs preserved.

Do not log full summary content by default. Summary text may contain sensitive user/task data.

### Metrics

Suggested counters/gauges:

- `compaction_attempt_total`
- `compaction_completed_total`
- `compaction_failed_total`
- `compaction_skipped_total`
- `compaction_duration_ms`
- `compaction_token_before`
- `compaction_token_after`
- `compaction_reduction_tokens`
- `compaction_history_items_before`
- `compaction_history_items_after`
- `compaction_generation`
- `compaction_failure_by_reason`
- `compaction_backend_total`

Labels:

- reason;
- phase;
- backend;
- provider;
- route;
- success/failure.

## 16. Rollout / Rollback

### Rollout

Phase 0: Inventory and tests.

- Add tests that capture desired new behavior before deleting old flow.
- Add fixture tests for old sessions.

Phase 1: New code introduced but not default.

- Add `CompactionController`, `LocalLlmSummary`, `build_compacted_history`.
- Wire behind temporary migration flag that selects exactly one compaction path.
- Do not run old and new paths together.

Phase 2: New path in test/staging.

- Enable new path in CI/regression environment.
- Assert no old prune/externalize/archive/summarizer events.
- Verify multi-provider mock tests without OpenAI remote endpoint.

Phase 3: New path default.

- Replace executor/runner wiring.
- Stop constructing `CompactionService` in `AgentExecutor::new`.
- Stop passing old service to `AgentRunnerContext`.
- Docs updated.

Phase 4: Remove old active modules.

- Delete or mark compatibility-only old modules.
- Remove temporary flag after regression window.
- Keep only serde/persistence shims.

### Rollback

Rollback is deployment/code-version rollback plus data compatibility, not dual active compaction fallback.

Rollback requirements:

- old sessions can still be loaded because new metadata uses serde defaults and old fields remain compatible;
- new `[OXIDE_COMPACTED_SUMMARY_V1]` summary should be acceptable as a normal `Summary` message if old code sees it, or rollback window should include a reader shim;
- no destructive R2 deletion occurs;
- source history is only replaced after successful compact, so failed rollout does not create partial compaction state;
- if new path causes provider failures, disable new path via migration flag during rollout window, then remove the old path only after acceptance.

After old active modules are removed, rollback is a code rollback to previous release. Do not keep old pipeline as a permanent runtime fallback.

## 17. Acceptance Criteria

The change is accepted only when all criteria are true:

- old active compaction agent/summarizer flow is removed or fully disabled from runtime;
- there is no code path where old `CompactionService` and new `CompactionController` can both compact in one turn;
- `LocalLlmSummary` is default and required backend;
- Oxide-Agent compaction works without OpenAI `/responses/compact`;
- multi-provider mode does not depend on OpenAI Responses API;
- `/responses/compact`, if future adapter exists, is optional and capability-gated;
- long sessions continue after compaction;
- repeated compaction creates no duplicate summaries;
- old summary messages are not copied into hot context as ordinary messages;
- tool-heavy conversations preserve valid tool call/tool result pairs;
- context overflow retry uses `ContextLimit`/`MidTurn`, not manual trigger;
- model downshift compaction runs when smaller context route requires it;
- pinned context/todos/approval/runtime state are preserved;
- old sessions with structured summaries/archive refs load and compact safely;
- no new R2 archive/payload objects are created by default compaction;
- Telegram progress renders new compaction status;
- Web event timeline records new compaction events;
- relevant unit, regression and e2e tests are updated;
- docs/config migration are described;
- rollback path exists without permanent dual-flow fallback.

## 18. Open Questions

1. Provider message role for compacted summary.

Existing `AgentRunner::convert_memory_to_messages(...)` passes `MessageRole::System` as `system`. Some providers may reject mid-history system messages. Coding agent must verify provider normalization and decide whether `[OXIDE_COMPACTED_SUMMARY_V1]` should be `System` or `User` role in provider-visible history.

2. Exact config migration for token thresholds.

`soft_warning_tokens` and `hard_compaction_tokens` exist today. Need decide whether to keep them as shims, convert them to percent/window logic, or add new config names. First version can map them conservatively, but docs must be clear.

3. R2 old object retention policy.

New compaction should not create archive/payload objects. But old R2 archive/payload objects remain. Need product/ops decision on retention duration and manual cleanup, outside first implementation.

4. Telegram/Web public API compatibility.

Web `event_variant_name(...)` exposes event names. Need confirm whether external clients rely on `pruning_applied`. If yes, keep enum variant compatibility but stop emitting it from new compaction.

5. Exact safe boundary for mid-turn compaction.

Coding agent must inspect full tool-loop implementation in `runner/tools.rs` and `runner/responses.rs` to ensure compaction is only run when active tool state can be preserved or when no open tool batch exists.

6. Whether old `dedup_superseded` and `error_retry_collapse` are needed outside compaction.

If they solve independent history hygiene problems, they should be re-homed as explicit cleanup utilities, not part of compaction.

7. Whether new summary metadata should be a new field on `AgentMessage` or embedded in `structured_summary`.

Preferred: new explicit metadata with serde defaults. But if schema churn must be minimized, use `structured_summary` only as a migration bridge and avoid producing old JSON shape.

## 19. Implementation Checklist

### `crates/oxide-agent-core/src/agent/compaction/mod.rs`

- Export new controller/task/history/local summary modules.
- Stop exporting old active modules once migration completes.
- Keep compatibility exports only if old persisted data or tests need them.

### `crates/oxide-agent-core/src/agent/compaction/types.rs`

- Replace old `CompactionTrigger` active semantics with separate reason/phase types.
- Add `CompactionReason`: `PreTurn`, `MidTurn`, `Manual`, `ContextLimit`, `ModelDownshift`.
- Add `CompactionPhase`: `PreSampling`, `MidTurn`, `Manual`, `ModelSwitch`.
- Add compact result metadata with token/item counts and backend/provider/route.
- Keep old outcome structs only as temporary shims if tests or old code still compile during migration.

### `crates/oxide-agent-core/src/agent/compaction/budget.rs`

- Simplify action model to compact-or-skip.
- Keep token estimation helpers.
- Remove prune/compact as separate actions from active flow.
- Add conservative route-aware threshold logic.
- Add model downshift fit check.

### `crates/oxide-agent-core/src/agent/compaction/prompt.rs`

- Replace JSON-only sidecar prompt with plain text handoff compact prompt.
- Add prompt builder inputs for previous summary, old messages, recent messages, pinned state and current task.
- Remove requirement to return JSON.

### New `crates/oxide-agent-core/src/agent/compaction/history.rs`

- Implement `is_compacted_summary_message(...)`.
- Implement detection for `[OXIDE_COMPACTED_SUMMARY_V1]`, `[COMPACTION_SUMMARY]`, `[BREADCRUMB_CARD]`.
- Implement `extract_previous_compacted_summary(...)`.
- Implement `build_compacted_history(...)`.
- Implement recent real user message selection.
- Implement pinned context preservation.
- Implement active valid tool pair preservation.
- Call recovery validator before replacement.
- Add unit tests.

### New `crates/oxide-agent-core/src/agent/compaction/local_llm_summary.rs`

- Implement provider-agnostic local summary backend.
- Use ordinary LLM text generation through existing `LlmClient` route APIs.
- No tools.
- No JSON parser.
- Timeout via config.
- Return plain text summary or error.

### New `crates/oxide-agent-core/src/agent/compaction/controller.rs`

- Implement `maybe_compact_before_sampling(...)`.
- Implement `maybe_compact_mid_turn(...)`.
- Implement `manual_compact(...)`.
- Implement `model_downshift_compact(...)`.
- Implement atomic `replace_compacted_history(...)` or delegate to `AgentMemory` method.
- Emit new progress events.
- Ensure old active pipeline is not invoked.

### `crates/oxide-agent-core/src/agent/memory.rs`

- Add new compacted summary constructor using `[OXIDE_COMPACTED_SUMMARY_V1]`.
- Add optional compaction metadata with serde defaults, or equivalent safe storage.
- Add or support `replace_compacted_history(...)` semantics.
- Ensure token count recalculation after replacement.
- Ensure repair changes are observable.
- Keep old `structured_summary`, `archive_ref`, `externalized_payload`, `pruned_artifact` deserialization.

### `crates/oxide-agent-core/src/agent/recovery.rs`

- Expose or add helper for validating compacted history tool pairs.
- Ensure no orphan tool result can pass into provider messages.
- Add tests for active tool pair preservation and invalid pair failure.

### `crates/oxide-agent-core/src/agent/runner/types.rs`

- Replace `compaction_service: Option<&CompactionService>` with `compaction_controller: Option<&CompactionController>`.
- Update `AgentRunnerContext::new_base(...)` signature and callers.
- Keep no old service reference in new path.

### `crates/oxide-agent-core/src/agent/runner/execution.rs`

- Remove active `run_iteration_compaction(...)`.
- Remove or replace `run_compaction_checkpoint(...)`.
- In `run_pre_llm_maintenance(...)`, call `maybe_compact_before_sampling(...)`.
- In context overflow branch, call mid-turn/context-limit compaction, not manual.
- Add post-response continuation compaction before follow-up sampling.
- Add model downshift compaction during route switch/failover.
- Refresh `ctx.messages` only after successful replacement.
- Update runner tests.

### `crates/oxide-agent-core/src/agent/executor.rs`

- Replace field `compaction_service: CompactionService` with controller.
- Update constructor and usage.

### `crates/oxide-agent-core/src/agent/executor/config.rs`

- Replace `CompactionService::default().with_summarizer(...)` construction.
- Build `CompactionController` with `LocalLlmSummary`.
- Keep configured compaction routes as local compact model configuration.
- Update debug logs to no longer say “summarizer routes” if misleading.

### `crates/oxide-agent-core/src/agent/executor/types.rs`

- Replace `RunnerContextServices` field.
- Update `PreparedExecution::build_runner_context(...)`.

### `crates/oxide-agent-core/src/agent/executor/compaction.rs`

- Replace `compact_current_context(...)` implementation with controller manual compact.
- Remove manual pruning event.
- Update logs and events.

### `crates/oxide-agent-core/src/agent/progress.rs`

- Add/update event variants for started/completed/failed/skipped.
- Add reason/phase/token/item fields.
- Remove active pruning/archive/externalized count status from compaction completed.
- Update `ProgressState::update(...)` status strings.
- Keep old variants only if compile compatibility requires during migration.

### `crates/oxide-agent-transport-telegram/src/bot/progress_render.rs`

- Update context rendering for new status.
- Remove test dependence on `PruningApplied` for compaction.
- Update `renders_compaction_status_and_warning`.

### `crates/oxide-agent-transport-web/src/web_transport.rs`

- Update `event_variant_name(...)` for `compaction_skipped` and changed events.
- Stop expecting active `pruning_applied` from compaction.
- Update web tests.

### `crates/oxide-agent-core/src/storage/compaction.rs`

- Keep old R2 archive/payload sink types only as compatibility if referenced.
- Do not wire them into new default compaction.
- Add tests that old references remain loadable.

### `crates/oxide-agent-core/src/agent/compaction/archive.rs`

- Remove active archive persistence calls.
- Keep `ArchiveRef` compatibility or move type to memory/storage compatibility module.

### `crates/oxide-agent-core/src/config.rs`

- Update compaction config semantics.
- Keep compaction model route config for `LocalLlmSummary`.
- Deprecate old staged pipeline threshold semantics.
- Add tests for route selection and threshold mapping.

### `.env.example`

- Replace “staged compaction pipeline still works” language.
- State default backend is provider-agnostic local LLM summary.
- State OpenAI `/responses/compact` is not required.
- Document any deprecated envs.

### `README.md` and docs

- Update automatic compression description.
- Explain runtime/session-level compaction.
- Explain multi-provider behavior without OpenAI remote compact.
- Explain old R2 archive/payload compatibility.
- Explain history repair and tool_call_id invariants under new compaction.

### Tests

- Add unit tests for history builder.
- Add unit tests for local summary backend success/failure.
- Add no-op failure tests.
- Add threshold tests.
- Add mid-turn context limit tests.
- Add model downshift tests.
- Add migration fixture tests.
- Add repeated compaction no-duplicate tests.
- Add long conversation e2e.
- Add tool-heavy e2e.
- Update Telegram/Web progress tests.
