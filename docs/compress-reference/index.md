# Dynamic Context Pruning — Code Reference for Rust Reimplementation

> Atomic reference extracted from opencode-dcp TypeScript codebase.
> All code blocks are verbatim from the source.

---

## Table of Contents

| #   | File                                                       | Topic                                                                                                   |
| --- | ---------------------------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| 1   | [01-data-structures.md](01-data-structures.md)             | Core types: `WithParts`, `CompressionBlock`, `PruneMessagesState`, `SessionState`, `ToolParameterEntry` |
| 2   | [02-tool-registration.md](02-tool-registration.md)         | Range mode & message mode tool schemas, mode selection at registration                                  |
| 3   | [03-compression-pipeline.md](03-compression-pipeline.md)   | Prepare → execute → finalize flow, full range-mode handler (5 phases)                                   |
| 4   | [04-boundary-resolution.md](04-boundary-resolution.md)     | Boundary lookup, resolution with validation, selection gathering, non-overlapping check                 |
| 5   | [05-block-nesting.md](05-block-nesting.md)                 | Parsing `(bN)` placeholders, expanding into stored summaries, `restoreSummary`                          |
| 6   | [06-state-application.md](06-state-application.md)         | Summary wrapping (`wrapCompressedSummary`), full `applyCompressionState` with delta calculation         |
| 7   | [07-transform-pipeline.md](07-transform-pipeline.md)       | 10-step message transform, `filterCompressedRanges`, synthetic message creation, tool pruning           |
| 8   | [08-protected-content.md](08-protected-content.md)         | Protected user messages & tool outputs (appended verbatim to summaries)                                 |
| 9   | [09-strategy-dedup.md](09-strategy-dedup.md)               | Tool call deduplication by signature, keep-last semantics                                               |
| 10  | [10-strategy-purge-errors.md](10-strategy-purge-errors.md) | Errored tool input purging after N turns                                                                |
| 11  | [11-protected-patterns.md](11-protected-patterns.md)       | Glob matching, file path extraction from tool params, protection checks                                 |
| 12  | [12-message-ids.md](12-message-ids.md)                     | `m0001`/`bN` ref formatting, stable ID assignment, XML tag generation                                   |
| 13  | [13-block-sync.md](13-block-sync.md)                       | Block deactivation/reactivation on message stream changes                                               |
| 14  | [14-nudge-injection.md](14-nudge-injection.md)             | Context-limit, turn-boundary, iteration-warning nudges; message ID injection                            |
| 15  | [15-config-defaults.md](15-config-defaults.md)             | All default config values, protected tools list, config layering order                                  |
| 16  | [16-examples.md](16-examples.md)                           | End-to-end walkthroughs (range, nesting, dedup, purge-errors) + architectural invariants                |
