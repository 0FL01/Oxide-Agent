//! Deterministic cleanup for superseded duplicate tool results.

use super::budget::estimate_message_tokens;
use super::types::{AgentMessageKind, CompactionSnapshot, DedupSupersededOutcome};
use crate::agent::memory::AgentMessage;
use crate::llm::ToolCall;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path};
use tracing::warn;

/// Placeholder written into older tool results once a later identical output supersedes them.
pub const SUPERSEDED_DEDUP_PLACEHOLDER: &str =
    "[deduplicated tool result: superseded by later identical output]";

/// Stage-3 scope contract for deterministic superseded-result deduplication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DedupSupersededContract {
    /// Stage-level toggle.
    pub enabled: bool,
    /// Read-only deterministic tools that are eligible for dedup.
    pub tool_allowlist: Vec<String>,
    /// Dedup applies only to old history prior to the latest summary boundary.
    pub only_before_latest_summary_boundary: bool,
    /// Dedup never touches the protected recent tool window.
    pub preserve_protected_recent_tool_window: bool,
    /// Dedup requires canonical tool-name and args equality.
    pub require_canonical_tool_and_args_match: bool,
    /// Dedup requires output fingerprint equality.
    pub require_output_fingerprint_match: bool,
    /// Dedup is blocked when mutating actions occurred between candidate reads.
    pub block_on_intermediate_mutating_actions: bool,
    /// Tools that always block dedup because their mutation scope is ambiguous.
    pub ambiguous_mutating_tool_allowlist: Vec<String>,
    /// Tools that block dedup when their path overlaps the read target.
    pub path_aware_mutating_tool_allowlist: Vec<String>,
    /// Placeholder message for superseded entries.
    pub superseded_placeholder: String,
}

