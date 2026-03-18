#![allow(missing_docs)]

use anyhow::{anyhow, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::config::get_sandboxd_socket;

use super::manager::{DockerSandboxManager, ExecResult, SandboxContainerRecord};
use super::scope::SandboxScope;

#[derive(Debug, Serialize, Deserialize)]
pub enum SandboxBrokerRequest {
    ListUserSandboxes {
        user_id: i64,
    },
    InspectSandboxByName {
        user_id: i64,
        container_name: String,
    },
    EnsureScopeSandbox {
        scope: SandboxScope,
        image_name: String,
    },
    RecreateScopeSandbox {
        scope: SandboxScope,
        image_name: String,
    },
    DeleteSandboxByName {
        user_id: i64,
        container_name: String,
    },
    CreateSandbox {
        scope: SandboxScope,
        image_name: String,
    },
    ExecCommand {
        scope: SandboxScope,
        image_name: String,
        command: String,
    },
    WriteFile {
        scope: SandboxScope,
        image_name: String,
        path: String,
        #[serde(with = "serde_bytes")]
        content: Vec<u8>,
    },
    ReadFile {
        scope: SandboxScope,
        image_name: String,
        path: String,
    },
    UploadFile {
        scope: SandboxScope,
        image_name: String,
        container_path: String,
        #[serde(with = "serde_bytes")]
        content: Vec<u8>,
    },
    DownloadFile {
        scope: SandboxScope,
        image_name: String,
        container_path: String,
    },
    GetUploadsSize {
        scope: SandboxScope,
        image_name: String,
    },
    CleanupOldDownloads {
        scope: SandboxScope,
        image_name: String,
    },
    Destroy {
        scope: SandboxScope,
        image_name: String,
    },
    Recreate {
        scope: SandboxScope,
        image_name: String,
    },
    FileSizeBytes {
        scope: SandboxScope,
        image_name: String,
        container_path: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum SandboxBrokerResponse {
    Sandboxes(Vec<SandboxContainerRecord>),
    Sandbox(Option<SandboxContainerRecord>),
    SandboxRecord(SandboxContainerRecord),
    Deleted(bool),
    ContainerCreated { container_id: Option<String> },
    ExecResult(ExecResult),
    Bytes(#[serde(with = "serde_bytes")] Vec<u8>),
    U64(u64),
    Unit,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct SandboxBrokerClient {
    socket_path: PathBuf,
}

impl SandboxBrokerClient {
    #[must_use]
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    #[must_use]
    pub fn from_env() -> Self {
        Self::new(get_sandboxd_socket())
    }

    async fn send_request(&self, request: &SandboxBrokerRequest) -> Result<SandboxBrokerResponse> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to connect to sandbox broker socket {}",
                    self.socket_path.display()
                )
            })?;
        write_frame(&mut stream, request).await?;
        read_frame(&mut stream).await
    }

    async fn send_exec_request(
        &self,
        request: &SandboxBrokerRequest,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<SandboxBrokerResponse> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to connect to sandbox broker socket {}",
                    self.socket_path.display()
                )
            })?;
        write_frame(&mut stream, request).await?;

        if let Some(token) = cancellation_token {
            tokio::select! {
                response = read_frame(&mut stream) => response,
                _ = token.cancelled() => Err(anyhow!("Command execution cancelled by user")),
            }
        } else {
            read_frame(&mut stream).await
        }
    }

    pub async fn list_user_sandboxes(&self, user_id: i64) -> Result<Vec<SandboxContainerRecord>> {
        match self
            .send_request(&SandboxBrokerRequest::ListUserSandboxes { user_id })
            .await?
        {
            SandboxBrokerResponse::Sandboxes(records) => Ok(records),
            SandboxBrokerResponse::Error(message) => Err(anyhow!(message)),
            response => Err(anyhow!("Unexpected broker response: {response:?}")),
        }
    }

    pub async fn inspect_sandbox_by_name(
        &self,
        user_id: i64,
        container_name: &str,
    ) -> Result<Option<SandboxContainerRecord>> {
        match self
            .send_request(&SandboxBrokerRequest::InspectSandboxByName {
                user_id,
                container_name: container_name.to_string(),
            })
            .await?
        {
            SandboxBrokerResponse::Sandbox(record) => Ok(record),
            SandboxBrokerResponse::Error(message) => Err(anyhow!(message)),
            response => Err(anyhow!("Unexpected broker response: {response:?}")),
        }
    }

    pub async fn ensure_scope_sandbox(
        &self,
        scope: SandboxScope,
        image_name: String,
    ) -> Result<SandboxContainerRecord> {
        match self
            .send_request(&SandboxBrokerRequest::EnsureScopeSandbox { scope, image_name })
            .await?
        {
            SandboxBrokerResponse::SandboxRecord(record) => Ok(record),
            SandboxBrokerResponse::Error(message) => Err(anyhow!(message)),
            response => Err(anyhow!("Unexpected broker response: {response:?}")),
        }
    }

    pub async fn recreate_scope_sandbox(
        &self,
        scope: SandboxScope,
        image_name: String,
    ) -> Result<SandboxContainerRecord> {
        match self
            .send_request(&SandboxBrokerRequest::RecreateScopeSandbox { scope, image_name })
            .await?
        {
            SandboxBrokerResponse::SandboxRecord(record) => Ok(record),
            SandboxBrokerResponse::Error(message) => Err(anyhow!(message)),
            response => Err(anyhow!("Unexpected broker response: {response:?}")),
        }
    }

    pub async fn delete_sandbox_by_name(&self, user_id: i64, container_name: &str) -> Result<bool> {
        match self
            .send_request(&SandboxBrokerRequest::DeleteSandboxByName {
                user_id,
                container_name: container_name.to_string(),
            })
            .await?
        {
            SandboxBrokerResponse::Deleted(deleted) => Ok(deleted),
            SandboxBrokerResponse::Error(message) => Err(anyhow!(message)),
            response => Err(anyhow!("Unexpected broker response: {response:?}")),
        }
    }

    pub async fn create_sandbox(
        &self,
        scope: SandboxScope,
        image_name: String,
    ) -> Result<Option<String>> {
        match self
            .send_request(&SandboxBrokerRequest::CreateSandbox { scope, image_name })
            .await?
        {
            SandboxBrokerResponse::ContainerCreated { container_id } => Ok(container_id),
            SandboxBrokerResponse::Error(message) => Err(anyhow!(message)),
            response => Err(anyhow!("Unexpected broker response: {response:?}")),
        }
    }

    pub async fn exec_command(
        &self,
        scope: SandboxScope,
        image_name: String,
        command: &str,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<ExecResult> {
        match self
            .send_exec_request(
                &SandboxBrokerRequest::ExecCommand {
                    scope,
                    image_name,
                    command: command.to_string(),
                },
                cancellation_token,
            )
            .await?
        {
            SandboxBrokerResponse::ExecResult(result) => Ok(result),
            SandboxBrokerResponse::Error(message) => Err(anyhow!(message)),
            response => Err(anyhow!("Unexpected broker response: {response:?}")),
        }
    }

    pub async fn write_file(
        &self,
        scope: SandboxScope,
        image_name: String,
        path: &str,
        content: &[u8],
    ) -> Result<()> {
        match self
            .send_request(&SandboxBrokerRequest::WriteFile {
                scope,
                image_name,
                path: path.to_string(),
                content: content.to_vec(),
            })
            .await?
        {
            SandboxBrokerResponse::Unit => Ok(()),
            SandboxBrokerResponse::Error(message) => Err(anyhow!(message)),
            response => Err(anyhow!("Unexpected broker response: {response:?}")),
        }
    }

    pub async fn read_file(
        &self,
        scope: SandboxScope,
        image_name: String,
        path: &str,
    ) -> Result<Vec<u8>> {
        match self
            .send_request(&SandboxBrokerRequest::ReadFile {
                scope,
                image_name,
                path: path.to_string(),
            })
            .await?
        {
            SandboxBrokerResponse::Bytes(content) => Ok(content),
            SandboxBrokerResponse::Error(message) => Err(anyhow!(message)),
            response => Err(anyhow!("Unexpected broker response: {response:?}")),
        }
    }

    pub async fn upload_file(
        &self,
        scope: SandboxScope,
        image_name: String,
        container_path: &str,
        content: &[u8],
    ) -> Result<()> {
        match self
            .send_request(&SandboxBrokerRequest::UploadFile {
                scope,
                image_name,
                container_path: container_path.to_string(),
                content: content.to_vec(),
            })
            .await?
        {
            SandboxBrokerResponse::Unit => Ok(()),
            SandboxBrokerResponse::Error(message) => Err(anyhow!(message)),
            response => Err(anyhow!("Unexpected broker response: {response:?}")),
        }
    }

    pub async fn download_file(
        &self,
        scope: SandboxScope,
        image_name: String,
        container_path: &str,
    ) -> Result<Vec<u8>> {
        match self
            .send_request(&SandboxBrokerRequest::DownloadFile {
                scope,
                image_name,
                container_path: container_path.to_string(),
            })
            .await?
        {
            SandboxBrokerResponse::Bytes(content) => Ok(content),
            SandboxBrokerResponse::Error(message) => Err(anyhow!(message)),
            response => Err(anyhow!("Unexpected broker response: {response:?}")),
        }
    }

    pub async fn get_uploads_size(&self, scope: SandboxScope, image_name: String) -> Result<u64> {
        match self
            .send_request(&SandboxBrokerRequest::GetUploadsSize { scope, image_name })
            .await?
        {
            SandboxBrokerResponse::U64(size) => Ok(size),
            SandboxBrokerResponse::Error(message) => Err(anyhow!(message)),
            response => Err(anyhow!("Unexpected broker response: {response:?}")),
        }
    }

    pub async fn cleanup_old_downloads(
        &self,
        scope: SandboxScope,
        image_name: String,
    ) -> Result<u64> {
        match self
            .send_request(&SandboxBrokerRequest::CleanupOldDownloads { scope, image_name })
            .await?
        {
            SandboxBrokerResponse::U64(count) => Ok(count),
            SandboxBrokerResponse::Error(message) => Err(anyhow!(message)),
            response => Err(anyhow!("Unexpected broker response: {response:?}")),
        }
    }

    pub async fn destroy(&self, scope: SandboxScope, image_name: String) -> Result<()> {
        match self
            .send_request(&SandboxBrokerRequest::Destroy { scope, image_name })
            .await?
        {
            SandboxBrokerResponse::Unit => Ok(()),
            SandboxBrokerResponse::Error(message) => Err(anyhow!(message)),
            response => Err(anyhow!("Unexpected broker response: {response:?}")),
        }
    }

    pub async fn recreate(&self, scope: SandboxScope, image_name: String) -> Result<()> {
        match self
            .send_request(&SandboxBrokerRequest::Recreate { scope, image_name })
            .await?
        {
            SandboxBrokerResponse::Unit => Ok(()),
            SandboxBrokerResponse::Error(message) => Err(anyhow!(message)),
            response => Err(anyhow!("Unexpected broker response: {response:?}")),
        }
    }

    pub async fn file_size_bytes(
        &self,
        scope: SandboxScope,
        image_name: String,
        container_path: &str,
    ) -> Result<u64> {
        match self
            .send_request(&SandboxBrokerRequest::FileSizeBytes {
                scope,
                image_name,
                container_path: container_path.to_string(),
            })
            .await?
        {
            SandboxBrokerResponse::U64(size) => Ok(size),
            SandboxBrokerResponse::Error(message) => Err(anyhow!(message)),
            response => Err(anyhow!("Unexpected broker response: {response:?}")),
        }
    }
}

