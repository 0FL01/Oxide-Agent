# API Documentation

Base URL: `http://127.0.0.1:8001`

## Endpoints

### GET /
Информация о сервисе.

```bash
curl http://127.0.0.1:8001/
```

**Response:**
```json
{
  "ok": true,
  "title": "Piper TTS API",
  "default_voice": "ruslan",
  "endpoints": ["/healthz", "/voices", "/v1/audio/speech", "/v1/audio/speech/stream"]
}
```

---

### GET /healthz
Проверка здоровья сервиса.

```bash
curl http://127.0.0.1:8001/healthz
```

**Response:**
```json
{
  "status": "ok",
  "default_voice": "ruslan",
  "voices_loaded": 4
}
```

---

### GET /voices
Список доступных голосов.

```bash
curl http://127.0.0.1:8001/voices
```

**Response:**
```json
{
  "default_voice": "ruslan",
  "voices": [
    {
      "alias": "denis",
      "model": "ru_RU-denis-medium",
      "path": "/path/to/models/ru_RU-denis-medium.onnx",
      "default": false
    },
    {
      "alias": "dmitri",
      "model": "ru_RU-dmitri-medium",
      "path": "/path/to/models/ru_RU-dmitri-medium.onnx",
      "default": false
    },
    {
      "alias": "irina",
      "model": "ru_RU-irina-medium",
      "path": "/path/to/models/ru_RU-irina-medium.onnx",
      "default": false
    },
    {
      "alias": "ruslan",
      "model": "ru_RU-ruslan-medium",
      "path": "/path/to/models/ru_RU-ruslan-medium.onnx",
      "default": true
    }
  ]
}
```

---

### POST /v1/audio/speech
Генерация речи. Поддерживаемые форматы: `ogg`, `mp3`, `wav`, `pcm`.

**Headers:**
- `Content-Type: application/json`

**Request Body:**
```json
{
  "text": "Текст для синтеза",
  "voice": "ruslan",
  "format": "ogg",
  "length_scale": null,
  "noise_scale": null,
  "noise_w_scale": null,
  "volume": 1.0,
  "normalize_audio": true,
  "speed": null,
  "sentence_silence": 0.0
}
```

| Параметр | Тип | Обязательный | По умолчанию | Описание |
|----------|-----|-------------|--------------|----------|
| `text` | string | Да | - | Текст для синтеза |
| `voice` | string | Нет | `ruslan` | Алиас голоса |
| `format` | `ogg` \| `mp3` \| `wav` \| `pcm` | Нет | `ogg` | Формат аудио |
| `length_scale` | float > 0 | Нет | `1.0` | Масштаб длительности (меньше = быстрее) |
| `speed` | float > 0 | Нет | `null` | Скорость речи (больше = быстрее) |
| `noise_scale` | float > 0 | Нет | `null` | Вариативность речи |
| `noise_w_scale` | float > 0 | Нет | `null` | Вариативность на уровне слов |
| `volume` | float > 0 | Нет | `1.0` | Громкость |
| `normalize_audio` | bool | Нет | `true` | Нормализация аудио |
| `sentence_silence` | float 0-2 | Нет | `0.0` | Пауза между предложениями (сек) |

**Headers ответа:**
- `Content-Type: audio/ogg \| audio/mpeg \| audio/wav \| audio/pcm`
- `Content-Disposition: attachment; filename="speech.{format}"`
- `X-Sample-Rate`: частота дискретизации
- `X-Voice`: название модели

**Example (натуральное звучание):**
```bash
curl -sS -X POST 'http://127.0.0.1:8001/v1/audio/speech' \
  -H 'Content-Type: application/json' \
  -d '{
    "text": "О-о-о, свежее мясо! Чую, пахнет наживой. Ты только посмотри на эти сочные кусочки, так и просятся на мой крюк. Не бегай от меня, я просто хочу отрезать тебе лишнее... ну, или всё сразу. У меня тут как раз место в пузе освободилось. Иди сюда, малявка! Сейчас мы тебя разделаем, присолим, и будет отличный ужин. Крюк в печень — никто не вечен, хе-хе-хе!",
    "voice": "ruslan",
    "format": "ogg",
    "speed": 0.9,
    "noise_scale": 0.62,
    "noise_w_scale": 0.78,
    "sentence_silence": 0.10,
    "volume": 1.0,
    "normalize_audio": true
  }' \
  -o /tmp/pudge-natural-1.ogg
```

**Example (быстрый синтез):**
```bash
curl -sS -X POST 'http://127.0.0.1:8001/v1/audio/speech' \
  -H 'Content-Type: application/json' \
  -d '{"text":"Привет!", "voice":"ruslan", "format":"ogg", "speed":1.5}' \
  -o /tmp/fast.ogg
```

**Example (с паузами):**
```bash
curl -sS -X POST 'http://127.0.0.1:8001/v1/audio/speech' \
  -H 'Content-Type: application/json' \
  -d '{"text":"Первое предложение. Второе предложение.", "voice":"ruslan", "format":"ogg", "sentence_silence":0.5}' \
  -o /tmp/pauses.ogg
```

---

### POST /v1/audio/speech/stream
Потоковая генерация речи. Только формат `pcm`.

**Headers:**
- `Content-Type: application/json`

**Request Body:** такой же, как для `/v1/audio/speech`, но `format` игнорируется (всегда `pcm`).

**Headers ответа:**
- `Content-Type: audio/pcm`
- `X-Voice`: название модели

**Example:**
```bash
curl -sS -X POST 'http://127.0.0.1:8001/v1/audio/speech/stream' \
  -H 'Content-Type: application/json' \
  -d '{"text":"Привет! Это потоковый синтез.", "voice":"ruslan"}' \
  -o /tmp/stream.pcm
```

## Ошибки

| Код | Описание |
|-----|----------|
| `400` | Пустой текст, неизвестный голос, неправильный формат |
| `500` | Ошибка генерации TTS |
| `503` | Сервис не готов (infference executor не инициализирован) |

**Example ответа об ошибке:**
```json
{
  "detail": "Unknown voice 'unknown'. Supported: denis, dmitri, irina, ruslan"
}
```
