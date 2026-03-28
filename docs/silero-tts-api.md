# Silero TTS API (RU)

## Назначение

Синтез русской речи через Silero TTS.

Поддерживается:

* обычный текст
* SSML
* выходные форматы: `wav`, `ogg`

Для Telegram voice использовать:

* `format=ogg`

---

## Base URL

```text
http://<host>:8001
```

---

## 1) Проверка сервиса

### `GET /healthz`

Проверка состояния сервиса.

#### Response `200`

```json
{
  "status": "ok",
  "device": "cpu",
  "model_path": "/app/models/v5_4_ru.pt",
  "default_speaker": "baya",
  "default_sample_rate": 48000,
  "supported_formats": ["wav", "ogg"],
  "supports_ssml": true,
  "ffmpeg_available": true,
  "ogg_opus_bitrate": "32k",
  "ogg_opus_application": "voip"
}
```

---

## 2) Синтез речи

### `POST /v1/audio/speech`

Синтезирует аудио из текста или SSML.

### Request

`Content-Type: application/json`

#### Body

```json
{
  "text": "Привет, мир",
  "speaker": "baya",
  "sample_rate": 48000,
  "format": "ogg",
  "ssml": false
}
```

### Параметры

| Поле          | Тип       | Обязательно | По умолчанию                  | Описание                                      |
| ------------- | --------- | ----------: | ----------------------------- | --------------------------------------------- |
| `text`        | `string`  |          да | —                             | Текст или SSML-разметка для синтеза           |
| `speaker`     | `string`  |         нет | значение `SILERO_SPEAKER`     | Голос модели                                  |
| `sample_rate` | `integer` |         нет | значение `SILERO_SAMPLE_RATE` | Частота дискретизации                         |
| `format`      | `string`  |         нет | `wav`                         | Формат ответа: `wav` или `ogg`                |
| `ssml`        | `boolean` |         нет | `false`                       | Если `true`, `text` интерпретируется как SSML |

### Допустимые значения

#### `sample_rate`

```text
8000
24000
48000
```

#### `format`

```text
wav
ogg
```

---

## Логика обработки `text`

### Обычный текст

Если `ssml=false`, поле `text` передается в TTS как обычный текст.

Пример:

```json
{
  "text": "Привет, это тест",
  "format": "wav"
}
```

### SSML

Если `ssml=true`, поле `text` передается в TTS как SSML.

Пример:

```json
{
  "text": "<speak>Привет<break time=\"500ms\"/>мир</speak>",
  "format": "ogg",
  "ssml": true
}
```

### Рекомендуемое поведение для агента

Если агент генерирует SSML, всегда отправлять:

```json
{
  "ssml": true
}
```

---

## Поддержка SSML

API принимает SSML как строку в `text`.

Рекомендуется оборачивать разметку в корневой тег:

```xml
<speak>...</speak>
```

Пример:

```xml
<speak>
  Привет
  <break time="300ms"/>
  это тест синтеза
</speak>
```

### Минимальный пример

```json
{
  "text": "<speak>Здравствуйте<break time=\"400ms\"/>это голосовой ответ</speak>",
  "speaker": "baya",
  "sample_rate": 48000,
  "format": "ogg",
  "ssml": true
}
```

### Замечание

API не нормализует семантику SSML. Корректность и поддержка конкретных тегов зависят от модели TTS.

---

## Response

### Успех `200`

Возвращает бинарное аудио.

#### Для `format=wav`

* `Content-Type: audio/wav`
* `Content-Disposition: attachment; filename="speech.wav"`

#### Для `format=ogg`

* `Content-Type: audio/ogg`
* `Content-Disposition: attachment; filename="speech.ogg"`

Тело ответа — бинарный аудиофайл.

---

## Ошибки

### `400 Bad Request`

Некорректные входные параметры.

#### Пустой текст

```json
{
  "detail": "Пустой text"
}
```

#### Недопустимый `sample_rate`

```json
{
  "detail": "Допустимые sample_rate: 8000, 24000, 48000"
}
```

### `422 Unprocessable Entity`

Ошибка схемы запроса.

Пример:

* отсутствует `text`
* передан неподдерживаемый `format`

### `500 Internal Server Error`

