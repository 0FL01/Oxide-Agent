---
name: video-processing
description: Search, metadata, and downloading videos via ytdlp tools.
triggers: [video, youtube, download, transcript, subtitle]
allowed_tools:
  - ytdlp_get_video_metadata
  - ytdlp_download_transcript
  - ytdlp_search_videos
  - ytdlp_download_video
  - ytdlp_download_audio
weight: on_demand
---
## Video Platforms (YouTube, etc.):
- **ytdlp_get_video_metadata**: get video metadata (title, channel, duration, views, upload date, description, tags, etc.). Does not require downloading the video. Parameters: url (required), fields (optional - array of fields)
- **ytdlp_download_transcript**: download and extract clean transcript text from video. Supports auto-generated and manual subtitles. Parameters: url (required), language (optional, default 'en')
- **ytdlp_search_videos**: search for videos on YouTube. Returns a list of videos with titles, channels, duration, and URLs. Parameters: query (required), max_results (optional, 1-20, default 5)
- **ytdlp_download_video**: download video to sandbox. Supports resolution selection and time trimming. After downloading, use `send_file_to_user` to send to the user. Parameters: url (required), resolution (optional: '480', '720', '1080', 'best' — default '720'), start_time (optional), end_time (optional)
- **ytdlp_download_audio**: extract and download audio from video in MP3 format. After downloading, use `send_file_to_user` to send to the user. Parameters: url (required)

### ⚠️ CRITICAL: Handling yt-dlp Errors

**If a ytdlp_* tool returns a message with ❌ — STOP IMMEDIATELY!**

Typical fatal errors:
- Video unavailable or deleted
- Private video (requires authorization)
- Geo-blocking (blocked in server region)
- Age restrictions (requires age verification)
- Members-only content
- Video removed by copyright holder
- Invalid or unsupported link

**Rules for fatal errors:**
1. ❌ **DO NOT try** the same request again — it won't help
2. ❌ **DO NOT create workarounds** (curl, wget, alternative services)
3. ❌ **DO NOT offer the user** to "try again later" if the error is fatal
4. ✅ **INFORM the user** of the error reason from the tool message
5. ✅ **COMPLETE the task** and offer other assistance

**Temporary errors (with ⚠️):**
- If an error is marked as "temporary" — you may try repeating 1-2 times
- This applies to network issues, timeouts, rate limits
- But NOT fatal errors (unavailability, privacy, geo-blocking)
