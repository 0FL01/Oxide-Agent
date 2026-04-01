# Browser Use Stage A

Этот документ фиксирует Stage A для следующей итерации интеграции `browser-use` в Oxide Agent.

## Цель

- сделать Browser Use потребителем модельного стека Oxide Agent, а не отдельным sidecar с ручным выбором bridge LLM
- обеспечить работу как минимум с `MiniMax` и `ZAI`
- сохранить компактный high-level tool surface v1, не раздувая первый релиз low-level действиями

## Решение

- Browser Use должен наследовать активный model route Oxide Agent по умолчанию
- `browser_use_bridge` должен принимать нормализованный `browser_llm_config`, а не полагаться только на `BROWSER_USE_BRIDGE_LLM_PROVIDER` / `BROWSER_USE_BRIDGE_LLM_MODEL`
- текущие bridge env-переменные считаются legacy fallback, а не основным способом выбора модели
- основной orchestration остается в Oxide Agent; Browser Use не становится вторым независимым агентом с отдельной модельной политикой

## Почему это нужно

- текущее v1 поведение требует отдельного LLM provider внутри Python sidecar
- это ломает ожидание, что Browser Use работает с теми же provider/model, что и основной агент
- это отдельно размазывает model selection, credentials и policy между Rust runtime и Python bridge
- для `MiniMax` и `ZAI` нужен единый механизм route inheritance, а не набор ad hoc env-переключателей в compose

## Contract Stage A

### Route Inheritance

По умолчанию `browser_use_run_task` должен использовать активный route текущей agent session.

Вводятся два режима:

- `inherit_active_route` — режим по умолчанию
- `browser_route_override` — optional explicit override для операторских исключений и fallback-сценариев

### Browser LLM Config

Rust-side provider передает в bridge нормализованный конфиг вида:

```json
{
  "provider": "minimax",
  "model": "MiniMax-M2.7",
  "api_base": "https://...",
  "api_key_ref": "env:MINIMAX_API_KEY",
  "supports_vision": true,
  "supports_tools": false
}
```

Минимальные поля Stage A:

- `provider`
- `model`
- `api_base` или другой provider endpoint, если нужен
- `api_key_ref` или другой безопасный способ разрешения секрета
- `supports_vision`

Поле `supports_tools` допускается как forward-compatible флаг и не обязано использоваться в первой реализации.

### Supported Providers

На этом этапе обязательно поддержать:

- `minimax`
- `zai`

Дополнительно допускается сохранить поддержку текущих bridge-side adapter-ов:

- `google`
- `anthropic`
- `browser_use`

Но они больше не должны быть единственным способом запуска Browser Use.

## Security Contract

- raw secrets не должны попадать в prompt, memory, compaction summary и tool transcript
- route inheritance не должен приводить к публикации provider credentials в JSON-ответах bridge
- credentials для bridge должны передаваться только server-to-server или разрешаться через secret reference
- session metadata и artifacts Browser Use не должны хранить открытые API keys

## Capability Contract

- vision не считается обязательным hard requirement всего Browser Use integration path
- vision рассматривается как capability конкретной route/model
- text-only route допустимы для простых browsing/extraction сценариев
- для UI-heavy задач, где без visual grounding качество резко падает, система должна либо:
  - предупредить о degraded mode
  - либо использовать explicit override на vision-capable route

## Scope Boundaries

Stage A фиксирует только архитектурный контракт и не включает реализацию:

- adapter layer в Python bridge
- mapping active Oxide route -> `browser_llm_config`
- secret delivery channel
- новые Browser Use tools за пределами v1

Эти изменения относятся к следующим implementation stages.

## Влияние на текущий v1

- текущий env-based bridge mode остается рабочим как временный fallback
- существующий compose не считается финальной целевой моделью выбора browser LLM
- операторская документация должна явно различать:
  - текущее v1 поведение
  - целевое Stage A направление с route inheritance

## Acceptance Criteria

Stage A считается завершенным, если:

- зафиксировано, что Browser Use наследует model route Oxide Agent по умолчанию
- зафиксирован нормализованный контракт `browser_llm_config`
- явно указано, что `MiniMax` и `ZAI` входят в обязательный минимальный охват
- явно описано, что vision является capability, а не обязательным глобальным требованием
- legacy env-based bridge configuration переведена в статус fallback, а не primary path
- перечислены post-v1 ограничения и scope boundaries

## Связь с Post-v1 Expansion

После стабилизации Stage A и последующих implementation stages можно расширять Browser Use дальше:

- добавить `browser_use_extract_content`
- добавить `browser_use_screenshot`
- решить, нужен ли low-level tool surface
- оценить persistent sessions / profile reuse
- оценить отдельные quotas и topic-level policy для browser automation

Расширение допускается только после стабильного запуска основного inheritance path, чтобы сохранить controlled evolution без раздувания surface area в первом релизе.
