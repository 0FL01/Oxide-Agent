//! Rendering-time pruning strategies: deduplication and purge-errors.
//!
//! These strategies are pure functions of the raw messages plus a `RenderPolicy`.
//! They do NOT mutate `CompactionState` or raw messages — the renderer applies
//! their output (sets of indices to prune/strip) during rendering.
//!
//! Stateless design (П0): don't store what can be computed. For personal-use
//! scale (small contexts), recomputing on every render is correct and avoids
//! stored-state drift.

use super::refs::MessageRef;
use crate::agent::compaction::AgentMessageKind;
use crate::agent::memory::AgentMessage;
use std::collections::{BTreeSet, HashMap};

/// Policy for rendering-time pruning strategies and ref injection.
///
/// Controls which tool outputs are exempt from dedup, how many recent turns
/// are protected from all strategies, and when old errored tool inputs are
/// purged. Defaults are conservative for personal-use scale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderPolicy {
    /// Tool names whose results are exempt from dedup pruning.
    pub protected_tools: Vec<String>,
    /// Number of recent turns protected from all strategies.
    /// A turn starts at a user message (`UserTask`, `UserTurn`, `RuntimeContext`).
    pub turn_protection: usize,
    /// Minimum number of turns before errored tool inputs are purged.
    /// Counted from the start of the conversation, not from the error.
    pub purge_error_age_turns: usize,
}

impl Default for RenderPolicy {
    fn default() -> Self {
        Self {
            protected_tools: Vec::new(),
            turn_protection: 3,
            purge_error_age_turns: 5,
        }
    }
}

impl RenderPolicy {
    /// Returns true if the given tool name is protected from dedup.
    fn is_tool_protected(&self, tool_name: &str) -> bool {
        self.protected_tools.iter().any(|t| t == tool_name)
    }
}

/// Compute the index of the first message in the protected recent-turns zone.
///
/// Messages at or after this index are protected from all strategies.
/// Returns `messages.len()` when no messages are protected (turn_protection=0),
/// and 0 when all turns are protected (fewer turns than turn_protection).
#[must_use]
pub fn protected_boundary(messages: &[AgentMessage], turn_protection: usize) -> usize {
    if turn_protection == 0 {
        return messages.len();
    }
    let user_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, msg)| is_turn_start(msg))
        .map(|(i, _)| i)
        .collect();

    if user_indices.len() <= turn_protection {
        return 0;
    }

    let protected_start = user_indices.len() - turn_protection;
    user_indices[protected_start]
}

/// Check if a message starts a new user turn.
fn is_turn_start(msg: &AgentMessage) -> bool {
    matches!(
        msg.resolved_kind(),
        AgentMessageKind::UserTask | AgentMessageKind::UserTurn | AgentMessageKind::RuntimeContext
    )
}

// ---------------------------------------------------------------------------
// Dedup strategy
// ---------------------------------------------------------------------------

/// Tool call details extracted from `AssistantToolCall` messages, keyed by call id.
struct ToolCallDetail {
    tool_name: String,
    args: String,
    file_path: Option<String>,
}

