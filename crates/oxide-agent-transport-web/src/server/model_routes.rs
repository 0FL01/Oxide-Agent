use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use oxide_agent_core::llm::DiscoveredLlmModel;
use oxide_agent_web_contracts::{
    ErrorCode, ErrorEnvelope, ListModelRoutesResponse, ModelRouteProtocolView,
    ModelRouteSourceView, ModelRouteView, ModelSelection,
};

use super::{api_error, authenticated_user, authenticated_user_with_csrf, AppState};

const DEFAULT_OPENCODE_GO_QUALIFIED_MODEL_ID: &str = "opencode-go/kimi-k2.6";
const MAX_MODEL_SELECTION_CHARS: usize = 128;

pub(crate) async fn api_list_model_routes(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ListModelRoutesResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    authenticated_user(&state, &headers).await?;
    Ok(Json(model_routes_response(&state, false).await))
}

pub(crate) async fn api_refresh_model_routes(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ListModelRoutesResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    authenticated_user_with_csrf(&state, &headers).await?;
    Ok(Json(model_routes_response(&state, true).await))
}

async fn model_routes_response(state: &AppState, refresh: bool) -> ListModelRoutesResponse {
    let llm = state.session_manager.llm_client();
    let mut models = Vec::new();
    if let Some(go_models) = if refresh {
        llm.refresh_opencode_go_models().await
    } else {
        llm.opencode_go_models().await
    } {
        models.extend(go_models);
    }
    if let Some(zen_models) = if refresh {
        llm.refresh_opencode_zen_models().await
    } else {
        llm.opencode_zen_models().await
    } {
        models.extend(zen_models);
    }
    let routes = models
        .into_iter()
        .map(model_route_view_from_discovered)
        .collect();

    ListModelRoutesResponse {
        provider_id: "opencode".to_string(),
        provider_available: opencode_provider_available(state),
        default_model_id: default_opencode_model_id(state),
        routes,
    }
}

fn model_route_view_from_discovered(model: DiscoveredLlmModel) -> ModelRouteView {
    let protocol = model_route_protocol_view(&model.protocol);
    ModelRouteView {
        provider_id: model.provider_id,
        model_id: model.model_id,
        qualified_id: model.qualified_id,
        display_name: model.display_name,
        protocol,
        supports_image_input: model.supports_image_input,
        source: model_route_source_view(&model.source),
        fetched_at: model.fetched_at,
        runnable: protocol != ModelRouteProtocolView::Unknown,
    }
}

fn model_route_protocol_view(value: &str) -> ModelRouteProtocolView {
    match value.trim().to_ascii_lowercase().as_str() {
        "openai_chat_completions" => ModelRouteProtocolView::OpenAiChatCompletions,
        "anthropic_messages" => ModelRouteProtocolView::AnthropicMessages,
        _ => ModelRouteProtocolView::Unknown,
    }
}

fn model_route_source_view(value: &str) -> ModelRouteSourceView {
    match value.trim().to_ascii_lowercase().as_str() {
        "network" => ModelRouteSourceView::Network,
        "cache" => ModelRouteSourceView::Cache,
        _ => ModelRouteSourceView::Fallback,
    }
}

fn opencode_provider_available(state: &AppState) -> bool {
    let llm = state.session_manager.llm_client();
    llm.is_provider_available("opencode-go")
        || llm.is_provider_available("opencode_go")
        || llm.is_provider_available("opencode-zen")
        || llm.is_provider_available("opencode_zen")
}

fn default_opencode_model_id(state: &AppState) -> Option<String> {
    state
        .session_manager
        .agent_settings()
        .get_configured_agent_model_routes()
        .into_iter()
        .find(|route| opencode_provider_prefix(&route.provider).is_some())
        .and_then(|route| qualified_opencode_model_id(&route.id, &route.provider))
}

pub(crate) fn default_session_model_selection(state: &AppState) -> ModelSelection {
    ModelSelection {
        qualified_id: default_opencode_model_id(state)
            .unwrap_or_else(|| DEFAULT_OPENCODE_GO_QUALIFIED_MODEL_ID.to_string()),
    }
}

pub(crate) fn canonical_model_selection(
    selection: ModelSelection,
) -> Result<ModelSelection, (StatusCode, Json<ErrorEnvelope>)> {
    let qualified_id = selection.qualified_id.trim();
    if qualified_id.chars().count() > MAX_MODEL_SELECTION_CHARS {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            format!("Model selection must be at most {MAX_MODEL_SELECTION_CHARS} characters."),
            false,
        ));
    }
    let (prefix, model_id) = parse_opencode_model_selection(qualified_id).ok_or_else(|| {
        api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            "Model selection must be an OpenCode Go or Zen model id.",
            false,
        )
    })?;
    if model_id.is_empty() || model_id.contains('/') {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            "Model selection must be an OpenCode Go or Zen model id.",
            false,
        ));
    }
    Ok(ModelSelection {
        qualified_id: format!("{prefix}/{model_id}"),
    })
}

fn parse_opencode_model_selection(value: &str) -> Option<(&'static str, &str)> {
    let value = value.trim();
    if let Some(model_id) = value.strip_prefix("opencode-go/") {
        return Some(("opencode-go", model_id.trim()));
    }
    if let Some(model_id) = value.strip_prefix("opencode-zen/") {
        return Some(("opencode-zen", model_id.trim()));
    }
    if value.contains('/') {
        return None;
    }
    Some(("opencode-go", value))
}

fn opencode_provider_prefix(provider: &str) -> Option<&'static str> {
    match provider
        .trim()
        .strip_prefix("llm-provider/")
        .unwrap_or(provider.trim())
        .replace('_', "-")
        .to_ascii_lowercase()
        .as_str()
    {
        "opencode-go" => Some("opencode-go"),
        "opencode-zen" => Some("opencode-zen"),
        _ => None,
    }
}

fn qualified_opencode_model_id(model_id: &str, provider: &str) -> Option<String> {
    let prefix = opencode_provider_prefix(provider)?;
    let model_id = model_id.trim();
    if model_id.starts_with("opencode-go/") || model_id.starts_with("opencode-zen/") {
        parse_opencode_model_selection(model_id).and_then(|(route_prefix, route_model_id)| {
            (route_prefix == prefix).then(|| format!("{route_prefix}/{route_model_id}"))
        })
    } else {
        Some(format!("{prefix}/{model_id}"))
    }
}
