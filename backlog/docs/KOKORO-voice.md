# Kokoro TTS API Reference

127.0.0.1:8000 - Здесь сейчас стоит сервер, он размещён локально на хосте где запущено само приложение (работает как сервис systemd, вне контернизации)

# Kokoro TTS API Reference

## Endpoints

### 1. File-based Audio Generation
```
POST http://127.0.0.1:8000/v1/audio/speech
Content-Type: application/json
```

Returns complete audio file in requested format (OGG/Opus, MP3, or WAV).
Best for Telegram voice messages and when you need a complete file.

### 2. Streaming PCM Generation
```
POST http://127.0.0.1:8000/v1/audio/speech/stream
Content-Type: application/json
```

Returns streaming PCM audio for progressive playback.
Best for real-time playback while audio is being generated.

## Request Parameters

| Parameter | Type | Description | Example |
|-----------|------|-------------|---------|
| `text` | string | Text to synthesize | `"Hello world"` |
| `lang` | string | Language code (only `"en"` supported) | `"en"` |
| `voice` | string | Voice name | `"af_bella"`, `"af_aoede"`, `"af_alloy"` |
| `speed` | float | Speech speed (default: 1.0) | `1.0` |
| `format` | string | Output format (only for `/v1/audio/speech`) | `"ogg"`, `"mp3"`, `"wav"`, `"pcm"` |

**Note:** `format` parameter is only available on `/v1/audio/speech`. For `/v1/audio/speech/stream`, format is always PCM.

## Available Voices
- `af_heart` (default)
- `af_bella`
- `af_aoede`
- `af_alloy`

## Audio Formats

### File Endpoint (`/v1/audio/speech`)
| Format | Codec | Content-Type | Best For |
|--------|-------|--------------|----------|
| `ogg` | Opus | `audio/ogg` | Telegram voice messages |
| `mp3` | MPEG | `audio/mpeg` | General playback |
| `wav` | PCM | `audio/wav` | Editing, lossless |
| `pcm` | - | - | Use streaming endpoint instead |

### Streaming Endpoint (`/v1/audio/speech/stream`)
- **Format**: Raw PCM (signed 16-bit)
- **Sample Rate**: 24000 Hz
- **Channels**: 1 (mono)
- **Content-Type**: `audio/pcm`

## Examples

### OGG Audio for Telegram (Recommended)
```bash
curl -s http://127.0.0.1:8000/v1/audio/speech \
  -H "Content-Type: application/json" \
  -o voice.ogg \
  -d '{
    "text": "Hello, this is a voice message for Telegram.",
    "lang": "en",
    "voice": "af_bella",
    "speed": 1.0,
    "format": "ogg"
  }'
```

### MP3 Audio
```bash
curl -s http://127.0.0.1:8000/v1/audio/speech \
  -H "Content-Type: application/json" \
  -o output.mp3 \
  -d '{
    "text": "This is an MP3 file.",
    "lang": "en",
    "voice": "af_bella",
    "speed": 1.0,
    "format": "mp3"
  }'
```

### WAV Audio
```bash
curl -s http://127.0.0.1:8000/v1/audio/speech \
  -H "Content-Type: application/json" \
  -o output.wav \
  -d '{
    "text": "This is a WAV file for editing.",
    "lang": "en",
    "voice": "af_bella",
    "speed": 1.0,
    "format": "wav"
  }'
```

### Streaming PCM
```bash
curl -s http://127.0.0.1:8000/v1/audio/speech/stream \
  -H "Content-Type: application/json" \
  -o output.pcm \
  -d '{
    "text": "Hello, this is streaming PCM audio.",
    "lang": "en",
    "voice": "af_bella",
    "speed": 1.0
  }'
```

### Long text (English)
```bash
curl -s http://127.0.0.1:8000/v1/audio/speech \
  -H "Content-Type: application/json" \
  -o long_output.ogg \
  -d '{
    "text": "The quick brown fox jumps over the lazy dog near the riverbank on a bright sunny morning while birds sing melodious tunes in the trees above.",
    "lang": "en",
    "voice": "af_bella",
    "speed": 1.0,
    "format": "ogg"
  }'
```



## Audio Conversion (Legacy)

If you have PCM files and need to convert them:

### Convert PCM to MP3
```bash
ffmpeg -f s16le -ar 24000 -ac 1 -i output.pcm output.mp3
```

### Convert PCM to OGG/Opus
```bash
ffmpeg -f s16le -ar 24000 -ac 1 -i output.pcm -c:a libopus output.ogg
```

### Convert with silence removal
```bash
ffmpeg -f s16le -ar 24000 -ac 1 -i output.pcm \
  -af "silenceremove=start_periods=1:start_silence=0.2:start_threshold=-40dB:detection=peak,aformat=dblp,areverse,silenceremove=start_periods=1:start_silence=0.2:start_threshold=-40dB:detection=peak,aformat=dblp,areverse" \
  output.mp3
```

## Performance

### Benchmark Results (CPU)
- **RTF (Real-Time Factor)**: ~0.23
- **Generation Speed**: ~4.4x real-time
- **Memory Usage**: ~1.5G RSS / 6.5G virtual (VM with ONNX model loaded)
- **Sample Rate**: 24000 Hz
- **Test**: OGG Opus generation with encoding overhead included

### Typical Generation Times
| Text Length | Audio Duration | Generation Time |
|-------------|----------------|-----------------|
| Short (2-3 sec) | 2-3 sec | ~0.5-0.7 sec |
| Medium (5-10 sec) | 5-10 sec | ~1.2-2.3 sec |
| Long (15-20 sec) | 15-20 sec | ~3.5-4.5 sec |

**Note:** File endpoint (`/v1/audio/speech`) adds ~200-500ms for encoding depending on format.

## Notes
- Text is automatically split into chunks by punctuation (`.!?`)
- Each chunk is processed independently
- File endpoint accumulates all audio before encoding
- Streaming endpoint returns progressive audio generation
- Supports English language only
- OGG/Opus format is recommended for Telegram voice messages