pub struct SandboxBrokerServer {
    listener: UnixListener,
    socket_path: PathBuf,
}

impl SandboxBrokerServer {
    pub async fn bind(socket_path: impl AsRef<Path>) -> Result<Self> {
        let socket_path = socket_path.as_ref().to_path_buf();
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent).await.with_context(|| {
                format!(
                    "Failed to create sandbox broker directory {}",
                    parent.display()
                )
            })?;
        }

        match fs::remove_file(&socket_path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Failed to remove existing sandbox broker socket {}",
                        socket_path.display()
                    )
                });
            }
        }

        let listener = UnixListener::bind(&socket_path).with_context(|| {
            format!(
                "Failed to bind sandbox broker socket {}",
                socket_path.display()
            )
        })?;
        fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o666))
            .await
            .with_context(|| {
                format!(
                    "Failed to set sandbox broker socket permissions for {}",
                    socket_path.display()
                )
            })?;

        Ok(Self {
            listener,
            socket_path,
        })
    }

    pub async fn bind_default() -> Result<Self> {
        Self::bind(get_sandboxd_socket()).await
    }

    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub async fn serve(self) -> Result<()> {
        loop {
            let (stream, _) = self
                .listener
                .accept()
                .await
                .context("Failed to accept sandbox broker connection")?;
            tokio::spawn(async move {
                if let Err(error) = handle_connection(stream).await {
                    warn!(error = %error, "Sandbox broker connection failed");
                }
            });
        }
    }
}

