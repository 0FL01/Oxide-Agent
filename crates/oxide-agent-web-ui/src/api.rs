use gloo_net::http::{Request, Response};
use oxide_agent_web_contracts::{
    AuthUserResponse, BootstrapRequest, CancelTaskResponse, ChangePasswordRequest,
    CreateSessionRequest, CreateSessionResponse, CreateTaskRequest, CreateTaskResponse,
    CreateTaskVersionRequest, CreateTaskVersionResponse, CurrentUserResponse, ErrorCode,
    ErrorEnvelope, GetSessionResponse, GetTaskProgressResponse, GetTaskResponse,
    ListSessionsResponse, ListTasksResponse, LoginRequest, OkResponse, PublicConfigResponse,
    RegisterRequest, ResumeTaskRequest, ResumeTaskResponse, TaskEventsResponse,
    UpdateSessionRequest, UpdateSessionResponse, UploadTaskAttachmentsResponse,
};
use serde::{de::DeserializeOwned, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiClient {
    csrf_token: Option<String>,
}

impl ApiClient {
    #[must_use]
    pub const fn new(csrf_token: Option<String>) -> Self {
        Self { csrf_token }
    }

    #[allow(dead_code)]
    pub async fn public_config(&self) -> Result<PublicConfigResponse, ApiClientError> {
        decode(Request::get("/api/v1/public-config").send().await?).await
    }

    pub async fn me(&self) -> Result<CurrentUserResponse, ApiClientError> {
        decode(with_credentials(Request::get("/api/v1/me")).send().await?).await
    }

    pub async fn login(&self, request: &LoginRequest) -> Result<AuthUserResponse, ApiClientError> {
        self.post("/api/v1/auth/login", request, false).await
    }

    pub async fn register(
        &self,
        request: &RegisterRequest,
    ) -> Result<AuthUserResponse, ApiClientError> {
        self.post("/api/v1/auth/register", request, false).await
    }

    pub async fn bootstrap(
        &self,
        request: &BootstrapRequest,
    ) -> Result<AuthUserResponse, ApiClientError> {
        self.post("/api/v1/auth/bootstrap", request, false).await
    }

    pub async fn logout(&self) -> Result<OkResponse, ApiClientError> {
        self.post_empty("/api/v1/auth/logout").await
    }

    pub async fn change_password(
        &self,
        request: &ChangePasswordRequest,
    ) -> Result<OkResponse, ApiClientError> {
        self.post("/api/v1/auth/change-password", request, true)
            .await
    }

    pub async fn list_sessions(&self) -> Result<ListSessionsResponse, ApiClientError> {
        decode(
            with_credentials(Request::get("/api/v1/sessions"))
                .send()
                .await?,
        )
        .await
    }

    pub async fn create_session(&self) -> Result<CreateSessionResponse, ApiClientError> {
        self.post("/api/v1/sessions", &CreateSessionRequest {}, true)
            .await
    }

    pub async fn get_session(
        &self,
        session_id: &str,
    ) -> Result<GetSessionResponse, ApiClientError> {
        decode(
            with_credentials(Request::get(&format!("/api/v1/sessions/{session_id}")))
                .send()
                .await?,
        )
        .await
    }

    #[allow(dead_code)]
    pub async fn update_session(
        &self,
        session_id: &str,
        request: &UpdateSessionRequest,
    ) -> Result<UpdateSessionResponse, ApiClientError> {
        let mut builder =
            with_credentials(Request::patch(&format!("/api/v1/sessions/{session_id}")))
                .header("Content-Type", "application/json");
        builder = self.with_csrf(builder)?;
        decode(builder.json(request)?.send().await?).await
    }

    #[allow(dead_code)]
    pub async fn delete_session(&self, session_id: &str) -> Result<OkResponse, ApiClientError> {
        let builder = self.with_csrf(with_credentials(Request::delete(&format!(
            "/api/v1/sessions/{session_id}"
        ))))?;
        decode(builder.send().await?).await
    }

    pub async fn list_tasks(&self, session_id: &str) -> Result<ListTasksResponse, ApiClientError> {
        decode(
            with_credentials(Request::get(&format!(
                "/api/v1/sessions/{session_id}/tasks"
            )))
            .send()
            .await?,
        )
        .await
    }

    pub async fn create_task(
        &self,
        session_id: &str,
        request: &CreateTaskRequest,
    ) -> Result<CreateTaskResponse, ApiClientError> {
        self.post(
            &format!("/api/v1/sessions/{session_id}/tasks"),
            request,
            true,
        )
        .await
    }

    pub async fn get_task(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<GetTaskResponse, ApiClientError> {
        decode(
            with_credentials(Request::get(&format!(
                "/api/v1/sessions/{session_id}/tasks/{task_id}"
            )))
            .send()
            .await?,
        )
        .await
    }

    #[allow(dead_code)]
    pub async fn task_progress(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<GetTaskProgressResponse, ApiClientError> {
        decode(
            with_credentials(Request::get(&format!(
                "/api/v1/sessions/{session_id}/tasks/{task_id}/progress"
            )))
            .send()
            .await?,
        )
        .await
    }

    pub async fn task_events(
        &self,
        session_id: &str,
        task_id: &str,
        after_seq: u64,
    ) -> Result<TaskEventsResponse, ApiClientError> {
        decode(
            with_credentials(Request::get(&format!(
                "/api/v1/sessions/{session_id}/tasks/{task_id}/events?after_seq={after_seq}&limit=100"
            )))
            .send()
            .await?,
        )
        .await
    }

    #[allow(dead_code)]
    pub async fn create_task_version(
        &self,
        session_id: &str,
        task_id: &str,
        request: &CreateTaskVersionRequest,
    ) -> Result<CreateTaskVersionResponse, ApiClientError> {
        let mut builder = with_credentials(Request::post(&format!(
            "/api/v1/sessions/{session_id}/tasks/{task_id}/versions"
        )))
        .header("Content-Type", "application/json");
        builder = self.with_csrf(builder)?;
        decode(builder.json(request)?.send().await?).await
    }

    pub async fn resume_task(
        &self,
        session_id: &str,
        task_id: &str,
        request: &ResumeTaskRequest,
    ) -> Result<ResumeTaskResponse, ApiClientError> {
        self.post(
            &format!("/api/v1/sessions/{session_id}/tasks/{task_id}/resume"),
            request,
            true,
        )
        .await
    }

    pub async fn upload_task_attachments(
        &self,
        session_id: &str,
        files: &[web_sys::File],
    ) -> Result<UploadTaskAttachmentsResponse, ApiClientError> {
        let form_data = web_sys::FormData::new().map_err(|error| {
            ApiClientError::Browser(format!("form data init failed: {error:?}"))
        })?;
        for file in files {
            form_data
                .append_with_blob_and_filename("files", file, &file.name())
                .map_err(|error| {
                    ApiClientError::Browser(format!(
                        "failed to append attachment '{}': {error:?}",
                        file.name()
                    ))
                })?;
        }

        let builder = self.with_csrf(with_credentials(Request::post(&format!(
            "/api/v1/sessions/{session_id}/uploads"
        ))))?;
        decode(builder.body(form_data)?.send().await?).await
    }

    pub async fn cancel_task(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<CancelTaskResponse, ApiClientError> {
        self.post_empty(&format!(
            "/api/v1/sessions/{session_id}/tasks/{task_id}/cancel"
        ))
        .await
    }

    async fn post<B, T>(&self, path: &str, body: &B, csrf: bool) -> Result<T, ApiClientError>
    where
        B: Serialize,
        T: DeserializeOwned,
    {
        let mut builder =
            with_credentials(Request::post(path)).header("Content-Type", "application/json");
        if csrf {
            builder = self.with_csrf(builder)?;
        }
        decode(builder.json(body)?.send().await?).await
    }

    async fn post_empty<T>(&self, path: &str) -> Result<T, ApiClientError>
    where
        T: DeserializeOwned,
    {
        let builder = self.with_csrf(with_credentials(Request::post(path)))?;
        decode(builder.send().await?).await
    }

    fn with_csrf(
        &self,
        builder: gloo_net::http::RequestBuilder,
    ) -> Result<gloo_net::http::RequestBuilder, ApiClientError> {
        let token = self
            .csrf_token
            .as_deref()
            .ok_or(ApiClientError::MissingCsrfToken)?;
        Ok(builder.header("X-CSRF-Token", token))
    }
}

fn with_credentials(builder: gloo_net::http::RequestBuilder) -> gloo_net::http::RequestBuilder {
    builder.credentials(web_sys::RequestCredentials::Include)
}

async fn decode<T>(response: Response) -> Result<T, ApiClientError>
where
    T: DeserializeOwned,
{
    let status = response.status();
    if (200..300).contains(&status) {
        return Ok(response.json::<T>().await?);
    }

    let envelope = response.json::<ErrorEnvelope>().await.ok();
    Err(ApiClientError::Api { status, envelope })
}

#[derive(Debug)]
pub enum ApiClientError {
    Transport(gloo_net::Error),
    Browser(String),
    Api {
        status: u16,
        envelope: Option<ErrorEnvelope>,
    },
    MissingCsrfToken,
}

impl fmt::Display for ApiClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "request failed: {error}"),
            Self::Browser(error) => write!(formatter, "browser request setup failed: {error}"),
            Self::Api {
                status,
                envelope: Some(envelope),
            } => write!(formatter, "{} ({status})", envelope.error.message),
            Self::Api {
                status,
                envelope: None,
            } => write!(formatter, "request failed with status {status}"),
            Self::MissingCsrfToken => write!(formatter, "CSRF token is missing"),
        }
    }
}

impl ApiClientError {
    #[must_use]
    pub fn error_code(&self) -> Option<&ErrorCode> {
        match self {
            Self::Api {
                envelope: Some(envelope),
                ..
            } => Some(&envelope.error.code),
            _ => None,
        }
    }
}

impl From<gloo_net::Error> for ApiClientError {
    fn from(error: gloo_net::Error) -> Self {
        Self::Transport(error)
    }
}
