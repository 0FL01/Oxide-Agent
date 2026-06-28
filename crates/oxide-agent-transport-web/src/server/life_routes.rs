#[cfg(feature = "storage-sqlx")]
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
    http::StatusCode,
};
#[cfg(feature = "storage-sqlx")]
use oxide_agent_web_contracts::ApiLifeInputSensitivity;
#[cfg(feature = "storage-sqlx")]
use oxide_agent_web_contracts::{
    ApiLifeEventResponse, ApiLifeFrictionPatternResponse, ApiLifeMemoryItemResponse,
    ApiLifeSupportProtocolResponse, ApiLifeTaskStateResponse, ApiLifeTurnResponse,
};
use oxide_agent_web_contracts::{
    ApiLifeEventsResponse, ApiLifeFrictionPatternsResponse, ApiLifeGenerationResponse,
    ApiLifeGenerationsResponse, ApiLifeLifecycleResponse, ApiLifeLinkTokenResponse,
    ApiLifeMemoriesResponse, ApiLifeProfileResponse, ApiLifeSoftResetRequest, ApiLifeStateResponse,
    ApiLifeSubmitRequest, ApiLifeSubmitResponse, ApiLifeSupportProtocolsResponse,
    ApiLifeTaskStatesResponse, ApiLifeTurnsResponse, ErrorCode, ErrorEnvelope,
};
use serde::Deserialize;

use super::{AppState, api_error, authenticated_user_with_csrf};

#[cfg(feature = "storage-sqlx")]
use async_trait::async_trait;
#[cfg(feature = "storage-sqlx")]
use oxide_agent_life::{
    domain::{
        FrictionPatternKind, LifeEvent, LifeFrictionPattern, LifeIdentityProvider, LifeLinkToken,
        LifeMemoryGeneration, LifeMemoryItem, LifeSourceTransport, LifeSupportProtocol,
        LifeTaskState, LifeTurn, LifeTurnRole, MemoryAuthority, MemoryGenerationId,
        MemoryGenerationStatus, MemoryItemId, MemoryItemKind, MemoryItemStatus, MemorySensitivity,
        PrincipalUserId, ProviderSubject, RedactionState, RunId, SupportStateStatus,
        TaskStateStatus, TimestampMillis,
    },
    gateway::{
        LifeGateway, LifeGatewayError, LifeInputSensitivity, LifeInputSubmission,
        LifePrincipalAllocator,
    },
    linking::{RawLifeLinkToken, hash_link_token},
    storage::{LifeStorageError, LifeStorageRepository},
};

pub(crate) async fn api_submit_life_input(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ApiLifeSubmitRequest>,
) -> Result<Json<ApiLifeSubmitResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    submit_life_input_for_user(&state, user.user_id, request).await
}

pub(crate) async fn api_get_life_state(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiLifeStateResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    life_state_for_user(&state, user.user_id).await
}

pub(crate) async fn api_create_life_link_token(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiLifeLinkTokenResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    create_life_link_token_for_user(&state, user.user_id).await
}

pub(crate) async fn api_list_life_generations(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiLifeGenerationsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    list_life_generations_for_user(&state, user.user_id).await
}

pub(crate) async fn api_get_life_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiLifeProfileResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    life_profile_for_user(&state, user.user_id).await
}

pub(crate) async fn api_list_life_turns(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<LifeTurnsQuery>,
) -> Result<Json<ApiLifeTurnsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    list_life_turns_for_user(&state, user.user_id, query).await
}

pub(crate) async fn api_list_life_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<LifeEventsQuery>,
) -> Result<Json<ApiLifeEventsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    list_life_events_for_user(&state, user.user_id, query).await
}

pub(crate) async fn api_list_life_memories(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiLifeMemoriesResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    list_life_memories_for_user(&state, user.user_id).await
}

pub(crate) async fn api_forget_life_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(memory_id): Path<String>,
) -> Result<Json<ApiLifeLifecycleResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    forget_life_memory_for_user(&state, user.user_id, memory_id).await
}

pub(crate) async fn api_list_life_task_states(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiLifeTaskStatesResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    list_life_task_states_for_user(&state, user.user_id).await
}

pub(crate) async fn api_list_life_friction_patterns(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiLifeFrictionPatternsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    list_life_friction_patterns_for_user(&state, user.user_id).await
}

pub(crate) async fn api_list_life_support_protocols(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiLifeSupportProtocolsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    list_life_support_protocols_for_user(&state, user.user_id).await
}

pub(crate) async fn api_soft_reset_life_generation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ApiLifeSoftResetRequest>,
) -> Result<Json<ApiLifeGenerationResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    soft_reset_life_generation_for_user(&state, user.user_id, request).await
}

pub(crate) async fn api_activate_life_generation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(generation_id): Path<String>,
) -> Result<Json<ApiLifeLifecycleResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    activate_life_generation_for_user(&state, user.user_id, generation_id).await
}

pub(crate) async fn api_wipe_life_generation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(generation_id): Path<String>,
) -> Result<Json<ApiLifeLifecycleResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    wipe_life_generation_for_user(&state, user.user_id, generation_id).await
}

pub(crate) async fn api_wipe_life_derived_generation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(generation_id): Path<String>,
) -> Result<Json<ApiLifeLifecycleResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    wipe_life_derived_generation_for_user(&state, user.user_id, generation_id).await
}