async fn handle_connection(mut stream: UnixStream) -> Result<()> {
    let request: SandboxBrokerRequest = read_frame(&mut stream).await?;
    let response = handle_request(request, &mut stream).await?;
    if let Some(response) = response {
        write_frame(&mut stream, &response).await?;
    }
    Ok(())
}

async fn docker_manager(scope: SandboxScope, image_name: String) -> Result<DockerSandboxManager> {
    DockerSandboxManager::new_with_image(scope, image_name).await
}

fn response_from_result<T>(
    result: Result<T>,
    map: impl FnOnce(T) -> SandboxBrokerResponse,
) -> SandboxBrokerResponse {
    match result {
        Ok(value) => map(value),
        Err(error) => SandboxBrokerResponse::Error(error.to_string()),
    }
}

async fn handle_create_sandbox(scope: SandboxScope, image_name: String) -> SandboxBrokerResponse {
    let mut manager = match docker_manager(scope, image_name).await {
        Ok(manager) => manager,
        Err(error) => return SandboxBrokerResponse::Error(error.to_string()),
    };

    response_from_result(manager.create_sandbox().await, |_| {
        SandboxBrokerResponse::ContainerCreated {
            container_id: manager.container_id().map(ToOwned::to_owned),
        }
    })
}

async fn handle_write_file(
    scope: SandboxScope,
    image_name: String,
    path: String,
    content: Vec<u8>,
) -> SandboxBrokerResponse {
    let mut manager = match docker_manager(scope, image_name).await {
        Ok(manager) => manager,
        Err(error) => return SandboxBrokerResponse::Error(error.to_string()),
    };

    if let Err(error) = manager.attach_existing_container().await {
        return SandboxBrokerResponse::Error(error.to_string());
    }

    response_from_result(manager.write_file(&path, &content).await, |_| {
        SandboxBrokerResponse::Unit
    })
}

