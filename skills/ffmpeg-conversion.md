---
name: ffmpeg-conversion
description: Video and audio conversion using ffmpeg (software acceleration only).
triggers: [ffmpeg, convert, encode, transcode, video, audio, mp4, mkv, avi, webm, mp3, aac, flac, wav, codec, bitrate]
allowed_tools: [execute_command, read_file, write_file, send_file_to_user, list_files]
weight: on_demand
---
## FFmpeg - Video and Audio Conversion

FFmpeg is available in the sandbox and supports conversion between various formats using **software acceleration only** (no GPU/hardware codecs).

### 🎬 Video Conversion

#### Basic Conversion (Container Change)
```bash
# MP4 → MKV (no transcoding)
ffmpeg -i input.mp4 -c copy output.mkv

# AVI → MP4 (no transcoding)
ffmpeg -i input.avi -c copy output.mp4
```

#### Conversion with Video Transcoding
```bash
# Any format → MP4 (H.264 software codec, CRF 23 quality)
ffmpeg -i input.mkv -c:v libx264 -crf 23 -preset medium -c:a copy output.mp4

# Any format → WebM (VP9 software codec)
ffmpeg -i input.mp4 -c:v libvpx-vp9 -crf 30 -b:v 0 -c:a libopus output.webm

# Conversion with resolution change (1080p → 720p)
ffmpeg -i input.mp4 -vf scale=1280:720 -c:v libx264 -crf 23 -preset medium -c:a copy output.mp4

# Conversion with FPS change (60fps → 30fps)
ffmpeg -i input.mp4 -r 30 -c:v libx264 -crf 23 -preset medium -c:a copy output.mp4
```

#### Popular Software Video Codecs
- **libx264** - H.264/AVC (universal, fast, compatible)
- **libx265** - H.265/HEVC (better compression, slower)
- **libvpx-vp9** - VP9 (open-source, for WebM)
- **libaom-av1** - AV1 (newest, best compression, very slow)

### 🎵 Audio Conversion

#### Basic Audio Conversion
```bash
# Any format → MP3 (CBR 192 kbps)
ffmpeg -i input.wav -c:a libmp3lame -b:a 192k output.mp3

# Any format → AAC (VBR quality 2, ~128 kbps)
ffmpeg -i input.flac -c:a aac -q:a 2 output.m4a

# Any format → FLAC (lossless)
ffmpeg -i input.mp3 -c:a flac output.flac

# Any format → Opus (VBR 128 kbps)
ffmpeg -i input.wav -c:a libopus -b:a 128k output.opus

# WAV → MP3 (VBR quality 0, ~245 kbps)
ffmpeg -i input.wav -c:a libmp3lame -q:a 0 output.mp3
```

#### Extracting Audio from Video
```bash
# Extract audio to MP3
ffmpeg -i video.mp4 -vn -c:a libmp3lame -b:a 192k audio.mp3

# Extract audio without transcoding
ffmpeg -i video.mkv -vn -c:a copy audio.aac
```

### ⚙️ Quality Parameters

#### Video (H.264/H.265)
- **CRF (Constant Rate Factor)**: 0-51, lower = better quality
  - `18` - visually lossless
  - `23` - **recommended** balance of quality/size
  - `28` - noticeable compression
- **Preset**: encoding speed
  - `ultrafast`, `superfast`, `veryfast`, `faster`, `fast`
  - `medium` - **recommended** balance
  - `slow`, `slower`, `veryslow` - better compression

#### Audio
- **MP3 Bitrate**: 128k (normal), 192k (good), 320k (excellent)
- **MP3 VBR**: `-q:a 0` (better) to `-q:a 9` (worse)
- **AAC VBR**: `-q:a 0` (better) to `-q:a 9` (worse)

### 🛠️ Additional Operations

#### Trimming Video/Audio by Time
```bash
# Trim starting at 00:01:30 for 45 seconds
ffmpeg -i input.mp4 -ss 00:01:30 -t 45 -c copy output.mp4

# Trim from start to 1 minute
ffmpeg -i input.mp4 -t 60 -c copy output.mp4
```

#### Merging Video/Audio
```bash
# Create file list
echo "file 'part1.mp4'" > list.txt
echo "file 'part2.mp4'" >> list.txt

# Merge
ffmpeg -f concat -safe 0 -i list.txt -c copy output.mp4
```

#### Replacing Audio in Video
```bash
# Replace audio track
ffmpeg -i video.mp4 -i audio.mp3 -c:v copy -c:a aac -map 0:v:0 -map 1:a:0 output.mp4
```

#### Changing Video Bitrate
```bash
# Set bitrate to 2 Mbps
ffmpeg -i input.mp4 -b:v 2M -c:a copy output.mp4
```

### 📊 Getting File Information

```bash
# Show detailed media file info
ffmpeg -i input.mp4

# Only basic characteristics (ffprobe)
ffprobe -v quiet -print_format json -show_format -show_streams input.mp4
```

### ⚠️ Important Rules and Limitations

**SOFTWARE Acceleration:**
- ❌ **DO NOT use** hardware codecs: `-c:v h264_nvenc`, `-c:v h264_qsv`, `-c:v h264_vaapi`, `-hwaccel cuda`
- ✅ **ALWAYS use** software codecs: `libx264`, `libx265`, `libvpx-vp9`, `libaom-av1`

**Performance:**
- Software encoding is slower than hardware (especially H.265 and AV1)
- Expect significant processing time for large files
- Use `preset medium` or `fast` for speed/quality balance

**Overwriting Files:**
- FFmpeg requires overwrite confirmation: add `-y` for automatic overwrite
```bash
ffmpeg -y -i input.mp4 -c:v libx264 output.mp4
```

**Path Handling:**
- After conversion use `send_file_to_user` to send the result
- Check for source file existence via `list_files` before conversion

**Error Handling:**
- If FFmpeg returns an error — analyze the message and inform the user of the reason
- Common issues: incompatible codec, corrupted file, insufficient space
- DO NOT try to infinitely repeat the same command on fatal error

### 📝 Typical Task Examples

**Example 1: Convert MKV to MP4 for compatibility**
```bash
# Transcode video to H.264, audio to AAC
ffmpeg -y -i video.mkv -c:v libx264 -crf 23 -preset medium -c:a aac -b:a 192k video.mp4
```

**Example 2: Compress large video**
```bash
# Reduce resolution to 720p and apply CRF 28
ffmpeg -y -i large_video.mp4 -vf scale=1280:720 -c:v libx264 -crf 28 -preset medium -c:a copy compressed.mp4
```

**Example 3: Extract audio from video to FLAC**
```bash
ffmpeg -y -i concert.mkv -vn -c:a flac concert.flac
```

**Example 4: Create GIF from video**
```bash
# Convert first 5 seconds to GIF 480p, 10 fps
ffmpeg -y -i input.mp4 -t 5 -vf "fps=10,scale=480:-1:flags=lanczos" -c:v gif output.gif
```

**Example 5: Convert to modern AV1 format**
```bash
# WARNING: AV1 is very slow for software encoding!
ffmpeg -y -i input.mp4 -c:v libaom-av1 -crf 30 -b:v 0 -c:a libopus -b:a 128k output.mkv
```