Ошибка синтеза или кодирования аудио.

Пример:

```json
{
  "detail": "TTS generation error: ffmpeg opus encode failed: ..."
}
```

---

### 1. OGG для Telegram voice

```bash
curl -X POST "http://localhost:8001/v1/audio/speech" \
  -H "Content-Type: application/json" \
  -d '{
    "text": "Это голосовое сообщение для Telegram",
    "speaker": "baya",
    "sample_rate": 48000,
    "format": "ogg",
    "ssml": false
  }' \
  --output speech.ogg
```

### 3. SSML + OGG

```bash
curl -X POST "http://localhost:8001/v1/audio/speech" \
  -H "Content-Type: application/json" \
  -d '{
    "text": "<speak>Привет<break time=\"500ms\"/>это SSML тест</speak>",
    "speaker": "baya",
    "sample_rate": 48000,
    "format": "ogg",
    "ssml": true
  }' \
  --output speech.ogg
```

---

# Рекомендуемый tool call для агента

## Plain text

```json
{
  "text": "Привет, чем могу помочь?",
  "speaker": "baya",
  "sample_rate": 48000,
  "format": "ogg",
  "ssml": false
}
```

## SSML

### Example 1
```json
{
  "text": "<speak>Привет<break time=\"250ms\"/>чем могу помочь?</speak>",
  "speaker": "baya",
  "sample_rate": 48000,
  "format": "ogg",
  "ssml": true
}
```

### Example 2
```json
{
  "text": "<speak><prosody pitch=\"high\">Привет</prosody><break time=\"180ms\"/>чем могу помочь?</speak>",
  "speaker": "baya",
  "sample_rate": 48000,
  "format": "ogg",
  "ssml": true,
  "humanize": true
}
```

---

# Короткая спецификация для OpenAPI / tool description

```yaml
POST /v1/audio/speech
content-type: application/json

requestBody:
  required: true
  content:
    application/json:
      schema:
        type: object
        required: [text]
        properties:
          text:
            type: string
            description: Text or SSML markup
          speaker:
            type: string
            default: baya
          sample_rate:
            type: integer
            enum: [8000, 24000, 48000]
            default: 48000
          format:
            type: string
            enum: [wav, ogg]
            default: wav
          ssml:
            type: boolean
            default: false

responses:
  "200":
    description: Generated speech audio
    content:
      audio/wav: {}
      audio/ogg: {}
  "400":
    description: Invalid request
  "422":
    description: Validation error
  "500":
    description: TTS generation error
```

## Список голосов

```json
{
  "default_speaker": "baya",
  "voices": [
    "aidar",
    "baya",
    "kseniya",
    "xenia"  ]
}
```

### Синтез

```json
{
  "text": "<speak>Привет<break time=\"250ms\"/>мир</speak>",
  "speaker": "baya",
  "sample_rate": 48000,
  "format": "ogg",
  "ssml": true
}
```

---

### Шаблон подачи

Для Telegram-агента хороший паттерн такой:

* приветствие: чуть выше тон
* основная фраза: normal / medium
* важный кусок: чуть медленнее
* короткие паузы 120–250 мс между блоками
* перед финальной мыслью пауза 250–400 мс

---

## Рабочий SSML-шаблон под `baya`

```xml
<speak>
  <prosody pitch="high">Привет.</prosody>
  <break time="180ms"/>
  Вот что я нашл+а.
  <break time="140ms"/>
  <prosody rate="slow">Сейчас коротко объясню самое важное.</prosody>
  <break time="220ms"/>
  Дальше м+ожно перейти к деталям.
</speak>
```

Для агента обычно хорошо звучат такие диапазоны:

* `break`: `120ms`–`300ms`
* `rate`: `slow` только на важных кусках
* `pitch`: `high` или `low` только локально, не на весь текст

`x-high` и `x-slow` чаще уже дают “театральность”.

---

## Что не стоит делать

* Один `<prosody rate="fast">` на весь текст.
* Один `<prosody pitch="high">` на весь текст.
* Слишком много `break` подряд.
* Длинные абзацы без запятых и без ручных ударений.
* Надеяться, что `ogg` или контейнер что-то “оживят” сами по себе.