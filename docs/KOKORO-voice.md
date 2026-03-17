# Kokoro TTS API Reference

127.0.0.1:8000 - Здесь сейчас стоит сервер, он размещён локально на хосте где запущено само приложение (работает как сервис systemd, вне контернизации)

## Endpoint
```
POST http://127.0.0.1:8000/v1/audio/speech/stream
Content-Type: application/json
```
## Request Parameters
| Parameter | Type | Description | Example |
|-----------|------|-------------|---------|
| `text` | string | Text to synthesize | `"Hello world"` |
| `lang` | string | Language code (`"en"` or `"ru"`) | `"en"` |
| `voice` | string | Voice name | `"af_bella"`, `"af_aoede"`, `"af_alloy"` |
| `speed` | float | Speech speed (default: 1.0) | `1.0` |
## Available Voices
- `af_bella` (default)
- `af_aoede`
- `af_alloy`
## Audio Format
- **Format**: Raw PCM (signed 16-bit)
- **Sample Rate**: 24000 Hz
- **Channels**: 1 (mono)
- **Content-Type**: `audio/pcm`
## Examples
### Short text (English)
```bash
curl -s http://127.0.0.1:8000/v1/audio/speech/stream \
  -H "Content-Type: application/json" \
  -o output.pcm \
  -d '{
    "text": "Hello, this is a test.",
    "lang": "en",
    "voice": "af_bella",
    "speed": 1.0
  }'
```
### Long text (English)
```bash
curl -s http://127.0.0.1:8000/v1/audio/speech/stream \
  -H "Content-Type: application/json" \
  -o output.pcm \
  -d '{
    "text": "The quick brown fox jumps over the lazy dog near the riverbank on a bright sunny morning while birds sing melodious tunes in the trees above.",
    "lang": "en",
    "voice": "af_bella",
    "speed": 1.0
  }'
```
### Long unique text (English benchmark)
```bash
curl -s http://127.0.0.1:8000/v1/audio/speech/stream \
  -H "Content-Type: application/json" \
  -o output.pcm \
  -d '{
    "text": "The quick brown fox jumps over lazy dog near riverbank on a bright sunny morning while birds sing melodious tunes in trees above. A gentle breeze flows through meadow as wildflowers sway gracefully in golden sunlight that illuminates peaceful countryside landscape.",
    "lang": "en",
    "voice": "af_bella",
    "speed": 1.0
  }'
```
### Russian text
```bash
curl -s http://127.0.0.1:8000/v1/audio/speech/stream \
  -H "Content-Type: application/json" \
  -o output.pcm \
  -d '{
    "text": "Привет, это тест генерации речи.",
    "lang": "ru",
    "voice": "af_bella",
    "speed": 1.0
  }'
```
## Audio Conversion
### Convert PCM to MP3
```bash
ffmpeg -f s16le -ar 24000 -ac 1 -i output.pcm output.mp3
```
### Convert with silence removal
```bash
ffmpeg -f s16le -ar 24000 -ac 1 -i output.pcm \
  -af "silenceremove=start_periods=1:start_silence=0.2:start_threshold=-40dB:detection=peak,aformat=dblp,areverse,silenceremove=start_periods=1:start_silence=0.2:start_threshold=-40dB:detection=peak,aformat=dblp,areverse" \
  output.mp3
```
## Performance
### Benchmark Results (CPU)
- **RTF (Real-Time Factor)**: 0.226
- **Generation Speed**: 4.43x real-time
- **Memory Usage**: ~4.8G / 8G (60%)
- **Sample Rate**: 24000 Hz
### Typical Generation Times
- Short phrase (2-3 sec): ~0.5-1 sec
- Medium text (5-10 sec): ~2-3 sec
- Long text (15-20 sec): ~4-5 sec
## Notes
- Text is automatically split into chunks by punctuation (`.!?`)
- Each chunk is processed independently
- Returns streaming response (progressive audio generation)
- Supports both English and Russian languages