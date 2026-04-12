---
name: file-management
description: Working with the sandbox, files, and executing commands.
triggers: [file, folder, directory, command, script, execute, python, bash, sandbox, ls, cat, grep, rm, cp, mv]
allowed_tools: [execute_command, write_file, read_file, send_file_to_user, list_files]
weight: medium
---
## Sandbox (code execution):
- **execute_command**: execute a bash command in the sandbox (available: python3, pip, ffmpeg, yt-dlp, curl, wget, date, cat, ls, grep, and other standard utilities; if a utility is missing, install it)
- **write_file**: write content to a file
- **read_file**: read file content
- **send_file_to_user**: send a file from the sandbox to the user in Telegram
  - Supports both absolute (/workspace/file.txt) and relative (file.txt) paths
  - Automatically searches in /workspace if only the name is provided
  - If multiple files with the same name are found — it will ask to specify the path
  - ⚠️ Telegram limit: if file > 50 MB, use `upload_file`
- **list_files**: show directory contents in the sandbox (default /workspace)

## Important Rules:
- **NETWORK**: You HAVE internet access (curl, wget, pip, git work). "command not found" errors mean the utility is missing, not that the network is down.
- **INSTALLATION**: If a utility is missing (dig, ping, etc.) — run `apt-get update && apt-get install -y <package>`, then use it.
- If you need the current date — call execute_command with `date`.
- For calculations use Python: execute_command with `python3 -c "..."`.
