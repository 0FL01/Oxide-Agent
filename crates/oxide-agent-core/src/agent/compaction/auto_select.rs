//! Automatic compression-range selection for engine-based compaction.
//!
//! When an automatic trigger fires (pre-sampling budget threshold, context-limit
//! overflow, model downshift, hook/manual request), there is no agent to provide
//! a [`CompressionSelection`]. This module computes the optimal contiguous range
//! to compress based on:
//!
//! 1. **Pinned prefix** — `TopicAgentsMd` and `UserTask` messages must always
//!    remain visible to the model. They are never included in a compression block.
//! 2. **Recent tail** — the most recent messages, kept visible so the model has
//!    working context. The tail boundary is computed from the target token budget
//!    and a minimum-turn floor, with tool-batch atomicity.
//! 3. **Compressible middle** — everything between the pinned prefix and the
//!    recent tail. Existing active blocks within this range are consumed by the
//!    new block (the engine handles this).
//!
//! The selection is a [`CompressionSelection::Range`] spanning the compressible
//! middle. If the middle is empty (nothing to compress), [`None`] is returned
//! and the caller should emit a `RuntimeCompactionSkipped` event.

use super::block::CompressionSelection;
use super::budget::count_tokens_cached;
use super::refs::MessageRef;
use super::state::CompactionState;
use super::types::{AgentMessageKind, CompactionPolicy, is_browser_live_tool};
use crate::agent::memory::AgentMessage;

/// Minimum number of user-role messages that must remain in the recent tail.
const MIN_RECENT_USER_TURNS: usize = 3;
const MIN_TARGET_TOKENS: usize = 4_000;
const BROWSER_RECENT_TAIL_CONTEXT_FRACTION: usize = 5;
const BROWSER_RECENT_TAIL_MAX_TOKENS: usize = 40_000;

/// Compute the index after all leading pinned messages.
///
/// Pinned messages are `TopicAgentsMd` and `UserTask` — they define the task
/// and topic instructions and must always be visible. Old `Summary` messages
/// (from the legacy system) are also treated as pinned to avoid compressing
/// them twice; the engine will handle them via the block graph.
fn pinned_prefix_end(messages: &[AgentMessage]) -> usize {
    messages
        .iter()
        .take_while(|msg| {
            matches!(
                msg.kind,
                AgentMessageKind::TopicAgentsMd
                    | AgentMessageKind::UserTask
                    | AgentMessageKind::Summary
            )
        })
        .count()
}

/// Compute the tail-start index: the first message of the recent tail.
///
/// Walks backwards from the end, accumulating tokens until `target_token_budget`
/// is reached. Tool batches (`AssistantToolCall` followed by consecutive
/// `ToolResult`s) are collected atomically — the tail boundary never splits
/// a batch. A minimum floor of [`MIN_RECENT_USER_TURNS`] user-role messages
/// is enforced, but only within the compressible region (between `prefix_end`
/// and the token-budget boundary). If fewer user turns exist in that region,
/// the tail stays at the token-budget position — the floor is best-effort,
/// not a hard wall that would make the entire session uncompressible.
fn tail_start(messages: &[AgentMessage], target_token_budget: usize, prefix_end: usize) -> usize {
    if messages.is_empty() {
        return 0;
    }

    let tool_batches = find_tool_batch_boundaries(messages);
    let mut accumulated = 0usize;
    let mut tail_start = messages.len();
    let mut user_turn_count = 0usize;

    let mut index = messages.len();
    while index > 0 {
        let prev = index - 1;

        // If this message is part of a tool batch, collect the entire batch.
        if let Some(&(batch_start, batch_end)) = tool_batches
            .iter()
            .find(|&&(start, end)| prev >= start && prev < end)
        {
            // If the batch extends beyond the current tail_start, it's already
            // included; skip to the batch start.
            if batch_end <= tail_start {
                // Compute batch cost and check budget before including.
                let batch_cost: usize = (batch_start..batch_end)
                    .map(|i| message_token_cost(&messages[i]))
                    .sum();
                if accumulated.saturating_add(batch_cost) > target_token_budget
                    && tail_start < messages.len()
                {
                    break;
                }
                for i in (batch_start..batch_end).rev() {
                    accumulated = accumulated.saturating_add(message_token_cost(&messages[i]));
                }
                tail_start = batch_start;
                index = batch_start;
            } else {
                index -= 1;
            }
            continue;
        }

        let cost = message_token_cost(&messages[prev]);
        if accumulated.saturating_add(cost) > target_token_budget && tail_start < messages.len() {
            break;
        }
        accumulated = accumulated.saturating_add(cost);
        tail_start = prev;
        index = prev;

        if is_user_role(&messages[prev]) {
            user_turn_count += 1;
        }
    }

    // Minimum floor: ensure at least MIN_RECENT_USER_TURNS user-role messages,
    // but only within the compressible region. Never extend past prefix_end —
    // doing so would consume the pinned prefix and leave nothing to compress.
    // If fewer user turns exist in the compressible region, the tail stays at
    // the token-budget position — the floor is best-effort, not a hard wall.
    if user_turn_count < MIN_RECENT_USER_TURNS {
        let original_tail = tail_start;
        extend_to_min_user_turns(messages, &mut tail_start, &mut user_turn_count, prefix_end);
        // If we couldn't find enough user turns and extended all the way to
        // prefix_end, revert — better to have a small tail with a compressible
        // middle than no compressible range at all.
        if user_turn_count < MIN_RECENT_USER_TURNS && tail_start <= prefix_end {
            tail_start = original_tail.max(prefix_end);
        }
    }

    tail_start
}