impl Default for DedupSupersededContract {
    fn default() -> Self {
        Self {
            enabled: true,
            tool_allowlist: vec!["read_file".to_string()],
            only_before_latest_summary_boundary: true,
            preserve_protected_recent_tool_window: true,
            require_canonical_tool_and_args_match: true,
            require_output_fingerprint_match: true,
            block_on_intermediate_mutating_actions: true,
            ambiguous_mutating_tool_allowlist: vec![
                "execute_command".to_string(),
                "sudo_exec".to_string(),
                "recreate_sandbox".to_string(),
            ],
            path_aware_mutating_tool_allowlist: vec![
                "write_file".to_string(),
                "apply_file_edit".to_string(),
            ],
            superseded_placeholder: SUPERSEDED_DEDUP_PLACEHOLDER.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DedupKey {
    canonical_tool_name: String,
    canonical_args_json: String,
    output_fingerprint: String,
}

#[derive(Debug, Clone)]
struct ReadFileCandidate {
    key: DedupKey,
    assistant_index: usize,
    result_index: usize,
    normalized_read_path: NormalizedPath,
    tool_name: String,
    preview: String,
}

struct DedupedMessage {
    message: AgentMessage,
    reclaimed_tokens: usize,
    reclaimed_chars: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedPath {
    is_absolute: bool,
    parts: Vec<String>,
}

const PREVIEW_CHARS: usize = 120;

/// Rewrite old duplicate tool results while preserving the latest full payload.
#[must_use]
pub fn dedup_superseded_tool_results(
    contract: &DedupSupersededContract,
    snapshot: &CompactionSnapshot,
    messages: &[AgentMessage],
) -> (Vec<AgentMessage>, DedupSupersededOutcome) {
    if !contract.enabled {
        return (messages.to_vec(), DedupSupersededOutcome::default());
    }

    let Some(latest_summary_boundary) = latest_summary_boundary(snapshot, contract) else {
        return (messages.to_vec(), DedupSupersededOutcome::default());
    };

    let candidates = collect_candidates(contract, snapshot, messages, latest_summary_boundary);
    if candidates.len() < 2 {
        return (messages.to_vec(), DedupSupersededOutcome::default());
    }

    let mut rewritten = messages.to_vec();
    let mut outcome = DedupSupersededOutcome::default();
    let mut latest_full_by_key: HashMap<DedupKey, ReadFileCandidate> = HashMap::new();

    for candidate in candidates.into_iter().rev() {
        let should_dedup = latest_full_by_key
            .get(&candidate.key)
            .is_some_and(|keeper| {
                !contract.block_on_intermediate_mutating_actions
                    || !has_intermediate_mutating_actions(
                        contract,
                        messages,
                        &candidate,
                        keeper.assistant_index,
                    )
            });

        if should_dedup {
            let Some(deduped) =
                rewrite_as_placeholder(contract, &candidate, messages.get(candidate.result_index))
            else {
                continue;
            };
            rewritten[candidate.result_index] = deduped.message;
            outcome.applied = true;
            outcome.deduplicated_count = outcome.deduplicated_count.saturating_add(1);
            outcome.reclaimed_tokens = outcome
                .reclaimed_tokens
                .saturating_add(deduped.reclaimed_tokens);
            outcome.reclaimed_chars = outcome
                .reclaimed_chars
                .saturating_add(deduped.reclaimed_chars);
            outcome.deduplicated_indices.push(candidate.result_index);
            continue;
        }

        latest_full_by_key.insert(candidate.key.clone(), candidate);
    }

    if outcome.applied {
        warn!(
            deduplicated_count = outcome.deduplicated_count,
            reclaimed_tokens = outcome.reclaimed_tokens,
            reclaimed_chars = outcome.reclaimed_chars,
            deduplicated_indices = ?outcome.deduplicated_indices,
            "Compaction deduplicated superseded tool results"
        );
    }

    (rewritten, outcome)
}

fn latest_summary_boundary(
    snapshot: &CompactionSnapshot,
    contract: &DedupSupersededContract,
) -> Option<usize> {
    let latest_summary_boundary = snapshot
        .entries
        .iter()
        .rev()
        .find(|entry| entry.kind == AgentMessageKind::Summary)
        .map(|entry| entry.index);

    if contract.only_before_latest_summary_boundary {
        latest_summary_boundary
    } else {
        Some(snapshot.entries.len())
    }
}

fn collect_candidates(
    contract: &DedupSupersededContract,
    snapshot: &CompactionSnapshot,
    messages: &[AgentMessage],
    latest_summary_boundary: usize,
) -> Vec<ReadFileCandidate> {
    let mut candidates = Vec::new();
    let mut index = 0usize;

    while index + 1 < snapshot.entries.len() {
        let assistant_entry = &snapshot.entries[index];
        let result_entry = &snapshot.entries[index + 1];

        let Some(candidate) = build_candidate(
            contract,
            assistant_entry,
            result_entry,
            messages,
            latest_summary_boundary,
        ) else {
            index += 1;
            continue;
        };

        candidates.push(candidate);
        index += 2;
    }

    candidates
}

fn build_candidate(
    contract: &DedupSupersededContract,
    assistant_entry: &super::types::ClassifiedMemoryEntry,
    result_entry: &super::types::ClassifiedMemoryEntry,
    messages: &[AgentMessage],
    latest_summary_boundary: usize,
) -> Option<ReadFileCandidate> {
    if assistant_entry.kind != AgentMessageKind::AssistantToolCall
        || result_entry.kind != AgentMessageKind::ToolResult
        || assistant_entry.index >= latest_summary_boundary
        || result_entry.index >= latest_summary_boundary
        || result_entry.is_pruned
        || result_entry.is_externalized
        || (contract.preserve_protected_recent_tool_window
            && (assistant_entry.preserve_in_raw_window || result_entry.preserve_in_raw_window))
    {
        return None;
    }

    let assistant_message = messages.get(assistant_entry.index)?;
    let result_message = messages.get(result_entry.index)?;
    let tool_calls = assistant_message.tool_calls.as_ref()?;
    let correlations = assistant_message.resolved_tool_call_correlations()?;
    if tool_calls.len() != 1 || correlations.len() != 1 {
        return None;
    }

    let tool_call = &tool_calls[0];
    let canonical_tool_name = canonicalize_tool_name(&tool_call.function.name);
    if !tool_allowed(contract, &canonical_tool_name) {
        return None;
    }

    let result_correlation = result_message.resolved_tool_call_correlation()?;
    if result_correlation.invocation_id != correlations[0].invocation_id {
        return None;
    }

    let result_tool_name = canonicalize_tool_name(result_message.tool_name.as_deref()?);
    if canonical_tool_name != result_tool_name {
        return None;
    }

    let canonical_args_json = if contract.require_canonical_tool_and_args_match {
        canonicalize_args_json(&tool_call.function.arguments)?
    } else {
        String::new()
    };
    let output_fingerprint = if contract.require_output_fingerprint_match {
        fingerprint_content(&result_message.content)
    } else {
        String::new()
    };
    Some(ReadFileCandidate {
        key: DedupKey {
            canonical_tool_name,
            canonical_args_json,
            output_fingerprint,
        },
        assistant_index: assistant_entry.index,
        result_index: result_entry.index,
        normalized_read_path: normalize_tool_path(
            &tool_call.function.name,
            &extract_path_argument(&tool_call.function.arguments)?,
        )?,
        tool_name: result_message.tool_name.clone()?,
        preview: build_preview(&result_message.content, PREVIEW_CHARS),
    })
}

fn rewrite_as_placeholder(
    contract: &DedupSupersededContract,
    candidate: &ReadFileCandidate,
    original: Option<&AgentMessage>,
) -> Option<DedupedMessage> {
    let original = original?.clone();
    let mut replacement = original.clone();
    replacement.content = build_placeholder(
        &contract.superseded_placeholder,
        &candidate.tool_name,
        &candidate.preview,
    );
    let reclaimed_tokens =
        estimate_message_tokens(&original).saturating_sub(estimate_message_tokens(&replacement));
    let reclaimed_chars = original
        .content
        .chars()
        .count()
        .saturating_sub(replacement.content.chars().count());
    Some(DedupedMessage {
        message: replacement,
        reclaimed_tokens,
        reclaimed_chars,
    })
}

fn has_intermediate_mutating_actions(
    contract: &DedupSupersededContract,
    messages: &[AgentMessage],
    candidate: &ReadFileCandidate,
    to_assistant_index: usize,
) -> bool {
    messages
        .iter()
        .enumerate()
        .skip(candidate.result_index.saturating_add(1))
        .take(to_assistant_index.saturating_sub(candidate.result_index.saturating_add(1)))
        .any(|(_, message)| message_may_mutate_read_target(contract, message, candidate))
}

fn message_may_mutate_read_target(
    contract: &DedupSupersededContract,
    message: &AgentMessage,
    candidate: &ReadFileCandidate,
) -> bool {
    if message.resolved_kind() != AgentMessageKind::AssistantToolCall {
        return false;
    }

    message.tool_calls.as_ref().is_some_and(|tool_calls| {
        tool_calls
            .iter()
            .any(|tool_call| tool_call_may_mutate_read_target(contract, tool_call, candidate))
    })
}

fn tool_call_may_mutate_read_target(
    contract: &DedupSupersededContract,
    tool_call: &ToolCall,
    candidate: &ReadFileCandidate,
) -> bool {
    let tool_name = canonicalize_tool_name(&tool_call.function.name);
    if contract
        .ambiguous_mutating_tool_allowlist
        .iter()
        .any(|allowed| canonicalize_tool_name(allowed) == tool_name)
    {
        return true;
    }

    if !contract
        .path_aware_mutating_tool_allowlist
        .iter()
        .any(|allowed| canonicalize_tool_name(allowed) == tool_name)
    {
        return false;
    }

    let Some(mutation_path) = extract_path_argument(&tool_call.function.arguments)
        .and_then(|path| normalize_tool_path(&tool_call.function.name, &path))
    else {
        return true;
    };

    paths_overlap(&candidate.normalized_read_path, &mutation_path)
}

fn tool_allowed(contract: &DedupSupersededContract, tool_name: &str) -> bool {
    contract
        .tool_allowlist
        .iter()
        .any(|allowed| canonicalize_tool_name(allowed) == tool_name)
}

fn canonicalize_tool_name(tool_name: &str) -> String {
    tool_name.trim().to_ascii_lowercase()
}

fn canonicalize_args_json(arguments: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(arguments).ok()?;
    serde_json::to_string(&sort_json_value(value)).ok()
}

fn extract_path_argument(arguments: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(arguments).ok()?;
    value.get("path")?.as_str().map(ToOwned::to_owned)
}

fn normalize_tool_path(tool_name: &str, raw_path: &str) -> Option<NormalizedPath> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return None;
    }

    let normalized_separators = trimmed.replace('\\', "/");
    let canonical_tool_name = canonicalize_tool_name(tool_name);
    let workspace_scoped = matches!(
        canonical_tool_name.as_str(),
        "read_file" | "write_file" | "apply_file_edit"
    );
    let normalized = if workspace_scoped && !normalized_separators.starts_with('/') {
        format!("/workspace/{normalized_separators}")
    } else {
        normalized_separators
    };
    let is_absolute = normalized.starts_with('/');
    let mut parts = Vec::new();

    for component in Path::new(&normalized).components() {
        match component {
            Component::Prefix(_) => return None,
            Component::RootDir => {}
            Component::CurDir => {}
            Component::ParentDir => {
                if let Some(last) = parts.last() {
                    if last != ".." {
                        parts.pop();
                    } else if !is_absolute {
                        parts.push("..".to_string());
                    }
                } else if !is_absolute {
                    parts.push("..".to_string());
                }
            }
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
        }
    }

    Some(NormalizedPath { is_absolute, parts })
}

fn paths_overlap(left: &NormalizedPath, right: &NormalizedPath) -> bool {
    if left.is_absolute != right.is_absolute {
        return false;
    }

    is_prefix_path(left, right) || is_prefix_path(right, left)
}

fn is_prefix_path(prefix: &NormalizedPath, candidate: &NormalizedPath) -> bool {
    prefix.parts.len() <= candidate.parts.len()
        && prefix
            .parts
            .iter()
            .zip(candidate.parts.iter())
            .all(|(left, right)| left == right)
}

fn sort_json_value(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(sort_json_value).collect()),
        Value::Object(map) => {
            let sorted = map.into_iter().collect::<BTreeMap<_, _>>();
            let mut canonical = serde_json::Map::with_capacity(sorted.len());
            for (key, value) in sorted {
                canonical.insert(key, sort_json_value(value));
            }
            Value::Object(canonical)
        }
        other => other,
    }
}