pub(crate) async fn api_privacy_hard_wipe_life(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiLifeLifecycleResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user_with_csrf(&state, &headers).await?;
    privacy_hard_wipe_life_for_user(&state, user.user_id).await
}

/// Query parameters for `GET /api/v1/life/turns`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LifeTurnsQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

/// Query parameters for `GET /api/v1/life/events`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LifeEventsQuery {
    pub run_id: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

const DEFAULT_LIFE_TURNS_PAGE_LIMIT: i64 = 50;
const MAX_LIFE_TURNS_PAGE_LIMIT: i64 = 200;
const DEFAULT_LIFE_EVENTS_PAGE_LIMIT: i64 = 100;
const MAX_LIFE_EVENTS_PAGE_LIMIT: i64 = 500;

#[cfg(feature = "storage-sqlx")]
async fn submit_life_input_for_user(
    state: &AppState,
    web_user_id: i64,
    request: ApiLifeSubmitRequest,
) -> Result<Json<ApiLifeSubmitResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let life_storage = state
        .life_storage()
        .ok_or_else(life_storage_unavailable_response)?;
    let principal = PrincipalUserId::new(web_user_id).map_err(life_domain_error_response)?;
    let gateway = LifeGateway::new(
        life_storage.as_ref().clone(),
        FixedPrincipalAllocator { principal },
    );
    let result = gateway
        .submit_life_input(LifeInputSubmission {
            provider: LifeIdentityProvider::Web,
            provider_subject: ProviderSubject::new(web_user_id.to_string())
                .map_err(life_domain_error_response)?,
            content: request.content,
            attachments: request.attachments,
            metadata: request.metadata,
            sensitivity: map_input_sensitivity(request.sensitivity),
        })
        .await
        .map_err(life_gateway_error_response)?;

    // Wake the life runtime to claim the input and start/attach a run.
    let run_id = if let Some(runtime) = state.life_runtime() {
        let outcome = runtime
            .wake(result.principal_user_id, result.input_id)
            .await
            .map_err(life_runtime_error_response)?;
        let run_id = match outcome {
            oxide_agent_life::runtime::WakeOutcome::Started { run_id, claimed } => {
                // Spawn the worker to execute the claimed run.
                if let Some(worker) = state.life_worker() {
                    tokio::spawn(async move {
                        if let Err(error) = worker.execute_claimed_run(*claimed).await {
                            tracing::error!("life run execution failed: {error}");
                        }
                    });
                }
                run_id
            }
            oxide_agent_life::runtime::WakeOutcome::AttachedToActive { run_id } => run_id,
        };
        Some(run_id.to_string())
    } else {
        None
    };

    Ok(Json(ApiLifeSubmitResponse {
        principal_user_id: result.principal_user_id.get(),
        memory_generation_id: result.memory_scope.memory_generation_id.to_string(),
        turn_id: result.turn_id.to_string(),
        input_id: result.input_id.to_string(),
        run_id,
    }))
}

#[cfg(feature = "storage-sqlx")]
async fn create_life_link_token_for_user(
    state: &AppState,
    web_user_id: i64,
) -> Result<Json<ApiLifeLinkTokenResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let life_storage = state
        .life_storage()
        .ok_or_else(life_storage_unavailable_response)?;
    let principal = PrincipalUserId::new(web_user_id).map_err(life_domain_error_response)?;
    let now = life_now().map_err(life_clock_error_response)?;
    let raw_token = RawLifeLinkToken::generate();
    let expires_at = TimestampMillis::new(now.get() + 15 * 60 * 1000);
    life_storage
        .insert_link_token(&LifeLinkToken {
            token_hash: hash_link_token(&raw_token),
            principal_user_id: principal,
            target_provider: LifeIdentityProvider::Telegram,
            expires_at,
            consumed_at: None,
            created_at: now,
        })
        .await
        .map_err(life_storage_error_response)?;

    Ok(Json(ApiLifeLinkTokenResponse {
        token: raw_token.as_str().to_owned(),
        target_provider: LifeIdentityProvider::Telegram.as_str().to_owned(),
        expires_at: expires_at.get(),
    }))
}

#[cfg(not(feature = "storage-sqlx"))]
async fn create_life_link_token_for_user(
    _state: &AppState,
    _web_user_id: i64,
) -> Result<Json<ApiLifeLinkTokenResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    Err(life_storage_unavailable_response())
}

#[cfg(feature = "storage-sqlx")]
async fn list_life_generations_for_user(
    state: &AppState,
    web_user_id: i64,
) -> Result<Json<ApiLifeGenerationsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let life_storage = state
        .life_storage()
        .ok_or_else(life_storage_unavailable_response)?;
    let principal = PrincipalUserId::new(web_user_id).map_err(life_domain_error_response)?;
    let generations = life_storage
        .list_memory_generations(principal)
        .await
        .map_err(life_storage_error_response)?
        .into_iter()
        .map(api_generation)
        .collect();
    Ok(Json(ApiLifeGenerationsResponse { generations }))
}

#[cfg(not(feature = "storage-sqlx"))]
async fn list_life_generations_for_user(
    _state: &AppState,
    _web_user_id: i64,
) -> Result<Json<ApiLifeGenerationsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    Err(life_storage_unavailable_response())
}