/// Extend the tail backward until at least [`MIN_RECENT_USER_TURNS`] user-role
/// messages are included, the message list is exhausted, or `prefix_end` is
/// reached — whichever comes first. Never extends past `prefix_end`, because
/// that would consume the pinned prefix and leave nothing to compress.
fn extend_to_min_user_turns(
    messages: &[AgentMessage],
    tail_start: &mut usize,
    user_turn_count: &mut usize,
    prefix_end: usize,
) {
    let mut index = *tail_start;
    while index > prefix_end && *user_turn_count < MIN_RECENT_USER_TURNS {
        index -= 1;
        if is_user_role(&messages[index]) {
            *user_turn_count += 1;
        }
        *tail_start = index;
    }
}

/// Find all tool-batch boundaries: `(start, end)` ranges where `start` is an
/// `AssistantToolCall` and `end` is the index after the last consecutive
/// `ToolResult`.
fn find_tool_batch_boundaries(messages: &[AgentMessage]) -> Vec<(usize, usize)> {
    let mut batches = Vec::new();
    let mut index = 0;
    while index < messages.len() {
        if messages[index].kind == AgentMessageKind::AssistantToolCall {
            let batch_start = index;
            let mut end = index + 1;
            while end < messages.len() && messages[end].kind == AgentMessageKind::ToolResult {
                end += 1;
            }
            batches.push((batch_start, end));
            index = end;
        } else {
            index += 1;
        }
    }
    batches
}

/// Check if a message has a user role (UserTask, RuntimeContext, UserTurn).
fn is_user_role(msg: &AgentMessage) -> bool {
    matches!(
        msg.kind,
        AgentMessageKind::UserTask | AgentMessageKind::RuntimeContext | AgentMessageKind::UserTurn
    )
}

/// Token cost of a single message (content + reasoning).
fn message_token_cost(msg: &AgentMessage) -> usize {
    let mut tokens = count_tokens_cached(&msg.content);
    if let Some(reasoning) = &msg.reasoning {
        tokens = tokens.saturating_add(count_tokens_cached(reasoning));
    }
    tokens
}

/// Compute the target token budget for the recent tail.
///
/// This is the number of tokens of history that should remain visible after
/// compaction. It's derived from the route's context window minus the system
/// prompt, tool schemas, and hard reserve, capped at the warning threshold.
pub fn target_history_tokens(
    context_window: usize,
    system_prompt_tokens: usize,
    tool_schema_tokens: usize,
    policy: &CompactionPolicy,
) -> usize {
    let warning_threshold_tokens =
        context_window.saturating_mul(policy.warning_threshold_percent as usize) / 100;
    let compact_threshold_tokens =
        context_window.saturating_mul(policy.compact_threshold_percent as usize) / 100;

    let request_overhead = system_prompt_tokens
        .saturating_add(tool_schema_tokens)
        .saturating_add(policy.hard_reserve_tokens);

    let warning_target = warning_threshold_tokens.saturating_sub(request_overhead);
    if warning_target >= MIN_TARGET_TOKENS {
        return warning_target;
    }

    let compact_target = compact_threshold_tokens.saturating_sub(request_overhead);
    if compact_target >= MIN_TARGET_TOKENS {
        return compact_target;
    }

    // Fall back to the remaining budget after overhead, clamped to a minimum.
    let hard_target = context_window.saturating_sub(request_overhead);
    hard_target.max(MIN_TARGET_TOKENS).min(context_window)
}