async fn handle_read_file(
    scope: SandboxScope,
    image_name: String,
    path: String,
) -> SandboxBrokerResponse {
    let mut manager = match docker_manager(scope, image_name).await {
        Ok(manager) => manager,
        Err(error) => return SandboxBrokerResponse::Error(error.to_string()),
    };

    response_from_result(manager.read_file(&path).await, SandboxBrokerResponse::Bytes)
}

async fn handle_upload_file(
    scope: SandboxScope,
    image_name: String,
    container_path: String,
    content: Vec<u8>,
) -> SandboxBrokerResponse {
    let mut manager = match docker_manager(scope, image_name).await {
        Ok(manager) => manager,
        Err(error) => return SandboxBrokerResponse::Error(error.to_string()),
    };

    if let Err(error) = manager.attach_existing_container().await {
        return SandboxBrokerResponse::Error(error.to_string());
    }

    response_from_result(manager.upload_file(&container_path, &content).await, |_| {
        SandboxBrokerResponse::Unit
    })
}

async fn handle_download_file(
    scope: SandboxScope,
    image_name: String,
    container_path: String,
) -> SandboxBrokerResponse {
    let mut manager = match docker_manager(scope, image_name).await {
        Ok(manager) => manager,
        Err(error) => return SandboxBrokerResponse::Error(error.to_string()),
    };

    if let Err(error) = manager.attach_existing_container().await {
        return SandboxBrokerResponse::Error(error.to_string());
    }

    response_from_result(
        manager.download_file(&container_path).await,
        SandboxBrokerResponse::Bytes,
    )
}

async fn handle_get_uploads_size(scope: SandboxScope, image_name: String) -> SandboxBrokerResponse {
    let mut manager = match docker_manager(scope, image_name).await {
        Ok(manager) => manager,
        Err(error) => return SandboxBrokerResponse::Error(error.to_string()),
    };

    response_from_result(manager.get_uploads_size().await, SandboxBrokerResponse::U64)
}