#[cfg(feature = "storage-sqlx")]
async fn soft_reset_life_generation_for_user(
    state: &AppState,
    web_user_id: i64,
    request: ApiLifeSoftResetRequest,
) -> Result<Json<ApiLifeGenerationResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let life_storage = state
        .life_storage()
        .ok_or_else(life_storage_unavailable_response)?;
    let principal = PrincipalUserId::new(web_user_id).map_err(life_domain_error_response)?;
    let seed_ids = request
        .seed_memory_ids
        .iter()
        .map(|id| parse_memory_item_id(id))
        .collect::<Result<Vec<_>, _>>()?;
    let generation = life_storage
        .soft_reset_memory_generation(
            principal,
            &seed_ids,
            life_now().map_err(life_clock_error_response)?,
            &request.reason,
        )
        .await
        .map_err(life_storage_error_response)?;
    Ok(Json(api_generation(generation)))
}

#[cfg(feature = "storage-sqlx")]
async fn life_profile_for_user(
    state: &AppState,
    web_user_id: i64,
) -> Result<Json<ApiLifeProfileResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let life_storage = state
        .life_storage()
        .ok_or_else(life_storage_unavailable_response)?;
    let principal_user_id =
        PrincipalUserId::new(web_user_id).map_err(life_domain_error_response)?;
    let principal = life_storage
        .principal(principal_user_id)
        .await
        .map_err(life_storage_error_response)?;
    let Some(principal) = principal else {
        return Ok(Json(ApiLifeProfileResponse {
            principal_user_id: web_user_id,
            profile_state: serde_json::json!({}),
            operating_profile: serde_json::json!({}),
            settings: serde_json::json!({}),
            schema_version: 1,
        }));
    };
    Ok(Json(ApiLifeProfileResponse {
        principal_user_id: principal.principal_user_id.get(),
        profile_state: principal.profile_state,
        operating_profile: principal.operating_profile,
        settings: principal.settings,
        schema_version: principal.schema_version,
    }))
}

#[cfg(not(feature = "storage-sqlx"))]
async fn life_profile_for_user(
    _state: &AppState,
    _web_user_id: i64,
) -> Result<Json<ApiLifeProfileResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    Err(life_storage_unavailable_response())
}

#[cfg(feature = "storage-sqlx")]
async fn list_life_turns_for_user(
    state: &AppState,
    web_user_id: i64,
    query: LifeTurnsQuery,
) -> Result<Json<ApiLifeTurnsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let life_storage = state
        .life_storage()
        .ok_or_else(life_storage_unavailable_response)?;
    let principal = PrincipalUserId::new(web_user_id).map_err(life_domain_error_response)?;
    let limit = query
        .limit
        .unwrap_or(DEFAULT_LIFE_TURNS_PAGE_LIMIT)
        .clamp(1, MAX_LIFE_TURNS_PAGE_LIMIT);
    let page = life_storage
        .list_turns_page(principal, query.cursor.as_deref(), limit)
        .await
        .map_err(life_storage_error_response)?;
    let turns = page.turns.into_iter().map(api_turn).collect();
    Ok(Json(ApiLifeTurnsResponse {
        turns,
        next_cursor: page.next_cursor,
    }))
}

#[cfg(not(feature = "storage-sqlx"))]
async fn list_life_turns_for_user(
    _state: &AppState,
    _web_user_id: i64,
    _query: LifeTurnsQuery,
) -> Result<Json<ApiLifeTurnsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    Err(life_storage_unavailable_response())
}

#[cfg(feature = "storage-sqlx")]
async fn list_life_events_for_user(
    state: &AppState,
    web_user_id: i64,
    query: LifeEventsQuery,
) -> Result<Json<ApiLifeEventsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let life_storage = state
        .life_storage()
        .ok_or_else(life_storage_unavailable_response)?;
    let principal = PrincipalUserId::new(web_user_id).map_err(life_domain_error_response)?;
    let run_id = query
        .run_id
        .as_deref()
        .map(|s| {
            s.parse::<uuid::Uuid>().map(RunId::from_uuid).map_err(|_| {
                life_storage_error_response(LifeStorageError::InvalidCursor(s.to_owned()))
            })
        })
        .transpose()?;
    let limit = query
        .limit
        .unwrap_or(DEFAULT_LIFE_EVENTS_PAGE_LIMIT)
        .clamp(1, MAX_LIFE_EVENTS_PAGE_LIMIT);
    let page = life_storage
        .list_events_page(principal, run_id, query.cursor.as_deref(), limit)
        .await
        .map_err(life_storage_error_response)?;
    let events = page.events.into_iter().map(api_event).collect();
    Ok(Json(ApiLifeEventsResponse {
        events,
        next_cursor: page.next_cursor,
    }))
}

#[cfg(not(feature = "storage-sqlx"))]
async fn list_life_events_for_user(
    _state: &AppState,
    _web_user_id: i64,
    _query: LifeEventsQuery,
) -> Result<Json<ApiLifeEventsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    Err(life_storage_unavailable_response())
}