/// Compute indices of tool result messages that are superseded by a newer
/// tool call with the same signature.
///
/// Rules:
/// - `read_file`: superseded only if a newer read for the same path exists AND
///   no write/edit (`write_file`, `apply_file_edit`) happened between them.
/// - Other tools: superseded by a newer call with the same `(tool_name, args)`.
/// - Protected tools (by name) are exempt.
/// - Messages in the protected recent-turns zone are exempt.
///
/// Returns a set of tool result message indices to prune.
#[must_use]
pub fn compute_superseded_tool_results(
    messages: &[AgentMessage],
    policy: &RenderPolicy,
) -> BTreeSet<usize> {
    let boundary = protected_boundary(messages, policy.turn_protection);
    let call_details = build_tool_call_details(messages);

    let mut superseded: BTreeSet<usize> = BTreeSet::new();

    // File-tool dedup state: tracks latest read and mutations per path.
    let mut latest_read_by_path: HashMap<String, usize> = HashMap::new();
    let mut mutated_since_latest_read: HashMap<String, bool> = HashMap::new();

    // General dedup state: tracks seen (tool_name, args) signatures.
    let mut seen_signatures: HashMap<String, usize> = HashMap::new();

    // Walk in reverse across ALL messages — protected messages are tracked
    // (they can supersede older ones) but are never themselves pruned.
    for (index, msg) in messages.iter().enumerate().rev() {
        let is_protected = index >= boundary;

        if msg.resolved_kind() == AgentMessageKind::ToolResult {
            let Some(tool_call_id) = msg
                .resolved_tool_call_correlation()
                .map(|c| c.invocation_id.as_str().to_string())
            else {
                continue;
            };
            let Some(detail) = call_details.get(&tool_call_id) else {
                continue;
            };

            if policy.is_tool_protected(&detail.tool_name) {
                continue;
            }

            if detail.tool_name == "read_file" {
                if let Some(path) = &detail.file_path {
                    let has_newer = latest_read_by_path.contains_key(path);
                    let mutated = mutated_since_latest_read
                        .get(path)
                        .copied()
                        .unwrap_or(false);
                    // Only prune if superseded AND outside protected zone.
                    if has_newer && !mutated && !is_protected {
                        superseded.insert(index);
                    }
                    latest_read_by_path.insert(path.clone(), index);
                    mutated_since_latest_read.insert(path.clone(), false);
                }
            } else {
                // General dedup by (tool_name, args) signature.
                let sig = format!("{}::{}", detail.tool_name, detail.args);
                if seen_signatures.contains_key(&sig) && !is_protected {
                    superseded.insert(index);
                }
                seen_signatures.insert(sig, index);
            }
        }

        // Track write/edit interventions for file-tool dedup.
        if msg.resolved_kind() == AgentMessageKind::AssistantToolCall
            && let Some(tool_calls) = msg.tool_calls.as_ref()
        {
            for tc in tool_calls {
                if matches!(tc.function.name.as_str(), "write_file" | "apply_file_edit")
                    && let Some(path) = file_tool_path(&tc.function.arguments)
                {
                    mutated_since_latest_read.insert(path, true);
                }
            }
        }
    }

    superseded
}

/// Build a map of tool_call_id → `ToolCallDetail` from all tool call messages.
fn build_tool_call_details(messages: &[AgentMessage]) -> HashMap<String, ToolCallDetail> {
    let mut map = HashMap::new();
    for msg in messages {
        if msg.resolved_kind() != AgentMessageKind::AssistantToolCall {
            continue;
        }
        let Some(tool_calls) = msg.tool_calls.as_ref() else {
            continue;
        };
        for tc in tool_calls {
            let path = file_tool_path(&tc.function.arguments);
            map.insert(
                tc.invocation_id().as_str().to_string(),
                ToolCallDetail {
                    tool_name: tc.function.name.clone(),
                    args: tc.function.arguments.clone(),
                    file_path: path,
                },
            );
        }
    }
    map
}

/// Extract the `path` field from a tool call's JSON arguments.
fn file_tool_path(arguments: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|value| {
            value
                .get("path")
                .and_then(|path| path.as_str())
                .map(str::trim)
                .map(str::to_string)
        })
        .filter(|path| !path.is_empty())
}

// ---------------------------------------------------------------------------
// Purge-errors strategy
// ---------------------------------------------------------------------------

/// Compute indices of `AssistantToolCall` messages whose tool inputs should be
/// stripped because the corresponding tool result was an error and is old enough.
///
/// A tool result is considered an error if it has `pruned_artifact` set (i.e.,
/// it was classified as a failure by the deterministic `tool_failure_summary`
/// system). The corresponding tool call's arguments are stripped if the tool
/// call is older than `turn_protection + purge_error_age_turns` turns.
///
/// Returns a set of `AssistantToolCall` message indices whose `tool_calls`
/// arguments should be replaced with a placeholder.
#[must_use]
pub fn compute_purge_error_inputs(
    messages: &[AgentMessage],
    policy: &RenderPolicy,
) -> BTreeSet<usize> {
    let boundary = protected_boundary(
        messages,
        policy.turn_protection + policy.purge_error_age_turns,
    );

    // Find errored tool results (pruned_artifact set) and their tool_call_ids.
    let errored_call_ids: BTreeSet<String> = messages
        .iter()
        .filter(|msg| msg.resolved_kind() == AgentMessageKind::ToolResult && msg.is_pruned())
        .filter_map(|msg| {
            msg.resolved_tool_call_correlation()
                .map(|c| c.invocation_id.as_str().to_string())
        })
        .collect();

    if errored_call_ids.is_empty() {
        return BTreeSet::new();
    }

    // Find AssistantToolCall messages that contain any of the errored call ids
    // and are outside the protected zone.
    let mut purge_indices = BTreeSet::new();
    for (index, msg) in messages.iter().enumerate() {
        if index >= boundary {
            break;
        }
        if msg.resolved_kind() != AgentMessageKind::AssistantToolCall {
            continue;
        }
        let Some(tool_calls) = msg.tool_calls.as_ref() else {
            continue;
        };
        let has_errored = tool_calls
            .iter()
            .any(|tc| errored_call_ids.contains(tc.invocation_id().as_str()));
        if has_errored {
            purge_indices.insert(index);
        }
    }

    purge_indices
}

