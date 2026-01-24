# Blueprint: Улучшение системы расчёта токенов (Zhipu AI / GLM-4.7)

Этот план направлен на переход от локальной эвристической оценки токенов к использованию точной статистики, предоставляемой API Zhipu AI (GLM-4.7). Это критически важно для корректного учета системных промптов, Chain of Thought (Reasoning) и контекста файлов.

### Проблема
Текущая реализация использует библиотеку `tiktoken` для локального подсчета, что приводит к значительным погрешностям:
1.  **System Prompt Ignorance**: Не учитывается вес системного промпта, даты и описания инструментов.
2.  **Reasoning Opaque**: Невозможно точно посчитать скрытые токены "мыслей" (CoT) модели.
3.  **Tokenizer Mismatch**: Локальный токенизатор (cl100k_base) может некорректно оценивать кириллицу и спецсимволы GLM-4.

### Решение: Синхронизация с биллингом API (Authority Source)
Использовать поля `usage` из ответа API для жесткой синхронизации счетчика токенов в памяти агента.

---

### План реализации

#### 1. Модификация структур данных (`src/llm/mod.rs`)
Добавить контейнер для статистики использования токенов.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenUsage {
    pub prompt_tokens: u32,      // Входящие (системный промпт + история + файлы)
    pub completion_tokens: u32,  // Ответ модели (включая thoughts)
    pub total_tokens: u32,       // Итого
}

pub struct ChatResponse {
    // ... существующие поля
    pub usage: Option<TokenUsage>, // Новое поле
}
```

#### 2. Апгрейд ZaiProvider (`src/llm/providers.rs`)
GLM-4.7 поддерживает стандарт OpenAI для стриминга статистики при наличии флага `stream_options`.

*   **Запрос**: В методе `chat_with_tools` добавить в JSON-тело:
    ```json
    "stream_options": { "include_usage": true }
    ```
*   **Структуры**: Обновить структуру `ZaiStreamChunk` (и связанные), добавив опциональное поле `usage`.
*   **Обработка стрима**: В `process_zai_stream` перехватывать чанк с полем `usage`. Обычно он приходит в самом конце потока (иногда вместе с `DONE` или в пустом `choices`).

#### 3. Синхронизация памяти (`src/agent/memory.rs`)
Вместо инкрементального подсчета (add_tokens) реализовать метод полной синхронизации.

*   Добавить метод:
    ```rust
    pub fn sync_token_count(&mut self, real_total_tokens: usize) {
        // Логирование расхождения для аналитики
        let diff = real_total_tokens as i64 - self.token_count as i64;
        tracing::debug!(
            "Token sync: local={}, real={}, diff={}", 
            self.token_count, 
            real_total_tokens, 
            diff
        );
        
        self.token_count = real_total_tokens;
    }
    ```

#### 4. Интеграция в цикл агента (`src/agent/executor.rs`)
Связывание ответа LLM и состояния памяти.

*   В методе `run_loop` после получения `response`:
    ```rust
    let response = self.llm_client.chat_with_tools(...).await?;
    
    // Сначала добавляем сообщение ассистента в историю (локально токены увеличатся эвристически)
    // ...
    
    // Затем, если есть точные данные, корректируем счетчик
    if let Some(usage) = &response.usage {
        // Важно: usage.total_tokens - это состояние ПОСЛЕ генерации ответа.
        // Оно включает System Prompt + History + New Response.
        // Это именно то значение, которое должно быть в memory.token_count.
        self.session.memory.sync_token_count(usage.total_tokens as usize);
    }
    ```

### Ожидаемые результаты
1.  **Точность 100%**: Счетчик в памяти будет соответствовать биллингу.
2.  **Безопасность**: Агент не вылетит за лимиты контекста из-за неучтенных файлов или длинных мыслей.
3.  **Прозрачность**: В логах будет видно реальное потребление токенов.
