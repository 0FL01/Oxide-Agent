use axum::{Json, extract::State, http::HeaderMap, http::StatusCode};
use oxide_agent_web_contracts::{
    ApiLifeStateResponse, ApiLifeSubmitRequest, ApiLifeSubmitResponse, ErrorCode, ErrorEnvelope,
};

use super::{AppState, api_error, authenticated_user_with_csrf};

#[cfg(feature = "storage-sqlx")]
use async_trait::async_trait;
#[cfg(feature = "storage-sqlx")]
use oxide_agent_life::{
    domain::{LifeIdentityProvider, PrincipalUserId, ProviderSubject},
    gateway::{LifeGateway, LifeGatewayError, LifeInputSubmission, LifePrincipalAllocator},
    storage::LifeStorageRepository,
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
        })
        .await
        .map_err(life_gateway_error_response)?;

    Ok(Json(ApiLifeSubmitResponse {
        principal_user_id: result.principal_user_id.get(),
        memory_generation_id: result.memory_scope.memory_generation_id.to_string(),
        turn_id: result.turn_id.to_string(),
        input_id: result.input_id.to_string(),
        run_id: result.run_id.map(|run_id| run_id.to_string()),
    }))
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
fn life_storage_error_response(
    error: oxide_agent_life::storage::LifeStorageError,
) -> (StatusCode, Json<ErrorEnvelope>) {
    api_error(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::BackendUnavailable,
        error.to_string(),
        true,
    )
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
        };

        assert_eq!(request.attachments[0]["id"], "a");
        assert_eq!(request.metadata["correlation_id"], "web-1");
    }
}