async fn handle_cleanup_old_downloads(
    scope: SandboxScope,
    image_name: String,
) -> SandboxBrokerResponse {
    let mut manager = match docker_manager(scope, image_name).await {
        Ok(manager) => manager,
        Err(error) => return SandboxBrokerResponse::Error(error.to_string()),
    };

    response_from_result(
        manager.cleanup_old_downloads().await,
        SandboxBrokerResponse::U64,
    )
}

async fn handle_destroy(scope: SandboxScope, image_name: String) -> SandboxBrokerResponse {
    let mut manager = match docker_manager(scope, image_name).await {
        Ok(manager) => manager,
        Err(error) => return SandboxBrokerResponse::Error(error.to_string()),
    };

    response_from_result(manager.destroy().await, |_| SandboxBrokerResponse::Unit)
}

async fn handle_recreate(scope: SandboxScope, image_name: String) -> SandboxBrokerResponse {
    let mut manager = match docker_manager(scope, image_name).await {
        Ok(manager) => manager,
        Err(error) => return SandboxBrokerResponse::Error(error.to_string()),
    };

    response_from_result(manager.recreate().await, |_| SandboxBrokerResponse::Unit)
}

async fn handle_file_size_bytes(
    scope: SandboxScope,
    image_name: String,
    container_path: String,
) -> SandboxBrokerResponse {
    let mut manager = match docker_manager(scope, image_name).await {
        Ok(manager) => manager,
        Err(error) => return SandboxBrokerResponse::Error(error.to_string()),
    };

    response_from_result(
        manager.file_size_bytes(&container_path, None).await,
        SandboxBrokerResponse::U64,
    )
}

async fn handle_exec_command(
    scope: SandboxScope,
    image_name: String,
    command: String,
    stream: &mut UnixStream,
) -> Result<Option<SandboxBrokerResponse>> {
    let mut manager = match docker_manager(scope, image_name).await {
        Ok(manager) => manager,
        Err(error) => return Ok(Some(SandboxBrokerResponse::Error(error.to_string()))),
    };

    let cancellation_token = CancellationToken::new();
    let exec = manager.exec_command(&command, Some(&cancellation_token));
    tokio::pin!(exec);

    let response = tokio::select! {
        result = &mut exec => {
            response_from_result(result, SandboxBrokerResponse::ExecResult)
        }
        disconnect = wait_for_peer_disconnect(stream) => {
            match disconnect {
                Ok(()) => {
                    info!("Sandbox broker client disconnected during exec; cancelling command");
                    cancellation_token.cancel();
                    let _ = (&mut exec).await;
                    return Ok(None);
                }
                Err(error) => SandboxBrokerResponse::Error(error.to_string()),
            }
        }
    };

    Ok(Some(response))
}

async fn handle_request(
    request: SandboxBrokerRequest,
    stream: &mut UnixStream,
) -> Result<Option<SandboxBrokerResponse>> {
    let response = match request {
        SandboxBrokerRequest::ListUserSandboxes { user_id } => response_from_result(
            DockerSandboxManager::list_user_sandboxes(user_id).await,
            SandboxBrokerResponse::Sandboxes,
        ),
        SandboxBrokerRequest::InspectSandboxByName {
            user_id,
            container_name,
        } => response_from_result(
            DockerSandboxManager::inspect_sandbox_by_name(user_id, &container_name).await,
            SandboxBrokerResponse::Sandbox,
        ),
        SandboxBrokerRequest::EnsureScopeSandbox { scope, image_name } => response_from_result(
            DockerSandboxManager::ensure_scope_sandbox_with_image(scope, image_name).await,
            SandboxBrokerResponse::SandboxRecord,
        ),
        SandboxBrokerRequest::RecreateScopeSandbox { scope, image_name } => response_from_result(
            DockerSandboxManager::recreate_scope_sandbox_with_image(scope, image_name).await,
            SandboxBrokerResponse::SandboxRecord,
        ),
        SandboxBrokerRequest::DeleteSandboxByName {
            user_id,
            container_name,
        } => response_from_result(
            DockerSandboxManager::delete_sandbox_by_name(user_id, &container_name).await,
            SandboxBrokerResponse::Deleted,
        ),
        SandboxBrokerRequest::CreateSandbox { scope, image_name } => {
            handle_create_sandbox(scope, image_name).await
        }
        SandboxBrokerRequest::ExecCommand {
            scope,
            image_name,
            command,
        } => return handle_exec_command(scope, image_name, command, stream).await,
        SandboxBrokerRequest::WriteFile {
            scope,
            image_name,
            path,
            content,
        } => handle_write_file(scope, image_name, path, content).await,
        SandboxBrokerRequest::ReadFile {
            scope,
            image_name,
            path,
        } => handle_read_file(scope, image_name, path).await,
        SandboxBrokerRequest::UploadFile {
            scope,
            image_name,
            container_path,
            content,
        } => handle_upload_file(scope, image_name, container_path, content).await,
        SandboxBrokerRequest::DownloadFile {
            scope,
            image_name,
            container_path,
        } => handle_download_file(scope, image_name, container_path).await,
        SandboxBrokerRequest::GetUploadsSize { scope, image_name } => {
            handle_get_uploads_size(scope, image_name).await
        }
        SandboxBrokerRequest::CleanupOldDownloads { scope, image_name } => {
            handle_cleanup_old_downloads(scope, image_name).await
        }
        SandboxBrokerRequest::Destroy { scope, image_name } => {
            handle_destroy(scope, image_name).await
        }
        SandboxBrokerRequest::Recreate { scope, image_name } => {
            handle_recreate(scope, image_name).await
        }
        SandboxBrokerRequest::FileSizeBytes {
            scope,
            image_name,
            container_path,
        } => handle_file_size_bytes(scope, image_name, container_path).await,
    };

    Ok(Some(response))
}