#[cfg(feature = "storage-sqlx")]
async fn list_life_memories_for_user(
    state: &AppState,
    web_user_id: i64,
) -> Result<Json<ApiLifeMemoriesResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let life_storage = state
        .life_storage()
        .ok_or_else(life_storage_unavailable_response)?;
    let principal = PrincipalUserId::new(web_user_id).map_err(life_domain_error_response)?;
    let Some(active) = life_storage
        .active_generation(principal)
        .await
        .map_err(life_storage_error_response)?
    else {
        return Ok(Json(ApiLifeMemoriesResponse {
            active_memory_generation_id: None,
            memories: Vec::new(),
            conflicts: Vec::new(),
        }));
    };
    let memories = life_storage
        .memory_items_for_generation(active.scope)
        .await
        .map_err(life_storage_error_response)?
        .into_iter()
        .map(api_memory)
        .collect();
    Ok(Json(ApiLifeMemoriesResponse {
        active_memory_generation_id: Some(active.scope.memory_generation_id.to_string()),
        memories,
        conflicts: Vec::new(),
    }))
}

#[cfg(not(feature = "storage-sqlx"))]
async fn list_life_memories_for_user(
    _state: &AppState,
    _web_user_id: i64,
) -> Result<Json<ApiLifeMemoriesResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    Err(life_storage_unavailable_response())
}

#[cfg(feature = "storage-sqlx")]
async fn forget_life_memory_for_user(
    state: &AppState,
    web_user_id: i64,
    memory_id: String,
) -> Result<Json<ApiLifeLifecycleResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let life_storage = state
        .life_storage()
        .ok_or_else(life_storage_unavailable_response)?;
    let principal = PrincipalUserId::new(web_user_id).map_err(life_domain_error_response)?;
    let memory_id = parse_memory_item_id(&memory_id)?;
    let deleted = life_storage
        .mark_memory_item_deleted(
            principal,
            memory_id,
            life_now().map_err(life_clock_error_response)?,
        )
        .await
        .map_err(life_storage_error_response)?;
    Ok(Json(ApiLifeLifecycleResponse {
        principal_user_id: web_user_id,
        memory_generation_id: None,
        status: if deleted { "deleted" } else { "not_found" }.to_owned(),
    }))
}

#[cfg(not(feature = "storage-sqlx"))]
async fn forget_life_memory_for_user(
    _state: &AppState,
    web_user_id: i64,
    memory_id: String,
) -> Result<Json<ApiLifeLifecycleResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let _ = (web_user_id, memory_id);
    Err(life_storage_unavailable_response())
}

#[cfg(feature = "storage-sqlx")]
async fn list_life_task_states_for_user(
    state: &AppState,
    web_user_id: i64,
) -> Result<Json<ApiLifeTaskStatesResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let life_storage = state
        .life_storage()
        .ok_or_else(life_storage_unavailable_response)?;
    let principal = PrincipalUserId::new(web_user_id).map_err(life_domain_error_response)?;
    let Some(active) = life_storage
        .active_generation(principal)
        .await
        .map_err(life_storage_error_response)?
    else {
        return Ok(Json(ApiLifeTaskStatesResponse {
            task_states: Vec::new(),
        }));
    };
    Ok(Json(ApiLifeTaskStatesResponse {
        task_states: life_storage
            .active_task_states(active.scope)
            .await
            .map_err(life_storage_error_response)?
            .into_iter()
            .map(api_task_state)
            .collect(),
    }))
}

#[cfg(not(feature = "storage-sqlx"))]
async fn list_life_task_states_for_user(
    _state: &AppState,
    _web_user_id: i64,
) -> Result<Json<ApiLifeTaskStatesResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    Err(life_storage_unavailable_response())
}

#[cfg(feature = "storage-sqlx")]
async fn list_life_friction_patterns_for_user(
    state: &AppState,
    web_user_id: i64,
) -> Result<Json<ApiLifeFrictionPatternsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let life_storage = state
        .life_storage()
        .ok_or_else(life_storage_unavailable_response)?;
    let principal = PrincipalUserId::new(web_user_id).map_err(life_domain_error_response)?;
    let Some(active) = life_storage
        .active_generation(principal)
        .await
        .map_err(life_storage_error_response)?
    else {
        return Ok(Json(ApiLifeFrictionPatternsResponse {
            friction_patterns: Vec::new(),
        }));
    };
    Ok(Json(ApiLifeFrictionPatternsResponse {
        friction_patterns: life_storage
            .active_friction_patterns(active.scope)
            .await
            .map_err(life_storage_error_response)?
            .into_iter()
            .map(api_friction_pattern)
            .collect(),
    }))
}

#[cfg(not(feature = "storage-sqlx"))]
async fn list_life_friction_patterns_for_user(
    _state: &AppState,
    _web_user_id: i64,
) -> Result<Json<ApiLifeFrictionPatternsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    Err(life_storage_unavailable_response())
}

#[cfg(feature = "storage-sqlx")]
async fn list_life_support_protocols_for_user(
    state: &AppState,
    web_user_id: i64,
) -> Result<Json<ApiLifeSupportProtocolsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let life_storage = state
        .life_storage()
        .ok_or_else(life_storage_unavailable_response)?;
    let principal = PrincipalUserId::new(web_user_id).map_err(life_domain_error_response)?;
    let Some(active) = life_storage
        .active_generation(principal)
        .await
        .map_err(life_storage_error_response)?
    else {
        return Ok(Json(ApiLifeSupportProtocolsResponse {
            support_protocols: Vec::new(),
        }));
    };
    Ok(Json(ApiLifeSupportProtocolsResponse {
        support_protocols: life_storage
            .active_support_protocols(active.scope)
            .await
            .map_err(life_storage_error_response)?
            .into_iter()
            .map(api_support_protocol)
            .collect(),
    }))
}

