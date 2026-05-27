//! Model route selection and structured-output routing policy.

use super::types::{AgentRunnerConfig, AgentRunnerContext};
use super::AgentRunner;
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
