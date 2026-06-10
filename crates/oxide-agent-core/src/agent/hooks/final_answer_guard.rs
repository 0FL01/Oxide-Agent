//! Evidence-aware soft guard for high-impact final answers.

use super::types::{HookContext, HookEvent, HookResult};
use crate::agent::hooks::Hook;
use crate::agent::research::{ResearchGuardDecision, ResearchSnapshot, ResearchSourcePriority};
use crate::config::is_research_audit_enabled;

const FRESHNESS_MARKERS: &[&str] = &[
    "currently",
    "current",
    "today",
    "now",
    "latest",
    "recent",
    "as of",
    "up to date",
    "актуальн",
    "сейчас",
    "сегодня",
    "последн",
];

const ALWAYS_HIGH_IMPACT_MARKERS: &[&str] = &[
    "price",
    "pricing",
    "cost",
    "tariff",
    "rate",
    "$",
    "version",
    "release",
    "changelog",
    "stable version",
    "legal",
    "law",
    "regulation",
    "compliance",
    "цена",
    "стоим",
    "тариф",
    "верси",
    "релиз",
    "закон",
    "юрид",
    "регул",
];

const FRESHNESS_DEPENDENT_MARKERS: &[&str] = &[
    "available",
    "availability",
    "outage",
    "status",
    "supported",
    "доступ",
    "статус",
];

/// Soft final-answer guard for unsupported volatile/high-impact claims.
pub struct FinalAnswerGuardHook;

impl FinalAnswerGuardHook {
    /// Create a final-answer guard hook.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for FinalAnswerGuardHook {
    fn default() -> Self {
        Self::new()
    }
}

impl Hook for FinalAnswerGuardHook {
    fn name(&self) -> &'static str {
        "final_answer_guard"
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        let HookEvent::AfterAgent { response } = event else {
            return HookResult::Continue;
        };

        let high_impact_detected = has_high_impact_marker(response);
        if !high_impact_detected {
            record_guard_decision(
                context,
                "allow",
                "no high-impact/current marker detected",
                false,
                false,
                None,
            );
            return HookResult::Continue;
        }

        if context.at_continuation_limit() {
            record_guard_decision(
                context,
                "skip_limit",
                "continuation limit reached",
                true,
                false,
                Some("current/high-impact claim"),
            );
            return HookResult::Continue;
        }

        let snapshot = context.research_runtime.map(|runtime| runtime.snapshot());
        let has_evidence = snapshot.as_ref().is_some_and(has_adequate_fetched_evidence);
        if has_evidence {
            record_guard_decision(
                context,
                "allow",
                "adequate fetched primary evidence observed",
                true,
                true,
                None,
            );
            return HookResult::Continue;
        }

        record_guard_decision(
            context,
            "force_iteration",
            "unsupported current/high-impact claim without fetched source evidence",
            true,
            false,
            Some("current/high-impact claim"),
        );

        HookResult::ForceIteration {
            reason: "final answer contains unsupported current/high-impact claims".to_string(),
            context: Some(next_action_context(context, snapshot.as_ref())),
        }
    }
}

fn record_guard_decision(
    context: &HookContext<'_>,
    decision: &str,
    reason: &str,
    high_impact_detected: bool,
    adequate_fetched_evidence: bool,
    unsupported_claim: Option<&str>,
) {
    if !is_research_audit_enabled() {
        return;
    }
    if let Some(runtime) = context.research_runtime {
        runtime.record_guard_decision(ResearchGuardDecision {
            decision: decision.to_string(),
            reason: reason.to_string(),
            high_impact_detected,
            adequate_fetched_evidence,
            unsupported_claim: unsupported_claim.map(str::to_string),
            continuation_count: context.continuation_count,
            continuation_limit: context.max_continuations,
        });
    }
}

