//! Model route selection and structured-output routing policy.

use super::AgentRunner;
use super::types::{AgentRunnerConfig, AgentRunnerContext};
use crate::agent::tool_runtime::v1_tool_runtime_enabled_for_model;
use crate::config::ModelInfo;
use crate::llm::{LlmClient, LlmError};
use std::borrow::Cow;
use std::time::Instant;
use tracing::warn;

impl AgentRunner {
    pub(super) fn select_model_route_index(
        &mut self,
        ctx: &AgentRunnerContext<'_>,
        exhausted_routes: &std::collections::HashSet<String>,
    ) -> Option<usize> {
        let now = Instant::now();
        let json_mode = self.structured_output_required_for_config(&ctx.config);
        let require_v1_tool_route = ctx.tool_runtime_registry.is_some()
            && ctx
                .config
                .model_routes
                .iter()
                .any(v1_tool_runtime_enabled_for_model);
        self.route_failover_state
            .route_quarantine
            .retain(|_, until| *until > now);

        if ctx.config.model_routes.is_empty() {
            return None;
        }

        if self.route_is_available(
            &ctx.config.model_routes[0],
            exhausted_routes,
            now,
            json_mode,
            !ctx.tools.is_empty(),
            require_v1_tool_route,
        ) {
            return Some(0);
        }

        let fallback_candidates: Vec<(usize, usize)> = ctx
            .config
            .model_routes
            .iter()
            .enumerate()
            .skip(1)
            .filter_map(|(index, route)| {
                self.route_is_available(
                    route,
                    exhausted_routes,
                    now,
                    json_mode,
                    !ctx.tools.is_empty(),
                    require_v1_tool_route,
                )
                .then_some((index, route.weight.max(1) as usize))
            })
            .collect();

        if fallback_candidates.is_empty() {
            return None;
        }

        let total_weight: usize = fallback_candidates.iter().map(|(_, weight)| *weight).sum();
        let slot = self.route_failover_state.fallback_cursor % total_weight;
        self.route_failover_state.fallback_cursor =
            (self.route_failover_state.fallback_cursor + 1) % total_weight;

        let mut cursor = slot;
        for (index, weight) in fallback_candidates {
            if cursor < weight {
                return Some(index);
            }
            cursor -= weight;
        }

        None
    }

    fn route_is_available(
        &self,
        route: &ModelInfo,
        exhausted_routes: &std::collections::HashSet<String>,
        now: Instant,
        json_mode: bool,
        has_tools: bool,
        require_v1_tool_route: bool,
    ) -> bool {
        let route_key = Self::route_key(route);
        let capabilities = LlmClient::provider_capabilities_for_model(route);
        (!require_v1_tool_route || v1_tool_runtime_enabled_for_model(route))
            && !Self::json_mode_forbids_route(json_mode, route)
            && capabilities.can_run_agent_tools()
            && capabilities.can_run_chat_with_tools_request(has_tools, json_mode)
            && !exhausted_routes.contains(&route_key)
            && self.llm_client.is_provider_available(&route.provider)
            && self
                .route_failover_state
                .route_quarantine
                .get(&route_key)
                .is_none_or(|until| *until <= now)
    }

    pub(super) fn json_mode_forbids_route(json_mode: bool, route: &ModelInfo) -> bool {
        json_mode
            && matches!(
                route.provider.trim().to_ascii_lowercase().as_str(),
                "chatgpt" | "openai-chatgpt" | "llm-provider/openai-chatgpt"
            )
    }

    pub(super) fn quarantine_model_route(
        &mut self,
        route: &ModelInfo,
        duration: std::time::Duration,
    ) {
        self.route_failover_state
            .route_quarantine
            .insert(Self::route_key(route), Instant::now() + duration);
    }

    pub(super) fn route_key(route: &ModelInfo) -> String {
        format!("{}:{}", route.provider, route.id)
    }

    fn active_model_info_for_config<'a>(
        &self,
        config: &'a AgentRunnerConfig,
    ) -> Result<Cow<'a, ModelInfo>, LlmError> {
        if let Some(provider) = config.model_provider.as_ref() {
            return Ok(Cow::Owned(ModelInfo {
                id: config.model_name.clone(),
                max_output_tokens: config.model_max_output_tokens,
                context_window_tokens: 0,
                provider: provider.clone(),
                weight: 1,
            }));
        }

        if let Some(route) = config.model_routes.first() {
            return Ok(Cow::Borrowed(route));
        }

        self.llm_client
            .get_model_info(&config.model_name)
            .map(Cow::Owned)
    }

    pub(super) fn structured_output_required_for_config(&self, config: &AgentRunnerConfig) -> bool {
        match self.active_model_info_for_config(config) {
            Ok(info) => Self::structured_output_required_for_model(info.as_ref()),
            Err(error) => {
                warn!(
                    model = config.model_name,
                    provider = config.model_provider.as_deref().unwrap_or("unknown"),
                    error = %error,
                    "Failed to resolve model info; defaulting to structured output"
                );
                true
            }
        }
    }

    pub(super) fn structured_output_required_for_model(model_info: &ModelInfo) -> bool {
        LlmClient::supports_structured_output_for_model(model_info)
    }
}