async fn wait_for_peer_disconnect(stream: &mut UnixStream) -> Result<()> {
    let mut buf = [0_u8; 1];
    let read = stream
        .read(&mut buf)
        .await
        .context("Failed while waiting for sandbox broker client disconnect")?;
    if read == 0 {
        Ok(())
    } else {
        Err(anyhow!(
            "Sandbox broker protocol violation: unexpected trailing bytes"
        ))
    }
}

async fn write_frame<T: Serialize>(stream: &mut UnixStream, value: &T) -> Result<()> {
    let payload = bincode::serialize(value).context("Failed to encode sandbox broker payload")?;
    let payload_len = u64::try_from(payload.len()).context("Sandbox broker payload too large")?;
    stream
        .write_u64(payload_len)
        .await
        .context("Failed to write sandbox broker frame size")?;
    stream
        .write_all(&payload)
        .await
        .context("Failed to write sandbox broker frame payload")?;
    stream
        .flush()
        .await
        .context("Failed to flush sandbox broker frame")?;
    debug!(bytes = payload_len, "Sandbox broker frame sent");
    Ok(())
}

async fn read_frame<T: DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
    let payload_len = stream
        .read_u64()
        .await
        .context("Failed to read sandbox broker frame size")?;
    let payload_size = usize::try_from(payload_len).context("Sandbox broker frame too large")?;
    let mut payload = vec![0_u8; payload_size];
    stream
        .read_exact(&mut payload)
        .await
        .context("Failed to read sandbox broker frame payload")?;
    bincode::deserialize(&payload).context("Failed to decode sandbox broker payload")
}