/// Compute the recent-tail target for a concrete memory transcript.
///
/// Browser Live observations are high-frequency, replaceable page state. Keeping
/// a warning-threshold-sized recent tail (~100k tokens on large routes) makes a
/// browser-heavy task re-enter compaction after every action. When Browser Live
/// output is present, cap the recent tail to a smaller model-facing working set;
/// older browser observations are summarized into compaction blocks while the
/// latest observation and screenshot artifact remain visible.
pub fn target_history_tokens_for_messages(
    messages: &[AgentMessage],
    context_window: usize,
    system_prompt_tokens: usize,
    tool_schema_tokens: usize,
    policy: &CompactionPolicy,
) -> usize {
    let base = target_history_tokens(
        context_window,
        system_prompt_tokens,
        tool_schema_tokens,
        policy,
    );

    if !contains_browser_live_tool_output(messages) {
        return base;
    }

    let request_overhead = system_prompt_tokens
        .saturating_add(tool_schema_tokens)
        .saturating_add(policy.hard_reserve_tokens);
    let available = context_window.saturating_sub(request_overhead);
    let browser_cap = (context_window / BROWSER_RECENT_TAIL_CONTEXT_FRACTION)
        .clamp(MIN_TARGET_TOKENS, BROWSER_RECENT_TAIL_MAX_TOKENS)
        .min(available);
    base.min(browser_cap.max(MIN_TARGET_TOKENS.min(available)))
}

fn contains_browser_live_tool_output(messages: &[AgentMessage]) -> bool {
    messages
        .iter()
        .any(|message| is_browser_live_tool(message.tool_name.as_deref()))
}

/// Select the compressible range for automatic compaction.
///
/// Returns `Some(CompressionSelection::Range)` if there are messages between
/// the pinned prefix and the recent tail that can be compressed. Returns `None`
/// if nothing to compress (empty middle, or all messages are pinned/tail).
///
/// The selection includes any existing active blocks within the range — the
/// engine will consume them when the new block is created.
///
/// # Arguments
///
/// * `messages` — raw agent memory messages.
/// * `state` — current compaction state (used to check for active blocks).
/// * `target_tokens` — target token budget for the recent tail.
pub fn select_automatic_compression_range(
    messages: &[AgentMessage],
    state: &CompactionState,
    target_tokens: usize,
) -> Option<CompressionSelection> {
    if messages.is_empty() {
        return None;
    }

    let prefix_end = pinned_prefix_end(messages);
    let tail = tail_start(messages, target_tokens, prefix_end);

    if prefix_end >= tail {
        return None;
    }

    // If there are active blocks, check if the range would partially overlap
    // any non-consumable block. The engine will consume blocks fully within
    // the range, but blocks that extend beyond the range boundaries would
    // cause a partial-overlap rejection. Adjust boundaries to avoid this.
    let (adjusted_start, adjusted_end) =
        adjust_for_active_blocks(messages, state, prefix_end, tail);

    if adjusted_start >= adjusted_end {
        return None;
    }

    let start = MessageRef::from_index(adjusted_start);
    let end = MessageRef::from_index(adjusted_end - 1);

    // Validate refs resolve correctly.
    if start.resolve(messages.len()).is_none() || end.resolve(messages.len()).is_none() {
        return None;
    }

    Some(CompressionSelection::Range { start, end })
}