#[cfg(test)]
mod tests {
    #![cfg_attr(
        not(oxide_module_llm_provider_opencode_go),
        allow(dead_code, unused_imports)
    )]

    use super::*;
    use crate::agent::context::{AgentContext, EphemeralSession};
    #[cfg(oxide_module_llm_provider_openai_chatgpt)]
    use crate::agent::runner::test_support::build_llm_client_for_provider;
    use crate::agent::runner::test_support::{
        build_llm_client, single_final_response_provider, stub_non_chat_methods,
    };
    use crate::agent::runner::{AgentRunnerConfig, AgentRunnerContext};
    use crate::agent::tool_runtime::ToolRegistry as RuntimeToolRegistry;
    use crate::config::AgentSettings;
    use crate::llm::{LlmClient, MockLlmProvider};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn json_mode_forbids_chatgpt_routes_only() {
        let chatgpt_routes = [
            ModelInfo {
                id: "gpt-5.4-mini".to_string(),
                provider: "chatgpt".to_string(),
                max_output_tokens: 32_000,
                context_window_tokens: 200_000,
                weight: 1,
            },
            ModelInfo {
                id: "gpt-5.4-mini".to_string(),
                provider: "openai-chatgpt".to_string(),
                max_output_tokens: 32_000,
                context_window_tokens: 200_000,
                weight: 1,
            },
            ModelInfo {
                id: "gpt-5.4-mini".to_string(),
                provider: "llm-provider/openai-chatgpt".to_string(),
                max_output_tokens: 32_000,
                context_window_tokens: 200_000,
                weight: 1,
            },
        ];
        let zai_route = ModelInfo {
            id: "glm-4.7".to_string(),
            provider: "zai".to_string(),
            max_output_tokens: 32_000,
            context_window_tokens: 200_000,
            weight: 1,
        };

        for route in chatgpt_routes {
            assert!(AgentRunner::json_mode_forbids_route(true, &route));
            assert!(!AgentRunner::json_mode_forbids_route(false, &route));
        }
        assert!(!AgentRunner::json_mode_forbids_route(true, &zai_route));
    }

    #[test]
    fn structured_output_requirement_uses_active_provider_without_registry_lookup() {
        let llm_client = build_llm_client(single_final_response_provider());
        let runner = AgentRunner::new(llm_client);
        let config = AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 8, 4, 60, 4096)
            .with_model_provider("llm-provider/opencode-go")
            .with_model_routes(vec![ModelInfo {
                id: "gpt-5.4-mini".to_string(),
                provider: "chatgpt".to_string(),
                max_output_tokens: 32_000,
                context_window_tokens: 200_000,
                weight: 1,
            }]);

        assert!(!runner.structured_output_required_for_config(&config));
    }

    #[test]
    fn structured_output_requirement_disables_chatgpt_primary_route() {
        let llm_client = build_llm_client(single_final_response_provider());
        let runner = AgentRunner::new(llm_client);
        let config = AgentRunnerConfig::new("gpt-5.4-mini".to_string(), 8, 4, 60, 4096)
            .with_model_provider("chatgpt")
            .with_model_routes(vec![ModelInfo {
                id: "gpt-5.4-mini".to_string(),
                provider: "chatgpt".to_string(),
                max_output_tokens: 32_000,
                context_window_tokens: 200_000,
                weight: 1,
            }]);

        assert!(!runner.structured_output_required_for_config(&config));
    }

    #[cfg(oxide_module_llm_provider_openai_chatgpt)]
    #[test]
    fn select_model_route_index_keeps_chatgpt_route_when_structured_output_is_disabled() {
        let llm_client = build_llm_client_for_provider(
            single_final_response_provider(),
            "chatgpt",
            "gpt-5.4-mini",
        );
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(768);
        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = Vec::new();
        let ctx = AgentRunnerContext {
            task: "Route selection regression",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-chatgpt-route-selection",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            storage: None,
            config: AgentRunnerConfig::new("gpt-5.4-mini".to_string(), 8, 4, 60, 4096)
                .with_model_provider("chatgpt")
                .with_model_routes(vec![ModelInfo {
                    id: "gpt-5.4-mini".to_string(),
                    provider: "chatgpt".to_string(),
                    max_output_tokens: 32_000,
                    context_window_tokens: 200_000,
                    weight: 1,
                }]),
        };

        assert_eq!(
            runner.select_model_route_index(&ctx, &std::collections::HashSet::new()),
            Some(0)
        );
    }

    #[cfg(oxide_module_llm_provider_opencode_go)]
    #[test]
    fn select_model_route_index_does_not_fail_over_typed_runtime_to_non_v1_route() {
        let mut opencode = MockLlmProvider::new();
        stub_non_chat_methods(&mut opencode);
        let mut openrouter = MockLlmProvider::new();
        stub_non_chat_methods(&mut openrouter);

        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-flash".to_string()),
            agent_model_provider: Some("llm-provider/opencode-go".to_string()),
            agent_model_max_output_tokens: Some(256),
            ..AgentSettings::default()
        };
        let mut llm = LlmClient::new(&settings);
        llm.register_provider("opencode-go".to_string(), Arc::new(opencode));
        llm.register_provider("openrouter".to_string(), Arc::new(openrouter));
        let llm_client = Arc::new(llm);

        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(768);
        let tools: Vec<crate::llm::ToolDefinition> = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = Vec::new();
        let ctx = AgentRunnerContext {
            task: "Typed runtime route selection",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: Some(Arc::new(RuntimeToolRegistry::new())),
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-typed-route-selection",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            storage: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 8, 4, 60, 4096)
                .with_model_provider("llm-provider/opencode-go")
                .with_model_routes(vec![
                    ModelInfo {
                        id: "deepseek-v4-flash".to_string(),
                        provider: "opencode-go".to_string(),
                        max_output_tokens: 4096,
                        context_window_tokens: 200_000,
                        weight: 1,
                    },
                    ModelInfo {
                        id: "deepseek-v4-flash".to_string(),
                        provider: "openrouter".to_string(),
                        max_output_tokens: 4096,
                        context_window_tokens: 200_000,
                        weight: 10,
                    },
                ]),
        };

        assert_eq!(
            runner.select_model_route_index(&ctx, &std::collections::HashSet::new()),
            Some(0)
        );

        let mut exhausted = std::collections::HashSet::new();
        exhausted.insert(AgentRunner::route_key(&ctx.config.model_routes[0]));
        assert_eq!(runner.select_model_route_index(&ctx, &exhausted), None);
    }

    #[cfg(oxide_module_llm_provider_opencode_go)]
    #[test]
    fn select_model_route_index_keeps_selected_opencode_zen_route_with_go_fallbacks() {
        let mut zen = MockLlmProvider::new();
        stub_non_chat_methods(&mut zen);
        let mut go = MockLlmProvider::new();
        stub_non_chat_methods(&mut go);

        let settings = AgentSettings {
            agent_model_id: Some("mimo-v2.5-free".to_string()),
            agent_model_provider: Some("llm-provider/opencode-zen".to_string()),
            agent_model_max_output_tokens: Some(256),
            ..AgentSettings::default()
        };
        let mut llm = LlmClient::new(&settings);
        llm.register_provider("opencode-zen".to_string(), Arc::new(zen));
        llm.register_provider("opencode-go".to_string(), Arc::new(go));
        let llm_client = Arc::new(llm);

        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(768);
        let tools: Vec<crate::llm::ToolDefinition> = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = Vec::new();
        let ctx = AgentRunnerContext {
            task: "Typed runtime Zen route selection",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: Some(Arc::new(RuntimeToolRegistry::new())),
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-typed-zen-route-selection",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            storage: None,
            config: AgentRunnerConfig::new("mimo-v2.5-free".to_string(), 8, 4, 60, 4096)
                .with_model_provider("llm-provider/opencode-zen")
                .with_model_routes(vec![
                    ModelInfo {
                        id: "opencode-zen/mimo-v2.5-free".to_string(),
                        provider: "opencode-zen".to_string(),
                        max_output_tokens: 4096,
                        context_window_tokens: 200_000,
                        weight: 1,
                    },
                    ModelInfo {
                        id: "opencode-go/qwen3.7-max".to_string(),
                        provider: "opencode-go".to_string(),
                        max_output_tokens: 4096,
                        context_window_tokens: 200_000,
                        weight: 1,
                    },
                ]),
        };

        assert_eq!(
            runner.select_model_route_index(&ctx, &std::collections::HashSet::new()),
            Some(0)
        );
    }

    #[test]
    fn structured_output_requirement_uses_primary_route_before_selection() {
        let llm_client = build_llm_client(single_final_response_provider());
        let runner = AgentRunner::new(llm_client);
        let config = AgentRunnerConfig::new("missing-model-name".to_string(), 8, 4, 60, 4096)
            .with_model_routes(vec![ModelInfo {
                id: "deepseek-v4-flash".to_string(),
                provider: "opencode-go".to_string(),
                max_output_tokens: 32_000,
                context_window_tokens: 200_000,
                weight: 1,
            }]);

        assert!(!runner.structured_output_required_for_config(&config));
    }
}