#[cfg(not(feature = "storage-sqlx"))]
async fn list_life_support_protocols_for_user(
    _state: &AppState,
    _web_user_id: i64,
) -> Result<Json<ApiLifeSupportProtocolsResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    Err(life_storage_unavailable_response())
}

#[cfg(not(feature = "storage-sqlx"))]
async fn soft_reset_life_generation_for_user(
    _state: &AppState,
    _web_user_id: i64,
    _request: ApiLifeSoftResetRequest,
) -> Result<Json<ApiLifeGenerationResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    Err(life_storage_unavailable_response())
}

#[cfg(feature = "storage-sqlx")]
async fn activate_life_generation_for_user(
    state: &AppState,
    web_user_id: i64,
    generation_id: String,
) -> Result<Json<ApiLifeLifecycleResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let life_storage = state
        .life_storage()
        .ok_or_else(life_storage_unavailable_response)?;
    let principal = PrincipalUserId::new(web_user_id).map_err(life_domain_error_response)?;
    let generation_id = parse_memory_generation_id(&generation_id)?;
    life_storage
        .activate_memory_generation(
            principal,
            generation_id,
            life_now().map_err(life_clock_error_response)?,
            "web life generation activation",
        )
        .await
        .map_err(life_storage_error_response)?;
    Ok(Json(ApiLifeLifecycleResponse {
        principal_user_id: web_user_id,
        memory_generation_id: Some(generation_id.to_string()),
        status: "active".to_owned(),
    }))
}

#[cfg(not(feature = "storage-sqlx"))]
async fn activate_life_generation_for_user(
    _state: &AppState,
    web_user_id: i64,
    generation_id: String,
) -> Result<Json<ApiLifeLifecycleResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let _ = (web_user_id, generation_id);
    Err(life_storage_unavailable_response())
}

#[cfg(feature = "storage-sqlx")]
async fn wipe_life_generation_for_user(
    state: &AppState,
    web_user_id: i64,
    generation_id: String,
) -> Result<Json<ApiLifeLifecycleResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let life_storage = state
        .life_storage()
        .ok_or_else(life_storage_unavailable_response)?;
    let principal = PrincipalUserId::new(web_user_id).map_err(life_domain_error_response)?;
    let generation_id = parse_memory_generation_id(&generation_id)?;
    life_storage
        .wipe_memory_generation(
            principal,
            generation_id,
            life_now().map_err(life_clock_error_response)?,
        )
        .await
        .map_err(life_storage_error_response)?;
    Ok(Json(ApiLifeLifecycleResponse {
        principal_user_id: web_user_id,
        memory_generation_id: Some(generation_id.to_string()),
        status: "deleted".to_owned(),
    }))
}

#[cfg(not(feature = "storage-sqlx"))]
async fn wipe_life_generation_for_user(
    _state: &AppState,
    web_user_id: i64,
    generation_id: String,
) -> Result<Json<ApiLifeLifecycleResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let _ = (web_user_id, generation_id);
    Err(life_storage_unavailable_response())
}

#[cfg(feature = "storage-sqlx")]
async fn wipe_life_derived_generation_for_user(
    state: &AppState,
    web_user_id: i64,
    generation_id: String,
) -> Result<Json<ApiLifeLifecycleResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let life_storage = state
        .life_storage()
        .ok_or_else(life_storage_unavailable_response)?;
    let principal = PrincipalUserId::new(web_user_id).map_err(life_domain_error_response)?;
    let generation_id = parse_memory_generation_id(&generation_id)?;
    life_storage
        .wipe_derived_generation(principal, generation_id)
        .await
        .map_err(life_storage_error_response)?;
    Ok(Json(ApiLifeLifecycleResponse {
        principal_user_id: web_user_id,
        memory_generation_id: Some(generation_id.to_string()),
        status: "derived_wiped".to_owned(),
    }))
}

#[cfg(not(feature = "storage-sqlx"))]
async fn wipe_life_derived_generation_for_user(
    _state: &AppState,
    web_user_id: i64,
    generation_id: String,
) -> Result<Json<ApiLifeLifecycleResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let _ = (web_user_id, generation_id);
    Err(life_storage_unavailable_response())
}

#[cfg(feature = "storage-sqlx")]
async fn privacy_hard_wipe_life_for_user(
    state: &AppState,
    web_user_id: i64,
) -> Result<Json<ApiLifeLifecycleResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let life_storage = state
        .life_storage()
        .ok_or_else(life_storage_unavailable_response)?;
    let principal = PrincipalUserId::new(web_user_id).map_err(life_domain_error_response)?;
    life_storage
        .privacy_hard_wipe_life_state(principal)
        .await
        .map_err(life_storage_error_response)?;
    Ok(Json(ApiLifeLifecycleResponse {
        principal_user_id: web_user_id,
        memory_generation_id: None,
        status: "hard_wiped".to_owned(),
    }))
}

