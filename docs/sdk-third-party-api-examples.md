# CLIProxyAPI: Примеры API запросов (Third Party Integration)

Настоящий документ описывает примеры использования `CLIProxyAPI` в качестве универсального OpenAI-совместимого шлюза для интеграции сторонних приложений (например, Telegram-ботов, скриптов, сторонних IDE).

Главная особенность `CLIProxyAPI` заключается в том, что он унифицирует доступ ко всем подключенным провайдерам. Для вашего приложения **нет разницы**, кто находится под капотом: проприетарный API Qwen или закрытый API OpenAI Codex. Вы всегда обращаетесь к прокси по единому стандарту OpenAI.

Прокси автоматически берет на себя:
- Балансировку между аккаунтами (Round-Robin).
- Бесшовную обработку лимитов (Rate Limits / Quotas).
- Инъекцию системных параметров (например, `reasoning.effort`).

---

## Общие параметры подключения
- **URL (Base URL):** `http://<ВАШ_IP_ИЛИ_ДОМЕН>:8317/v1`
- **Эндпоинт:** `/chat/completions`
- **Метод:** `POST`
- **Авторизация:** Bearer-токен (значение из `api-keys` в вашем `config.yaml`, по умолчанию `opencode-proxy-key`).

---

## 1. Qwen AI Chat (Базовый текстовый запрос)

Запрос маршрутизируется в балансировщик Qwen. В качестве модели указывается настроенный алиас `openai/qwen3.5-plus`.

```bash
curl -X POST http://localhost:8317/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer opencode-proxy-key" \
  -d '{
    "model": "openai/qwen3.5-plus",
    "messages": [
      {
        "role": "system",
        "content": "Ты полезный AI-ассистент."
      },
      {
        "role": "user",
        "content": "Напиши функцию на Python для вычисления чисел Фибоначчи."
      }
    ],
    "temperature": 0.7,
    "stream": false
  }'
```

---

## 2. Qwen AI Chat (Structured Output / Strict JSON Schema)

Пример запроса со строгой схемой ответа `json_schema`, который идеально подойдет для детерминированного парсинга ответов в Telegram-боте. Прокси корректно пробрасывает структуру в Qwen API.

```bash
curl -X POST http://localhost:8317/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer opencode-proxy-key" \
  -d '{
    "model": "openai/qwen3.5-plus",
    "messages": [
      {
        "role": "user",
        "content": "Выведи информацию о смартфоне iPhone 15 Pro."
      }
    ],
    "response_format": {
      "type": "json_schema",
      "json_schema": {
        "name": "smartphone_info",
        "strict": true,
        "schema": {
          "type": "object",
          "properties": {
            "brand": { "type": "string" },
            "model": { "type": "string" },
            "release_year": { "type": "number" },
            "features": {
              "type": "array",
              "items": { "type": "string" }
            }
          },
          "required": ["brand", "model", "release_year", "features"],
          "additionalProperties": false
        }
      }
    },
    "stream": false
  }'
```

---

## 3. OpenAI Codex (Базовый запрос с логикой рассуждений)

Несмотря на то, что под капотом прокси использует внутренние механизмы авторизации Codex, наружу он выставляет модель как обычную GPT. В нашем конфиге это `openai/gpt-5.3-codex`.

*Примечание: Согласно `config.yaml`, прокси принудительно внедряет параметр `reasoning.effort: "medium"` на лету для моделей `gpt-5*` по протоколу `codex`.*

```bash
curl -X POST http://localhost:8317/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer opencode-proxy-key" \
  -d '{
    "model": "openai/gpt-5.3-codex",
    "messages": [
      {
        "role": "user",
        "content": "Отрефакторь этот код: \n\n```js\nfunction sum(a,b){return a+b}\n```"
      }
    ],
    "stream": false
  }'
```

---

## 4. Использование Tools (Function Calling)

Прокси полностью поддерживает вызов функций (как для Qwen, так и для Codex/GPT). Пример запроса, если боту нужно дать ИИ возможность дергать ваши внутренние API:

```bash
curl -X POST http://localhost:8317/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer opencode-proxy-key" \
  -d '{
    "model": "openai/qwen3.5-plus",
    "messages": [
      {
        "role": "user",
        "content": "Какая погода сейчас в Дубае?"
      }
    ],
    "tools": [
      {
        "type": "function",
        "function": {
          "name": "get_weather",
          "description": "Получает текущую погоду в указанном городе",
          "parameters": {
            "type": "object",
            "properties": {
              "location": {
                "type": "string",
                "description": "Название города, например Дубай"
              }
            },
            "required": ["location"]
          }
        }
      }
    ],
    "tool_choice": "auto",
    "stream": false
  }'
```

---

## 5. Формат ответа (Единый для всех)

Независимо от того, какую модель вы выбрали (`openai/qwen3.5-plus` или `openai/gpt-5.3-codex`), ваше стороннее приложение получит абсолютно стандартный ответ в формате OpenAI. Этот ответ можно парсить стандартными библиотеками (например, официальным SDK `openai-python` или `openai-node`):

```json
{
  "id": "chatcmpl-123",
  "object": "chat.completion",
  "created": 1700000000,
  "model": "openai/qwen3.5-plus",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "Здесь будет текст ответа модели..."
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": 15,
    "completion_tokens": 120,
    "total_tokens": 135
  }
}
```