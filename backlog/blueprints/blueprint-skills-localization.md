# Blueprint: Skills Localization (Russian to English)

**Status**: Draft
**Feature**: Localization of Agent Skills
**Slug**: skills-localization

This blueprint outlines the systematic translation of all agent skill files in `@skills/` from Russian to English. The goal is to fully localize the agent's operating language, ensuring triggers and instructions are optimized for English-speaking users and LLM reasoning.

## Phase 1: Core Personality & Workflow Logic [ ]

**Goal**: Translate the fundamental operating rules and task planning capabilities. This defines "who" the agent is.

**Resource Context**:
- 📄 `skills/core.md`
- 📄 `skills/task-planning.md`

**Steps**:
1. [ ] **Verify JSON Schema**: Check `core.md` to ensure the JSON response schema examples remain strictly valid JSON (keys in quotes, no trailing commas).
2. [ ] **Translate `core.md`**:
   - **Frontmatter**:
     - `description`: "Basic agent behavior rules, response format, and context management."
     - `triggers`: `[rules, format, context, memory, instruction]`
   - **Body**: Translate "Важные правила" (Important Rules), "КРИТИЧЕСКИ ВАЖНО" (CRITICAL), and Memory sections.
   - **Constraint**: Ensure the JSON structure instructions are crystal clear in English ("You MUST return ONLY valid JSON...").
3. [ ] **Translate `task-planning.md`**:
   - **Frontmatter**:
     - `description`: "Multistep task planning and status management via write_todos."
     - `triggers`: `[plan, step, research, compare, analysis, todo list, tasks]`
   - **Body**: Translate "When to use", "How to use", and the task lifecycle (pending -> in_progress -> completed).

## Phase 2: System Capabilities & Delegation [ ]

**Goal**: Localize instructions for file operations, sandbox usage, and sub-agent delegation.

**Resource Context**:
- 📄 `skills/file-management.md`
- 📄 `skills/file-hosting.md`
- 📄 `skills/delegation_manager.md`

**Steps**:
1. [ ] **Translate `file-management.md`**:
   - **Frontmatter**: `triggers`: `[file, folder, directory, command, script, execute, python, bash, sandbox, ls, cat, grep, rm, cp, mv]`
   - **Body**: Translate Sandbox rules (Network access, apt-get usage, tool availability).
2. [ ] **Translate `file-hosting.md`**:
   - **Frontmatter**: `triggers`: `[upload, link, gofile, 50mb, 4gb]`
   - **Body**: Translate the decision logic (Send direct vs Upload) and the **CRITICAL** cleanup rules (Upload -> Check Link -> Delete Local).
3. [ ] **Translate `delegation_manager.md`**:
   - **Frontmatter**: `triggers`: `[delegate, subagent, subtask, research, overview, comparison, dataset, git, clone, repo, scan, file reading, indexing, study]`
   - **Body**: Translate "When to delegate" (bulk work, file system heavy ops) and "How to phrase tasks".

## Phase 3: Specialized Tools & Media [ ]

**Goal**: Localize domain-specific skills for web search, media processing, and reporting.

**Resource Context**:
- 📄 `skills/web-search.md`
- 📄 `skills/video-processing.md`
- 📄 `skills/ffmpeg-conversion.md`
- 📄 `skills/html-report.md`

**Steps**:
1. [ ] **Translate `web-search.md`**:
   - **Frontmatter**: `triggers`: `[find, search, look up, current, news, docs]`
   - **Body**: Clarify `web_search` (current info) vs `web_extract` (reading content).
2. [ ] **Translate `video-processing.md`**:
   - **Frontmatter**: `triggers`: `[video, youtube, download, transcript, subtitle]`
   - **Body**: Translate `yt-dlp` tool usage and **CRITICAL** error handling (Fatal vs Temporary errors).
3. [ ] **Translate `ffmpeg-conversion.md`**:
   - **Frontmatter**: `triggers`: `[ffmpeg, convert, encode, transcode, video, audio, codec, bitrate]`
   - **Body**: Translate all explanatory text.
   - **Code**: Translate comments inside bash blocks (e.g., `# MP4 -> MKV (no transcoding)`).
4. [ ] **Translate `html-report.md`**:
   - **Frontmatter**: `triggers`: `[html, report, web, design, page, css, layout]`
   - **Body**: Translate "Playful Material Design 3" principles and CSS class descriptions.

## Phase 4: Final Validation [ ]

**Goal**: Ensure all markdown files have valid YAML frontmatter and consistent terminology.

**Steps**:
1. [ ] **Manual Review**: Check that no Russian text remains in descriptions or comments.
2. [ ] **Consistency Check**: Ensure terms "Sandbox", "Sub-agent", and "Tool" are capitalized and used consistently across all files.
