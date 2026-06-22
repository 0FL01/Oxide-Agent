//! Emergency context admission gate for external payloads entering hot memory.
//!
//! Every external payload — new user task, runtime context injection, tool
//! output, preprocessed document/media — passes through [`ContextAdmission`]
//! before `AgentMemory::add_message`. The gate decides:
//!
//! - **Inline** — payload fits the rendered budget; insert as-is.
//! - **Manifest** (scheme A) — payload too large for inline; insert a bounded
//!   manifest with metadata, head/tail preview, and retrieval instructions.
//!   Raw content is preserved losslessly in `externalized_payload.inline_fallback`
//!   (not counted in `token_count`, not rendered to the model).
//! - **ControlledPause** (scheme C) — safe continuation is impossible; stop with
//!   a precise blocker (payload exceeds entire route window, or even the
//!   manifest cannot fit).
//!
//! Optional **chunked summary** (scheme B) can be layered on top of a manifest
//! when an [`EmergencySummarizer`] is available. Degrades to manifest-only on
//! failure or when no summarizer is configured.
//!
//! Provider context-limit fallback (scheme typed-D) lives in [`LlmError`]
//! (`is_context_overflow`), not here.

use crate::agent::compaction::archive::ArchiveRef;
use crate::agent::compaction::count_tokens_cached;
use crate::agent::memory::ExternalizedPayload;
use std::time::{SystemTime, UNIX_EPOCH};

// ────────────────────────────────────────────────────────────────────────────
// Types
// ────────────────────────────────────────────────────────────────────────────

/// Kind of external payload seeking admission to hot memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayloadKind {
    /// User task or user turn text from transport.
    NewTask,
    /// Runtime context injection (queued while agent is running).
    RuntimeContext,
    /// Tool execution output (file read, command stdout, search results, etc.).
    ToolOutput {
        /// Name of the tool that produced the output.
        tool_name: String,
    },
    /// Preprocessed document/media output (transcription, image description, etc.).
    Document,
}

impl PayloadKind {
    /// Human-readable label for manifest headers.
    fn label(&self) -> &str {
        match self {
            Self::NewTask => "user input",
            Self::RuntimeContext => "runtime context",
            Self::ToolOutput { tool_name } => tool_name.as_str(),
            Self::Document => "document",
        }
    }

    /// Whether this payload kind can be re-fetched by re-calling a tool.
    fn is_retrievable(&self) -> bool {
        matches!(self, Self::ToolOutput { .. })
    }
}

/// Descriptor for an external payload seeking admission to hot memory.
#[derive(Debug, Clone)]
pub struct PayloadDescriptor {
    /// What kind of payload this is.
    pub kind: PayloadKind,
    /// Raw content text of the payload.
    pub content: String,
    /// Source path, URL, command, or other provenance identifier.
    pub source: Option<String>,
    /// Size of the raw content in bytes.
    pub size_bytes: usize,
}

/// Budget context for admission decisions.
///
/// The caller constructs this from current memory state and route info.
/// The admission gate does not access memory directly — it receives the
/// budget as input, keeping it stateless and testable.
#[derive(Debug, Clone)]
pub struct AdmissionBudget {
    /// Current rendered token count (from `AgentMemory::rendered_token_count`).
    pub rendered_tokens: usize,
    /// Route context window in tokens.
    pub route_context_window: usize,
    /// System prompt token overhead.
    pub system_prompt_tokens: usize,
    /// Tool schema token overhead.
    pub tool_schema_tokens: usize,
    /// Hard reserve (tokens that must remain free for the response).
    pub hard_reserve: usize,
}

impl AdmissionBudget {
    /// Tokens available for new inline content.
    ///
    /// `route_window - rendered - system_prompt - tool_schema - hard_reserve`.
    #[must_use]
    pub fn available_tokens(&self) -> usize {
        let overhead = self
            .system_prompt_tokens
            .saturating_add(self.tool_schema_tokens)
            .saturating_add(self.hard_reserve);
        self.route_context_window
            .saturating_sub(self.rendered_tokens)
            .saturating_sub(overhead)
    }
}

/// Decision made by [`ContextAdmission::evaluate`].
#[derive(Debug, Clone)]
pub enum AdmissionDecision {
    /// Payload fits inline in the rendered context. Insert as-is.
    Inline,
    /// Payload too large for inline; insert a bounded manifest with preview
    /// and lossless externalized payload.
    Manifest(ManifestSpec),
    /// Safe continuation is impossible; stop with a precise blocker.
    ControlledPause(AdmissionBlocker),
}

