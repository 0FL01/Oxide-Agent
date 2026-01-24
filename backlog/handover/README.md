# Handover Notes

This directory contains handover notes for completed major tasks and architectural changes.

## Format

Each handover note follows this structure:
- **Filename:** `{task-slug}.md` (short descriptive name)
- **Content:**
  - Timestamp (ISO 8601)
  - What was planned
  - What was done
  - Mines/blocks encountered and solutions
  - Remaining TODOs
  - Quality metrics
  - Git commits

## Handover Notes

| Date | Task | Status | Commit |
|-------|-------|---------|---------|

---

## Purpose

Handover notes serve as:
1. **Knowledge transfer** — Between developers and teams
2. **Architecture documentation** — Decisions made and rationale
3. **Historical record** — What was changed and why
4. **Future reference** — Known limitations, TODOs, future work

## Usage

When starting work on a feature or debugging an issue:
1. Check this directory for relevant handover notes
2. Read the "What was done" section for implementation details
3. Review "Mines/blocks" for known issues and solutions
4. Check "Remaining TODOs" for future work

## Creating New Handover Notes

Template:
```markdown
# Handover Note: {Task Name}

**Timestamp:** YYYY-MM-DDTHH:MM:SSZ
**Team:** {Team Name}
**Status:** {COMPLETED/IN PROGRESS/BLOCKED}

---

## 📋 ЧТО ПЛАНИРОВАЛИ

...

## ✅ ЧТО СДЕЛАЛИ

...

## 💥 НА КАКИХ МИНАХ ПОДОРВАЛИСЬ И КАК ИСПРАВИЛИ

...

## 📊 СТАТУС ВЫПОЛНЕНИЯ

...

## 🎯 ОСТАВШИЕСЯ TODO

...

## ✅ КАЧЕСТВЕННЫЕ ПОКАЗАТЕЛИ

...

## 📝 Git Commits

...

## 🎉 ИТОГ

...

---

**Handover complete.** 🚀
```

Save as `{task-slug}.md` in this directory.