// ---------------------------------------------------------------------------
// Ref injection
// ---------------------------------------------------------------------------

/// Format a `MessageRef` tag for injection into rendered content.
#[must_use]
pub fn ref_tag(raw_index: usize) -> String {
    format!("<{}>", MessageRef::from_index(raw_index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::memory::AgentMessage;
    use crate::llm::{ToolCall, ToolCallFunction};

    // --- Helpers ---

    fn user_msg(content: &str) -> AgentMessage {
        AgentMessage::user_task(content)
    }

    fn user_turn(content: &str) -> AgentMessage {
        AgentMessage::user(content)
    }

    fn assistant_msg(content: &str) -> AgentMessage {
        AgentMessage::assistant(content)
    }

    fn tool_call_msg(id: &str, tool_name: &str, args: &str) -> AgentMessage {
        let call = ToolCall::new(
            id.to_string(),
            ToolCallFunction {
                name: tool_name.to_string(),
                arguments: args.to_string(),
            },
            false,
        );
        AgentMessage::assistant_with_tools("calling tool", vec![call])
    }

    fn tool_result_msg(id: &str, tool_name: &str, content: &str) -> AgentMessage {
        AgentMessage::tool(id, tool_name, content)
    }

    fn tool_result_pruned(id: &str, tool_name: &str) -> AgentMessage {
        let mut msg = AgentMessage::tool(id, tool_name, "error content");
        msg.pruned_artifact = Some(crate::agent::memory::PrunedArtifact {
            estimated_tokens: 100,
            original_chars: 50,
            preview: "error preview".to_string(),
            archive_ref: None,
        });
        msg
    }

    // --- Turn protection / boundary ---

    #[test]
    fn protected_boundary_with_enough_turns() {
        let messages = vec![
            user_msg("task 1"),       // 0
            assistant_msg("reply 1"), // 1
            user_turn("follow 1"),    // 2
            assistant_msg("reply 2"), // 3
            user_turn("follow 2"),    // 4
            assistant_msg("reply 3"), // 5
        ];
        // 3 user turns, protect 2 → boundary at index 2 (start of 2nd-to-last turn)
        let boundary = protected_boundary(&messages, 2);
        assert_eq!(boundary, 2);
    }

    #[test]
    fn protected_boundary_all_protected() {
        let messages = vec![
            user_msg("task 1"),       // 0
            assistant_msg("reply 1"), // 1
            user_turn("follow 1"),    // 2
        ];
        // 2 turns, protect 3 → all protected
        let boundary = protected_boundary(&messages, 3);
        assert_eq!(boundary, 0);
    }

    #[test]
    fn protected_boundary_zero_protection() {
        let messages = vec![
            user_msg("task 1"),       // 0
            assistant_msg("reply 1"), // 1
        ];
        // turn_protection=0 → no messages protected → boundary = len
        let boundary = protected_boundary(&messages, 0);
        assert_eq!(boundary, 2);
    }

    #[test]
    fn protected_boundary_empty_messages() {
        // Empty messages, turn_protection=3 → all (zero) protected → boundary = 0
        let boundary = protected_boundary(&[], 3);
        assert_eq!(boundary, 0);
    }

    /// Policy with no turn protection — for testing dedup/purge logic in isolation.
    fn no_protection_policy() -> RenderPolicy {
        RenderPolicy {
            turn_protection: 0,
            ..RenderPolicy::default()
        }
    }

    // --- Dedup: file tools ---

    #[test]
    fn dedup_superseded_read_file() {
        let messages = vec![
            user_msg("read a file"),                             // 0
            tool_call_msg("c1", "read_file", r#"{"path":"a"}"#), // 1
            tool_result_msg("c1", "read_file", "content v1"),    // 2
            assistant_msg("checking"),                           // 3
            tool_call_msg("c2", "read_file", r#"{"path":"a"}"#), // 4
            tool_result_msg("c2", "read_file", "content v2"),    // 5
        ];
        let policy = no_protection_policy();
        let superseded = compute_superseded_tool_results(&messages, &policy);
        assert!(superseded.contains(&2)); // First read is superseded
        assert!(!superseded.contains(&5)); // Second read is kept
    }

    #[test]
    fn dedup_read_file_with_write_intervention() {
        let messages = vec![
            user_msg("read and write"),                           // 0
            tool_call_msg("c1", "read_file", r#"{"path":"a"}"#),  // 1
            tool_result_msg("c1", "read_file", "content v1"),     // 2
            tool_call_msg("c2", "write_file", r#"{"path":"a"}"#), // 3
            tool_result_msg("c2", "write_file", "ok"),            // 4
            tool_call_msg("c3", "read_file", r#"{"path":"a"}"#),  // 5
            tool_result_msg("c3", "read_file", "content v2"),     // 6
        ];
        let policy = no_protection_policy();
        let superseded = compute_superseded_tool_results(&messages, &policy);
        // First read is NOT superseded because write happened between reads
        assert!(!superseded.contains(&2));
        assert!(!superseded.contains(&6));
    }

    #[test]
    fn dedup_read_file_with_edit_intervention() {
        let messages = vec![
            user_msg("read and edit"),                                 // 0
            tool_call_msg("c1", "read_file", r#"{"path":"a"}"#),       // 1
            tool_result_msg("c1", "read_file", "content v1"),          // 2
            tool_call_msg("c2", "apply_file_edit", r#"{"path":"a"}"#), // 3
            tool_result_msg("c2", "apply_file_edit", "ok"),            // 4
            tool_call_msg("c3", "read_file", r#"{"path":"a"}"#),       // 5
            tool_result_msg("c3", "read_file", "content v2"),          // 6
        ];
        let policy = no_protection_policy();
        let superseded = compute_superseded_tool_results(&messages, &policy);
        assert!(!superseded.contains(&2)); // Write intervened
        assert!(!superseded.contains(&6));
    }

    #[test]
    fn dedup_different_paths_not_superseded() {
        let messages = vec![
            user_msg("read different files"),                    // 0
            tool_call_msg("c1", "read_file", r#"{"path":"a"}"#), // 1
            tool_result_msg("c1", "read_file", "content a"),     // 2
            tool_call_msg("c2", "read_file", r#"{"path":"b"}"#), // 3
            tool_result_msg("c2", "read_file", "content b"),     // 4
        ];
        let policy = no_protection_policy();
        let superseded = compute_superseded_tool_results(&messages, &policy);
        assert!(superseded.is_empty()); // Different paths, no dedup
    }

    #[test]
    fn dedup_protected_tool_exempt() {
        let messages = vec![
            user_msg("read a file"),                             // 0
            tool_call_msg("c1", "read_file", r#"{"path":"a"}"#), // 1
            tool_result_msg("c1", "read_file", "content v1"),    // 2
            tool_call_msg("c2", "read_file", r#"{"path":"a"}"#), // 3
            tool_result_msg("c2", "read_file", "content v2"),    // 4
        ];
        let policy = RenderPolicy {
            protected_tools: vec!["read_file".to_string()],
            turn_protection: 0,
            ..RenderPolicy::default()
        };
        let superseded = compute_superseded_tool_results(&messages, &policy);
        assert!(superseded.is_empty()); // Protected tool
    }

    #[test]
    fn dedup_respects_turn_protection() {
        // 5 turns, protect 3 → boundary at turn 3 (index 6)
        let messages = vec![
            user_msg("t1"),                                      // 0
            tool_call_msg("c1", "read_file", r#"{"path":"a"}"#), // 1
            tool_result_msg("c1", "read_file", "v1"),            // 2
            user_turn("t2"),                                     // 3
            tool_call_msg("c2", "read_file", r#"{"path":"a"}"#), // 4
            tool_result_msg("c2", "read_file", "v2"),            // 5
            user_turn("t3"),                                     // 6
            user_turn("t4"),                                     // 7
            user_turn("t5"),                                     // 8
        ];
        // turn_protection=3 → boundary at user_indices[5-3]=user_indices[2]=index 6
        // Messages 0-5 are outside protected zone
        let policy = RenderPolicy::default();
        let superseded = compute_superseded_tool_results(&messages, &policy);
        // c1 result (index 2) should be superseded by c2 result (index 5)
        assert!(superseded.contains(&2));
    }

    // --- Dedup: non-file tools ---

    #[test]
    fn dedup_non_file_tool_same_args() {
        let messages = vec![
            user_msg("search"),                                   // 0
            tool_call_msg("c1", "web_search", r#"{"q":"rust"}"#), // 1
            tool_result_msg("c1", "web_search", "result 1"),      // 2
            assistant_msg("checking"),                            // 3
            tool_call_msg("c2", "web_search", r#"{"q":"rust"}"#), // 4
            tool_result_msg("c2", "web_search", "result 2"),      // 5
        ];
        let policy = no_protection_policy();
        let superseded = compute_superseded_tool_results(&messages, &policy);
        assert!(superseded.contains(&2)); // Same tool+args, first is superseded
        assert!(!superseded.contains(&5));
    }

    #[test]
    fn dedup_non_file_tool_different_args() {
        let messages = vec![
            user_msg("search"),                                     // 0
            tool_call_msg("c1", "web_search", r#"{"q":"rust"}"#),   // 1
            tool_result_msg("c1", "web_search", "result 1"),        // 2
            tool_call_msg("c2", "web_search", r#"{"q":"python"}"#), // 3
            tool_result_msg("c2", "web_search", "result 2"),        // 4
        ];
        let policy = no_protection_policy();
        let superseded = compute_superseded_tool_results(&messages, &policy);
        assert!(superseded.is_empty()); // Different args, no dedup
    }

    // --- Purge-errors ---

    #[test]
    fn purge_errors_strips_old_errored_inputs() {
        // Turn 1: error occurs
        // Turns 2-10: normal work (enough turns to exceed purge threshold)
        let mut messages = vec![
            user_msg("task"),                             // 0
            tool_call_msg("c1", "execute_command", "{}"), // 1
            tool_result_pruned("c1", "execute_command"),  // 2 (errored)
        ];
        // Add enough turns to exceed turn_protection(3) + purge_error_age(5) = 8
        for i in 0..9 {
            messages.push(user_turn(&format!("follow {i}")));
            messages.push(assistant_msg("ok"));
        }
        let policy = RenderPolicy::default();
        let purge = compute_purge_error_inputs(&messages, &policy);
        // The errored tool call at index 1 should be purged (it's old enough)
        assert!(purge.contains(&1));
    }

    #[test]
    fn purge_errors_protects_recent_errors() {
        // Only 3 turns total — not enough to exceed purge threshold
        let messages = vec![
            user_msg("task"),                             // 0
            tool_call_msg("c1", "execute_command", "{}"), // 1
            tool_result_pruned("c1", "execute_command"),  // 2 (errored)
            user_turn("follow 1"),                        // 3
            assistant_msg("ok"),                          // 4
            user_turn("follow 2"),                        // 5
            assistant_msg("ok"),                          // 6
        ];
        let policy = RenderPolicy::default();
        let purge = compute_purge_error_inputs(&messages, &policy);
        // Only 3 turns, purge threshold = 3+5=8, not old enough
        assert!(purge.is_empty());
    }

    #[test]
    fn purge_errors_no_pruned_artifacts() {
        let messages = vec![
            user_msg("task"),                               // 0
            tool_call_msg("c1", "execute_command", "{}"),   // 1
            tool_result_msg("c1", "execute_command", "ok"), // 2 (success, no pruned)
        ];
        let policy = RenderPolicy::default();
        let purge = compute_purge_error_inputs(&messages, &policy);
        assert!(purge.is_empty());
    }

    // --- Ref tags ---

    #[test]
    fn ref_tag_format() {
        assert_eq!(ref_tag(0), "<m0001>");
        assert_eq!(ref_tag(5), "<m0006>");
        assert_eq!(ref_tag(999), "<m1000>");
    }

    #[test]
    fn render_policy_default() {
        let policy = RenderPolicy::default();
        assert_eq!(policy.turn_protection, 3);
        assert_eq!(policy.purge_error_age_turns, 5);
        assert!(policy.protected_tools.is_empty());
    }
}