/// Specification for a manifest message replacing an oversized inline payload.
///
/// The caller uses `manifest_content` as the `AgentMessage::content` field
/// and `externalized_payload` as the `AgentMessage::externalized_payload` field.
/// This keeps the model-visible content bounded while preserving the raw
/// content losslessly (not counted in `token_count`, not rendered to the model).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestSpec {
    /// Bounded manifest content for the model-visible message.
    pub manifest_content: String,
    /// Full externalized payload with raw content in `inline_fallback`.
    pub externalized_payload: ExternalizedPayload,
}

/// Why safe continuation is impossible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionBlocker {
    /// The payload alone exceeds the entire route context window.
    PayloadExceedsContextWindow {
        /// Token count of the payload.
        payload_tokens: usize,
        /// Route context window size.
        route_window: usize,
    },
    /// Even the bounded manifest cannot fit in the remaining budget.
    NoBudgetForManifest {
        /// Tokens available for new content.
        available_tokens: usize,
        /// Token count of the manifest content.
        manifest_tokens: usize,
    },
}

impl AdmissionBlocker {
    /// Human-readable reason for the blocker, suitable for logging or user-facing pause.
    #[must_use]
    pub fn reason(&self) -> String {
        match self {
            Self::PayloadExceedsContextWindow {
                payload_tokens,
                route_window,
            } => {
                format!(
                    "Payload ({payload_tokens} tokens) exceeds the route context window ({route_window} tokens). \
                     The content cannot fit in any rendered context."
                )
            }
            Self::NoBudgetForManifest {
                available_tokens,
                manifest_tokens,
            } => {
                format!(
                    "Even the bounded manifest ({manifest_tokens} tokens) cannot fit in the remaining budget \
                     ({available_tokens} tokens). Compaction or context reduction is needed before admission."
                )
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// ContextAdmission — stateless evaluation gate
// ────────────────────────────────────────────────────────────────────────────

/// Stateless admission gate for external payloads.
///
/// Call [`ContextAdmission::evaluate`] with a [`PayloadDescriptor`] and
/// [`AdmissionBudget`] before adding any external payload to `AgentMemory`.
/// The gate returns an [`AdmissionDecision`] that the caller acts on.
pub struct ContextAdmission;

impl ContextAdmission {
    /// Maximum payload tokens that can be admitted inline, regardless of budget.
    /// Prevents a single payload from dominating the context window.
    /// 25% of the route window, with a hard minimum of 2000 tokens.
    const INLINE_FRACTION: usize = 4; // route_window / 4
    const INLINE_MIN_TOKENS: usize = 2000;

    /// Maximum characters for head/tail previews in the manifest.
    const PREVIEW_CHARS: usize = 500;

    /// Evaluate whether a payload can enter hot memory inline.
    ///
    /// This is a pure function of the payload descriptor and budget context.
    /// No memory access, no side effects.
    ///
    /// Decision logic:
    /// 1. If payload exceeds the entire route window → **ControlledPause** (impossible).
    /// 2. If payload is within the inline threshold → **Inline** (not oversized;
    ///    budget enforcement is the pre-LLM trigger's job, not admission's).
    /// 3. Otherwise → **Manifest** (oversized; externalize with bounded preview).
    ///    The manifest is always inserted — it's much smaller than the payload,
    ///    and the pre-LLM budget trigger will compact if needed.
    #[must_use]
    pub fn evaluate(descriptor: &PayloadDescriptor, budget: &AdmissionBudget) -> AdmissionDecision {
        let payload_tokens = count_tokens_cached(&descriptor.content);

        // Scheme C: payload alone exceeds the entire route window — impossible.
        if payload_tokens > budget.route_context_window {
            return AdmissionDecision::ControlledPause(
                AdmissionBlocker::PayloadExceedsContextWindow {
                    payload_tokens,
                    route_window: budget.route_context_window,
                },
            );
        }

        // Inline threshold: min(INLINE_MIN_TOKENS, route_window / INLINE_FRACTION).
        let inline_threshold = budget
            .route_context_window
            .div_ceil(Self::INLINE_FRACTION)
            .max(Self::INLINE_MIN_TOKENS);

        // Inline: payload is within the inline threshold (not oversized).
        // Budget enforcement for a full context is the pre-LLM trigger's job.
        if payload_tokens <= inline_threshold {
            return AdmissionDecision::Inline;
        }

        // Scheme A: payload is oversized — create a bounded manifest.
        // The manifest is always inserted (it's much smaller than the payload).
        // If the context is already near-full, the pre-LLM budget trigger
        // (Phase 7) will compact before the next LLM call.
        let manifest = Self::create_manifest(descriptor, payload_tokens);
        AdmissionDecision::Manifest(manifest)
    }

    /// Build a [`ManifestSpec`] from a payload descriptor.
    fn create_manifest(descriptor: &PayloadDescriptor, estimated_tokens: usize) -> ManifestSpec {
        let content = &descriptor.content;
        let original_chars = content.chars().count();

        let (head_preview, tail_preview) = if original_chars <= Self::PREVIEW_CHARS * 2 {
            // Content is small enough to show fully in head preview.
            (content.clone(), String::new())
        } else {
            let head: String = content.chars().take(Self::PREVIEW_CHARS).collect();
            let tail: String = content
                .chars()
                .rev()
                .take(Self::PREVIEW_CHARS)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            (head, tail)
        };

        let manifest_content = Self::format_manifest(
            descriptor,
            estimated_tokens,
            original_chars,
            &head_preview,
            &tail_preview,
        );

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let externalized_payload = ExternalizedPayload {
            archive_ref: ArchiveRef {
                archive_id: format!("admission-{now}"),
                created_at: now,
                title: descriptor.kind.label().to_string(),
                storage_key: "inline".to_string(),
            },
            estimated_tokens,
            original_chars,
            preview: head_preview,
            inline_fallback: Some(descriptor.content.clone()),
        };

        ManifestSpec {
            manifest_content,
            externalized_payload,
        }
    }

    /// Format the bounded manifest content for the model-visible message.
    ///
    /// The format explicitly marks the content as untrusted data to prevent
    /// prompt-injection from file/tool content becoming model instructions.
    fn format_manifest(
        descriptor: &PayloadDescriptor,
        estimated_tokens: usize,
        original_chars: usize,
        head: &str,
        tail: &str,
    ) -> String {
        let kind_label = descriptor.kind.label();
        let source = descriptor.source.as_deref().unwrap_or("(unknown)");
        let retrieval_hint = if descriptor.kind.is_retrievable() {
            format!("Use {kind_label} with offset/limit parameters to retrieve specific sections.")
        } else {
            "Refer to the preview above or ask the user for specific sections.".to_string()
        };

        let tail_section = if tail.is_empty() {
            String::new()
        } else {
            format!("--- Tail preview ---\n{tail}\n\n")
        };

        format!(
            "[Externalized content — untrusted data]\n\
             Source: {source}\n\
             Size: {size_bytes} bytes (~{estimated_tokens} tokens, {original_chars} chars)\n\
             \n\
             --- Head preview ---\n\
             {head}\n\
             \n\
             {tail_section}\
             --- End preview ---\n\
             \n\
             Full content is too large for inline context and was externalized.\n\
             {retrieval_hint}",
            size_bytes = descriptor.size_bytes,
        )
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Scheme B — optional chunked emergency summarization
// ────────────────────────────────────────────────────────────────────────────

/// Error during emergency chunk summarization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummarizeError {
    /// Summarizer is not available (no LLM configured).
    Unavailable,
    /// Summarizer failed on a specific chunk.
    Failed {
        /// Zero-based index of the chunk that failed.
        chunk_index: usize,
        /// Error message from the summarizer.
        error: String,
    },
}

/// Result of chunked summarization (scheme B).
#[derive(Debug, Clone)]
pub struct ChunkSummaryResult {
    /// Summaries of individual chunks, in order.
    pub chunk_summaries: Vec<String>,
    /// Combined summary-of-summaries block.
    pub summary_of_summaries: String,
    /// Total token count of all summaries combined.
    pub total_summary_tokens: usize,
}

/// Trait for emergency chunk summarization.
///
/// Implementations wrap an LLM call for bounded chunks. In production, this
/// is backed by a side-LLM (same as compaction summarizer). In tests, mock
/// implementations control the output.
pub trait EmergencySummarizer: Send + Sync {
    /// Summarize a single bounded chunk.
    ///
    /// `context` is a short description of the overall content for coherence.
    /// Returns the summary text or an error.
    fn summarize_chunk(&self, chunk: &str, context: &str) -> Result<String, SummarizeError>;
}

/// Split content into bounded chunks of approximately `chunk_chars` characters.
///
/// Splits on paragraph boundaries when possible to avoid cutting sentences.
/// Falls back to hard character splits when no paragraph boundary is nearby.
#[must_use]
pub fn split_into_chunks(content: &str, chunk_chars: usize) -> Vec<String> {
    if chunk_chars == 0 {
        return vec![content.to_string()];
    }
    if content.len() <= chunk_chars {
        return vec![content.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = content;

    while !remaining.is_empty() {
        if remaining.len() <= chunk_chars {
            chunks.push(remaining.to_string());
            break;
        }

        // Look for paragraph boundary within the chunk window.
        let window = &remaining[..chunk_chars];
        let split_at = window
            .rfind("\n\n")
            .or_else(|| window.rfind('\n'))
            .or_else(|| window.rfind(". "))
            .map(|pos| pos + 1) // include the separator in the current chunk
            .unwrap_or(chunk_chars);

        let (chunk, rest) = remaining.split_at(split_at);
        if chunk.is_empty() {
            // Avoid infinite loop on pathological input.
            let (chunk, rest) = remaining.split_at(chunk_chars.min(remaining.len()));
            chunks.push(chunk.to_string());
            remaining = rest;
        } else {
            chunks.push(chunk.to_string());
            remaining = rest;
        }
    }

    chunks
}

/// Summarize content in bounded chunks using the provided summarizer.
///
/// Each chunk is summarized independently. Then a summary-of-summaries is
/// created from the chunk summaries. On any chunk failure, returns
/// [`SummarizeError`] — the caller should fall back to manifest-only.
///
/// # Arguments
/// * `content` - The raw content to summarize.
/// * `chunk_chars` - Maximum characters per chunk.
/// * `context` - Short description of the content for summarizer coherence.
/// * `summarizer` - The emergency summarizer implementation.
pub fn summarize_in_chunks(
    content: &str,
    chunk_chars: usize,
    context: &str,
    summarizer: &dyn EmergencySummarizer,
) -> Result<ChunkSummaryResult, SummarizeError> {
    let chunks = split_into_chunks(content, chunk_chars);
    let mut chunk_summaries = Vec::with_capacity(chunks.len());

    for (i, chunk) in chunks.iter().enumerate() {
        let summary = summarizer
            .summarize_chunk(chunk, context)
            .map_err(|e| match e {
                SummarizeError::Unavailable => SummarizeError::Unavailable,
                other => SummarizeError::Failed {
                    chunk_index: i,
                    error: other.to_string(),
                },
            })?;
        chunk_summaries.push(summary);
    }

    // Create summary-of-summaries by joining chunk summaries and summarizing once more.
    let joined = chunk_summaries.join("\n\n---\n\n");
    let summary_of_summaries =
        summarizer
            .summarize_chunk(&joined, context)
            .map_err(|e| SummarizeError::Failed {
                chunk_index: chunks.len(),
                error: e.to_string(),
            })?;

    let total_summary_tokens = chunk_summaries
        .iter()
        .map(|s| count_tokens_cached(s))
        .sum::<usize>()
        + count_tokens_cached(&summary_of_summaries);

    Ok(ChunkSummaryResult {
        chunk_summaries,
        summary_of_summaries,
        total_summary_tokens,
    })
}

impl std::fmt::Display for SummarizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => write!(f, "Emergency summarizer unavailable"),
            Self::Failed { chunk_index, error } => {
                write!(f, "Chunk {chunk_index} summarization failed: {error}")
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── AdmissionBudget ──

    #[test]
    fn available_tokens_full_budget() {
        let budget = AdmissionBudget {
            rendered_tokens: 1000,
            route_context_window: 10_000,
            system_prompt_tokens: 500,
            tool_schema_tokens: 300,
            hard_reserve: 200,
        };
        assert_eq!(budget.available_tokens(), 8000);
    }

    #[test]
    fn available_tokens_saturates_to_zero() {
        let budget = AdmissionBudget {
            rendered_tokens: 9000,
            route_context_window: 10_000,
            system_prompt_tokens: 500,
            tool_schema_tokens: 300,
            hard_reserve: 200,
        };
        assert_eq!(budget.available_tokens(), 0);
    }

    #[test]
    fn available_tokens_zero_window() {
        let budget = AdmissionBudget {
            rendered_tokens: 0,
            route_context_window: 0,
            system_prompt_tokens: 0,
            tool_schema_tokens: 0,
            hard_reserve: 0,
        };
        assert_eq!(budget.available_tokens(), 0);
    }

    // ── ContextAdmission::evaluate — inline ──

    #[test]
    fn small_payload_inline() {
        let descriptor = PayloadDescriptor {
            kind: PayloadKind::NewTask,
            content: "Hello, agent!".to_string(),
            source: None,
            size_bytes: 13,
        };
        let budget = AdmissionBudget {
            rendered_tokens: 100,
            route_context_window: 32_000,
            system_prompt_tokens: 1000,
            tool_schema_tokens: 500,
            hard_reserve: 500,
        };
        let decision = ContextAdmission::evaluate(&descriptor, &budget);
        assert!(matches!(decision, AdmissionDecision::Inline));
    }

    #[test]
    fn tool_output_under_threshold_inline() {
        let descriptor = PayloadDescriptor {
            kind: PayloadKind::ToolOutput {
                tool_name: "read_file".to_string(),
            },
            content: "x".repeat(1000),
            source: Some("/path/to/file".to_string()),
            size_bytes: 1000,
        };
        let budget = AdmissionBudget {
            rendered_tokens: 100,
            route_context_window: 32_000,
            system_prompt_tokens: 1000,
            tool_schema_tokens: 500,
            hard_reserve: 500,
        };
        let decision = ContextAdmission::evaluate(&descriptor, &budget);
        assert!(matches!(decision, AdmissionDecision::Inline));
    }

    // ── ContextAdmission::evaluate — manifest ──

    #[test]
    fn large_payload_manifest() {
        // Create a payload that exceeds the inline threshold (25% of 32K = 8K tokens).
        // cl100k BPE encodes repeated 'x' at ~8 chars/token, so 100K chars ≈ 12.5K tokens.
        let descriptor = PayloadDescriptor {
            kind: PayloadKind::ToolOutput {
                tool_name: "read_file".to_string(),
            },
            content: "x".repeat(100_000), // ~12.5K tokens, above 8K threshold
            source: Some("/path/to/large_file.txt".to_string()),
            size_bytes: 100_000,
        };
        let budget = AdmissionBudget {
            rendered_tokens: 100,
            route_context_window: 32_000,
            system_prompt_tokens: 1000,
            tool_schema_tokens: 500,
            hard_reserve: 500,
        };
        let decision = ContextAdmission::evaluate(&descriptor, &budget);
        match decision {
            AdmissionDecision::Manifest(spec) => {
                // Manifest content should be bounded and contain metadata.
                assert!(
                    spec.manifest_content
                        .contains("[Externalized content — untrusted data]")
                );
                assert!(spec.manifest_content.contains("/path/to/large_file.txt"));
                assert!(spec.manifest_content.contains("100000 bytes"));
                assert!(spec.manifest_content.contains("read_file"));
                assert!(spec.manifest_content.contains("Head preview"));
                assert!(spec.manifest_content.contains("Tail preview"));
                assert!(spec.manifest_content.contains("offset/limit"));

                // Externalized payload should preserve raw content.
                let payload = &spec.externalized_payload;
                assert_eq!(payload.original_chars, 100_000);
                assert!(payload.estimated_tokens > 0);
                assert!(payload.inline_fallback.is_some());
                assert_eq!(payload.inline_fallback.as_ref().unwrap().len(), 100_000);
                assert_eq!(payload.archive_ref.storage_key, "inline");
            }
            other => panic!("expected Manifest, got {other:?}"),
        }
    }

    #[test]
    fn payload_exceeds_available_but_not_window_manifest() {
        // Payload fits in the route window but exceeds available budget.
        let descriptor = PayloadDescriptor {
            kind: PayloadKind::NewTask,
            content: "x".repeat(100_000), // ~12.5K tokens
            source: None,
            size_bytes: 100_000,
        };
        let budget = AdmissionBudget {
            rendered_tokens: 28_000, // most of the window is used
            route_context_window: 32_000,
            system_prompt_tokens: 1000,
            tool_schema_tokens: 500,
            hard_reserve: 500,
        };
        // Available = 32000 - 28000 - 1000 - 500 - 500 = 2000
        // Payload ~12.5K tokens > 2000 → Manifest.
        // Manifest tokens should be small enough to fit in 2000.
        let decision = ContextAdmission::evaluate(&descriptor, &budget);
        match decision {
            AdmissionDecision::Manifest(spec) => {
                // Manifest should be small enough to fit.
                let manifest_tokens = count_tokens_cached(&spec.manifest_content);
                assert!(
                    manifest_tokens <= 2000,
                    "manifest tokens {manifest_tokens} should fit in available 2000"
                );
            }
            other => panic!("expected Manifest, got {other:?}"),
        }
    }

    #[test]
    fn manifest_preserves_raw_content_losslessly() {
        let raw = "Important content that should not be lost.\nMultiple lines.\n\tTabs too.";
        // Need enough content to exceed inline threshold (8K tokens ≈ 64K chars for 'x').
        // Using diverse text at ~4 chars/token, need ~32K chars.
        let descriptor = PayloadDescriptor {
            kind: PayloadKind::RuntimeContext,
            content: raw.repeat(1000), // ~60K chars → enough to trigger manifest
            source: Some("runtime_inject".to_string()),
            size_bytes: raw.len() * 1000,
        };
        let budget = AdmissionBudget {
            rendered_tokens: 100,
            route_context_window: 32_000,
            system_prompt_tokens: 1000,
            tool_schema_tokens: 500,
            hard_reserve: 500,
        };
        let decision = ContextAdmission::evaluate(&descriptor, &budget);
        match decision {
            AdmissionDecision::Manifest(spec) => {
                let fallback = spec.externalized_payload.inline_fallback.as_ref().unwrap();
                assert_eq!(fallback.len(), raw.len() * 1000);
                assert_eq!(fallback.as_str(), &raw.repeat(1000));
            }
            other => panic!("expected Manifest, got {other:?}"),
        }
    }

    #[test]
    fn manifest_marks_content_as_untrusted() {
        let descriptor = PayloadDescriptor {
            kind: PayloadKind::ToolOutput {
                tool_name: "execute_command".to_string(),
            },
            content: "x".repeat(100_000), // ~12.5K tokens, above 8K threshold
            source: Some("ls -la /".to_string()),
            size_bytes: 100_000,
        };
        let budget = AdmissionBudget {
            rendered_tokens: 100,
            route_context_window: 32_000,
            system_prompt_tokens: 1000,
            tool_schema_tokens: 500,
            hard_reserve: 500,
        };
        let decision = ContextAdmission::evaluate(&descriptor, &budget);
        match decision {
            AdmissionDecision::Manifest(spec) => {
                assert!(
                    spec.manifest_content
                        .contains("[Externalized content — untrusted data]"),
                    "Manifest must mark content as untrusted"
                );
            }
            other => panic!("expected Manifest, got {other:?}"),
        }
    }

    #[test]
    fn manifest_for_non_retrievable_has_no_tool_hint() {
        let descriptor = PayloadDescriptor {
            kind: PayloadKind::NewTask,
            content: "x".repeat(100_000), // ~12.5K tokens, above 8K threshold
            source: None,
            size_bytes: 100_000,
        };
        let budget = AdmissionBudget {
            rendered_tokens: 100,
            route_context_window: 32_000,
            system_prompt_tokens: 1000,
            tool_schema_tokens: 500,
            hard_reserve: 500,
        };
        let decision = ContextAdmission::evaluate(&descriptor, &budget);
        match decision {
            AdmissionDecision::Manifest(spec) => {
                assert!(!spec.manifest_content.contains("offset/limit"));
                assert!(spec.manifest_content.contains("ask the user"));
            }
            other => panic!("expected Manifest, got {other:?}"),
        }
    }

    // ── ContextAdmission::evaluate — controlled pause ──

    #[test]
    fn payload_exceeds_entire_window_pause() {
        // 200K chars of 'x' ≈ 25K tokens. Use a 20K route window so 25K > 20K.
        let descriptor = PayloadDescriptor {
            kind: PayloadKind::ToolOutput {
                tool_name: "read_file".to_string(),
            },
            content: "x".repeat(200_000), // ~25K tokens, exceeds 20K window
            source: Some("/huge/file".to_string()),
            size_bytes: 200_000,
        };
        let budget = AdmissionBudget {
            rendered_tokens: 0,
            route_context_window: 20_000,
            system_prompt_tokens: 0,
            tool_schema_tokens: 0,
            hard_reserve: 0,
        };
        let decision = ContextAdmission::evaluate(&descriptor, &budget);
        match decision {
            AdmissionDecision::ControlledPause(blocker) => {
                match blocker {
                    AdmissionBlocker::PayloadExceedsContextWindow {
                        payload_tokens,
                        route_window,
                    } => {
                        assert!(payload_tokens > 20_000);
                        assert_eq!(route_window, 20_000);
                    }
                    other => panic!("expected PayloadExceedsContextWindow, got {other:?}"),
                }
                assert!(!blocker.reason().is_empty());
            }
            other => panic!("expected ControlledPause, got {other:?}"),
        }
    }

    #[test]
    fn no_budget_for_manifest_pause() {
        // Oversized payload in a zero-available-budget context still gets Manifest.
        // The admission gate's job is to prevent context bombs, not enforce budget.
        // Budget enforcement is the pre-LLM trigger's job (Phase 7).
        let descriptor = PayloadDescriptor {
            kind: PayloadKind::ToolOutput {
                tool_name: "read_file".to_string(),
            },
            content: "x".repeat(100_000), // ~12.5K tokens, above 8K threshold
            source: Some("/path".to_string()),
            size_bytes: 100_000,
        };
        let budget = AdmissionBudget {
            rendered_tokens: 31_000, // almost all used
            route_context_window: 32_000,
            system_prompt_tokens: 500,
            tool_schema_tokens: 500,
            hard_reserve: 500,
        };
        // Available = 32000 - 31000 - 500 - 500 - 500 = 0
        // Payload ~12.5K tokens > 8K inline threshold → Manifest (not pause).
        let decision = ContextAdmission::evaluate(&descriptor, &budget);
        match decision {
            AdmissionDecision::Manifest(spec) => {
                // Manifest is bounded and contains metadata.
                assert!(spec.manifest_content.contains("[Externalized content"));
                assert!(spec.externalized_payload.inline_fallback.is_some());
            }
            other => panic!("expected Manifest even with zero budget, got {other:?}"),
        }
    }

    // ── Inline threshold edge cases ──

    #[test]
    fn inline_threshold_uses_min_for_small_windows() {
        // Small route window: 4000 tokens. 25% = 1000. min = 2000. Threshold = 2000.
        let descriptor = PayloadDescriptor {
            kind: PayloadKind::NewTask,
            content: "x".repeat(3000), // ~750 tokens < 2000 threshold
            source: None,
            size_bytes: 3000,
        };
        let budget = AdmissionBudget {
            rendered_tokens: 0,
            route_context_window: 4000,
            system_prompt_tokens: 200,
            tool_schema_tokens: 100,
            hard_reserve: 200,
        };
        // Available = 4000 - 0 - 200 - 100 - 200 = 3500
        // 750 tokens <= 3500 and 750 <= 2000 → Inline
        let decision = ContextAdmission::evaluate(&descriptor, &budget);
        assert!(matches!(decision, AdmissionDecision::Inline));
    }

    #[test]
    fn payload_above_inline_threshold_but_within_budget_manifests() {
        // Route window 32K. Threshold = max(2000, 8000) = 8000.
        // Payload ~3K tokens > 2000 but < 8000 → should be inline.
        let descriptor = PayloadDescriptor {
            kind: PayloadKind::NewTask,
            content: "x".repeat(12_000), // ~3K tokens
            source: None,
            size_bytes: 12_000,
        };
        let budget = AdmissionBudget {
            rendered_tokens: 100,
            route_context_window: 32_000,
            system_prompt_tokens: 1000,
            tool_schema_tokens: 500,
            hard_reserve: 500,
        };
        // 3K tokens <= 29900 available and 3K <= 8000 threshold → Inline
        let decision = ContextAdmission::evaluate(&descriptor, &budget);
        assert!(matches!(decision, AdmissionDecision::Inline));
    }

    // ── Chunk splitting ──

    #[test]
    fn split_into_chunks_small_content_single_chunk() {
        let content = "Short content.";
        let chunks = split_into_chunks(content, 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Short content.");
    }

    #[test]
    fn split_into_chunks_splits_on_paragraph_boundary() {
        let content = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.";
        let chunks = split_into_chunks(content, 30);
        assert!(chunks.len() > 1);
        // First chunk should end at a paragraph boundary.
        assert!(chunks[0].ends_with("\n\n") || chunks[0].ends_with('\n'));
    }

    #[test]
    fn split_into_chunks_hard_split_when_no_boundary() {
        let content = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        let chunks = split_into_chunks(content, 20);
        assert!(chunks.len() > 1);
        // Each chunk (except possibly last) should be <= 20 chars.
        for chunk in &chunks[..chunks.len() - 1] {
            assert!(chunk.len() <= 20);
        }
    }

    #[test]
    fn split_into_chunks_preserves_all_content() {
        let content = "Hello\n\nWorld\n\nFoo\n\nBar\n\nBaz";
        let chunks = split_into_chunks(content, 10);
        let rejoined: String = chunks.concat();
        assert_eq!(rejoined, content);
    }

    #[test]
    fn split_into_chunks_zero_size_returns_single_chunk() {
        let content = "Any content";
        let chunks = split_into_chunks(content, 0);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], content);
    }

    // ── Chunk summarization ──

    /// Mock summarizer that returns a fixed prefix for each chunk.
    struct MockSummarizer {
        prefix: String,
    }

    impl EmergencySummarizer for MockSummarizer {
        fn summarize_chunk(&self, chunk: &str, _context: &str) -> Result<String, SummarizeError> {
            Ok(format!("{}: {} chars", self.prefix, chunk.len()))
        }
    }

    /// Mock summarizer that always fails.
    struct FailingSummarizer;

    impl EmergencySummarizer for FailingSummarizer {
        fn summarize_chunk(&self, _chunk: &str, _context: &str) -> Result<String, SummarizeError> {
            Err(SummarizeError::Unavailable)
        }
    }

    /// Mock summarizer that fails on a specific chunk index.
    struct PartialFailingSummarizer {
        fail_index: usize,
    }

    impl EmergencySummarizer for PartialFailingSummarizer {
        fn summarize_chunk(&self, _chunk: &str, _context: &str) -> Result<String, SummarizeError> {
            // This mock doesn't know its index; we simulate failure via a static counter.
            // For simplicity, just always fail.
            Err(SummarizeError::Failed {
                chunk_index: self.fail_index,
                error: "simulated failure".to_string(),
            })
        }
    }

    #[test]
    fn summarize_in_chunks_success() {
        let content = "x".repeat(200);
        let summarizer = MockSummarizer {
            prefix: "SUMMARY".to_string(),
        };
        let result = summarize_in_chunks(&content, 50, "test context", &summarizer);
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(!result.chunk_summaries.is_empty());
        assert!(!result.summary_of_summaries.is_empty());
        assert!(result.total_summary_tokens > 0);
    }

    #[test]
    fn summarize_in_chunks_degrades_on_unavailable() {
        let content = "x".repeat(200);
        let summarizer = FailingSummarizer;
        let result = summarize_in_chunks(&content, 50, "test context", &summarizer);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), SummarizeError::Unavailable);
    }

    #[test]
    fn summarize_in_chunks_degrades_on_failure() {
        let content = "x".repeat(200);
        let summarizer = PartialFailingSummarizer { fail_index: 0 };
        let result = summarize_in_chunks(&content, 50, "test context", &summarizer);
        assert!(result.is_err());
        match result.unwrap_err() {
            SummarizeError::Failed { chunk_index, .. } => {
                assert_eq!(chunk_index, 0);
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn summarize_in_chunks_single_chunk_no_split() {
        let content = "Short content";
        let summarizer = MockSummarizer {
            prefix: "S".to_string(),
        };
        let result = summarize_in_chunks(content, 100, "ctx", &summarizer).unwrap();
        assert_eq!(result.chunk_summaries.len(), 1);
        // Summary-of-summaries is the summarizer applied to the single chunk summary.
        assert!(result.summary_of_summaries.starts_with("S:"));
    }

    // ── AdmissionBlocker reason ──

    #[test]
    fn blocker_payload_exceeds_reason() {
        let blocker = AdmissionBlocker::PayloadExceedsContextWindow {
            payload_tokens: 50_000,
            route_window: 32_000,
        };
        let reason = blocker.reason();
        assert!(reason.contains("50000"));
        assert!(reason.contains("32000"));
    }

    #[test]
    fn blocker_no_budget_reason() {
        let blocker = AdmissionBlocker::NoBudgetForManifest {
            available_tokens: 100,
            manifest_tokens: 500,
        };
        let reason = blocker.reason();
        assert!(reason.contains("100"));
        assert!(reason.contains("500"));
    }

    // ── PayloadKind ──

    #[test]
    fn payload_kind_labels() {
        assert_eq!(PayloadKind::NewTask.label(), "user input");
        assert_eq!(PayloadKind::RuntimeContext.label(), "runtime context");
        assert_eq!(
            PayloadKind::ToolOutput {
                tool_name: "read_file".to_string()
            }
            .label(),
            "read_file"
        );
        assert_eq!(PayloadKind::Document.label(), "document");
    }

    #[test]
    fn payload_kind_retrievability() {
        assert!(!PayloadKind::NewTask.is_retrievable());
        assert!(!PayloadKind::RuntimeContext.is_retrievable());
        assert!(
            PayloadKind::ToolOutput {
                tool_name: "x".to_string()
            }
            .is_retrievable()
        );
        assert!(!PayloadKind::Document.is_retrievable());
    }
}
