---
name: file-hosting
description: Uploading large files via upload_file and getting a link.
triggers: [upload, link, gofile, 50mb, 4gb]
allowed_tools: [upload_file]
weight: medium
---
## File Hosting (when Telegram does not accept):
- **upload_file**: upload a file from the sandbox to GoFile and get a link for the user
  - ALWAYS use for files > 50 MB (Telegram limit)
  - Upload limit: 4 GB. If the file is larger — the task is impossible
  - After successful upload, the file is deleted from the sandbox

## FILES: Delivery to User
- Up to 50 MB: `send_file_to_user`
- 50 MB – 4 GB: `upload_file`
- Over 4 GB: task impossible — inform the user of refusal

## ⚠️ CRITICAL: Sandbox Cleanup after Upload

**MANDATORY rules for working with `upload_file`:**

1. **Check upload success:**
   - After calling `upload_file`, **MUST** check the tool result
   - Ensure a valid file link is received (starts with `https://`)
   - If the result is an error — **DO NOT delete** the file, inform the user of the problem

2. **Delete file after successful upload:**
   - ✅ If upload is successful and link is received — **IMMEDIATELY delete** the file from the sandbox
   - Use command: `execute_command` with `rm /workspace/filename`
   - **DO NOT litter in the sandbox** — it is a shared resource

3. **Sequence of actions:**
   ```
   1. Call upload_file with path to file
   2. Receive result and check for link
   3. If link exists → delete file via execute_command
   4. If error → DO NOT delete file, inform user
   ```

**Example of correct workflow:**
```markdown
User: "Upload big file video.mp4 to cloud"

Agent:
1. [Calls upload_file for /workspace/video.mp4]
2. [Receives result: "https://gofile.io/d/abc123"]
3. [Checks: link is valid ✅]
4. [Calls execute_command: "rm /workspace/video.mp4"]
5. Answer: "File successfully uploaded: https://gofile.io/d/abc123. Local copy deleted from sandbox."
```

**❌ ERRORS to avoid:**
- DO NOT delete the file BEFORE checking the upload result
- DO NOT leave files in the sandbox after successful upload
- DO NOT delete the file if the upload failed