#[cfg(test)]
mod tests {
    use super::{SandboxBrokerClient, SandboxBrokerServer};
    use crate::config::get_sandbox_image;
    use crate::sandbox::scope::SandboxScope;
    use anyhow::{bail, Context, Result};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_socket_path(test_name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("oxide-agent-{test_name}-{nonce}.sock"))
    }

    fn unique_scope(test_name: &str) -> SandboxScope {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        SandboxScope::new(991_337, format!("test:{test_name}:{nonce}"))
    }

    #[tokio::test]
    #[ignore = "Requires Docker daemon"]
    async fn broker_download_file_roundtrip_reads_existing_container_file() -> Result<()> {
        let socket_path = unique_socket_path("broker-download-roundtrip");
        let server = SandboxBrokerServer::bind(&socket_path)
            .await
            .context("bind sandbox broker test server")?;
        let server_task = tokio::spawn(server.serve());

        let client = SandboxBrokerClient::new(&socket_path);
        let scope = unique_scope("broker-download-roundtrip");
        let image_name = get_sandbox_image();
        let file_path = "/workspace/audit_raw/AUDIT_REPORT.md";
        let container_name = scope.container_name();

        let exec_result = client
            .exec_command(
                scope.clone(),
                image_name.clone(),
                "mkdir -p /workspace/audit_raw && printf 'audit ok' > /workspace/audit_raw/AUDIT_REPORT.md",
                None,
            )
            .await;

        let download_result = client
            .download_file(scope.clone(), image_name.clone(), file_path)
            .await;

        let cleanup_result = client
            .delete_sandbox_by_name(scope.owner_id(), &container_name)
            .await;
        server_task.abort();
        let _ = server_task.await;
        let _ = tokio::fs::remove_file(&socket_path).await;

        exec_result.context("create file in broker-backed sandbox")?;
        cleanup_result.context("cleanup broker-backed sandbox after test")?;

        let content = download_result.context(
            "broker should download a file created by a previous request for the same sandbox scope",
        )?;

        if content != b"audit ok" {
            bail!(
                "unexpected file content from broker download: {:?}",
                String::from_utf8_lossy(&content)
            );
        }

        Ok(())
    }

    #[tokio::test]
    #[ignore = "Requires Docker daemon"]
    async fn broker_write_file_roundtrip_persists_to_existing_container() -> Result<()> {
        let socket_path = unique_socket_path("broker-write-roundtrip");
        let server = SandboxBrokerServer::bind(&socket_path)
            .await
            .context("bind sandbox broker test server")?;
        let server_task = tokio::spawn(server.serve());

        let client = SandboxBrokerClient::new(&socket_path);
        let scope = unique_scope("broker-write-roundtrip");
        let image_name = get_sandbox_image();
        let file_path = "/workspace/audit_raw/WRITE_REPORT.md";
        let container_name = scope.container_name();

        let exec_result = client
            .exec_command(
                scope.clone(),
                image_name.clone(),
                "mkdir -p /workspace/audit_raw",
                None,
            )
            .await;

        let write_result = client
            .write_file(scope.clone(), image_name.clone(), file_path, b"write ok")
            .await;

        let read_result = client
            .read_file(scope.clone(), image_name.clone(), file_path)
            .await;

        let cleanup_result = client
            .delete_sandbox_by_name(scope.owner_id(), &container_name)
            .await;
        server_task.abort();
        let _ = server_task.await;
        let _ = tokio::fs::remove_file(&socket_path).await;

        exec_result.context("prepare directory in broker-backed sandbox")?;
        write_result.context(
            "broker should write into a sandbox created by a previous request for the same scope",
        )?;
        cleanup_result.context("cleanup broker-backed sandbox after test")?;

        let content = read_result.context("read file written through broker")?;
        if content != b"write ok" {
            bail!(
                "unexpected file content from broker write roundtrip: {:?}",
                String::from_utf8_lossy(&content)
            );
        }

        Ok(())
    }

    #[tokio::test]
    #[ignore = "Requires Docker daemon"]
    async fn broker_upload_file_roundtrip_persists_to_existing_container() -> Result<()> {
        let socket_path = unique_socket_path("broker-upload-roundtrip");
        let server = SandboxBrokerServer::bind(&socket_path)
            .await
            .context("bind sandbox broker test server")?;
        let server_task = tokio::spawn(server.serve());

        let client = SandboxBrokerClient::new(&socket_path);
        let scope = unique_scope("broker-upload-roundtrip");
        let image_name = get_sandbox_image();
        let file_path = "/workspace/audit_raw/UPLOAD_REPORT.md";
        let container_name = scope.container_name();

        let exec_result = client
            .exec_command(
                scope.clone(),
                image_name.clone(),
                "mkdir -p /workspace/audit_raw",
                None,
            )
            .await;

        let upload_result = client
            .upload_file(scope.clone(), image_name.clone(), file_path, b"upload ok")
            .await;

        let download_result = client
            .download_file(scope.clone(), image_name.clone(), file_path)
            .await;

        let cleanup_result = client
            .delete_sandbox_by_name(scope.owner_id(), &container_name)
            .await;
        server_task.abort();
        let _ = server_task.await;
        let _ = tokio::fs::remove_file(&socket_path).await;

        exec_result.context("prepare upload directory in broker-backed sandbox")?;
        upload_result.context(
            "broker should upload into a sandbox created by a previous request for the same scope",
        )?;
        cleanup_result.context("cleanup broker-backed sandbox after test")?;

        let content = download_result.context("download file uploaded through broker")?;
        if content != b"upload ok" {
            bail!(
                "unexpected file content from broker upload roundtrip: {:?}",
                String::from_utf8_lossy(&content)
            );
        }

        Ok(())
    }
}