#[cfg(not(feature = "storage-sqlx"))]
async fn privacy_hard_wipe_life_for_user(
    _state: &AppState,
    web_user_id: i64,
) -> Result<Json<ApiLifeLifecycleResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let _ = web_user_id;
    Err(life_storage_unavailable_response())
}

#[cfg(not(feature = "storage-sqlx"))]
async fn submit_life_input_for_user(
    _state: &AppState,
    _web_user_id: i64,
    _request: ApiLifeSubmitRequest,
) -> Result<Json<ApiLifeSubmitResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    Err(life_storage_unavailable_response())
}

#[cfg(feature = "storage-sqlx")]
async fn life_state_for_user(
    state: &AppState,
    web_user_id: i64,
) -> Result<Json<ApiLifeStateResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let life_storage = state
        .life_storage()
        .ok_or_else(life_storage_unavailable_response)?;
    let principal = PrincipalUserId::new(web_user_id).map_err(life_domain_error_response)?;
    let active = life_storage
        .active_generation(principal)
        .await
        .map_err(life_storage_error_response)?;

    Ok(Json(ApiLifeStateResponse {
        principal_user_id: web_user_id,
        active_memory_generation_id: active
            .map(|generation| generation.scope.memory_generation_id.to_string()),
    }))
}

#[cfg(not(feature = "storage-sqlx"))]
async fn life_state_for_user(
    _state: &AppState,
    web_user_id: i64,
) -> Result<Json<ApiLifeStateResponse>, (StatusCode, Json<ErrorEnvelope>)> {
    let _ = web_user_id;
    Err(life_storage_unavailable_response())
}

#[cfg(feature = "storage-sqlx")]
struct FixedPrincipalAllocator {
    principal: PrincipalUserId,
}

#[cfg(feature = "storage-sqlx")]
#[async_trait]
impl LifePrincipalAllocator for FixedPrincipalAllocator {
    async fn allocate_principal_user_id(&self) -> Result<PrincipalUserId, LifeGatewayError> {
        Ok(self.principal)
    }
}

fn life_storage_unavailable_response() -> (StatusCode, Json<ErrorEnvelope>) {
    api_error(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::BackendUnavailable,
        "Life mode requires SQLx/Postgres storage.",
        true,
    )
}

#[cfg(feature = "storage-sqlx")]
fn life_gateway_error_response(error: LifeGatewayError) -> (StatusCode, Json<ErrorEnvelope>) {
    match error {
        LifeGatewayError::EmptyContent => api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            error.to_string(),
            false,
        ),
        LifeGatewayError::PrivateSecretRefused => api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationError,
            error.to_string(),
            false,
        ),
        LifeGatewayError::Storage(error) => life_storage_error_response(error),
        LifeGatewayError::Domain(error) => life_domain_error_response(error),
        LifeGatewayError::Clock(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            error,
            true,
        ),
    }
}

#[cfg(feature = "storage-sqlx")]
fn life_runtime_error_response(
    error: oxide_agent_life::runtime::LifeRuntimeError,
) -> (StatusCode, Json<ErrorEnvelope>) {
    match error {
        oxide_agent_life::runtime::LifeRuntimeError::Storage(error) => {
            life_storage_error_response(error)
        }
        oxide_agent_life::runtime::LifeRuntimeError::Clock(msg) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            msg,
            true,
        ),
        oxide_agent_life::runtime::LifeRuntimeError::NotClaimedAndNoActiveRun { .. } => api_error(
            StatusCode::CONFLICT,
            ErrorCode::Internal,
            error.to_string(),
            true,
        ),
    }
}

#[cfg(feature = "storage-sqlx")]
fn map_input_sensitivity(value: ApiLifeInputSensitivity) -> LifeInputSensitivity {
    match value {
        ApiLifeInputSensitivity::Normal => LifeInputSensitivity::Normal,
        ApiLifeInputSensitivity::Redacted => LifeInputSensitivity::Redacted,
        ApiLifeInputSensitivity::PrivateSecret => LifeInputSensitivity::PrivateSecret,
    }
}

#[cfg(feature = "storage-sqlx")]
fn api_generation(generation: LifeMemoryGeneration) -> ApiLifeGenerationResponse {
    ApiLifeGenerationResponse {
        memory_generation_id: generation.memory_generation_id.to_string(),
        generation_number: generation.generation_number,
        status: generation_status(generation.status).to_owned(),
        source_generation_id: generation
            .source_generation_id
            .map(|generation_id| generation_id.to_string()),
        build_reason: generation.build_reason,
        build_policy: generation.build_policy,
        source_scope: generation.source_scope,
        comparison_report: generation.comparison_report,
        activated_at: generation.activated_at.map(TimestampMillis::get),
        created_at: generation.created_at.get(),
        updated_at: generation.updated_at.get(),
    }
}

#[cfg(feature = "storage-sqlx")]
fn api_turn(turn: LifeTurn) -> ApiLifeTurnResponse {
    ApiLifeTurnResponse {
        turn_id: turn.turn_id.to_string(),
        run_id: turn.run_id.map(|run_id| run_id.to_string()),
        role: turn_role(turn.role).to_owned(),
        source_transport: source_transport(turn.source_transport).to_owned(),
        source_ref: turn.source_ref,
        content: turn.content,
        attachments: turn.attachments,
        transport_metadata: turn.transport_metadata,
        redaction_state: redaction_state(turn.redaction_state).to_owned(),
        created_at: turn.created_at.get(),
    }
}

