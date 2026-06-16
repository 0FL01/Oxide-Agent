## Что менять

1. **Добавить профиль `zai` в `openai_base`**

   В `crates/oxide-agent-core/src/llm/providers/openai_base/profile.rs` добавить `OpenAICompatibleProfile::zai()` и `resolve_profile("zai")`.

   Минимально профиль должен сохранить текущее поведение ZAI:

   * base URL: `https://api.z.ai/api/coding/paas/v4`
   * temperature: `0.95`
   * tools: включены
   * native JSON mode только когда `json_mode && !tools`
   * `thinking: enabled` обычно
   * `thinking: disabled` при native JSON mode
   * `reasoning_content` читать отдельно от обычного `content`
   * structured output включать только для моделей, где текущий ZAI provider это разрешает

2. **Добавить streaming в `openai_base` через `reqwest`**

   Сейчас `openai_base` фактически делает `"stream": false` и парсит обычный JSON response. Для ZAI это нельзя просто оставить как есть, потому что текущий `zai` provider в agent/tool режиме работает через stream.

   Нужно добавить отдельный путь:

   * для профиля `zai`, если не native JSON-only:

     * ставить `"stream": true`
     * отправлять обычный Chat Completions request через `reqwest`
     * читать SSE chunks
     * парсить `data: ...`
     * игнорировать `[DONE]`
   * для `json_mode && !tools` оставить non-stream request с `response_format: { "type": "json_object" }`

3. **Перенести ZAI streaming aggregator**

   Это самая важная часть. Из текущего `zai` provider нужно перенести поведение, а не SDK:

   * накапливать `choices[0].delta.content`
   * накапливать `choices[0].delta.reasoning_content`
   * собирать fragmented `tool_calls`
   * склеивать `function.arguments` по частям
   * сохранять provider tool call id
   * читать `finish_reason`
   * читать `usage`, если он приходит в stream
   * корректно обрабатывать пустой ответ

   Без этого агент начнёт ломаться именно в tool-calling сценариях, причём не обязательно сразу очевидно.

4. **ZAI-специфику держать внутри `openai_base` profile, не provider**

   Я бы вынес в профиль/утилиты:

   * `zai_supports_structured_output(model_id)`
   * ZAI body policy для `thinking`
   * ZAI streaming policy
   * ZAI rate-limit parser
   * capability mapping для `glm-*`

   То есть не делать новый provider, не делать SDK wrapper, не оставлять `zai.rs`.

5. **Удалить dedicated provider**

   Удалить:

   * `crates/oxide-agent-core/src/llm/providers/zai.rs`
   * `crates/oxide-agent-core/src/llm/providers/zai/`
   * feature `llm-zai`
   * dependency `zai-rs`
   * registration из `providers/modules.rs`
   * registration из `capabilities/compiled.rs`
   * упоминания `llm-provider/zai`
   * `zai_rs` из `RUST_LOG`

   Старый route `provider = "zai"` я бы **не поддерживал**. Раз миграция сольная и приоритет — чистота, пусть старые конфиги падают явно.

6. **Обновить конфиги и docs**

   Вместо:

   ```env
   ZAI_API_KEY=...
   AGENT_MODEL_PROVIDER=zai
   ```

   сделать:

   ```env
   OPENAI_BASE_PROVIDERS__1__NAME=zai
   OPENAI_BASE_PROVIDERS__1__API_BASE=https://api.z.ai/api/coding/paas/v4
   OPENAI_BASE_PROVIDERS__1__API_KEY=...
   OPENAI_BASE_PROVIDERS__1__PROFILE=zai

   AGENT_MODEL_PROVIDER=openai-base:zai
   AGENT_MODEL_ID=glm-4.7
   ```

   Я бы не оставлял `ZAI_API_KEY` как legacy fallback. Это снова создаст отдельную ветку поддержки.

7. **Тесты, которые обязательно нужны**

   Минимальный набор:

   * `openai_base:zai` строит body со `stream: true` для tools
   * `json_mode && !tools` строит body со `stream: false`, `response_format`, `thinking: disabled`
   * обычный ZAI chat строит `thinking: enabled`
   * SSE parser склеивает обычный content
   * SSE parser склеивает `reasoning_content`
   * SSE parser склеивает fragmented tool arguments
   * SSE parser сохраняет tool call id
   * ZAI 429 body с `next_flush_time` превращается в нормальный rate-limit wait
   * старый `provider = "zai"` больше не проходит config validation
   * `provider = "openai-base:zai"` проходит

## Мины

Главные:

* **Streaming tool calls.** Нельзя просто включить stream и читать `content`. Tool calls приходят кусками, arguments тоже кусками.
* **`reasoning_content`.** Сейчас ZAI отдаёт reasoning отдельно. Если потерять это поле, деградирует reasoning telemetry/UX.
* **JSON mode + tools.** Текущий код намеренно не включает native JSON mode при tools. Это нужно сохранить.
* **`thinking`.** Для ZAI это body field, но generic OpenAI-compatible провайдеры могут его не принять. Отправлять только в `profile = zai`.
* **`with_tool_stream(true)`.** Сейчас оно включается только для части моделей: `glm-4.7`/`glm-4.6`. В raw `reqwest` не надо слепо отправлять аналогичный флаг всем GLM-моделям.
* **Rate limit.** Generic OpenAI Base, вероятно, смотрит в основном на `Retry-After`. У ZAI есть `next_flush_time`; это надо сохранить.
* **Audio/media fallback.** В `llm/client.rs` есть ZAI-only sentinel `ZAI_FALLBACK_TO_MEDIA`. После удаления provider это станет мёртвым и вводящим в заблуждение кодом. Удалить или заменить generic capability fallback.
* **Тестовые mock-и.** В e2e есть `SequencedZaiProvider`; даже если это просто mock, для “полностью удалить zai из кода” его тоже лучше переименовать в generic `SequencedLlmProvider`.

## Рекомендуемый порядок

1. Сначала добавить `profile = zai` в `openai_base`.
2. Потом добавить `reqwest` SSE streaming parser.
3. Потом перенести ZAI quirks: `thinking`, `reasoning_content`, tool delta aggregation, rate-limit parser.
4. Потом переключить env/examples/routes на `openai-base:zai`.
5. Потом удалить `zai-rs`, `llm-zai`, `zai.rs`, `zai/`.
6. Потом чистка docs/tests/snapshots.
7. Финально прогнать features/profile-full и live test с реальным `glm-*`.

Итоговая архитектура должна быть такой: **один OpenAI-compatible transport на `reqwest`, а ZAI — только профиль поведения внутри `openai_base`**. Это самый чистый вариант под твой приоритет: меньше provider-кода, меньше SDK-зависимостей, меньше отдельных feature gates.
