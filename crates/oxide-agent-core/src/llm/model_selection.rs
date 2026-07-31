use crate::config::{
    DEFAULT_AGENT_MODEL_CONTEXT_WINDOW_TOKENS, DEFAULT_AGENT_MODEL_MAX_OUTPUT_TOKENS, ModelInfo,
};

const OPENCODE_GO_OWNER: &str = "opencode-go";
const OPENCODE_ZEN_OWNER: &str = "opencode-zen";
const OPENAI_BASE_OWNER_PREFIX: &str = "openai-base:";

/// Returns the stable persisted identifier for a configured model route.
#[must_use]
pub fn canonical_model_id(model: &ModelInfo) -> Option<String> {
    let owner = canonical_provider_owner(&model.provider)?;
    let model_id = model.id.trim();
    if model_id.is_empty() {
        return None;
    }

    let model_id = model_id
        .strip_prefix(&format!("{owner}/"))
        .unwrap_or(model_id);
    (!model_id.is_empty()).then(|| format!("{owner}/{model_id}"))
}

/// Resolves a persisted model selection to an executable route.
///
/// Configured routes take precedence and retain their explicit token limits.
/// Provider-discovered routes use the main-agent defaults.
#[must_use]
pub fn resolve_model_selection(
    selected_id: &str,
    configured_routes: &[ModelInfo],
) -> Option<ModelInfo> {
    let (owner, model_id) = parse_canonical_model_id(selected_id)?;

    if let Some(configured) = configured_routes
        .iter()
        .find(|route| canonical_model_id(route).as_deref() == Some(selected_id))
    {
        let mut route = configured.clone();
        match owner {
            OPENCODE_GO_OWNER | OPENCODE_ZEN_OWNER => {
                route.id = selected_id.to_string();
                route.provider = owner.to_string();
            }
            owner if valid_openai_base_owner(owner) => {
                route.id = model_id.to_string();
                route.provider = owner.to_string();
            }
            _ => {}
        }
        if route.max_output_tokens == 0 {
            route.max_output_tokens = DEFAULT_AGENT_MODEL_MAX_OUTPUT_TOKENS;
        }
        if route.context_window_tokens == 0 {
            route.context_window_tokens = DEFAULT_AGENT_MODEL_CONTEXT_WINDOW_TOKENS;
        }
        return Some(route);
    }

    let (id, provider) = match owner {
        OPENCODE_GO_OWNER => (selected_id.to_string(), OPENCODE_GO_OWNER.to_string()),
        OPENCODE_ZEN_OWNER => (selected_id.to_string(), OPENCODE_ZEN_OWNER.to_string()),
        owner if valid_openai_base_owner(owner) => (model_id.to_string(), owner.to_string()),
        _ => return None,
    };

    Some(ModelInfo {
        id,
        provider,
        max_output_tokens: DEFAULT_AGENT_MODEL_MAX_OUTPUT_TOKENS,
        context_window_tokens: DEFAULT_AGENT_MODEL_CONTEXT_WINDOW_TOKENS,
    })
}

fn canonical_provider_owner(provider: &str) -> Option<String> {
    let provider = provider.trim();
    if provider.is_empty() {
        return None;
    }
    let owner = provider
        .strip_prefix("llm-provider/")
        .unwrap_or(provider)
        .replace('_', "-")
        .to_ascii_lowercase();
    (!owner.is_empty() && !owner.contains('/')).then_some(owner)
}

fn parse_canonical_model_id(value: &str) -> Option<(&str, &str)> {
    if value != value.trim() || value.starts_with("llm-provider/") {
        return None;
    }
    let (owner, model_id) = value.split_once('/')?;
    if owner.is_empty()
        || model_id.is_empty()
        || owner != owner.trim()
        || model_id != model_id.trim()
    {
        return None;
    }
    Some((owner, model_id))
}

fn valid_openai_base_owner(owner: &str) -> bool {
    owner
        .strip_prefix(OPENAI_BASE_OWNER_PREFIX)
        .is_some_and(|instance| {
            !instance.is_empty()
                && instance
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(id: &str, provider: &str, max_output_tokens: u32) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            provider: provider.to_string(),
            max_output_tokens,
            context_window_tokens: max_output_tokens * 4,
        }
    }

    #[test]
    fn canonical_id_removes_module_and_duplicate_owner_prefixes() {
        assert_eq!(
            canonical_model_id(&route(
                "opencode-go/mimo-v2.5",
                "llm-provider/opencode-go",
                1_000,
            )),
            Some("opencode-go/mimo-v2.5".to_string())
        );
        assert_eq!(
            canonical_model_id(&route(
                "google/gemini-3-flash-preview",
                "llm-provider/openrouter",
                1_000,
            )),
            Some("openrouter/google/gemini-3-flash-preview".to_string())
        );
    }

    #[test]
    fn configured_route_wins_with_its_token_limits() {
        let configured = route("opencode-go/mimo-v2.5", "llm-provider/opencode-go", 7_000);

        let resolved =
            resolve_model_selection("opencode-go/mimo-v2.5", std::slice::from_ref(&configured))
                .expect("configured route");
        assert_eq!(resolved.id, configured.id);
        assert_eq!(resolved.provider, "opencode-go");
        assert_eq!(resolved.max_output_tokens, configured.max_output_tokens);
        assert_eq!(
            resolved.context_window_tokens,
            configured.context_window_tokens
        );
    }

    #[test]
    fn discovered_routes_use_existing_agent_defaults() {
        assert_eq!(
            resolve_model_selection("opencode-go/kimi-k2.6", &[]),
            Some(ModelInfo {
                id: "opencode-go/kimi-k2.6".to_string(),
                provider: "opencode-go".to_string(),
                max_output_tokens: DEFAULT_AGENT_MODEL_MAX_OUTPUT_TOKENS,
                context_window_tokens: DEFAULT_AGENT_MODEL_CONTEXT_WINDOW_TOKENS,
            })
        );
        assert_eq!(
            resolve_model_selection("openai-base:zai/glm-5", &[])
                .expect("OpenAI Base route")
                .id,
            "glm-5"
        );
    }

    #[test]
    fn invalid_or_unconfigured_non_discovered_selection_is_rejected() {
        assert!(resolve_model_selection("llm-provider/opencode-go/model", &[]).is_none());
        assert!(resolve_model_selection("openrouter/google/model", &[]).is_none());
        assert!(resolve_model_selection("opencode-go/", &[]).is_none());
    }
}