fn has_high_impact_marker(response: &str) -> bool {
    let normalized = response.to_lowercase();
    let has_always_high_impact = ALWAYS_HIGH_IMPACT_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker));
    let has_freshness = FRESHNESS_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker));
    let has_freshness_dependent_marker = FRESHNESS_DEPENDENT_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker));

    has_always_high_impact || (has_freshness && has_freshness_dependent_marker)
}

fn has_adequate_fetched_evidence(snapshot: &ResearchSnapshot) -> bool {
    snapshot.observations.iter().any(|observation| {
        observation.success
            && !observation.snippet_only
            && observation.source_priority == ResearchSourcePriority::Primary
            && observation.kind.as_deref() == Some("fetch")
    })
}

fn next_action_context(context: &HookContext<'_>, snapshot: Option<&ResearchSnapshot>) -> String {
    let mut message = String::from(
        "Final answer guard: the draft contains current/high-impact claims without fetched source evidence. ",
    );
    if context.has_tool("searxng_search") && context.has_tool("crawl4ai_markdown") {
        message.push_str(
            "Run a targeted `searxng_search`, fetch the strongest primary source with `crawl4ai_markdown`, then answer with source-backed wording. ",
        );
    } else if context.has_tool("crawl4ai_markdown") {
        message.push_str(
            "Fetch a relevant source with `crawl4ai_markdown`, then answer with source-backed wording. ",
        );
    } else {
        message.push_str(
            "Either gather source evidence with available tools or revise the answer to avoid current/pricing/version/legal/status claims. ",
        );
    }

    if let Some(snapshot) = snapshot {
        if !snapshot.search_leads.is_empty() && snapshot.fetched_sources.is_empty() {
            message.push_str("Search snippets alone are not sufficient; fetch at least one source URL before finalizing. ");
        }
        if !snapshot.failures.is_empty() {
            message.push_str("If fetching is blocked, state the blocker instead of presenting the claim as verified. ");
        }
    }

    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::hooks::Hook;
    use crate::agent::memory::AgentMemory;
    use crate::agent::providers::TodoList;
    use crate::agent::research::ResearchRuntime;
    use crate::agent::tool_runtime::{
        ModelMetadata, OutputNormalizer, ProviderMetadata, ToolBatchId, ToolExecutionContext,
        ToolInvocation, ToolName, ToolRuntimeConfig, TurnId,
    };
    use crate::agent::{SessionId, tool_runtime::ToolCallId};
    use crate::llm::{InvocationId, ToolDefinition};
    use chrono::Utc;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    fn invocation(tool_name: &str) -> ToolInvocation {
        let config = ToolRuntimeConfig::default();
        ToolInvocation {
            session_id: SessionId::from(1),
            turn_id: TurnId::from("turn"),
            batch_id: ToolBatchId::from("batch"),
            batch_index: 0,
            invocation_id: InvocationId::new("call-runtime"),
            tool_call_id: ToolCallId::from("call-provider"),
            provider_tool_call_id: None,
            tool_name: ToolName::from(tool_name),
            raw_provider_payload: json!({}),
            raw_arguments: json!({}).to_string(),
            normalized_arguments: json!({}),
            cancellation_token: CancellationToken::new(),
            timeout: config.timeout,
            execution_context: ToolExecutionContext::new(config.artifact_dir.clone()),
            provider_metadata: ProviderMetadata {
                provider: "test".to_string(),
                protocol: "chat_like".to_string(),
            },
            model_metadata: ModelMetadata {
                model: "test-model".to_string(),
            },
            working_directory: None,
            environment_metadata: None,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
        }
    }

    fn search_tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    fn context<'a>(
        todos: &'a TodoList,
        memory: &'a AgentMemory,
        runtime: Option<&'a ResearchRuntime>,
    ) -> HookContext<'a> {
        static TOOLS: std::sync::OnceLock<Vec<ToolDefinition>> = std::sync::OnceLock::new();
        HookContext::new(todos, memory, 0, 0, 4)
            .with_research_runtime(runtime)
            .with_available_tools(TOOLS.get_or_init(|| {
                vec![
                    search_tool("searxng_search"),
                    search_tool("crawl4ai_markdown"),
                ]
            }))
    }

    #[test]
    fn conceptual_final_answer_passes() {
        let hook = FinalAnswerGuardHook::new();
        let todos = TodoList::new();
        let memory = AgentMemory::new(1024);

        let result = hook.handle(
            &HookEvent::AfterAgent {
                response: "Rust ownership helps prevent data races without a garbage collector."
                    .to_string(),
            },
            &context(&todos, &memory, None),
        );

        assert!(matches!(result, HookResult::Continue));
    }

    #[test]
    fn conceptual_supported_wording_without_freshness_passes() {
        let hook = FinalAnswerGuardHook::new();
        let todos = TodoList::new();
        let memory = AgentMemory::new(1024);

        let result = hook.handle(
            &HookEvent::AfterAgent {
                response: "Rust supports traits as a way to define shared behavior.".to_string(),
            },
            &context(&todos, &memory, None),
        );

        assert!(matches!(result, HookResult::Continue));
    }

    #[test]
    fn unsupported_high_impact_claim_forces_iteration() {
        let _guard = crate::config::test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        crate::testing::test_remove_env("RESEARCH_AUDIT_ENABLED");
        let hook = FinalAnswerGuardHook::new();
        let todos = TodoList::new();
        let memory = AgentMemory::new(1024);
        let runtime = ResearchRuntime::new();

        let result = hook.handle(
            &HookEvent::AfterAgent {
                response: "The current price is $10 today.".to_string(),
            },
            &context(&todos, &memory, Some(&runtime)),
        );

        assert!(matches!(result, HookResult::ForceIteration { .. }));
        let audit = runtime.audit_payload();
        assert_eq!(audit["final_guard_decision"]["decision"], "force_iteration");
        assert_eq!(audit["unsupported_claims"][0], "current/high-impact claim");
    }

    #[test]
    fn fetched_primary_evidence_allows_high_impact_claim() {
        let hook = FinalAnswerGuardHook::new();
        let todos = TodoList::new();
        let memory = AgentMemory::new(1024);
        let runtime = ResearchRuntime::new();
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig::default());
        let mut output = normalizer.success(&invocation("crawl4ai_markdown"), "markdown", "");
        output.structured_payload = Some(json!({
            "provider": "crawl4ai_markdown",
            "kind": "fetch",
            "url": "https://example.test/pricing",
            "final_url": "https://example.test/pricing",
            "status_code": 200,
            "truncated": false
        }));
        runtime.record_tool_output(&output);

        let result = hook.handle(
            &HookEvent::AfterAgent {
                response: "The current price is $10 today.".to_string(),
            },
            &context(&todos, &memory, Some(&runtime)),
        );

        assert!(matches!(result, HookResult::Continue));
    }

    #[test]
    fn snippet_only_search_evidence_is_not_sufficient() {
        let hook = FinalAnswerGuardHook::new();
        let todos = TodoList::new();
        let memory = AgentMemory::new(1024);
        let runtime = ResearchRuntime::new();
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig::default());
        let mut output = normalizer.success(&invocation("searxng_search"), "markdown", "");
        output.structured_payload = Some(json!({
            "provider": "searxng_search",
            "kind": "search",
            "query": "example pricing",
            "snippet_only": true,
            "results": [
                { "title": "Pricing", "url": "https://example.test/pricing", "snippet": "$10" }
            ]
        }));
        runtime.record_tool_output(&output);

        let result = hook.handle(
            &HookEvent::AfterAgent {
                response: "The current price is $10 today.".to_string(),
            },
            &context(&todos, &memory, Some(&runtime)),
        );

        assert!(matches!(result, HookResult::ForceIteration { .. }));
    }
}
