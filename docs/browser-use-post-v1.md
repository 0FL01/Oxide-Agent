# Browser Use Post-v1 Decisions

Этот документ фиксирует следующий decision slice после базового rollout Browser Use в Oxide Agent.

## Контекст

К этому моменту в системе уже есть:

- self-hosted `browser_use` sidecar и HTTP bridge
- route inheritance из Oxide Agent для совместимых provider-ов
- high-level tools:
  - `browser_use_run_task`
  - `browser_use_get_session`
  - `browser_use_close_session`
  - `browser_use_extract_content`
  - `browser_use_screenshot`

Следующий выбор был между двумя направлениями:

- расширять low-level Browser Use surface (`click`, `type`, `tabs`, `eval`, `scroll`, `wait` и т.д.)
- идти в сторону persistent session/profile reuse

## Решение

- default tool surface остается high-level
- low-level Browser Use surface откладывается
- следующим implementation priority становится persistent session/profile reuse
- перед low-level surface обязательно вводятся отдельные quota/policy механизмы для browser automation

## Почему выбран profile reuse, а не low-level surface

- текущий high-level surface уже покрывает основной продуктовый сценарий: открыть сайт, пройти шаги, забрать результат, при необходимости дочитать страницу и снять screenshot
- raw low-level actions резко раздувают tool surface и усложняют policy, approvals, observability и recovery
- profile reuse дает более практичную пользу раньше:
  - повторный вход в сервисы
  - сохранение cookies/session state
  - меньше лишних логинов и повторной навигации
  - устойчивее длительные topic-scoped workflows
- low-level surface без квот и topic policy слишком легко превращается в "мини-Playwright shell" внутри агента, чего текущий rollout сознательно избегал

## Low-level Surface Decision

На этом этапе фиксируется:

- не добавлять в default agent surface инструменты уровня:
  - `browser_click`
  - `browser_type`
  - `browser_scroll`
  - `browser_eval`
  - `browser_switch_tab`
  - `browser_close_tab`
  - и другие атомарные browser actions
- не пробрасывать весь OSS CLI/MCP surface Browser Use в main agent без дополнительного policy layer
- не смешивать текущий high-level orchestration path с raw browser-control path в одном релизе

Если low-level surface когда-либо появится позже, он должен быть:

- явно experimental
- topic-gated / manager-gated
- с отдельными квотами
- с отдельной observability и audit-моделью
- с минимальным allowlist, а не полным Browser Use CLI surface

## Profile Reuse Decision

Следующим направлением считается controlled persistent reuse, но в ограниченном виде.

### Что разрешаем как целевой next step

- topic-scoped browser profile reuse
- profile storage отдельно от ephemeral session metadata
- привязку reuse к topic/context, а не ко всему пользователю глобально
- повторное использование profile между задачами одного topic, даже если конкретная runtime session была закрыта

### Что пока не разрешаем

- shared profile между разными topic
- shared profile между разными пользователями
- бесконтрольные долгоживущие browser processes
- произвольные persistent browser sandboxes без lifecycle/cleanup policy
- profile reuse как implicit behavior для всех задач по умолчанию

## Контракт следующей реализации

Следующая implementation stage должна опираться на такие правила:

### 1. Разделение session и profile

- `session_id` остается runtime-идентификатором живой или недавно созданной browser session
- persistent reuse не должен зависеть только от `session_id`
- для reuse должен появиться отдельный слой идентичности, например `profile_id`, `profile_scope` или эквивалентная abstraction

### 2. Scope

- reuse должен быть topic-scoped
- DM fallback допускается только по тем же правилам, что и остальная topic-aware инфраструктура
- cross-topic reuse по умолчанию запрещен

### 3. Lifecycle

- profile reuse должен иметь явный create/attach/detach/cleanup lifecycle
- должно быть понятно, когда profile считается активным, idle, stale, deleted
- cleanup policy должна предотвращать бесконечный рост volume и накопление старых login-state данных

### 4. Storage

- browser profile data хранится отдельно от session JSON и artifacts
- metadata по profile должна быть читаемой из control plane
- в metadata нельзя хранить raw secrets

### 5. Policy and Quotas

- до появления low-level tools нужно добавить отдельные browser automation limits:
  - max concurrent browser sessions
  - max retained profiles per topic/user
  - max profile TTL / idle TTL
  - optional policy, какие topic вообще могут использовать persistent reuse
- topic-level enable/disable для Browser Use уже есть, но для persistent reuse нужен отдельный policy toggle

## Implications for Tool Surface

До следующего policy slice рабочий Browser Use surface остается таким:

- `browser_use_run_task`
- `browser_use_get_session`
- `browser_use_close_session`
- `browser_use_extract_content`
- `browser_use_screenshot`

Следующая реализация должна стремиться добавить reuse без взрыва surface area. Предпочтительный путь:

- расширить существующий high-level contract reuse-параметрами
- не вводить десятки новых атомарных browser tools

## Explicit Non-goals

В этом decision slice сознательно НЕ утверждается:

- полный low-level Browser Use API для main agent
- перенос всего Browser Use MCP/CLI surface в Oxide Agent
- глобальный persistent browser для всех задач
- profile reuse без manager/policy control
- автоматическое reuse для всех Browser Use вызовов без явного scope

## Acceptance Criteria

Decision slice считается зафиксированным, если:

- явно указано, что low-level surface откладывается
- явно указано, что следующим priority становится persistent session/profile reuse
- зафиксировано topic-scoped разделение session и profile
- зафиксировано требование отдельных policy/quota перед low-level expansion
- сохранен compact high-level surface как основной рабочий путь