#[cfg(feature = "storage-sqlx")]
fn api_event(event: LifeEvent) -> ApiLifeEventResponse {
    ApiLifeEventResponse {
        event_id: event.event_id.to_string(),
        run_id: event.run_id.to_string(),
        seq: event.seq,
        kind: event.kind,
        payload: event.payload,
        created_at: event.created_at.get(),
    }
}

#[cfg(feature = "storage-sqlx")]
fn api_memory(memory: LifeMemoryItem) -> ApiLifeMemoryItemResponse {
    ApiLifeMemoryItemResponse {
        memory_id: memory.memory_id.to_string(),
        memory_generation_id: memory.memory_generation_id.to_string(),
        kind: memory_kind(memory.kind).to_owned(),
        authority: authority(memory.authority).to_owned(),
        status: memory_status(memory.status).to_owned(),
        text: memory.text,
        structured: memory.structured,
        tags: memory.tags,
        evidence_turn_ids: memory
            .evidence_turn_ids
            .into_iter()
            .map(|turn_id| turn_id.to_string())
            .collect(),
        sensitivity: sensitivity(memory.sensitivity).to_owned(),
        created_at: memory.created_at.get(),
        updated_at: memory.updated_at.get(),
    }
}

#[cfg(feature = "storage-sqlx")]
fn api_task_state(task: LifeTaskState) -> ApiLifeTaskStateResponse {
    ApiLifeTaskStateResponse {
        task_state_id: task.task_state_id.to_string(),
        project_key: task.project_key,
        current_goal: task.current_goal,
        why: task.why,
        current_state: task.current_state,
        next_action: task.next_action,
        open_loops: task.open_loops,
        blockers: task.blockers,
        status: task_status(task.status).to_owned(),
        updated_at: task.updated_at.get(),
    }
}

#[cfg(feature = "storage-sqlx")]
fn api_friction_pattern(pattern: LifeFrictionPattern) -> ApiLifeFrictionPatternResponse {
    ApiLifeFrictionPatternResponse {
        pattern_id: pattern.pattern_id.to_string(),
        kind: friction_kind(pattern.kind).to_owned(),
        trigger_descriptor: pattern.trigger_descriptor,
        preferred_response: pattern.preferred_response,
        authority: authority(pattern.authority).to_owned(),
        status: support_status(pattern.status).to_owned(),
        updated_at: pattern.updated_at.get(),
    }
}

#[cfg(feature = "storage-sqlx")]
fn api_support_protocol(protocol: LifeSupportProtocol) -> ApiLifeSupportProtocolResponse {
    ApiLifeSupportProtocolResponse {
        protocol_id: protocol.protocol_id.to_string(),
        name: protocol.name,
        trigger_descriptor: protocol.trigger_descriptor,
        steps: protocol.steps,
        priority: protocol.priority,
        authority: authority(protocol.authority).to_owned(),
        status: support_status(protocol.status).to_owned(),
        updated_at: protocol.updated_at.get(),
    }
}

#[cfg(feature = "storage-sqlx")]
const fn generation_status(status: MemoryGenerationStatus) -> &'static str {
    match status {
        MemoryGenerationStatus::Building => "building",
        MemoryGenerationStatus::Active => "active",
        MemoryGenerationStatus::Archived => "archived",
        MemoryGenerationStatus::Failed => "failed",
        MemoryGenerationStatus::Deleted => "deleted",
    }
}

#[cfg(feature = "storage-sqlx")]
const fn turn_role(role: LifeTurnRole) -> &'static str {
    match role {
        LifeTurnRole::User => "user",
        LifeTurnRole::Assistant => "assistant",
        LifeTurnRole::System => "system",
        LifeTurnRole::Tool => "tool",
    }
}

#[cfg(feature = "storage-sqlx")]
const fn source_transport(transport: LifeSourceTransport) -> &'static str {
    match transport {
        LifeSourceTransport::Web => "web",
        LifeSourceTransport::Telegram => "telegram",
        LifeSourceTransport::Internal => "internal",
    }
}

#[cfg(feature = "storage-sqlx")]
const fn redaction_state(state: RedactionState) -> &'static str {
    match state {
        RedactionState::Clean => "clean",
        RedactionState::Redacted => "redacted",
        RedactionState::SecretBlocked => "secret_blocked",
    }
}

#[cfg(feature = "storage-sqlx")]
const fn memory_kind(kind: MemoryItemKind) -> &'static str {
    match kind {
        MemoryItemKind::Biography => "biography",
        MemoryItemKind::Preference => "preference",
        MemoryItemKind::ProjectPrinciple => "project_principle",
        MemoryItemKind::Procedure => "procedure",
        MemoryItemKind::Decision => "decision",
        MemoryItemKind::Episode => "episode",
        MemoryItemKind::OperatingRule => "operating_rule",
        MemoryItemKind::FrictionPattern => "friction_pattern",
        MemoryItemKind::SupportProtocol => "support_protocol",
    }
}