fn fingerprint_content(content: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(content.as_bytes());
    format!("{:x}", digest.finalize())
}

fn build_preview(content: &str, preview_chars: usize) -> String {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return "(empty output)".to_string();
    }

    let mut preview: String = trimmed.chars().take(preview_chars).collect();
    if trimmed.chars().count() > preview_chars {
        preview.push_str("...");
    }
    preview
}

fn build_placeholder(prefix: &str, tool_name: &str, preview: &str) -> String {
    format!("[deduplicated tool result]\ntool: {tool_name}\nreason: superseded by later identical output\nstatus: {prefix}\npreview:\n{preview}")
}

#[cfg(test)]
mod tests {
    use super::{dedup_superseded_tool_results, DedupSupersededContract};
    use crate::agent::compaction::{classify_hot_memory_with_policy, CompactionPolicy};
    use crate::agent::memory::AgentMessage;
    use crate::llm::{ToolCall, ToolCallFunction};

    fn tool_call(id: &str, name: &str, arguments: &str) -> ToolCall {
        ToolCall::new(
            id.to_string(),
            ToolCallFunction {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
            false,
        )
    }

    #[test]
    fn stage_three_contract_is_conservative_and_enabled() {
        let contract = DedupSupersededContract::default();
        assert!(contract.enabled);
        assert_eq!(contract.tool_allowlist, vec!["read_file"]);
        assert!(contract.only_before_latest_summary_boundary);
        assert!(contract.preserve_protected_recent_tool_window);
        assert!(contract.require_canonical_tool_and_args_match);
        assert!(contract.require_output_fingerprint_match);
        assert!(contract.block_on_intermediate_mutating_actions);
        assert_eq!(
            contract.ambiguous_mutating_tool_allowlist,
            vec!["execute_command", "sudo_exec", "recreate_sandbox"]
        );
        assert_eq!(
            contract.path_aware_mutating_tool_allowlist,
            vec!["write_file", "apply_file_edit"]
        );
        assert!(contract
            .superseded_placeholder
            .contains("superseded by later identical output"));
    }

    #[test]
    fn dedup_rewrites_older_identical_read_file_result() {
        let messages = vec![
            AgentMessage::assistant_with_tools(
                "Read first copy",
                vec![tool_call(
                    "call-1",
                    "read_file",
                    r#"{"path":"src/lib.rs","line":1}"#,
                )],
            ),
            AgentMessage::tool("call-1", "read_file", "alpha\nbeta\ngamma"),
            AgentMessage::assistant_with_tools(
                "Read second copy",
                vec![tool_call(
                    "call-2",
                    "read_file",
                    r#"{"line":1,"path":"src/lib.rs"}"#,
                )],
            ),
            AgentMessage::tool("call-2", "read_file", "alpha\nbeta\ngamma"),
            AgentMessage::summary("[Previous context compressed]\n- earlier work preserved"),
            AgentMessage::assistant_with_tools(
                "Keep recent raw window",
                vec![tool_call("call-3", "search", "{}")],
            ),
            AgentMessage::tool("call-3", "search", "recent result"),
        ];
        let snapshot = classify_hot_memory_with_policy(
            &messages,
            &CompactionPolicy {
                protected_tool_window_tokens: 1,
                ..CompactionPolicy::default()
            },
            None,
        );

        let (rewritten, outcome) = dedup_superseded_tool_results(
            &DedupSupersededContract::default(),
            &snapshot,
            &messages,
        );

        assert!(outcome.applied);
        assert_eq!(outcome.deduplicated_count, 1);
        assert_eq!(outcome.deduplicated_indices, vec![1]);
        assert!(rewritten[1].content.contains("[deduplicated tool result]"));
        assert!(rewritten[1].content.contains("tool: read_file"));
        assert!(rewritten[1].content.contains("alpha\nbeta\ngamma"));
        assert_eq!(rewritten[3].content, "alpha\nbeta\ngamma");
    }

    #[test]
    fn dedup_allows_unrelated_non_mutating_tool_between_reads() {
        let messages = vec![
            AgentMessage::assistant_with_tools(
                "Read first copy",
                vec![tool_call("call-1", "read_file", r#"{"path":"src/lib.rs"}"#)],
            ),
            AgentMessage::tool("call-1", "read_file", "same output"),
            AgentMessage::assistant_with_tools(
                "Search in between",
                vec![tool_call("call-2", "search", r#"{"query":"lib"}"#)],
            ),
            AgentMessage::tool("call-2", "search", "result"),
            AgentMessage::assistant_with_tools(
                "Read second copy",
                vec![tool_call("call-3", "read_file", r#"{"path":"src/lib.rs"}"#)],
            ),
            AgentMessage::tool("call-3", "read_file", "same output"),
            AgentMessage::summary("[Previous context compressed]\n- earlier work preserved"),
            AgentMessage::assistant_with_tools(
                "Keep recent raw window",
                vec![tool_call("call-4", "search", "{}")],
            ),
            AgentMessage::tool("call-4", "search", "recent result"),
        ];
        let snapshot = classify_hot_memory_with_policy(
            &messages,
            &CompactionPolicy {
                protected_tool_window_tokens: 1,
                ..CompactionPolicy::default()
            },
            None,
        );

        let (rewritten, outcome) = dedup_superseded_tool_results(
            &DedupSupersededContract::default(),
            &snapshot,
            &messages,
        );

        assert!(outcome.applied);
        assert_eq!(outcome.deduplicated_indices, vec![1]);
        assert!(rewritten[1].content.contains("[deduplicated tool result]"));
        assert_eq!(rewritten[5].content, "same output");
    }

    #[test]
    fn dedup_blocks_when_related_write_file_intervenes() {
        let messages = vec![
            AgentMessage::assistant_with_tools(
                "Read first copy",
                vec![tool_call("call-1", "read_file", r#"{"path":"src/lib.rs"}"#)],
            ),
            AgentMessage::tool("call-1", "read_file", "same output"),
            AgentMessage::assistant_with_tools(
                "Write file in between",
                vec![tool_call(
                    "call-2",
                    "write_file",
                    r#"{"path":"/workspace/src/./lib.rs","content":"changed"}"#,
                )],
            ),
            AgentMessage::tool("call-2", "write_file", "ok"),
            AgentMessage::assistant_with_tools(
                "Read second copy",
                vec![tool_call(
                    "call-3",
                    "read_file",
                    r#"{"path":"/workspace/src/lib.rs"}"#,
                )],
            ),
            AgentMessage::tool("call-3", "read_file", "same output"),
            AgentMessage::summary("[Previous context compressed]\n- earlier work preserved"),
            AgentMessage::assistant_with_tools(
                "Keep recent raw window",
                vec![tool_call("call-4", "search", "{}")],
            ),
            AgentMessage::tool("call-4", "search", "recent result"),
        ];
        let snapshot = classify_hot_memory_with_policy(
            &messages,
            &CompactionPolicy {
                protected_tool_window_tokens: 1,
                ..CompactionPolicy::default()
            },
            None,
        );

        let (rewritten, outcome) = dedup_superseded_tool_results(
            &DedupSupersededContract::default(),
            &snapshot,
            &messages,
        );

        assert!(!outcome.applied);
        assert_eq!(rewritten[1].content, "same output");
        assert_eq!(rewritten[5].content, "same output");
    }

    #[test]
    fn dedup_allows_unrelated_write_file_between_reads() {
        let messages = vec![
            AgentMessage::assistant_with_tools(
                "Read first copy",
                vec![tool_call("call-1", "read_file", r#"{"path":"src/lib.rs"}"#)],
            ),
            AgentMessage::tool("call-1", "read_file", "same output"),
            AgentMessage::assistant_with_tools(
                "Write unrelated file",
                vec![tool_call(
                    "call-2",
                    "write_file",
                    r#"{"path":"docs/readme.md","content":"changed"}"#,
                )],
            ),
            AgentMessage::tool("call-2", "write_file", "ok"),
            AgentMessage::assistant_with_tools(
                "Read second copy",
                vec![tool_call("call-3", "read_file", r#"{"path":"src/lib.rs"}"#)],
            ),
            AgentMessage::tool("call-3", "read_file", "same output"),
            AgentMessage::summary("[Previous context compressed]\n- earlier work preserved"),
            AgentMessage::assistant_with_tools(
                "Keep recent raw window",
                vec![tool_call("call-4", "search", "{}")],
            ),
            AgentMessage::tool("call-4", "search", "recent result"),
        ];
        let snapshot = classify_hot_memory_with_policy(
            &messages,
            &CompactionPolicy {
                protected_tool_window_tokens: 1,
                ..CompactionPolicy::default()
            },
            None,
        );

        let (rewritten, outcome) = dedup_superseded_tool_results(
            &DedupSupersededContract::default(),
            &snapshot,
            &messages,
        );

        assert!(outcome.applied);
        assert_eq!(outcome.deduplicated_indices, vec![1]);
        assert!(rewritten[1].content.contains("[deduplicated tool result]"));
        assert_eq!(rewritten[5].content, "same output");
    }

    #[test]
    fn dedup_blocks_when_ambiguous_command_intervenes() {
        let messages = vec![
            AgentMessage::assistant_with_tools(
                "Read first copy",
                vec![tool_call("call-1", "read_file", r#"{"path":"src/lib.rs"}"#)],
            ),
            AgentMessage::tool("call-1", "read_file", "same output"),
            AgentMessage::assistant_with_tools(
                "Run shell command in between",
                vec![tool_call(
                    "call-2",
                    "execute_command",
                    r#"{"command":"grep -n foo src/lib.rs"}"#,
                )],
            ),
            AgentMessage::tool(
                "call-2",
                "execute_command",
                r#"{"ok":true,"stdout":"1:foo","stderr":"","exit_code":0}"#,
            ),
            AgentMessage::assistant_with_tools(
                "Read second copy",
                vec![tool_call("call-3", "read_file", r#"{"path":"src/lib.rs"}"#)],
            ),
            AgentMessage::tool("call-3", "read_file", "same output"),
            AgentMessage::summary("[Previous context compressed]\n- earlier work preserved"),
            AgentMessage::assistant_with_tools(
                "Keep recent raw window",
                vec![tool_call("call-4", "search", "{}")],
            ),
            AgentMessage::tool("call-4", "search", "recent result"),
        ];
        let snapshot = classify_hot_memory_with_policy(
            &messages,
            &CompactionPolicy {
                protected_tool_window_tokens: 1,
                ..CompactionPolicy::default()
            },
            None,
        );

        let (rewritten, outcome) = dedup_superseded_tool_results(
            &DedupSupersededContract::default(),
            &snapshot,
            &messages,
        );

        assert!(!outcome.applied);
        assert_eq!(rewritten[1].content, "same output");
        assert_eq!(rewritten[5].content, "same output");
    }

    #[test]
    fn dedup_blocks_when_related_apply_file_edit_intervenes() {
        let messages = vec![
            AgentMessage::assistant_with_tools(
                "Read first copy",
                vec![tool_call(
                    "call-1",
                    "read_file",
                    r#"{"path":"/workspace/src/lib.rs"}"#,
                )],
            ),
            AgentMessage::tool("call-1", "read_file", "same output"),
            AgentMessage::assistant_with_tools(
                "Edit file in between",
                vec![tool_call(
                    "call-2",
                    "apply_file_edit",
                    r#"{"path":"src/lib.rs","search":"foo","replace":"bar"}"#,
                )],
            ),
            AgentMessage::tool("call-2", "apply_file_edit", "ok"),
            AgentMessage::assistant_with_tools(
                "Read second copy",
                vec![tool_call("call-3", "read_file", r#"{"path":"src/lib.rs"}"#)],
            ),
            AgentMessage::tool("call-3", "read_file", "same output"),
            AgentMessage::summary("[Previous context compressed]\n- earlier work preserved"),
            AgentMessage::assistant_with_tools(
                "Keep recent raw window",
                vec![tool_call("call-4", "search", "{}")],
            ),
            AgentMessage::tool("call-4", "search", "recent result"),
        ];
        let snapshot = classify_hot_memory_with_policy(
            &messages,
            &CompactionPolicy {
                protected_tool_window_tokens: 1,
                ..CompactionPolicy::default()
            },
            None,
        );

        let (rewritten, outcome) = dedup_superseded_tool_results(
            &DedupSupersededContract::default(),
            &snapshot,
            &messages,
        );

        assert!(!outcome.applied);
        assert_eq!(rewritten[1].content, "same output");
        assert_eq!(rewritten[5].content, "same output");
    }

    #[test]
    fn dedup_requires_latest_summary_boundary() {
        let messages = vec![
            AgentMessage::assistant_with_tools(
                "Read first copy",
                vec![tool_call("call-1", "read_file", r#"{"path":"src/lib.rs"}"#)],
            ),
            AgentMessage::tool("call-1", "read_file", "same output"),
            AgentMessage::assistant_with_tools(
                "Read second copy",
                vec![tool_call("call-2", "read_file", r#"{"path":"src/lib.rs"}"#)],
            ),
            AgentMessage::tool("call-2", "read_file", "same output"),
        ];
        let snapshot = classify_hot_memory_with_policy(
            &messages,
            &CompactionPolicy {
                protected_tool_window_tokens: 1,
                ..CompactionPolicy::default()
            },
            None,
        );

        let (rewritten, outcome) = dedup_superseded_tool_results(
            &DedupSupersededContract::default(),
            &snapshot,
            &messages,
        );

        assert!(!outcome.applied);
        assert_eq!(rewritten[1].content, "same output");
        assert_eq!(rewritten[3].content, "same output");
    }
}
