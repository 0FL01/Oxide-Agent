# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- File upload support: users can now send documents to the agent in Telegram
- Files are automatically saved to `/workspace/uploads/` in the sandbox
- 1 GB total upload limit per session
- Smart file type hints for common formats (code, data, archives, images, etc.)
- File download support: agent can send files from sandbox to user via `send_file_to_user` tool
- 50 MB file size limit for downloads (Telegram API limit)