#[cfg(feature = "storage-sqlx")]
const fn authority(authority: MemoryAuthority) -> &'static str {
    match authority {
        MemoryAuthority::UserAsserted => "user_asserted",
        MemoryAuthority::UserConfirmed => "user_confirmed",
        MemoryAuthority::CuratorSuggested => "curator_suggested",
        MemoryAuthority::SystemDerived => "system_derived",
    }
}

#[cfg(feature = "storage-sqlx")]
const fn memory_status(status: MemoryItemStatus) -> &'static str {
    match status {
        MemoryItemStatus::Active => "active",
        MemoryItemStatus::Superseded => "superseded",
        MemoryItemStatus::Deleted => "deleted",
        MemoryItemStatus::Candidate => "candidate",
    }
}

#[cfg(feature = "storage-sqlx")]
const fn sensitivity(sensitivity: MemorySensitivity) -> &'static str {
    match sensitivity {
        MemorySensitivity::Clean => "clean",
        MemorySensitivity::Personal => "personal",
        MemorySensitivity::Redacted => "redacted",
        MemorySensitivity::SecretBlocked => "secret_blocked",
    }
}

#[cfg(feature = "storage-sqlx")]
const fn task_status(status: TaskStateStatus) -> &'static str {
    match status {
        TaskStateStatus::Active => "active",
        TaskStateStatus::Paused => "paused",
        TaskStateStatus::Completed => "completed",
        TaskStateStatus::Abandoned => "abandoned",
    }
}

#[cfg(feature = "storage-sqlx")]
const fn friction_kind(kind: FrictionPatternKind) -> &'static str {
    match kind {
        FrictionPatternKind::OverloadTrigger => "overload_trigger",
        FrictionPatternKind::TaskInitiationBarrier => "task_initiation_barrier",
        FrictionPatternKind::ContextLoss => "context_loss",
        FrictionPatternKind::CommunicationMismatch => "communication_mismatch",
        FrictionPatternKind::SensoryOrEnergyConstraint => "sensory_or_energy_constraint",
    }
}

#[cfg(feature = "storage-sqlx")]
const fn support_status(status: SupportStateStatus) -> &'static str {
    match status {
        SupportStateStatus::Active => "active",
        SupportStateStatus::Superseded => "superseded",
        SupportStateStatus::Deleted => "deleted",
        SupportStateStatus::Candidate => "candidate",
    }
}

#[cfg(feature = "storage-sqlx")]
fn parse_memory_generation_id(
    value: &str,
) -> Result<MemoryGenerationId, (StatusCode, Json<ErrorEnvelope>)> {
    uuid::Uuid::parse_str(value)
        .map(MemoryGenerationId::from_uuid)
        .map_err(|error| validation_error(format!("invalid memory_generation_id: {error}")))
}

#[cfg(feature = "storage-sqlx")]
fn parse_memory_item_id(value: &str) -> Result<MemoryItemId, (StatusCode, Json<ErrorEnvelope>)> {
    uuid::Uuid::parse_str(value)
        .map(MemoryItemId::from_uuid)
        .map_err(|error| validation_error(format!("invalid memory_id: {error}")))
}

#[cfg(feature = "storage-sqlx")]
fn life_now() -> Result<TimestampMillis, String> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?;
    Ok(TimestampMillis::new(
        i64::try_from(duration.as_millis()).map_err(|error| error.to_string())?,
    ))
}

#[cfg(feature = "storage-sqlx")]
fn life_clock_error_response(error: String) -> (StatusCode, Json<ErrorEnvelope>) {
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::Internal,
        error,
        true,
    )
}

#[cfg(feature = "storage-sqlx")]
fn validation_error(message: String) -> (StatusCode, Json<ErrorEnvelope>) {
    api_error(
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::ValidationError,
        message,
        false,
    )
}

#[cfg(feature = "storage-sqlx")]
fn life_storage_error_response(
    error: oxide_agent_life::storage::LifeStorageError,
) -> (StatusCode, Json<ErrorEnvelope>) {
    let (status, code, retryable) = match &error {
        LifeStorageError::InvalidCursor(_) => {
            (StatusCode::BAD_REQUEST, ErrorCode::ValidationError, false)
        }
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::BackendUnavailable,
            true,
        ),
    };
    api_error(status, code, error.to_string(), retryable)
}

#[cfg(feature = "storage-sqlx")]
fn life_domain_error_response(
    error: oxide_agent_life::errors::LifeDomainError,
) -> (StatusCode, Json<ErrorEnvelope>) {
    api_error(
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::ValidationError,
        error.to_string(),
        false,
    )
}

#[cfg(test)]
mod tests {
    use oxide_agent_web_contracts::ApiLifeSubmitRequest;

    #[test]
    fn life_submit_contract_keeps_transport_metadata_separate() {
        let request = ApiLifeSubmitRequest {
            content: "continue".to_string(),
            attachments: serde_json::json!([{"kind":"file","id":"a"}]),
            metadata: serde_json::json!({"correlation_id":"web-1"}),
            sensitivity: oxide_agent_web_contracts::ApiLifeInputSensitivity::Normal,
        };

        assert_eq!(request.attachments[0]["id"], "a");
        assert_eq!(request.metadata["correlation_id"], "web-1");
    }
}