/// Adjust the range boundaries to avoid partial overlaps with active blocks.
///
/// If an active block extends before `start` (its direct indices include
/// indices < `start`), move `start` past the block's last direct index.
/// If an active block extends after `end` (its direct indices include
/// indices >= `end`), move `end` before the block's first direct index.
///
/// Blocks fully within the range are not adjusted — they will be consumed.
fn adjust_for_active_blocks(
    messages: &[AgentMessage],
    state: &CompactionState,
    start: usize,
    end: usize,
) -> (usize, usize) {
    let blocks = state.blocks();
    if !blocks.values().any(|b| b.is_active()) {
        return (start, end);
    }

    let mut adjusted_start = start;
    let mut adjusted_end = end;

    for block in blocks.values() {
        if !block.is_active() {
            continue;
        }

        let direct = block.direct_message_indices();
        if direct.is_empty() {
            continue;
        }

        let block_first = direct[0];
        let block_last = direct[direct.len() - 1];

        // Block extends before the range → move start past the block.
        if block_first < start && block_last >= start {
            adjusted_start = adjusted_start.max(block_last + 1);
        }

        // Block extends after the range → move end before the block.
        if block_first < end && block_last >= end {
            adjusted_end = adjusted_end.min(block_first);
        }
    }

    // Ensure we don't reference beyond the message list.
    adjusted_end = adjusted_end.min(messages.len());

    (adjusted_start, adjusted_end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::{BlockRef, CompactionEngine, SummaryPart};
    use crate::agent::memory::AgentMessage;

    fn make_user_turns(count: usize, content: &str) -> Vec<AgentMessage> {
        (0..count)
            .map(|i| AgentMessage::user_turn(format!("{content} {i}")))
            .collect()
    }

    /// Target small enough to force a tail of exactly 3 user turns.
    const TAIL_TARGET: usize = 5;

    #[test]
    fn empty_messages_returns_none() {
        let state = CompactionState::default();
        let result = select_automatic_compression_range(&[], &state, TAIL_TARGET);
        assert!(result.is_none());
    }

    #[test]
    fn only_pinned_returns_none() {
        let state = CompactionState::default();
        let messages = vec![
            AgentMessage::topic_agents_md("# Topic\nInstructions."),
            AgentMessage::user_task("Do the thing."),
        ];
        let result = select_automatic_compression_range(&messages, &state, TAIL_TARGET);
        assert!(result.is_none());
    }

    #[test]
    fn only_tail_returns_none() {
        let state = CompactionState::default();
        // 2 user turns with very large target — both fit in tail, nothing to compress.
        let messages = make_user_turns(2, "turn");
        let result = select_automatic_compression_range(&messages, &state, 10_000);
        assert!(result.is_none());
    }

    #[test]
    fn pinned_and_tail_only_returns_none() {
        let state = CompactionState::default();
        let messages = vec![
            AgentMessage::topic_agents_md("# Topic"),
            AgentMessage::user_task("Do the thing."),
            AgentMessage::user_turn("recent 1"),
            AgentMessage::user_turn("recent 2"),
            AgentMessage::user_turn("recent 3"),
        ];
        let result = select_automatic_compression_range(&messages, &state, TAIL_TARGET);
        assert!(result.is_none());
    }

    #[test]
    fn compressible_middle_returns_range() {
        let state = CompactionState::default();
        let messages = vec![
            AgentMessage::topic_agents_md("# Topic"),
            AgentMessage::user_task("Do the thing."),
            AgentMessage::user_turn("old 1"),
            AgentMessage::user_turn("old 2"),
            AgentMessage::user_turn("old 3"),
            AgentMessage::user_turn("recent 1"),
            AgentMessage::user_turn("recent 2"),
            AgentMessage::user_turn("recent 3"),
        ];
        let result = select_automatic_compression_range(&messages, &state, TAIL_TARGET);
        assert!(result.is_some());
        let selection = result.unwrap();
        match selection {
            CompressionSelection::Range { start, end } => {
                assert_eq!(start.to_index(), 2); // After pinned prefix
                assert_eq!(end.to_index(), 4); // Before 3-turn tail
            }
            _ => panic!("expected Range selection"),
        }
    }

    #[test]
    fn tool_batch_atomicity_in_tail() {
        use crate::llm::{ToolCall, ToolCallFunction};
        let state = CompactionState::default();
        let messages = vec![
            AgentMessage::user_task("Do the thing."), // 0 - pinned
            AgentMessage::user_turn("old 1"),         // 1
            AgentMessage::user_turn("old 2"),         // 2
            AgentMessage::assistant_with_tools(
                // 3 - tool batch start
                "Let me search.",
                vec![ToolCall::new(
                    "call-1",
                    ToolCallFunction {
                        name: "search".to_string(),
                        arguments: "{}".to_string(),
                    },
                    false,
                )],
            ),
            AgentMessage::tool("call-1", "search", "result"), // 4 - tool batch end
            AgentMessage::user_turn("recent 1"),              // 5
            AgentMessage::user_turn("recent 2"),              // 6
            AgentMessage::user_turn("recent 3"),              // 7
        ];

        // Small target budget — only recent 3 user turns fit in tail.
        let result = select_automatic_compression_range(&messages, &state, TAIL_TARGET);
        assert!(result.is_some());
        match result.unwrap() {
            CompressionSelection::Range { start, end } => {
                let start_idx = start.to_index();
                let end_idx = end.to_index();
                // Tool batch is at indices 3-4. Verify it's not split:
                // either both in compressible range or both in tail.
                let batch_start = 3;
                let batch_end = 4;
                let both_in_range = start_idx <= batch_start && end_idx >= batch_end;
                let both_in_tail = start_idx > batch_end || end_idx < batch_start;
                assert!(
                    both_in_range || both_in_tail,
                    "tool batch must not be split: range [{start_idx}, {end_idx}], batch [{batch_start}, {batch_end}]"
                );
            }
            _ => panic!("expected Range"),
        }
    }

    #[test]
    fn tool_heavy_session_with_few_user_turns_compacts() {
        use crate::llm::{ToolCall, ToolCallFunction};
        let state = CompactionState::default();
        // Reproduces the production bug: 1 UserTask + many AssistantToolCall/ToolResult
        // pairs with large outputs. Only 0 user turns in tail → extend_to_min_user_turns
        // must NOT extend past prefix_end (making everything tail and skipping compaction).
        let mut messages = vec![
            AgentMessage::topic_agents_md("# Topic"),     // 0 - pinned
            AgentMessage::user_task("Research laptops."), // 1 - pinned
        ];
        // Add 6 tool batches (assistant call + tool result) with large content.
        for i in 0..6 {
            messages.push(AgentMessage::assistant_with_tools(
                format!("Step {i}"),
                vec![ToolCall::new(
                    &format!("call-{i}"),
                    ToolCallFunction {
                        name: "browser_execute".to_string(),
                        arguments: "{}".to_string(),
                    },
                    false,
                )],
            ));
            messages.push(AgentMessage::tool(
                &format!("call-{i}"),
                "browser_execute",
                &"x".repeat(500), // Large tool output
            ));
        }
        // prefix_end = 2 (TopicAgentsMd + UserTask)
        // tail_start: walks back from end, accumulates tokens. With TAIL_TARGET=5,
        // the first tool result (~125 tokens) already exceeds budget.
        // user_turn_count = 0 (no user-role messages in tail).
        // extend_to_min_user_turns: tries to go back to find user turns, but
        // stops at prefix_end=2. No user turns between 2 and tail_start.
        // tail stays at token-budget position → compressible middle exists.
        let result = select_automatic_compression_range(&messages, &state, TAIL_TARGET);
        assert!(
            result.is_some(),
            "tool-heavy session with few user turns must produce a compressible range, not skip"
        );
        match result.unwrap() {
            CompressionSelection::Range { start, end } => {
                assert_eq!(start.to_index(), 2, "range starts after pinned prefix");
                // End is before the tail (which contains the last tool batch).
                assert!(
                    end.to_index() < messages.len() - 1,
                    "range end must not include the tail"
                );
            }
            _ => panic!("expected Range"),
        }
    }

    #[test]
    fn target_history_tokens_warning_threshold() {
        let tokens = target_history_tokens(128_000, 5_000, 2_000, &CompactionPolicy::default());
        // warning_threshold = 128_000 * 65% = 83_200
        // target = 83_200 - 5_000 - 2_000 - 8_192 = 68_008
        assert_eq!(tokens, 68_008);
    }

    #[test]
    fn target_history_tokens_min_floor() {
        let tokens = target_history_tokens(10_000, 5_000, 2_000, &CompactionPolicy::default());
        // warning_threshold = 10_000 * 65% = 6_500
        // warning_target = 6_500 - 5_000 - 2_000 - 8_192 = -8_692 → 0 (saturating)
        // compact_target = 10_000 * 85% = 8_500 - 15_192 → 0
        // hard_target = 10_000 - 15_192 → 0, max with 4_000 = 4_000, min with 10_000 = 4_000
        assert_eq!(tokens, 4_000);
    }

    #[test]
    fn browser_history_target_is_capped_below_warning_tail() {
        let messages = vec![
            AgentMessage::user_task("Find listings"),
            AgentMessage::tool("call-browser", "browser_execute", "browser payload"),
        ];

        let tokens = target_history_tokens_for_messages(
            &messages,
            200_000,
            8_000,
            7_000,
            &CompactionPolicy::default(),
        );

        assert_eq!(tokens, BROWSER_RECENT_TAIL_MAX_TOKENS);
    }

    #[test]
    fn non_browser_history_target_keeps_warning_tail() {
        let messages = vec![
            AgentMessage::user_task("Read files"),
            AgentMessage::tool("call-read", "read_file", "file payload"),
        ];

        let base = target_history_tokens(200_000, 8_000, 7_000, &CompactionPolicy::default());
        let tokens = target_history_tokens_for_messages(
            &messages,
            200_000,
            8_000,
            7_000,
            &CompactionPolicy::default(),
        );

        assert_eq!(tokens, base);
        assert!(tokens > BROWSER_RECENT_TAIL_MAX_TOKENS);
    }

    #[test]
    fn active_block_in_range_is_included() {
        let mut state = CompactionState::default();
        let messages = vec![
            AgentMessage::user_task("Do the thing."),
            AgentMessage::user_turn("old 1"),
            AgentMessage::user_turn("old 2"),
            AgentMessage::user_turn("mid 1"),
            AgentMessage::user_turn("mid 2"),
            AgentMessage::user_turn("recent 1"),
            AgentMessage::user_turn("recent 2"),
            AgentMessage::user_turn("recent 3"),
        ];

        // Create a block covering "old 1" and "old 2" (indices 1-2) via engine.
        let selection = CompressionSelection::Range {
            start: MessageRef::from_index(1),
            end: MessageRef::from_index(2),
        };
        let block_ref = CompactionEngine::apply_compression(
            &mut state,
            &messages,
            &selection,
            vec![SummaryPart::Text("Old context summary.".to_string())],
        )
        .expect("engine creates block");
        assert!(block_ref.as_u32() > 0);

        // Selection should include the active block (indices 1-2) and the
        // compressible middle (indices 3-4), up to the tail (indices 5-7).
        let result = select_automatic_compression_range(&messages, &state, TAIL_TARGET);
        assert!(result.is_some());
        match result.unwrap() {
            CompressionSelection::Range { start, end } => {
                assert_eq!(start.to_index(), 1); // Include active block
                assert_eq!(end.to_index(), 4); // Before tail
            }
            _ => panic!("expected Range"),
        }
    }

    #[test]
    fn active_block_extending_into_tail_is_adjusted() {
        let mut state = CompactionState::default();
        let messages = vec![
            AgentMessage::user_task("Do the thing."),
            AgentMessage::user_turn("old 1"),
            AgentMessage::user_turn("old 2"),
            AgentMessage::user_turn("recent 1"), // index 3
            AgentMessage::user_turn("recent 2"),
            AgentMessage::user_turn("recent 3"),
        ];

        // Create a block covering indices 2-3 (extends into tail).
        let selection = CompressionSelection::Range {
            start: MessageRef::from_index(2),
            end: MessageRef::from_index(3),
        };
        let _ = CompactionEngine::apply_compression(
            &mut state,
            &messages,
            &selection,
            vec![SummaryPart::Text("Summary.".to_string())],
        )
        .expect("engine creates block");

        // Selection should be adjusted to end before the block (index 2).
        let result = select_automatic_compression_range(&messages, &state, TAIL_TARGET);
        if let Some(CompressionSelection::Range { start, end }) = result {
            assert_eq!(start.to_index(), 1);
            assert!(end.to_index() < 2, "end should be before block start");
        }
    }

    #[test]
    fn summary_messages_treated_as_pinned() {
        let state = CompactionState::default();
        let messages = vec![
            AgentMessage::topic_agents_md("# Topic"),
            AgentMessage::user_task("Do the thing."),
            // Summary-kind message — treated as pinned.
            AgentMessage::summary("Old summary."),
            AgentMessage::user_turn("old 1"),
            AgentMessage::user_turn("old 2"),
            AgentMessage::user_turn("recent 1"),
            AgentMessage::user_turn("recent 2"),
            AgentMessage::user_turn("recent 3"),
        ];

        let result = select_automatic_compression_range(&messages, &state, TAIL_TARGET);
        assert!(result.is_some());
        match result.unwrap() {
            CompressionSelection::Range { start, end } => {
                // Pinned prefix = 3 (TopicAgentsMd + UserTask + Summary)
                assert_eq!(start.to_index(), 3);
                assert_eq!(end.to_index(), 4); // Before 3-turn tail
            }
            _ => panic!("expected Range"),
        }
    }

    #[test]
    fn large_target_budget_includes_everything_in_tail() {
        let state = CompactionState::default();
        let messages = vec![
            AgentMessage::user_task("Do the thing."),
            AgentMessage::user_turn("msg 1"),
            AgentMessage::user_turn("msg 2"),
            AgentMessage::user_turn("msg 3"),
        ];

        // Very large target — entire history fits in tail.
        let result = select_automatic_compression_range(&messages, &state, 1_000_000);
        // 3 user turns as tail, but there are only 3 user turns total.
        // pinned_prefix_end = 1 (UserTask), tail_start = 1 (all 3 turns fit).
        // prefix_end >= tail → None.
        assert!(result.is_none());
    }

    #[test]
    fn engine_accepts_automatic_selection() {
        let state = CompactionState::default();
        let messages = vec![
            AgentMessage::user_task("Do the thing."),
            AgentMessage::user_turn("old 1"),
            AgentMessage::user_turn("old 2"),
            AgentMessage::user_turn("old 3"),
            AgentMessage::user_turn("recent 1"),
            AgentMessage::user_turn("recent 2"),
            AgentMessage::user_turn("recent 3"),
        ];

        let selection =
            select_automatic_compression_range(&messages, &state, TAIL_TARGET).expect("selection");
        let mut engine_state = CompactionState::default();
        let block_ref = CompactionEngine::apply_compression(
            &mut engine_state,
            &messages,
            &selection,
            vec![SummaryPart::Text("Summary of old context.".to_string())],
        )
        .expect("engine accepts selection");

        assert!(engine_state.has_active_blocks());
        assert_eq!(block_ref, BlockRef::new(1));
    }

    #[test]
    fn pinned_prefix_end_skips_topic_and_task() {
        let messages = vec![
            AgentMessage::topic_agents_md("# Topic"),
            AgentMessage::user_task("Do the thing."),
            AgentMessage::user_turn("not pinned"),
            AgentMessage::user_task("second task"), // not leading → not pinned
        ];
        assert_eq!(pinned_prefix_end(&messages), 2);
    }

    #[test]
    fn tail_start_with_assistant_messages() {
        let messages = vec![
            AgentMessage::user_task("Do the thing."),
            AgentMessage::user("old context ".repeat(100)),
            AgentMessage::assistant("response 1"),
            AgentMessage::user_turn("recent 1"),
            AgentMessage::assistant("response 2"),
            AgentMessage::user_turn("recent 2"),
            AgentMessage::user_turn("recent 3"),
        ];

        // Small target — tail should start early. prefix_end=1 (UserTask is pinned).
        let tail = tail_start(&messages, 50, 1);
        assert!(tail <= 3, "tail should include recent messages only");
    }

    #[test]
    fn find_tool_batch_boundaries_basic() {
        use crate::llm::{ToolCall, ToolCallFunction};
        let messages = vec![
            AgentMessage::user_task("Do the thing."),
            AgentMessage::assistant_with_tools(
                "Let me search.",
                vec![ToolCall::new(
                    "call-1",
                    ToolCallFunction {
                        name: "search".to_string(),
                        arguments: "{}".to_string(),
                    },
                    false,
                )],
            ),
            AgentMessage::tool("call-1", "search", "result 1"),
            AgentMessage::tool("call-1", "search", "result 2"), // same call_id but different
            AgentMessage::user_turn("after search"),
            AgentMessage::assistant_with_tools(
                "Another search.",
                vec![ToolCall::new(
                    "call-2",
                    ToolCallFunction {
                        name: "read_file".to_string(),
                        arguments: "{}".to_string(),
                    },
                    false,
                )],
            ),
            AgentMessage::tool("call-2", "read_file", "file content"),
        ];

        let batches = find_tool_batch_boundaries(&messages);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0], (1, 4)); // AssistantToolCall + 2 ToolResults
        assert_eq!(batches[1], (5, 7)); // AssistantToolCall + 1 ToolResult
    }
}
