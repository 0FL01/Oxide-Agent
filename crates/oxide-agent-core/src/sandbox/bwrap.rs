//! Bubblewrap sandbox backend.

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant, SystemTime};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tracing::{debug, info};
use uuid::Uuid;

use super::traits::apply_sandbox_file_edit;
use super::{
    ExecResult, SandboxApplyFileEditResult, SandboxContainerRecord, SandboxEditReadGuard,
    SandboxFileEdit, SandboxFileListing, SandboxScope,
};

const BWRAP_DEFAULT_BIN: &str = "bwrap";
const BWRAP_DEFAULT_IMAGE: &str = "debian-13-dev";
const BWRAP_DEFAULT_NET: BwrapNetworkMode = BwrapNetworkMode::Host;
const BWRAP_DEFAULT_ROOT_MODE: BwrapRootMode = BwrapRootMode::OverlayRw;
const BWRAP_DEFAULT_TIMEOUT_SECS: u64 = 60;
const BWRAP_DEFAULT_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const BWRAP_DEFAULT_MAX_READ_FILE_BYTES: u64 = 50 * 1024 * 1024;
const BWRAP_METADATA_SCHEMA_VERSION: u32 = 1;
const BWRAP_IMAGE_BOOTSTRAP_OFF: &str = "off";
const BWRAP_IMAGE_BOOTSTRAP_DOWNLOAD: &str = "download";
const WORKSPACE_PREFIX: &str = "/workspace";
const MAX_LIST_ENTRIES: usize = 100;
const MAX_LIST_DEPTH: usize = 3;

/// Bubblewrap sandbox manager.
#[derive(Clone)]
pub(crate) struct BwrapSandboxManager {
    config: BwrapSandboxConfig,
    scope: SandboxScope,
    state: BwrapScopeState,
    instance_id: String,
}

#[derive(Debug, Clone)]
struct BwrapSandboxConfig {
    bwrap_bin: PathBuf,
    image_id: String,
    manifest_path: Option<PathBuf>,
    manifest_sha256: Option<String>,
    package_manager: Option<String>,
    rootfs: PathBuf,
    state_dir: PathBuf,
    lock_dir: PathBuf,
    root_upper_dir: Option<PathBuf>,
    pinned_system_dir: Option<PathBuf>,
    net: BwrapNetworkMode,
    root_mode: BwrapRootMode,
    command_timeout: Duration,
    lock_timeout: Duration,
    max_output_bytes: usize,
    max_read_file_bytes: u64,
    allow_overlay: bool,
    disable_nested_userns: bool,
    resolv_conf: BwrapResolvConf,
    default_shell: String,
    default_workdir: String,
    default_env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
struct BwrapScopeState {
    scope_name: String,
    scope_dir: PathBuf,
    workspace: PathBuf,
    system_dir: PathBuf,
    system_upper: PathBuf,
    system_work: PathBuf,
    tmp: PathBuf,
    active: PathBuf,
    metadata: PathBuf,
    lock: PathBuf,
}

struct ScopeLock {
    file: File,
}

impl Drop for ScopeLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OutputTruncation {
    original_bytes: usize,
    captured_bytes: usize,
}

#[derive(Debug)]
struct CappedOutput {
    bytes: Vec<u8>,
    original_bytes: usize,
    max_bytes: usize,
}

impl CappedOutput {
    const fn empty() -> Self {
        Self {
            bytes: Vec::new(),
            original_bytes: 0,
            max_bytes: 0,
        }
    }

    fn into_output(self) -> (String, Option<OutputTruncation>) {
        let truncation = (self.original_bytes > self.bytes.len()).then_some(OutputTruncation {
            original_bytes: self.original_bytes,
            captured_bytes: self.max_bytes,
        });
        (
            String::from_utf8_lossy(&self.bytes).into_owned(),
            truncation,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum BwrapNetworkMode {
    Host,
    None,
}

impl fmt::Display for BwrapNetworkMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Host => "host",
            Self::None => "none",
        })
    }
}

impl FromStr for BwrapNetworkMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "host" => Ok(Self::Host),
            "none" => Ok(Self::None),
            invalid => Err(anyhow!(
                "Invalid BWRAP_NET='{invalid}'. Valid values: host, none."
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum BwrapRootMode {
    ReadOnly,
    OverlayRw,
}

impl fmt::Display for BwrapRootMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ReadOnly => "ro",
            Self::OverlayRw => "overlay-rw",
        })
    }
}

impl FromStr for BwrapRootMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "ro" => Ok(Self::ReadOnly),
            "overlay-rw" => Ok(Self::OverlayRw),
            "tmp-overlay" => Err(anyhow!(
                "BWRAP_ROOT_MODE=tmp-overlay is not supported in the MVP. Valid values: overlay-rw, ro."
            )),
            invalid => Err(anyhow!(
                "Invalid BWRAP_ROOT_MODE='{invalid}'. Valid values: overlay-rw, ro."
            )),
        }
    }
}

#[derive(Debug, Clone)]
enum BwrapResolvConf {
    Auto,
    None,
    Path(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BwrapImageBootstrapMode {
    Off,
    Download,
}

impl fmt::Display for BwrapImageBootstrapMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Off => BWRAP_IMAGE_BOOTSTRAP_OFF,
            Self::Download => BWRAP_IMAGE_BOOTSTRAP_DOWNLOAD,
        })
    }
}

impl FromStr for BwrapImageBootstrapMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            BWRAP_IMAGE_BOOTSTRAP_OFF => Ok(Self::Off),
            BWRAP_IMAGE_BOOTSTRAP_DOWNLOAD => Ok(Self::Download),
            invalid => Err(anyhow!(
                "Invalid BWRAP_IMAGE_BOOTSTRAP='{invalid}'. Valid values: off, download."
            )),
        }
    }
}

#[derive(Debug, Clone)]
struct BwrapImageBootstrapConfig {
    mode: BwrapImageBootstrapMode,
    image_id: String,
    image_store: PathBuf,
    lock_dir: PathBuf,
    rootfs_override: Option<PathBuf>,
    url: Option<String>,
    sha256: Option<String>,
    package_manager: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct BwrapImageManifest {
    schema_version: u32,
    id: String,
    arch: String,
    #[serde(default)]
    package_manager: Option<String>,
    #[serde(default = "default_manifest_rootfs")]
    rootfs: String,
    #[serde(default = "default_manifest_shell")]
    default_shell: String,
    #[serde(default = "default_manifest_workdir")]
    default_workdir: String,
    #[serde(default = "default_manifest_env")]
    default_env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BwrapScopeMetadata {
    schema_version: u32,
    backend: String,
    scope_name: String,
    owner_id: i64,
    namespace: String,
    chat_id: Option<i64>,
    thread_id: Option<i64>,
    image_id: String,
    #[serde(default)]
    image_manifest_path: Option<String>,
    image_manifest_sha256: Option<String>,
    #[serde(default)]
    package_manager: Option<String>,
    rootfs: String,
    workspace: String,
    #[serde(default)]
    system_dir: Option<String>,
    root_mode: BwrapRootMode,
    network_mode: BwrapNetworkMode,
    created_at: i64,
    updated_at: i64,
    generation: u64,
}

pub(crate) fn preflight_from_env() -> Result<()> {
    let bwrap_bin = resolve_executable(&env_string("BWRAP_BIN", BWRAP_DEFAULT_BIN)?)?;
    let help = bwrap_help_stdout(&bwrap_bin)?;
    if env_bool("BWRAP_DISABLE_NESTED_USERNS", true)? && !help.contains("--disable-userns") {
        bail!(
            "BWRAP_DISABLE_NESTED_USERNS=true, but {} does not support --disable-userns. Upgrade bubblewrap or set BWRAP_DISABLE_NESTED_USERNS=false for development only.",
            bwrap_bin.display()
        );
    }
    Ok(())
}

pub(crate) async fn bootstrap_image_from_env() -> Result<()> {
    BwrapImageBootstrapConfig::from_env()?
        .bootstrap_if_needed()
        .await
}

impl BwrapSandboxManager {
    /// Create a bwrap manager for the provided scope.
    pub(crate) async fn new(scope: SandboxScope) -> Result<Self> {
        preflight_from_env()?;
        bootstrap_image_from_env().await?;
        let mut config = BwrapSandboxConfig::from_env()?;
        let initial_state = BwrapScopeState::new(&config, &scope);
        if let Some(metadata) = BwrapScopeMetadata::read(&initial_state.metadata)? {
            config.apply_scope_pin(&metadata)?;
        }
        let state = BwrapScopeState::new(&config, &scope);
        config.validate()?;

        let instance_id = format!("bwrap:{}", scope.stable_name());
        Ok(Self {
            config,
            scope,
            state,
            instance_id,
        })
    }

    pub(crate) fn is_running(&self) -> bool {
        self.state.metadata.is_file()
    }

    pub(crate) fn container_id(&self) -> Option<&str> {
        Some(&self.instance_id)
    }

    pub(crate) const fn scope(&self) -> &SandboxScope {
        &self.scope
    }

    pub(crate) async fn create_sandbox(&mut self) -> Result<()> {
        let _lock = self.lock_scope()?;
        self.ensure_scope_dirs_locked()?;
        self.write_or_refresh_metadata_locked(false)?;
        info!(
            scope = %self.state.scope_name,
            image_id = %self.config.image_id,
            rootfs = %self.config.rootfs.display(),
            "Bwrap sandbox state is ready"
        );
        Ok(())
    }

    pub(crate) async fn exec_command(
        &mut self,
        cmd: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<ExecResult> {
        let _lock = self.lock_scope()?;
        self.ensure_scope_dirs_locked()?;
        self.write_or_refresh_metadata_locked(false)?;
        self.config.validate()?;

        let work_dir = self.prepare_overlay_workdir()?;
        let resolv_conf = self.prepare_resolv_conf_bind()?;
        let args = self.bwrap_args(work_dir.as_deref(), resolv_conf.as_deref(), cmd);
        debug!(
            scope = %self.state.scope_name,
            root_mode = %self.config.root_mode,
            network_mode = %self.config.net,
            "Executing bwrap sandbox command"
        );

        let result = self.run_bwrap(args, cancellation_token).await;
        if let Some(work_dir) = work_dir {
            let _ = fs::remove_dir_all(work_dir);
        }
        result
    }

    pub(crate) async fn write_file(&mut self, path: &str, content: &[u8]) -> Result<()> {
        let _lock = self.lock_scope()?;
        self.ensure_scope_dirs_locked()?;
        let host_path = self.resolve_workspace_path(path)?;
        if let Some(parent) = host_path.parent() {
            self.ensure_workspace_parent(parent)?;
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create workspace parent {}", parent.display())
            })?;
            self.ensure_workspace_parent(parent)?;
        }
        if host_path
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            bail!("Refusing to write through symlink: {path}");
        }
        fs::write(&host_path, content)
            .with_context(|| format!("Failed to write workspace file {}", host_path.display()))?;
        Ok(())
    }

    pub(crate) async fn read_file(&mut self, path: &str) -> Result<Vec<u8>> {
        let _lock = self.lock_scope()?;
        self.ensure_scope_dirs_locked()?;
        let host_path = self.resolve_workspace_path(path)?;
        self.ensure_regular_file(&host_path, path)?;
        let size = host_path.metadata()?.len();
        if size > self.config.max_read_file_bytes {
            bail!(
                "Refusing to read {path}: file is {size} bytes, limit is {} bytes",
                self.config.max_read_file_bytes
            );
        }
        fs::read(&host_path)
            .with_context(|| format!("Failed to read workspace file {}", host_path.display()))
    }

    pub(crate) async fn apply_file_edit(
        &mut self,
        path: &str,
        edit: SandboxFileEdit,
        read_guard: Option<SandboxEditReadGuard>,
    ) -> Result<SandboxApplyFileEditResult> {
        let _lock = self.lock_scope()?;
        self.ensure_scope_dirs_locked()?;
        let host_path = self.resolve_workspace_path(path)?;
        self.ensure_regular_file(&host_path, path)?;
        let size = host_path.metadata()?.len();
        if size > self.config.max_read_file_bytes {
            bail!(
                "Refusing to edit {path}: file is {size} bytes, read limit is {} bytes",
                self.config.max_read_file_bytes
            );
        }

        let current = fs::read(&host_path)
            .with_context(|| format!("Failed to read workspace file {}", host_path.display()))?;
        let applied = apply_sandbox_file_edit(path, &current, &edit, read_guard.as_ref())?;
        if applied.result.changed {
            if host_path
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                bail!("Refusing to write through symlink: {path}");
            }
            fs::write(&host_path, &applied.updated).with_context(|| {
                format!("Failed to write workspace file {}", host_path.display())
            })?;
        }
        Ok(applied.result)
    }

    pub(crate) async fn upload_file(&mut self, path: &str, content: &[u8]) -> Result<()> {
        self.write_file(path, content).await
    }

    pub(crate) async fn download_file(&mut self, path: &str) -> Result<Vec<u8>> {
        self.read_file(path).await
    }

    pub(crate) async fn get_uploads_size(&mut self) -> Result<u64> {
        let _lock = self.lock_scope()?;
        self.ensure_scope_dirs_locked()?;
        let uploads = self.state.workspace.join("uploads");
        dir_size(&uploads)
    }

    pub(crate) async fn cleanup_old_downloads(&mut self) -> Result<u64> {
        let _lock = self.lock_scope()?;
        self.ensure_scope_dirs_locked()?;
        cleanup_old_files(
            &self.state.workspace.join("downloads"),
            Duration::from_secs(7 * 24 * 60 * 60),
        )
    }

    pub(crate) async fn destroy(&mut self) -> Result<()> {
        let _lock = self.lock_scope()?;
        self.remove_scope_state_locked()?;
        Ok(())
    }

    fn remove_scope_state_locked(&self) -> Result<()> {
        if !self.state.system_dir.starts_with(&self.state.scope_dir) {
            remove_dir_if_exists(&self.state.system_dir)?;
        }
        if self.state.scope_dir.exists() {
            fs::remove_dir_all(&self.state.scope_dir).with_context(|| {
                format!(
                    "Failed to remove bwrap scope state {}",
                    self.state.scope_dir.display()
                )
            })?;
        }
        Ok(())
    }

    pub(crate) async fn recreate(&mut self) -> Result<()> {
        let _lock = self.lock_scope()?;
        let previous = BwrapScopeMetadata::read(&self.state.metadata)?;
        remove_dir_if_exists(&self.state.workspace)?;
        remove_dir_if_exists(&self.state.system_upper)?;
        remove_dir_if_exists(&self.state.system_work)?;
        remove_dir_if_exists(&self.state.tmp)?;
        remove_dir_if_exists(&self.state.active)?;
        self.ensure_scope_dirs_locked()?;
        self.write_metadata_locked(
            previous
                .as_ref()
                .map_or(1, |metadata| metadata.generation + 1),
        )?;
        Ok(())
    }

    pub(crate) async fn file_size_bytes(
        &mut self,
        path: &str,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<u64> {
        let _lock = self.lock_scope()?;
        self.ensure_scope_dirs_locked()?;
        let host_path = self.resolve_workspace_path(path)?;
        self.ensure_regular_file(&host_path, path)?;
        Ok(host_path.metadata()?.len())
    }

    pub(crate) async fn list_files(&mut self, path: &str) -> Result<SandboxFileListing> {
        let _lock = self.lock_scope()?;
        self.ensure_scope_dirs_locked()?;
        let host_path = self.resolve_workspace_path(path)?;
        let mut entries = Vec::new();
        list_workspace_entries(&self.state.workspace, &host_path, 0, &mut entries)?;
        Ok(SandboxFileListing {
            path: path.to_string(),
            listing: entries.join("\n"),
            stderr: String::new(),
            exit_code: 0,
        })
    }

    pub(crate) async fn list_user_sandboxes(user_id: i64) -> Result<Vec<SandboxContainerRecord>> {
        let config = BwrapSandboxConfig::from_env()?;
        let mut records = Vec::new();
        if !config.state_dir.exists() {
            return Ok(records);
        }
        for entry in fs::read_dir(&config.state_dir)? {
            let entry = entry?;
            let metadata_path = entry.path().join("metadata.json");
            let Some(metadata) = BwrapScopeMetadata::read(&metadata_path)? else {
                continue;
            };
            if metadata.owner_id == user_id {
                records.push(metadata.to_record());
            }
        }
        records.sort_by(|left, right| left.container_name.cmp(&right.container_name));
        Ok(records)
    }

    pub(crate) async fn inspect_sandbox_by_name(
        user_id: i64,
        container_name: &str,
    ) -> Result<Option<SandboxContainerRecord>> {
        Ok(Self::list_user_sandboxes(user_id)
            .await?
            .into_iter()
            .find(|record| record.container_name == container_name))
    }

    pub(crate) async fn ensure_scope_sandbox(
        scope: SandboxScope,
    ) -> Result<SandboxContainerRecord> {
        let mut manager = Self::new(scope).await?;
        manager.create_sandbox().await?;
        manager.current_record()
    }

    pub(crate) async fn recreate_scope_sandbox(
        scope: SandboxScope,
    ) -> Result<SandboxContainerRecord> {
        let mut manager = Self::new(scope).await?;
        manager.recreate().await?;
        manager.current_record()
    }

    pub(crate) async fn delete_sandbox_by_name(user_id: i64, container_name: &str) -> Result<bool> {
        let config = BwrapSandboxConfig::from_env()?;
        let metadata_path = config.state_dir.join(container_name).join("metadata.json");
        let Some(metadata) = BwrapScopeMetadata::read(&metadata_path)? else {
            return Ok(false);
        };
        if metadata.owner_id != user_id {
            return Ok(false);
        }
        let scope_dir = config.state_dir.join(container_name);
        if let Some(system_dir) = metadata.system_dir.as_deref() {
            remove_dir_if_exists(Path::new(system_dir))?;
        } else if let Some(root_upper_dir) = &config.root_upper_dir {
            remove_dir_if_exists(&root_upper_dir.join(container_name))?;
        }
        remove_dir_if_exists(&scope_dir)?;
        Ok(true)
    }

    fn current_record(&self) -> Result<SandboxContainerRecord> {
        let metadata = BwrapScopeMetadata::read(&self.state.metadata)?
            .ok_or_else(|| anyhow!("bwrap sandbox metadata is missing after create"))?;
        Ok(metadata.to_record())
    }

    fn lock_scope(&self) -> Result<ScopeLock> {
        ensure_configured_dir("BWRAP_LOCK_DIR", &self.config.lock_dir)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.state.lock)
            .with_context(|| format!("Failed to open bwrap lock {}", self.state.lock.display()))?;
        let started_at = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if started_at.elapsed() >= self.config.lock_timeout {
                        bail!(
                            "Timed out after {}s waiting for bwrap scope lock {}. Another command, recreate, or destroy operation is still active for this scope.",
                            self.config.lock_timeout.as_secs(),
                            self.state.scope_name
                        );
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("Failed to lock bwrap scope {}", self.state.scope_name)
                    });
                }
            }
        }
        Ok(ScopeLock { file })
    }

    fn ensure_scope_dirs_locked(&self) -> Result<()> {
        ensure_configured_dir("BWRAP_STATE_DIR", &self.config.state_dir)?;
        for path in [
            &self.state.scope_dir,
            &self.state.workspace,
            &self.state.system_upper,
            &self.state.system_work,
            &self.state.tmp,
            &self.state.active,
        ] {
            fs::create_dir_all(path)
                .with_context(|| format!("Failed to create bwrap state dir {}", path.display()))?;
        }
        Ok(())
    }

    fn write_or_refresh_metadata_locked(&self, force_new_generation: bool) -> Result<()> {
        let generation = if force_new_generation {
            BwrapScopeMetadata::read(&self.state.metadata)?
                .map_or(1, |metadata| metadata.generation + 1)
        } else {
            BwrapScopeMetadata::read(&self.state.metadata)?
                .map_or(1, |metadata| metadata.generation)
        };
        self.write_metadata_locked(generation)
    }

    fn write_metadata_locked(&self, generation: u64) -> Result<()> {
        let previous = BwrapScopeMetadata::read(&self.state.metadata)?;
        let now = Utc::now().timestamp();
        let metadata = BwrapScopeMetadata {
            schema_version: BWRAP_METADATA_SCHEMA_VERSION,
            backend: "bwrap".to_string(),
            scope_name: self.state.scope_name.clone(),
            owner_id: self.scope.owner_id(),
            namespace: self.scope.namespace().to_string(),
            chat_id: self.scope.chat_id(),
            thread_id: self.scope.thread_id(),
            image_id: self.config.image_id.clone(),
            image_manifest_path: self
                .config
                .manifest_path
                .as_ref()
                .map(|path| path.display().to_string()),
            image_manifest_sha256: self.config.manifest_sha256.clone(),
            package_manager: self.config.package_manager.clone(),
            rootfs: self.config.rootfs.display().to_string(),
            workspace: self.state.workspace.display().to_string(),
            system_dir: Some(self.state.system_dir.display().to_string()),
            root_mode: self.config.root_mode,
            network_mode: self.config.net,
            created_at: previous.map_or(now, |metadata| metadata.created_at),
            updated_at: now,
            generation,
        };
        let bytes = serde_json::to_vec_pretty(&metadata)?;
        fs::write(&self.state.metadata, bytes).with_context(|| {
            format!(
                "Failed to write bwrap metadata {}",
                self.state.metadata.display()
            )
        })
    }

    fn prepare_overlay_workdir(&self) -> Result<Option<PathBuf>> {
        if self.config.root_mode != BwrapRootMode::OverlayRw {
            return Ok(None);
        }
        let work_dir = self
            .state
            .system_work
            .join(format!("exec-{}", Uuid::new_v4()));
        fs::create_dir_all(&work_dir)
            .with_context(|| format!("Failed to create overlay workdir {}", work_dir.display()))?;
        Ok(Some(work_dir))
    }

    fn bwrap_args(
        &self,
        work_dir: Option<&Path>,
        resolv_conf: Option<&Path>,
        command: &str,
    ) -> Vec<OsString> {
        let mut args = vec![
            "--unshare-user".into(),
            "--uid".into(),
            "0".into(),
            "--gid".into(),
            "0".into(),
            "--unshare-pid".into(),
            "--unshare-ipc".into(),
            "--unshare-uts".into(),
            "--unshare-cgroup-try".into(),
            "--die-with-parent".into(),
            "--new-session".into(),
            "--clearenv".into(),
        ];

        if self.config.net == BwrapNetworkMode::None {
            args.push("--unshare-net".into());
        }

        for (key, value) in &self.config.default_env {
            args.push("--setenv".into());
            args.push(key.into());
            args.push(value.into());
        }

        match self.config.root_mode {
            BwrapRootMode::ReadOnly => {
                args.push("--ro-bind".into());
                args.push(self.config.rootfs.clone().into_os_string());
                args.push("/".into());
            }
            BwrapRootMode::OverlayRw => {
                args.push("--overlay-src".into());
                args.push(self.config.rootfs.clone().into_os_string());
                args.push("--overlay".into());
                args.push(self.state.system_upper.clone().into_os_string());
                args.push(
                    work_dir
                        .expect("overlay workdir must exist")
                        .as_os_str()
                        .into(),
                );
                args.push("/".into());
            }
        }

        args.extend([
            "--proc".into(),
            "/proc".into(),
            "--dev".into(),
            "/dev".into(),
            "--tmpfs".into(),
            "/tmp".into(),
            "--bind".into(),
            self.state.workspace.clone().into_os_string(),
            WORKSPACE_PREFIX.into(),
        ]);

        if let Some(resolv_conf) = resolv_conf {
            args.push("--ro-bind".into());
            args.push(resolv_conf.as_os_str().into());
            args.push("/etc/resolv.conf".into());
        }

        args.extend(["--chdir".into(), self.config.default_workdir.clone().into()]);

        if self.config.disable_nested_userns {
            args.push("--disable-userns".into());
        }

        args.push(self.config.default_shell.clone().into());
        args.push("-lc".into());
        args.push(command.into());
        args
    }

    fn prepare_resolv_conf_bind(&self) -> Result<Option<PathBuf>> {
        if self.config.net != BwrapNetworkMode::Host {
            return Ok(None);
        }
        match &self.config.resolv_conf {
            BwrapResolvConf::None => Ok(None),
            BwrapResolvConf::Path(path) => {
                let bytes = fs::read(path).with_context(|| {
                    format!("Failed to read configured resolver {}", path.display())
                })?;
                self.ensure_resolv_conf_bind_target(&bytes)?;
                Ok(Some(path.clone()))
            }
            BwrapResolvConf::Auto => {
                let host = PathBuf::from("/etc/resolv.conf");
                if !host.exists() {
                    return Ok(None);
                }
                let bytes = fs::read(&host)
                    .with_context(|| format!("Failed to read host resolver {}", host.display()))?;
                let path = self.state.scope_dir.join("resolv.conf");
                fs::write(&path, &bytes).with_context(|| {
                    format!("Failed to stage bwrap resolver config {}", path.display())
                })?;
                self.ensure_resolv_conf_bind_target(&bytes)?;
                Ok(Some(path))
            }
        }
    }

    fn ensure_resolv_conf_bind_target(&self, bytes: &[u8]) -> Result<()> {
        let rootfs_target = self.config.rootfs.join("etc/resolv.conf");
        match rootfs_target.symlink_metadata() {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                return Ok(());
            }
            Ok(metadata) if metadata.is_dir() => {
                bail!(
                    "Bwrap resolver bind target is a directory in rootfs: {}",
                    rootfs_target.display()
                );
            }
            Ok(_) | Err(_) if self.config.root_mode != BwrapRootMode::OverlayRw => {
                bail!(
                    "Bwrap resolver bind target {} is missing or not a regular file. Use BWRAP_ROOT_MODE=overlay-rw or add /etc/resolv.conf to the rootfs image.",
                    rootfs_target.display()
                );
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(error).with_context(|| {
                    format!(
                        "Failed to inspect bwrap resolver bind target {}",
                        rootfs_target.display()
                    )
                });
            }
            _ => {}
        }

        let upper_target = self.state.system_upper.join("etc/resolv.conf");
        let upper_parent = upper_target
            .parent()
            .ok_or_else(|| anyhow!("Invalid bwrap resolver upper path"))?;
        ensure_no_symlink_escape(&self.state.system_upper, upper_parent)?;
        fs::create_dir_all(upper_parent).with_context(|| {
            format!(
                "Failed to create bwrap resolver upper dir {}",
                upper_parent.display()
            )
        })?;
        ensure_no_symlink_escape(&self.state.system_upper, upper_parent)?;
        if upper_target
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            bail!(
                "Refusing bwrap resolver upper symlink: {}",
                upper_target.display()
            );
        }
        fs::write(&upper_target, bytes).with_context(|| {
            format!(
                "Failed to create bwrap resolver bind target {}",
                upper_target.display()
            )
        })?;
        Ok(())
    }

    async fn run_bwrap(
        &self,
        args: Vec<OsString>,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<ExecResult> {
        let mut command = Command::new(&self.config.bwrap_bin);
        command
            .args(args)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(unix)]
        {
            command.process_group(0);
        }
        let mut child = command.spawn().with_context(|| {
            format!(
                "Failed to spawn bwrap at {}. Install bubblewrap (`apk add bubblewrap` or `apt install bubblewrap`), set BWRAP_BIN=/path/to/bwrap, or choose another sandbox backend with SANDBOX_BACKEND=docker|broker.",
                self.config.bwrap_bin.display()
            )
        })?;
        let child_pid = child.id();
        let stdout_task = child
            .stdout
            .take()
            .map(|stdout| tokio::spawn(read_capped_counted(stdout, self.config.max_output_bytes)));
        let stderr_task = child
            .stderr
            .take()
            .map(|stderr| tokio::spawn(read_capped_counted(stderr, self.config.max_output_bytes)));

        let timeout = tokio::time::sleep(self.config.command_timeout);
        tokio::pin!(timeout);

        let status = if let Some(token) = cancellation_token {
            tokio::select! {
                status = child.wait() => status.context("Failed to wait for bwrap command")?,
                () = &mut timeout => {
                    let cleanup = cleanup_bwrap_child(&mut child, child_pid).await;
                    let _ = await_capped_output(stdout_task).await;
                    let _ = await_capped_output(stderr_task).await;
                    bail!("Bwrap command timed out after {}s and the {cleanup}.", self.config.command_timeout.as_secs());
                }
                () = token.cancelled() => {
                    let cleanup = cleanup_bwrap_child(&mut child, child_pid).await;
                    let _ = await_capped_output(stdout_task).await;
                    let _ = await_capped_output(stderr_task).await;
                    bail!("Bwrap command cancelled by user and the {cleanup}.");
                }
            }
        } else {
            tokio::select! {
                status = child.wait() => status.context("Failed to wait for bwrap command")?,
                () = &mut timeout => {
                    let cleanup = cleanup_bwrap_child(&mut child, child_pid).await;
                    let _ = await_capped_output(stdout_task).await;
                    let _ = await_capped_output(stderr_task).await;
                    bail!("Bwrap command timed out after {}s and the {cleanup}.", self.config.command_timeout.as_secs());
                }
            }
        };

        let stdout_capture = await_capped_output(stdout_task).await;
        let stderr_capture = await_capped_output(stderr_task).await;
        let (stdout, stdout_truncation) = stdout_capture.into_output();
        let (mut stderr, stderr_truncation) = stderr_capture.into_output();
        if let Some(truncation) = &stdout_truncation {
            stderr.push_str(&format!(
                "\n[oxide-agent] stdout truncated by BWRAP_MAX_OUTPUT_BYTES: captured {} of {} bytes",
                truncation.captured_bytes,
                truncation.original_bytes
            ));
        }
        if let Some(truncation) = &stderr_truncation {
            stderr.push_str(&format!(
                "\n[oxide-agent] stderr truncated by BWRAP_MAX_OUTPUT_BYTES: captured {} of {} bytes",
                truncation.captured_bytes,
                truncation.original_bytes
            ));
        }

        Ok(ExecResult {
            stdout,
            stderr,
            exit_code: i64::from(status.code().unwrap_or(-1)),
        })
    }

    fn resolve_workspace_path(&self, requested: &str) -> Result<PathBuf> {
        resolve_workspace_path(&self.state.workspace, requested)
    }

    fn ensure_workspace_parent(&self, parent: &Path) -> Result<()> {
        ensure_no_symlink_escape(&self.state.workspace, parent)
    }

    fn ensure_regular_file(&self, host_path: &Path, requested: &str) -> Result<()> {
        ensure_no_symlink_escape(&self.state.workspace, host_path)?;
        let metadata = host_path
            .symlink_metadata()
            .with_context(|| format!("Workspace file not found: {requested}"))?;
        if metadata.file_type().is_symlink() {
            bail!("Refusing to follow workspace symlink: {requested}");
        }
        if !metadata.is_file() {
            bail!("Workspace path is not a regular file: {requested}");
        }
        Ok(())
    }
}

impl BwrapImageBootstrapConfig {
    fn from_env() -> Result<Self> {
        let image_id = env_string("BWRAP_IMAGE", BWRAP_DEFAULT_IMAGE)?;
        let state_root = absolute_path(".oxide/sandbox")?;
        let image_store = absolute_path_env("BWRAP_IMAGE_STORE", state_root.join("images"))?;
        let state_dir = absolute_path_env("BWRAP_STATE_DIR", state_root.join("scopes"))?;
        let lock_dir = absolute_path_env(
            "BWRAP_LOCK_DIR",
            state_dir
                .parent()
                .map_or_else(|| state_root.join("locks"), |parent| parent.join("locks")),
        )?;
        Ok(Self {
            mode: env_parse("BWRAP_IMAGE_BOOTSTRAP", BwrapImageBootstrapMode::Off)?,
            image_id,
            image_store,
            lock_dir,
            rootfs_override: optional_path_env("BWRAP_ROOTFS")?,
            url: optional_string_env("BWRAP_IMAGE_URL"),
            sha256: optional_string_env("BWRAP_IMAGE_SHA256"),
            package_manager: optional_string_env("BWRAP_IMAGE_PACKAGE_MANAGER"),
        })
    }

    async fn bootstrap_if_needed(&self) -> Result<()> {
        if self.mode == BwrapImageBootstrapMode::Off || self.rootfs_override.is_some() {
            return Ok(());
        }
        validate_bootstrap_image_id(&self.image_id)?;
        ensure_configured_dir("BWRAP_IMAGE_STORE", &self.image_store)?;
        let image_dir = self.image_dir();
        if image_dir.join("image.json").is_file() {
            return Ok(());
        }
        let _lock = lock_image_bootstrap(&self.lock_dir, &self.image_id)?;
        if image_dir.join("image.json").is_file() {
            return Ok(());
        }
        if image_dir.exists() {
            bail!(
                "Bwrap image directory {} exists but image.json is missing. Remove the partial directory or choose another BWRAP_IMAGE.",
                image_dir.display()
            );
        }
        match self.mode {
            BwrapImageBootstrapMode::Off => Ok(()),
            BwrapImageBootstrapMode::Download => self.download_image(&image_dir).await,
        }
    }

    fn image_dir(&self) -> PathBuf {
        self.image_store.join(&self.image_id)
    }

    async fn download_image(&self, image_dir: &Path) -> Result<()> {
        let url = self
            .url
            .as_deref()
            .ok_or_else(|| anyhow!("BWRAP_IMAGE_BOOTSTRAP=download requires BWRAP_IMAGE_URL."))?;
        let expected_sha256 = self.sha256.as_deref().ok_or_else(|| {
            anyhow!("BWRAP_IMAGE_BOOTSTRAP=download requires BWRAP_IMAGE_SHA256.")
        })?;
        let expected_sha256 = normalize_sha256(expected_sha256)?;
        let staging_dir =
            self.image_store
                .join(format!(".{}.bootstrap-{}", self.image_id, Uuid::new_v4()));
        let result = self
            .download_image_to_staging(url, &expected_sha256, &staging_dir, image_dir)
            .await;
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging_dir);
        }
        result
    }

    async fn download_image_to_staging(
        &self,
        url: &str,
        expected_sha256: &str,
        staging_dir: &Path,
        image_dir: &Path,
    ) -> Result<()> {
        fs::create_dir_all(staging_dir).with_context(|| {
            format!(
                "Failed to create bwrap image bootstrap staging dir {}",
                staging_dir.display()
            )
        })?;
        let tarball = staging_dir.join("rootfs.tarball");
        let actual_sha256 = download_bootstrap_tarball(url, &tarball).await?;
        if actual_sha256 != expected_sha256 {
            bail!(
                "Checksum mismatch for BWRAP_IMAGE_URL={url}. expected: {expected_sha256}; actual: {actual_sha256}"
            );
        }
        let rootfs = staging_dir.join("rootfs");
        fs::create_dir_all(&rootfs)
            .with_context(|| format!("Failed to create rootfs dir {}", rootfs.display()))?;
        extract_bootstrap_tarball(&tarball, &rootfs, url)?;
        prepare_bootstrap_rootfs(&rootfs)?;
        write_bootstrap_image_metadata(self, staging_dir, url, expected_sha256)?;
        fs::remove_file(&tarball)
            .with_context(|| format!("Failed to remove temporary tarball {}", tarball.display()))?;
        fs::rename(staging_dir, image_dir).with_context(|| {
            format!(
                "Failed to publish bwrap image {} from {}",
                image_dir.display(),
                staging_dir.display()
            )
        })?;
        info!(
            image_id = %self.image_id,
            image_dir = %image_dir.display(),
            "Bootstrapped bwrap rootfs image"
        );
        Ok(())
    }
}

impl BwrapSandboxConfig {
    fn from_env() -> Result<Self> {
        let bwrap_bin = resolve_executable(&env_string("BWRAP_BIN", BWRAP_DEFAULT_BIN)?)?;
        let image_id = env_string("BWRAP_IMAGE", BWRAP_DEFAULT_IMAGE)?;
        let state_root = absolute_path(".oxide/sandbox")?;
        let image_store = absolute_path_env("BWRAP_IMAGE_STORE", state_root.join("images"))?;
        let state_dir = absolute_path_env("BWRAP_STATE_DIR", state_root.join("scopes"))?;
        let lock_dir = absolute_path_env(
            "BWRAP_LOCK_DIR",
            state_dir
                .parent()
                .map_or_else(|| state_root.join("locks"), |parent| parent.join("locks")),
        )?;
        let rootfs_override = optional_path_env("BWRAP_ROOTFS")?;
        let root_upper_dir = optional_path_env("BWRAP_ROOT_UPPER_DIR")?;
        let net = env_parse("BWRAP_NET", BWRAP_DEFAULT_NET)?;
        let root_mode = env_parse("BWRAP_ROOT_MODE", BWRAP_DEFAULT_ROOT_MODE)?;
        let allow_overlay = env_bool("BWRAP_ALLOW_OVERLAY", true)?;
        if root_mode == BwrapRootMode::OverlayRw && !allow_overlay {
            bail!("BWRAP_ALLOW_OVERLAY=false requires BWRAP_ROOT_MODE=ro.");
        }
        let command_timeout = Duration::from_secs(env_u64(
            "BWRAP_COMMAND_TIMEOUT_SECS",
            std::env::var("SANDBOX_EXEC_TIMEOUT_SECS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(BWRAP_DEFAULT_TIMEOUT_SECS),
        )?);
        let lock_timeout = Duration::from_secs(env_u64(
            "BWRAP_RECREATE_LOCK_TIMEOUT_SECS",
            command_timeout.as_secs().saturating_add(5),
        )?);
        let max_output_bytes = env_usize("BWRAP_MAX_OUTPUT_BYTES", BWRAP_DEFAULT_MAX_OUTPUT_BYTES)?;
        let max_read_file_bytes = env_u64(
            "BWRAP_MAX_READ_FILE_BYTES",
            BWRAP_DEFAULT_MAX_READ_FILE_BYTES,
        )?;
        let disable_nested_userns = env_bool("BWRAP_DISABLE_NESTED_USERNS", true)?;
        let resolv_conf = parse_resolv_conf()?;

        let (manifest, manifest_path, manifest_sha256, rootfs) =
            if let Some(rootfs) = rootfs_override {
                validate_direct_rootfs_override(&rootfs)?;
                let manifest_path = rootfs.parent().map(|parent| parent.join("image.json"));
                let (manifest, loaded_manifest_path, manifest_sha256) =
                    match manifest_path.as_ref().filter(|path| path.is_file()) {
                        Some(path) => {
                            let (manifest, manifest_sha256) = load_manifest(path)?;
                            (manifest, Some(path.clone()), manifest_sha256)
                        }
                        None => (BwrapImageManifest::fallback(&image_id), None, None),
                    };
                (manifest, loaded_manifest_path, manifest_sha256, rootfs)
            } else {
                let image_dir = image_store.join(&image_id);
                let manifest_path = image_dir.join("image.json");
                if !manifest_path.is_file() {
                    bail!(
                        "Bwrap image manifest not found at {}. {}",
                        manifest_path.display(),
                        bwrap_rootfs_hint()
                    );
                }
                let (manifest, manifest_sha256) = load_manifest(&manifest_path)?;
                let rootfs = resolve_image_rootfs(&image_dir, &manifest.rootfs)?;
                (manifest, Some(manifest_path), manifest_sha256, rootfs)
            };

        let mut default_env = default_manifest_env();
        default_env.extend(manifest.default_env.clone());

        Ok(Self {
            bwrap_bin,
            image_id: manifest.id.clone(),
            manifest_path: manifest_path.map(absolute_path).transpose()?,
            manifest_sha256,
            package_manager: manifest.package_manager.clone(),
            rootfs: absolute_maybe_existing_path(&rootfs)?,
            state_dir,
            lock_dir,
            root_upper_dir,
            pinned_system_dir: None,
            net,
            root_mode,
            command_timeout,
            lock_timeout,
            max_output_bytes,
            max_read_file_bytes,
            allow_overlay,
            disable_nested_userns,
            resolv_conf,
            default_shell: manifest.default_shell,
            default_workdir: manifest.default_workdir,
            default_env,
        })
    }

    fn apply_scope_pin(&mut self, metadata: &BwrapScopeMetadata) -> Result<()> {
        if metadata.backend != "bwrap" {
            bail!(
                "Invalid bwrap metadata backend '{}' in existing scope {}",
                metadata.backend,
                metadata.scope_name
            );
        }
        self.image_id.clone_from(&metadata.image_id);
        self.manifest_path = metadata.image_manifest_path.as_deref().map(PathBuf::from);
        self.manifest_sha256 = metadata.image_manifest_sha256.clone();
        self.package_manager = metadata.package_manager.clone();
        self.rootfs = absolute_existing_path(Path::new(&metadata.rootfs))?;
        self.pinned_system_dir = metadata.system_dir.as_deref().map(PathBuf::from);
        self.root_mode = metadata.root_mode;
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        if self.root_mode == BwrapRootMode::OverlayRw && !self.allow_overlay {
            bail!("BWRAP_ALLOW_OVERLAY=false requires BWRAP_ROOT_MODE=ro.");
        }
        if self.command_timeout.is_zero() {
            bail!("BWRAP_COMMAND_TIMEOUT_SECS must be greater than zero.");
        }
        if self.lock_timeout.is_zero() {
            bail!("BWRAP_RECREATE_LOCK_TIMEOUT_SECS must be greater than zero.");
        }
        if self.max_output_bytes == 0 {
            bail!("BWRAP_MAX_OUTPUT_BYTES must be greater than zero.");
        }
        if self.max_read_file_bytes == 0 {
            bail!("BWRAP_MAX_READ_FILE_BYTES must be greater than zero.");
        }
        if self.disable_nested_userns && !bwrap_supports_disable_userns(&self.bwrap_bin)? {
            bail!(
                "BWRAP_DISABLE_NESTED_USERNS=true, but {} does not support --disable-userns. Upgrade bubblewrap or set BWRAP_DISABLE_NESTED_USERNS=false for development only.",
                self.bwrap_bin.display()
            );
        }
        validate_rootfs(self)?;
        if let Some(root_upper_dir) = &self.root_upper_dir {
            validate_root_upper_dir(root_upper_dir, &self.rootfs)?;
        }
        Ok(())
    }
}

impl BwrapScopeState {
    fn new(config: &BwrapSandboxConfig, scope: &SandboxScope) -> Self {
        let scope_name = scope.stable_name();
        let scope_dir = config.state_dir.join(&scope_name);
        let system_dir = config.root_upper_dir.as_ref().map_or_else(
            || scope_dir.join("system"),
            |parent| parent.join(&scope_name),
        );
        let system_dir = config.pinned_system_dir.clone().unwrap_or(system_dir);
        Self {
            workspace: scope_dir.join("workspace"),
            system_dir: system_dir.clone(),
            system_upper: system_dir.join("upper"),
            system_work: system_dir.join("work"),
            tmp: scope_dir.join("tmp"),
            active: scope_dir.join("active"),
            metadata: scope_dir.join("metadata.json"),
            lock: config.lock_dir.join(format!("{scope_name}.lock")),
            scope_name,
            scope_dir,
        }
    }
}

impl BwrapScopeMetadata {
    fn read(path: &Path) -> Result<Option<Self>> {
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = fs::read(path)
            .with_context(|| format!("Failed to read bwrap metadata {}", path.display()))?;
        let metadata = serde_json::from_slice(&bytes)
            .with_context(|| format!("Invalid bwrap metadata JSON {}", path.display()))?;
        Ok(Some(metadata))
    }

    fn to_record(&self) -> SandboxContainerRecord {
        let mut labels = HashMap::from([
            ("agent.sandbox".to_string(), "true".to_string()),
            ("agent.sandbox_backend".to_string(), "bwrap".to_string()),
            ("agent.user_id".to_string(), self.owner_id.to_string()),
            ("agent.scope".to_string(), self.namespace.clone()),
            ("agent.rootfs".to_string(), self.rootfs.clone()),
            ("agent.workspace_dir".to_string(), self.workspace.clone()),
            (
                "agent.state_dir".to_string(),
                Path::new(&self.workspace)
                    .parent()
                    .map_or_else(String::new, |path| path.display().to_string()),
            ),
            ("agent.root_mode".to_string(), self.root_mode.to_string()),
            (
                "agent.network_mode".to_string(),
                self.network_mode.to_string(),
            ),
            ("agent.updated_at".to_string(), self.updated_at.to_string()),
        ]);
        if let Some(path) = &self.image_manifest_path {
            labels.insert("agent.image_manifest_path".to_string(), path.clone());
        }
        if let Some(sha256) = &self.image_manifest_sha256 {
            labels.insert("agent.image_manifest_sha256".to_string(), sha256.clone());
        }
        if let Some(package_manager) = &self.package_manager {
            labels.insert("agent.package_manager".to_string(), package_manager.clone());
        }
        if let Some(chat_id) = self.chat_id {
            labels.insert("agent.chat_id".to_string(), chat_id.to_string());
        }
        if let Some(thread_id) = self.thread_id {
            labels.insert("agent.thread_id".to_string(), thread_id.to_string());
        }

        SandboxContainerRecord {
            container_id: format!("bwrap:{}", self.scope_name),
            container_name: self.scope_name.clone(),
            image: Some(self.image_id.clone()),
            created_at: Some(self.created_at),
            state: Some("ready".to_string()),
            status: Some(self.status_text()),
            running: false,
            user_id: Some(self.owner_id),
            scope: Some(self.namespace.clone()),
            chat_id: self.chat_id,
            thread_id: self.thread_id,
            labels,
        }
    }

    fn status_text(&self) -> String {
        let package_manager = self.package_manager.as_deref().unwrap_or("unknown");
        let manifest = self.image_manifest_path.as_deref().unwrap_or("none");
        format!(
            "bwrap root_mode={} net={} package_manager={} manifest={} rootfs={}",
            self.root_mode, self.network_mode, package_manager, manifest, self.rootfs
        )
    }
}

impl BwrapImageManifest {
    fn fallback(image_id: &str) -> Self {
        Self {
            schema_version: 1,
            id: image_id.to_string(),
            arch: host_arch().to_string(),
            package_manager: None,
            rootfs: ".".to_string(),
            default_shell: default_manifest_shell(),
            default_workdir: default_manifest_workdir(),
            default_env: default_manifest_env(),
        }
    }
}

fn default_manifest_rootfs() -> String {
    "rootfs".to_string()
}

fn default_manifest_shell() -> String {
    "/bin/sh".to_string()
}

fn default_manifest_workdir() -> String {
    WORKSPACE_PREFIX.to_string()
}

fn default_manifest_env() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("HOME".to_string(), WORKSPACE_PREFIX.to_string()),
        (
            "PATH".to_string(),
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
        ),
        ("LANG".to_string(), "C.UTF-8".to_string()),
        ("TMPDIR".to_string(), "/tmp".to_string()),
    ])
}

fn load_manifest(path: &Path) -> Result<(BwrapImageManifest, Option<String>)> {
    let bytes = fs::read(path)
        .with_context(|| format!("Failed to read bwrap manifest {}", path.display()))?;
    let manifest: BwrapImageManifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("Invalid bwrap image manifest {}", path.display()))?;
    if manifest.schema_version != 1 {
        bail!(
            "Invalid bwrap image manifest {}: schema_version must be 1",
            path.display()
        );
    }
    let rootfs_path = Path::new(&manifest.rootfs);
    if rootfs_path.is_absolute() {
        bail!(
            "Invalid bwrap image manifest {}: rootfs must be relative",
            path.display()
        );
    }
    if manifest.rootfs.trim().is_empty()
        || rootfs_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        bail!(
            "Invalid bwrap image manifest {}: rootfs must stay under the image directory",
            path.display()
        );
    }
    if manifest.default_workdir != WORKSPACE_PREFIX {
        bail!(
            "Invalid bwrap image manifest {}: default_workdir must be /workspace for MVP",
            path.display()
        );
    }
    if manifest.arch != host_arch() {
        bail!(
            "Invalid bwrap image manifest {}: arch '{}' does not match host arch '{}'",
            path.display(),
            manifest.arch,
            host_arch()
        );
    }
    Ok((manifest, Some(format!("{:x}", Sha256::digest(&bytes)))))
}

fn validate_rootfs(config: &BwrapSandboxConfig) -> Result<()> {
    if !config.rootfs.is_dir() {
        bail!(
            "Bwrap backend selected, but rootfs not found at {}. {}",
            config.rootfs.display(),
            bwrap_rootfs_hint()
        );
    }
    for required in ["proc", "dev", "tmp", "workspace"] {
        let path = config.rootfs.join(required);
        if !path.is_dir() {
            bail!(
                "Bwrap rootfs {} is missing required /{} directory.",
                config.rootfs.display(),
                required
            );
        }
    }
    let shell_path = config
        .default_shell
        .strip_prefix('/')
        .ok_or_else(|| anyhow!("Bwrap manifest default_shell must be absolute"))?;
    if !config.rootfs.join(shell_path).is_file() {
        bail!(
            "Bwrap rootfs {} does not contain default shell {}.",
            config.rootfs.display(),
            config.default_shell
        );
    }
    Ok(())
}

fn validate_root_upper_dir(root_upper_dir: &Path, rootfs: &Path) -> Result<()> {
    if root_upper_dir.exists() && !root_upper_dir.is_dir() {
        bail!(
            "BWRAP_ROOT_UPPER_DIR must be a directory: {}",
            root_upper_dir.display()
        );
    }
    if root_upper_dir
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        bail!(
            "BWRAP_ROOT_UPPER_DIR must not be a symlink: {}",
            root_upper_dir.display()
        );
    }
    let root_upper_dir = root_upper_dir
        .parent()
        .unwrap_or(root_upper_dir)
        .canonicalize()
        .unwrap_or_else(|_| root_upper_dir.to_path_buf());
    if root_upper_dir.starts_with(rootfs) {
        bail!(
            "BWRAP_ROOT_UPPER_DIR must not be inside the bwrap rootfs image: {}",
            root_upper_dir.display()
        );
    }
    Ok(())
}

fn validate_direct_rootfs_override(rootfs: &Path) -> Result<()> {
    if rootfs
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        bail!("BWRAP_ROOTFS must not be a symlink: {}", rootfs.display());
    }
    Ok(())
}

fn bwrap_rootfs_hint() -> String {
    let mut hint =
        "Run scripts/build-bwrap-rootfs-debian.sh, set BWRAP_IMAGE_BOOTSTRAP=download, or set BWRAP_IMAGE/BWRAP_ROOTFS.".to_string();
    if std::env::var_os("SANDBOX_IMAGE").is_some() {
        hint.push_str(" SANDBOX_IMAGE is Docker-only and is ignored by SANDBOX_BACKEND=bwrap.");
    }
    hint
}

fn validate_bootstrap_image_id(image_id: &str) -> Result<()> {
    if image_id.trim().is_empty()
        || image_id.contains('/')
        || image_id.contains('\\')
        || image_id == "."
        || image_id == ".."
    {
        bail!("BWRAP_IMAGE_BOOTSTRAP=download requires a simple BWRAP_IMAGE directory name.");
    }
    Ok(())
}

fn normalize_sha256(value: &str) -> Result<String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.len() != 64 || !normalized.chars().all(|char| char.is_ascii_hexdigit()) {
        bail!("BWRAP_IMAGE_SHA256 must be a 64-character hex SHA-256 digest.");
    }
    Ok(normalized)
}

fn lock_image_bootstrap(lock_dir: &Path, image_id: &str) -> Result<ScopeLock> {
    ensure_configured_dir("BWRAP_LOCK_DIR", lock_dir)?;
    let lock_path = lock_dir.join(format!("image-bootstrap-{image_id}.lock"));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("Failed to open bwrap image lock {}", lock_path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("Failed to lock bwrap image {image_id}"))?;
    Ok(ScopeLock { file })
}

async fn download_bootstrap_tarball(url: &str, destination: &Path) -> Result<String> {
    if url.trim_start().starts_with("file://") {
        return copy_bootstrap_tarball_file(url, destination);
    }
    let parsed = url::Url::parse(url).with_context(|| format!("Invalid BWRAP_IMAGE_URL={url}"))?;
    match parsed.scheme() {
        "http" | "https" => download_bootstrap_tarball_http(url, destination).await,
        scheme => bail!("BWRAP_IMAGE_URL must use http, https, or file; got '{scheme}'."),
    }
}

fn copy_bootstrap_tarball_file(url: &str, destination: &Path) -> Result<String> {
    let parsed = url::Url::parse(url).with_context(|| format!("Invalid BWRAP_IMAGE_URL={url}"))?;
    let source = parsed
        .to_file_path()
        .map_err(|()| anyhow!("Invalid file URL for BWRAP_IMAGE_URL={url}"))?;
    validate_bootstrap_source_file(&source)?;
    copy_file_with_sha256(&source, destination)
}

async fn download_bootstrap_tarball_http(url: &str, destination: &Path) -> Result<String> {
    let mut response = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to download BWRAP_IMAGE_URL={url}"))?
        .error_for_status()
        .with_context(|| format!("BWRAP_IMAGE_URL returned an error status: {url}"))?;
    let mut file = File::create(destination)
        .with_context(|| format!("Failed to create download file {}", destination.display()))?;
    let mut hasher = Sha256::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("Failed to read BWRAP_IMAGE_URL={url}"))?
    {
        file.write_all(&chunk)
            .with_context(|| format!("Failed to write download file {}", destination.display()))?;
        hasher.update(&chunk);
    }
    file.flush()
        .with_context(|| format!("Failed to flush download file {}", destination.display()))?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_bootstrap_source_file(path: &Path) -> Result<()> {
    let metadata = path
        .symlink_metadata()
        .with_context(|| format!("BWRAP_IMAGE_URL source file not found: {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!(
            "BWRAP_IMAGE_URL file source must not be a symlink: {}",
            path.display()
        );
    }
    if !metadata.is_file() {
        bail!(
            "BWRAP_IMAGE_URL file source must be a regular file: {}",
            path.display()
        );
    }
    Ok(())
}

fn copy_file_with_sha256(source: &Path, destination: &Path) -> Result<String> {
    let mut input = File::open(source)
        .with_context(|| format!("Failed to open source tarball {}", source.display()))?;
    let mut output = File::create(destination)
        .with_context(|| format!("Failed to create download file {}", destination.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .with_context(|| format!("Failed to read source tarball {}", source.display()))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .with_context(|| format!("Failed to write download file {}", destination.display()))?;
        hasher.update(&buffer[..read]);
    }
    output
        .flush()
        .with_context(|| format!("Failed to flush download file {}", destination.display()))?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn extract_bootstrap_tarball(tarball: &Path, rootfs: &Path, url: &str) -> Result<()> {
    let mut command = std::process::Command::new("tar");
    command.arg("--extract");
    if url.ends_with(".tar.gz") || url.ends_with(".tgz") {
        command.arg("--gzip");
    } else if url.ends_with(".tar.xz") || url.ends_with(".txz") {
        command.arg("--xz");
    }
    let output = command
        .arg("--file")
        .arg(tarball)
        .arg("--directory")
        .arg(rootfs)
        .arg("--numeric-owner")
        .output()
        .with_context(|| "Failed to run tar while bootstrapping bwrap image. Install tar or disable BWRAP_IMAGE_BOOTSTRAP.")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Failed to extract bwrap rootfs tarball with tar: {stderr}");
    }
    Ok(())
}

fn prepare_bootstrap_rootfs(rootfs: &Path) -> Result<()> {
    for directory in ["proc", "dev", "workspace"] {
        fs::create_dir_all(rootfs.join(directory))
            .with_context(|| format!("Failed to create required rootfs /{directory} directory"))?;
    }
    let tmp = rootfs.join("tmp");
    fs::create_dir_all(&tmp).context("Failed to create required rootfs /tmp directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o1777))
            .context("Failed to chmod rootfs /tmp")?;
    }
    if !rootfs.join("bin/sh").is_file() {
        bail!("Downloaded bwrap rootfs is missing /bin/sh.");
    }
    Ok(())
}

fn write_bootstrap_image_metadata(
    config: &BwrapImageBootstrapConfig,
    image_dir: &Path,
    source_url: &str,
    source_sha256: &str,
) -> Result<()> {
    let created_at = Utc::now().to_rfc3339();
    let manifest = serde_json::json!({
        "schema_version": 1,
        "id": config.image_id.clone(),
        "arch": host_arch(),
        "rootfs": "rootfs",
        "default_shell": "/bin/sh",
        "default_workdir": WORKSPACE_PREFIX,
        "package_manager": config.package_manager.clone(),
        "default_env": default_manifest_env(),
        "provenance": {
            "builder": "oxide-agent bwrap image bootstrap",
            "source": source_url,
            "source_sha256": source_sha256,
            "created_at": created_at,
        }
    });
    let provenance = serde_json::json!({
        "builder": "oxide-agent bwrap image bootstrap",
        "source": source_url,
        "source_sha256": source_sha256,
        "created_at": created_at,
        "host_arch": host_arch(),
    });
    write_bootstrap_json_files(image_dir, &manifest, &provenance)
}

fn write_bootstrap_json_files(
    image_dir: &Path,
    manifest: &serde_json::Value,
    provenance: &serde_json::Value,
) -> Result<()> {
    let manifest_bytes = serde_json::to_vec_pretty(manifest)?;
    let provenance_bytes = serde_json::to_vec_pretty(provenance)?;
    fs::write(image_dir.join("image.json"), &manifest_bytes)
        .with_context(|| format!("Failed to write {}", image_dir.join("image.json").display()))?;
    fs::write(image_dir.join("provenance.json"), &provenance_bytes).with_context(|| {
        format!(
            "Failed to write {}",
            image_dir.join("provenance.json").display()
        )
    })?;
    let checksums = format!(
        "{:x}  image.json\n{:x}  provenance.json\n",
        Sha256::digest(&manifest_bytes),
        Sha256::digest(&provenance_bytes)
    );
    fs::write(image_dir.join("checksums.txt"), checksums).with_context(|| {
        format!(
            "Failed to write {}",
            image_dir.join("checksums.txt").display()
        )
    })
}

fn resolve_image_rootfs(image_dir: &Path, rootfs: &str) -> Result<PathBuf> {
    let image_dir = image_dir.canonicalize().with_context(|| {
        format!(
            "Failed to canonicalize bwrap image directory {}",
            image_dir.display()
        )
    })?;
    let rootfs = image_dir.join(rootfs);
    let canonical_rootfs = rootfs.canonicalize().with_context(|| {
        format!(
            "Bwrap backend selected, but rootfs not found at {}. {}",
            rootfs.display(),
            bwrap_rootfs_hint()
        )
    })?;
    if !canonical_rootfs.starts_with(&image_dir) {
        bail!(
            "Bwrap image rootfs {} resolves outside image directory {}. Refusing unsafe rootfs symlink.",
            canonical_rootfs.display(),
            image_dir.display()
        );
    }
    Ok(canonical_rootfs)
}

fn bwrap_supports_disable_userns(bwrap_bin: &Path) -> Result<bool> {
    Ok(bwrap_help_stdout(bwrap_bin)?.contains("--disable-userns"))
}

fn bwrap_help_stdout(bwrap_bin: &Path) -> Result<String> {
    let output = std::process::Command::new(bwrap_bin)
        .arg("--help")
        .output()
        .with_context(|| {
            format!(
                "Bwrap backend selected, but BWRAP_BIN='{}' was not found or is not executable. Install bubblewrap (`apk add bubblewrap` or `apt install bubblewrap`), set BWRAP_BIN=/path/to/bwrap, or choose another sandbox backend with SANDBOX_BACKEND=docker|broker.",
                bwrap_bin.display()
            )
        })?;
    if !output.status.success() {
        bail!(
            "Bwrap backend selected, but '{}' failed to run --help. Install bubblewrap, set BWRAP_BIN=/path/to/bwrap, or choose SANDBOX_BACKEND=docker|broker.",
            bwrap_bin.display()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn resolve_executable(value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    if path.components().count() > 1 || path.is_absolute() {
        if path.is_file() {
            return absolute_existing_path(&path);
        }
        bail!(
            "Bwrap backend selected, but BWRAP_BIN='{}' was not found or is not executable. Install bubblewrap (`apk add bubblewrap` or `apt install bubblewrap`), set BWRAP_BIN=/path/to/bwrap, or choose another sandbox backend with SANDBOX_BACKEND=docker|broker.",
            value
        );
    }
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(value);
        if candidate.is_file() {
            return absolute_existing_path(&candidate);
        }
    }
    bail!(
        "Bwrap backend selected, but BWRAP_BIN='{value}' was not found or is not executable. Install bubblewrap (`apk add bubblewrap` or `apt install bubblewrap`), set BWRAP_BIN=/path/to/bwrap, or choose another sandbox backend with SANDBOX_BACKEND=docker|broker."
    )
}

fn env_string(key: &str, default: &str) -> Result<String> {
    Ok(std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string()))
}

fn optional_string_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_u64(key: &str, default: u64) -> Result<u64> {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => value
            .parse()
            .with_context(|| format!("{key} must be a positive integer")),
        _ => Ok(default),
    }
}

fn env_usize(key: &str, default: usize) -> Result<usize> {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => value
            .parse()
            .with_context(|| format!("{key} must be a positive integer")),
        _ => Ok(default),
    }
}

fn env_bool(key: &str, default: bool) -> Result<bool> {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            invalid => Err(anyhow!(
                "{key} must be true, false, 1, or 0; got '{invalid}'"
            )),
        },
        _ => Ok(default),
    }
}

fn env_parse<T>(key: &str, default: T) -> Result<T>
where
    T: FromStr<Err = anyhow::Error>,
{
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => value.parse(),
        _ => Ok(default),
    }
}

fn absolute_path_env(key: &str, default: PathBuf) -> Result<PathBuf> {
    match std::env::var_os(key) {
        Some(value) if !value.is_empty() => absolute_path(value),
        _ => absolute_path(default),
    }
}

fn optional_path_env(key: &str) -> Result<Option<PathBuf>> {
    match std::env::var_os(key) {
        Some(value) if !value.is_empty() => absolute_path(value).map(Some),
        _ => Ok(None),
    }
}

fn absolute_path(path: impl AsRef<Path>) -> Result<PathBuf> {
    let path = path.as_ref();
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn absolute_existing_path(path: impl AsRef<Path>) -> Result<PathBuf> {
    let absolute = absolute_path(path)?;
    absolute
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", absolute.display()))
}

fn absolute_maybe_existing_path(path: impl AsRef<Path>) -> Result<PathBuf> {
    let absolute = absolute_path(path)?;
    if absolute.exists() {
        return absolute
            .canonicalize()
            .with_context(|| format!("Path does not exist: {}", absolute.display()));
    }
    Ok(absolute)
}

fn parse_resolv_conf() -> Result<BwrapResolvConf> {
    let value = env_string("BWRAP_RESOLV_CONF", "auto")?;
    match value.trim() {
        "auto" => Ok(BwrapResolvConf::Auto),
        "none" => Ok(BwrapResolvConf::None),
        path => {
            let path = absolute_path(path)?;
            validate_resolv_conf_path(&path)?;
            Ok(BwrapResolvConf::Path(path))
        }
    }
}

fn validate_resolv_conf_path(path: &Path) -> Result<()> {
    let metadata = path
        .symlink_metadata()
        .with_context(|| format!("BWRAP_RESOLV_CONF path does not exist: {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!(
            "BWRAP_RESOLV_CONF must not be a symlink: {}",
            path.display()
        );
    }
    if !metadata.is_file() {
        bail!(
            "BWRAP_RESOLV_CONF must be a regular file: {}",
            path.display()
        );
    }
    Ok(())
}

fn host_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => other,
    }
}

fn resolve_workspace_path(workspace: &Path, requested: &str) -> Result<PathBuf> {
    if requested.as_bytes().contains(&0) {
        bail!("Workspace path contains NUL byte");
    }
    let requested = requested.trim();
    if requested.is_empty() || requested == WORKSPACE_PREFIX {
        return Ok(workspace.to_path_buf());
    }

    let relative = if let Some(stripped) = requested.strip_prefix("/workspace/") {
        stripped
    } else if requested.starts_with('/') {
        bail!("Absolute sandbox paths must start with /workspace/: {requested}");
    } else {
        requested
    };

    let path = Path::new(relative);
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => bail!("Workspace path must not contain '..': {requested}"),
            Component::RootDir | Component::Prefix(_) => {
                bail!("Workspace path must be relative or under /workspace: {requested}");
            }
        }
    }
    Ok(workspace.join(normalized))
}

fn ensure_no_symlink_escape(workspace: &Path, target: &Path) -> Result<()> {
    let relative = target.strip_prefix(workspace).map_err(|_| {
        anyhow!(
            "Resolved workspace path escaped workspace: {}",
            target.display()
        )
    })?;
    let mut cursor = workspace.to_path_buf();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            continue;
        };
        cursor.push(part);
        if cursor
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            bail!(
                "Refusing workspace symlink path component: {}",
                cursor.display()
            );
        }
    }
    Ok(())
}

fn list_workspace_entries(
    workspace: &Path,
    current: &Path,
    depth: usize,
    entries: &mut Vec<String>,
) -> Result<()> {
    if entries.len() >= MAX_LIST_ENTRIES {
        return Ok(());
    }
    ensure_no_symlink_escape(workspace, current)?;
    let metadata = current
        .symlink_metadata()
        .with_context(|| format!("Workspace path not found: {}", current.display()))?;
    let relative = current.strip_prefix(workspace).unwrap_or(current);
    let display = if relative.as_os_str().is_empty() {
        WORKSPACE_PREFIX.to_string()
    } else {
        format!("{WORKSPACE_PREFIX}/{}", relative.display())
    };
    if metadata.is_dir() {
        entries.push(format!("{display}/"));
        if depth >= MAX_LIST_DEPTH {
            return Ok(());
        }
        let mut children = fs::read_dir(current)?.collect::<std::result::Result<Vec<_>, _>>()?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            list_workspace_entries(workspace, &child.path(), depth + 1, entries)?;
            if entries.len() >= MAX_LIST_ENTRIES {
                break;
            }
        }
    } else if metadata.is_file() {
        entries.push(format!("{display} ({} bytes)", metadata.len()));
    }
    Ok(())
}

fn dir_size(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total += dir_size(&entry.path())?;
        } else if metadata.is_file() {
            total += metadata.len();
        }
    }
    Ok(total)
}

fn cleanup_old_files(path: &Path, max_age: Duration) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let cutoff = SystemTime::now()
        .checked_sub(max_age)
        .ok_or_else(|| anyhow!("Invalid cleanup age"))?;
    let mut deleted = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            deleted += cleanup_old_files(&entry.path(), max_age)?;
        } else if metadata.is_file() && metadata.modified().is_ok_and(|modified| modified < cutoff)
        {
            fs::remove_file(entry.path())?;
            deleted += 1;
        }
    }
    Ok(deleted)
}

fn remove_dir_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)
            .with_context(|| format!("Failed to remove directory {}", path.display()))?;
    }
    Ok(())
}

fn ensure_configured_dir(env_key: &str, path: &Path) -> Result<()> {
    if !path.exists() {
        return fs::create_dir_all(path)
            .with_context(|| format!("Failed to create {env_key} directory {}", path.display()));
    }

    let metadata = path
        .symlink_metadata()
        .with_context(|| format!("Failed to inspect {env_key} path {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!("{env_key} must not be a symlink: {}", path.display());
    }
    if !metadata.is_dir() {
        bail!("{env_key} must be a directory: {}", path.display());
    }
    Ok(())
}

async fn read_capped_counted<R>(mut reader: R, max_bytes: usize) -> CappedOutput
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::with_capacity(max_bytes.min(8192));
    let mut original_bytes = 0usize;
    let mut chunk = [0_u8; 8192];

    loop {
        let Ok(read) = reader.read(&mut chunk).await else {
            break;
        };
        if read == 0 {
            break;
        }
        original_bytes = original_bytes.saturating_add(read);
        let remaining = max_bytes.saturating_sub(bytes.len());
        if remaining > 0 {
            bytes.extend_from_slice(&chunk[..read.min(remaining)]);
        }
    }

    CappedOutput {
        bytes,
        original_bytes,
        max_bytes,
    }
}

async fn await_capped_output(task: Option<tokio::task::JoinHandle<CappedOutput>>) -> CappedOutput {
    match task {
        Some(task) => task.await.unwrap_or_else(|_| CappedOutput::empty()),
        None => CappedOutput::empty(),
    }
}

async fn cleanup_bwrap_child(child: &mut tokio::process::Child, pid: Option<u32>) -> &'static str {
    match child.try_wait() {
        Ok(Some(_)) => return "process already exited",
        Ok(None) => {}
        Err(_) => return "process cleanup status could not be inspected",
    }

    if let Some(pid) = pid {
        let _ = send_process_group_signal(pid, "TERM").await;
        if wait_for_bwrap_child(child, Duration::from_secs(2)).await {
            return "process group terminated";
        }

        if send_process_group_signal(pid, "KILL").await
            && wait_for_bwrap_child(child, Duration::from_secs(2)).await
        {
            return "process group was killed";
        }
    }

    if child.kill().await.is_ok() && wait_for_bwrap_child(child, Duration::from_secs(2)).await {
        return "process was killed";
    }

    "process cleanup failed"
}

async fn wait_for_bwrap_child(child: &mut tokio::process::Child, duration: Duration) -> bool {
    matches!(
        tokio::time::timeout(duration, child.wait()).await,
        Ok(Ok(_))
    )
}

async fn send_process_group_signal(pid: u32, signal: &str) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .arg(format!("-{signal}"))
            .arg("--")
            .arg(format!("-{pid}"))
            .status()
            .await
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[cfg(not(unix))]
    {
        let _ = (pid, signal);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{
        host_arch, load_manifest, resolve_workspace_path, BwrapNetworkMode, BwrapRootMode,
        BwrapSandboxManager, WORKSPACE_PREFIX,
    };
    use crate::sandbox::{SandboxEditReadGuard, SandboxFileEdit, SandboxScope};
    use sha2::{Digest, Sha256};
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    const BWRAP_TEST_ENV_KEYS: &[&str] = &[
        "BWRAP_ALLOW_OVERLAY",
        "BWRAP_BIN",
        "BWRAP_COMMAND_TIMEOUT_SECS",
        "BWRAP_DISABLE_NESTED_USERNS",
        "BWRAP_IMAGE",
        "BWRAP_IMAGE_BOOTSTRAP",
        "BWRAP_IMAGE_PACKAGE_MANAGER",
        "BWRAP_IMAGE_SHA256",
        "BWRAP_IMAGE_STORE",
        "BWRAP_IMAGE_URL",
        "BWRAP_LOCK_DIR",
        "BWRAP_MAX_OUTPUT_BYTES",
        "BWRAP_MAX_READ_FILE_BYTES",
        "BWRAP_NET",
        "BWRAP_RECREATE_LOCK_TIMEOUT_SECS",
        "BWRAP_RESOLV_CONF",
        "BWRAP_ROOT_MODE",
        "BWRAP_ROOT_UPPER_DIR",
        "BWRAP_ROOTFS",
        "BWRAP_STATE_DIR",
        "SANDBOX_EXEC_TIMEOUT_SECS",
        "SANDBOX_IMAGE",
    ];

    struct EnvGuard {
        previous: Vec<(&'static str, Option<OsString>)>,
    }

    impl EnvGuard {
        fn capture(keys: &'static [&'static str]) -> Self {
            Self {
                previous: keys
                    .iter()
                    .map(|key| (*key, std::env::var_os(key)))
                    .collect(),
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.previous {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    #[test]
    fn workspace_path_accepts_relative_and_workspace_absolute_paths() {
        let workspace = Path::new("/tmp/scope/workspace");

        assert_eq!(
            resolve_workspace_path(workspace, "foo.txt").unwrap(),
            Path::new("/tmp/scope/workspace/foo.txt")
        );
        assert_eq!(
            resolve_workspace_path(workspace, "dir/foo.txt").unwrap(),
            Path::new("/tmp/scope/workspace/dir/foo.txt")
        );
        assert_eq!(
            resolve_workspace_path(workspace, "/workspace/foo.txt").unwrap(),
            Path::new("/tmp/scope/workspace/foo.txt")
        );
        assert_eq!(
            resolve_workspace_path(workspace, "/workspace/dir/foo.txt").unwrap(),
            Path::new("/tmp/scope/workspace/dir/foo.txt")
        );
    }

    #[test]
    fn workspace_path_rejects_escape_forms() {
        let workspace = Path::new("/tmp/scope/workspace");

        for path in [
            "..",
            "../x",
            "/workspace/../x",
            "/etc/passwd",
            "dir/../../x",
            "bad\0path",
        ] {
            assert!(
                resolve_workspace_path(workspace, path).is_err(),
                "{path} should be rejected"
            );
        }
    }

    #[test]
    fn bwrap_modes_parse_valid_values() {
        assert_eq!(
            "host".parse::<BwrapNetworkMode>().unwrap(),
            BwrapNetworkMode::Host
        );
        assert_eq!(
            "none".parse::<BwrapNetworkMode>().unwrap(),
            BwrapNetworkMode::None
        );
        assert_eq!(
            "overlay-rw".parse::<BwrapRootMode>().unwrap(),
            BwrapRootMode::OverlayRw
        );
        assert_eq!(
            "ro".parse::<BwrapRootMode>().unwrap(),
            BwrapRootMode::ReadOnly
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bwrap_state_lifecycle_persists_workspace_and_recreate_wipes_it() {
        let _env_lock = crate::config::test_env_mutex()
            .lock()
            .expect("test env mutex poisoned");
        let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
        let temp = tempfile::tempdir().expect("temp dir");
        let rootfs = temp.path().join("rootfs");
        create_fake_rootfs(&rootfs);
        let fake_bwrap = temp.path().join("bwrap");
        create_fake_bwrap(&fake_bwrap);

        std::env::set_var("BWRAP_ALLOW_OVERLAY", "true");
        std::env::set_var("BWRAP_BIN", &fake_bwrap);
        std::env::set_var("BWRAP_COMMAND_TIMEOUT_SECS", "5");
        std::env::set_var("BWRAP_DISABLE_NESTED_USERNS", "false");
        std::env::set_var("BWRAP_IMAGE", "test-dev");
        std::env::set_var("BWRAP_LOCK_DIR", temp.path().join("locks"));
        std::env::set_var("BWRAP_MAX_OUTPUT_BYTES", "1024");
        std::env::set_var("BWRAP_MAX_READ_FILE_BYTES", "1024");
        std::env::set_var("BWRAP_NET", "none");
        std::env::set_var("BWRAP_RESOLV_CONF", "none");
        std::env::set_var("BWRAP_ROOT_MODE", "overlay-rw");
        std::env::set_var("BWRAP_ROOTFS", &rootfs);
        std::env::set_var("BWRAP_STATE_DIR", temp.path().join("scopes"));
        std::env::remove_var("SANDBOX_EXEC_TIMEOUT_SECS");

        let scope =
            SandboxScope::new(42, "topic-alpha").with_transport_metadata(Some(1001), Some(77));
        let mut manager = BwrapSandboxManager::new(scope.clone()).await.unwrap();

        manager.create_sandbox().await.unwrap();
        assert!(manager.is_running());
        assert_eq!(
            manager.current_record().unwrap().container_name,
            scope.stable_name()
        );

        manager
            .write_file("notes/todo.txt", b"hello")
            .await
            .unwrap();
        assert_eq!(
            manager
                .read_file("/workspace/notes/todo.txt")
                .await
                .unwrap(),
            b"hello"
        );
        let listing = manager.list_files("/workspace").await.unwrap();
        assert!(listing.listing.contains("/workspace/notes/"));
        assert!(listing.listing.contains("/workspace/notes/todo.txt"));
        assert_eq!(
            manager
                .file_size_bytes("notes/todo.txt", None)
                .await
                .unwrap(),
            5
        );

        manager.recreate().await.unwrap();
        assert!(manager.read_file("notes/todo.txt").await.is_err());
        let record = manager.current_record().unwrap();
        assert_eq!(
            record.container_id,
            format!("bwrap:{}", scope.stable_name())
        );
        assert_eq!(
            record.labels.get("agent.sandbox_backend"),
            Some(&"bwrap".to_string())
        );

        manager.destroy().await.unwrap();
        assert!(!temp
            .path()
            .join("scopes")
            .join(scope.stable_name())
            .exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bwrap_workspace_file_ops_reject_symlink_escapes() {
        let _env_lock = crate::config::test_env_mutex()
            .lock()
            .expect("test env mutex poisoned");
        let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
        let temp = tempfile::tempdir().expect("temp dir");
        let rootfs = temp.path().join("rootfs");
        create_fake_rootfs(&rootfs);
        std::fs::create_dir_all(rootfs.join("etc")).expect("fake rootfs etc");
        std::fs::write(rootfs.join("etc/resolv.conf"), b"").expect("fake rootfs resolv");
        let fake_bwrap = temp.path().join("bwrap");
        create_fake_bwrap(&fake_bwrap);

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "topic-symlinks"))
            .await
            .unwrap();
        manager.create_sandbox().await.unwrap();

        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside).expect("outside dir");
        std::fs::write(outside.join("secret.txt"), b"secret").expect("outside secret");

        symlink(&outside, manager.state.workspace.join("linked-dir")).expect("parent symlink");
        assert!(manager
            .write_file("linked-dir/new.txt", b"nope")
            .await
            .is_err());
        assert!(manager.list_files("linked-dir").await.is_err());

        symlink(
            outside.join("secret.txt"),
            manager.state.workspace.join("secret-link.txt"),
        )
        .expect("final symlink");
        assert!(manager.read_file("secret-link.txt").await.is_err());
        assert!(manager
            .write_file("secret-link.txt", b"nope")
            .await
            .is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bwrap_apply_file_edit_is_guarded_under_scope_lock() {
        let _env_lock = crate::config::test_env_mutex()
            .lock()
            .expect("test env mutex poisoned");
        let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
        let temp = tempfile::tempdir().expect("temp dir");
        let rootfs = temp.path().join("rootfs");
        create_fake_rootfs(&rootfs);
        let fake_bwrap = temp.path().join("bwrap");
        create_fake_bwrap(&fake_bwrap);

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "topic-apply-edit"))
            .await
            .unwrap();
        manager.create_sandbox().await.unwrap();
        manager
            .write_file("notes.txt", b"alpha\nbeta\n")
            .await
            .unwrap();
        let current = manager.read_file("notes.txt").await.unwrap();
        let read_guard = SandboxEditReadGuard {
            sha256: format!("{:x}", Sha256::digest(&current)),
            bytes: current.len(),
        };

        let result = manager
            .apply_file_edit(
                "notes.txt",
                SandboxFileEdit {
                    search: "beta".to_string(),
                    replace: "gamma".to_string(),
                    expected_replacements: 1,
                },
                Some(read_guard.clone()),
            )
            .await
            .unwrap();
        assert!(result.changed);
        assert_eq!(result.replacements, 1);
        assert_eq!(result.previous_sha256, read_guard.sha256);
        assert_eq!(
            manager.read_file("notes.txt").await.unwrap(),
            b"alpha\ngamma\n"
        );

        manager
            .write_file("notes.txt", b"changed elsewhere\n")
            .await
            .unwrap();
        let stale_error = manager
            .apply_file_edit(
                "notes.txt",
                SandboxFileEdit {
                    search: "changed".to_string(),
                    replace: "updated".to_string(),
                    expected_replacements: 1,
                },
                Some(read_guard),
            )
            .await
            .err()
            .expect("stale read guard should fail")
            .to_string();
        assert!(stale_error.contains("file changed after last read"));

        let fresh = manager.read_file("notes.txt").await.unwrap();
        let fresh_guard = SandboxEditReadGuard {
            sha256: format!("{:x}", Sha256::digest(&fresh)),
            bytes: fresh.len(),
        };
        let count_error = manager
            .apply_file_edit(
                "notes.txt",
                SandboxFileEdit {
                    search: "missing".to_string(),
                    replace: "updated".to_string(),
                    expected_replacements: 1,
                },
                Some(fresh_guard),
            )
            .await
            .err()
            .expect("replacement count mismatch should fail")
            .to_string();
        assert!(count_error.contains("expected 1 replacements, found 0"));
        assert_eq!(
            manager.read_file("notes.txt").await.unwrap(),
            b"changed elsewhere\n"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bwrap_invocation_args_encode_network_root_modes_and_bind_policy() {
        let _env_lock = crate::config::test_env_mutex()
            .lock()
            .expect("test env mutex poisoned");
        let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
        let temp = tempfile::tempdir().expect("temp dir");
        let rootfs = temp.path().join("rootfs");
        create_fake_rootfs(&rootfs);
        std::fs::create_dir_all(rootfs.join("etc")).expect("fake rootfs etc");
        std::fs::write(rootfs.join("etc/resolv.conf"), b"").expect("fake rootfs resolv");
        let fake_bwrap = temp.path().join("bwrap");
        create_fake_bwrap(&fake_bwrap);

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        std::env::set_var("BWRAP_NET", "host");
        std::env::set_var("BWRAP_RESOLV_CONF", "auto");
        std::env::set_var("BWRAP_ROOT_MODE", "overlay-rw");
        let overlay_manager = BwrapSandboxManager::new(SandboxScope::new(42, "args-overlay-host"))
            .await
            .unwrap();
        let work_dir = overlay_manager
            .prepare_overlay_workdir()
            .unwrap()
            .expect("overlay mode should create a workdir");
        let overlay_args =
            args_to_strings(overlay_manager.bwrap_args(Some(&work_dir), None, "true"));

        assert!(overlay_args.contains(&"--overlay-src".to_string()));
        assert!(overlay_args.contains(&"--overlay".to_string()));
        assert!(contains_arg_pair(
            &overlay_args,
            "--bind",
            &overlay_manager.state.workspace.display().to_string()
        ));
        assert!(contains_arg_pair(
            &overlay_args,
            "--chdir",
            WORKSPACE_PREFIX
        ));
        assert!(!overlay_args.contains(&"--unshare-net".to_string()));
        assert_args_do_not_bind_host_control_paths(&overlay_args);

        if Path::new("/etc/resolv.conf").exists() {
            overlay_manager.ensure_scope_dirs_locked().unwrap();
            let staged_resolv = overlay_manager
                .prepare_resolv_conf_bind()
                .unwrap()
                .expect("auto resolver should stage a bind source");
            assert!(staged_resolv.starts_with(&overlay_manager.state.scope_dir));
            assert_ne!(staged_resolv, PathBuf::from("/etc/resolv.conf"));
            assert!(!staged_resolv
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink());
            let overlay_args_with_resolv = args_to_strings(overlay_manager.bwrap_args(
                Some(&work_dir),
                Some(&staged_resolv),
                "true",
            ));
            assert!(contains_arg_pair(
                &overlay_args_with_resolv,
                "--ro-bind",
                &staged_resolv.display().to_string()
            ));
        }

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        std::env::set_var("BWRAP_NET", "none");
        std::env::set_var("BWRAP_ROOT_MODE", "ro");
        let readonly_manager = BwrapSandboxManager::new(SandboxScope::new(42, "args-ro-none"))
            .await
            .unwrap();
        let readonly_args = args_to_strings(readonly_manager.bwrap_args(None, None, "printf ok"));

        assert!(readonly_args.contains(&"--unshare-net".to_string()));
        assert!(contains_arg_pair(
            &readonly_args,
            "--ro-bind",
            &readonly_manager.config.rootfs.display().to_string()
        ));
        assert!(!readonly_args.contains(&"--overlay".to_string()));
        assert!(contains_arg_pair(
            &readonly_args,
            "--bind",
            &readonly_manager.state.workspace.display().to_string()
        ));
        assert_args_do_not_bind_host_control_paths(&readonly_args);
        assert!(readonly_args.ends_with(&[
            "/bin/sh".to_string(),
            "-lc".to_string(),
            "printf ok".to_string()
        ]));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bwrap_auto_resolver_creates_overlay_bind_target_when_rootfs_file_is_missing() {
        let _env_lock = crate::config::test_env_mutex()
            .lock()
            .expect("test env mutex poisoned");
        let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
        let temp = tempfile::tempdir().expect("temp dir");
        let rootfs = temp.path().join("rootfs");
        create_fake_rootfs(&rootfs);
        let fake_bwrap = temp.path().join("bwrap");
        create_fake_bwrap(&fake_bwrap);

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        std::env::set_var("BWRAP_NET", "host");
        std::env::set_var("BWRAP_RESOLV_CONF", "auto");
        std::env::set_var("BWRAP_ROOT_MODE", "overlay-rw");
        let overlay_manager = BwrapSandboxManager::new(SandboxScope::new(42, "resolv-overlay"))
            .await
            .unwrap();
        overlay_manager.ensure_scope_dirs_locked().unwrap();

        if Path::new("/etc/resolv.conf").exists() {
            let staged_resolv = overlay_manager
                .prepare_resolv_conf_bind()
                .unwrap()
                .expect("auto resolver should stage a bind source");
            let upper_target = overlay_manager.state.system_upper.join("etc/resolv.conf");
            assert!(upper_target.is_file());
            assert_eq!(
                std::fs::read(&upper_target).unwrap(),
                std::fs::read(staged_resolv).unwrap()
            );
        }

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        std::env::set_var("BWRAP_NET", "host");
        std::env::set_var("BWRAP_RESOLV_CONF", "auto");
        std::env::set_var("BWRAP_ROOT_MODE", "ro");
        let readonly_manager = BwrapSandboxManager::new(SandboxScope::new(42, "resolv-ro"))
            .await
            .unwrap();
        readonly_manager.ensure_scope_dirs_locked().unwrap();

        if Path::new("/etc/resolv.conf").exists() {
            let readonly_error = readonly_manager
                .prepare_resolv_conf_bind()
                .err()
                .expect("missing readonly resolver target should fail")
                .to_string();
            assert!(readonly_error.contains("BWRAP_ROOT_MODE=overlay-rw"));
            assert!(readonly_error.contains("/etc/resolv.conf"));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bwrap_config_errors_are_actionable() {
        let _env_lock = crate::config::test_env_mutex()
            .lock()
            .expect("test env mutex poisoned");
        let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
        let temp = tempfile::tempdir().expect("temp dir");
        let rootfs = temp.path().join("rootfs");
        create_fake_rootfs(&rootfs);
        let fake_bwrap = temp.path().join("bwrap");
        create_fake_bwrap(&fake_bwrap);

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        std::env::set_var("BWRAP_BIN", temp.path().join("missing-bwrap"));
        let missing_bwrap = BwrapSandboxManager::new(SandboxScope::new(42, "missing-bwrap"))
            .await
            .err()
            .expect("missing bwrap should fail")
            .to_string();
        assert!(missing_bwrap.contains("BWRAP_BIN"));
        assert!(missing_bwrap.contains("Install bubblewrap"));

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        std::env::set_var("BWRAP_ROOTFS", temp.path().join("missing-rootfs"));
        let missing_rootfs = BwrapSandboxManager::new(SandboxScope::new(42, "missing-rootfs"))
            .await
            .err()
            .expect("missing rootfs should fail")
            .to_string();
        assert!(missing_rootfs.contains("rootfs not found"));
        assert!(missing_rootfs.contains("scripts/build-bwrap-rootfs-debian.sh"));

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        let rootfs_symlink = temp.path().join("rootfs-symlink");
        symlink(&rootfs, &rootfs_symlink).expect("rootfs symlink");
        std::env::set_var("BWRAP_ROOTFS", &rootfs_symlink);
        let rootfs_symlink_error =
            BwrapSandboxManager::new(SandboxScope::new(42, "rootfs-symlink"))
                .await
                .err()
                .expect("rootfs symlink should fail")
                .to_string();
        assert!(rootfs_symlink_error.contains("BWRAP_ROOTFS"));
        assert!(rootfs_symlink_error.contains("must not be a symlink"));

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        std::env::set_var("BWRAP_RESOLV_CONF", temp.path());
        let resolv_dir_error = BwrapSandboxManager::new(SandboxScope::new(42, "resolv-dir"))
            .await
            .err()
            .expect("resolv dir should fail")
            .to_string();
        assert!(resolv_dir_error.contains("BWRAP_RESOLV_CONF"));
        assert!(resolv_dir_error.contains("regular file"));

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        let resolv_file = temp.path().join("resolv.conf");
        std::fs::write(&resolv_file, b"nameserver 127.0.0.1\n").expect("resolv file");
        let resolv_symlink = temp.path().join("resolv-link.conf");
        symlink(&resolv_file, &resolv_symlink).expect("resolv symlink");
        std::env::set_var("BWRAP_RESOLV_CONF", &resolv_symlink);
        let resolv_symlink_error =
            BwrapSandboxManager::new(SandboxScope::new(42, "resolv-symlink"))
                .await
                .err()
                .expect("resolv symlink should fail")
                .to_string();
        assert!(resolv_symlink_error.contains("BWRAP_RESOLV_CONF"));
        assert!(resolv_symlink_error.contains("must not be a symlink"));

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        std::env::remove_var("BWRAP_ROOTFS");
        std::env::set_var("BWRAP_IMAGE_STORE", temp.path().join("empty-images"));
        std::env::set_var("SANDBOX_IMAGE", "agent-sandbox:custom");
        let docker_image_only =
            BwrapSandboxManager::new(SandboxScope::new(42, "docker-image-only"))
                .await
                .err()
                .expect("missing bwrap image should fail")
                .to_string();
        assert!(docker_image_only.contains("Bwrap image manifest not found"));
        assert!(docker_image_only.contains("BWRAP_IMAGE/BWRAP_ROOTFS"));
        assert!(docker_image_only.contains("SANDBOX_IMAGE is Docker-only"));

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        let image_store = temp.path().join("images");
        let unsafe_image = image_store.join("unsafe-rootfs-link");
        std::fs::create_dir_all(&unsafe_image).expect("unsafe image dir");
        std::fs::write(
            unsafe_image.join("image.json"),
            format!(
                r#"{{
  "schema_version": 1,
  "id": "unsafe-rootfs-link",
  "arch": "{}",
  "rootfs": "rootfs",
  "default_shell": "/bin/sh",
  "default_workdir": "/workspace"
}}"#,
                host_arch()
            ),
        )
        .expect("unsafe image manifest");
        symlink(&rootfs, unsafe_image.join("rootfs")).expect("unsafe rootfs symlink");
        std::env::remove_var("BWRAP_ROOTFS");
        std::env::set_var("BWRAP_IMAGE_STORE", &image_store);
        std::env::set_var("BWRAP_IMAGE", "unsafe-rootfs-link");
        let unsafe_rootfs_symlink =
            BwrapSandboxManager::new(SandboxScope::new(42, "unsafe-rootfs-symlink"))
                .await
                .err()
                .expect("unsafe rootfs symlink should fail")
                .to_string();
        assert!(unsafe_rootfs_symlink.contains("resolves outside image directory"));
        assert!(unsafe_rootfs_symlink.contains("unsafe rootfs symlink"));

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        std::env::set_var("BWRAP_ROOT_MODE", "tmp-overlay");
        let unsupported_root_mode =
            BwrapSandboxManager::new(SandboxScope::new(42, "unsupported-root-mode"))
                .await
                .err()
                .expect("unsupported root mode should fail")
                .to_string();
        assert!(unsupported_root_mode.contains("tmp-overlay is not supported"));
        assert!(unsupported_root_mode.contains("overlay-rw, ro"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bwrap_lock_timeout_defaults_to_command_timeout_plus_five_and_rejects_zero() {
        let _env_lock = crate::config::test_env_mutex()
            .lock()
            .expect("test env mutex poisoned");
        let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
        let temp = tempfile::tempdir().expect("temp dir");
        let rootfs = temp.path().join("rootfs");
        create_fake_rootfs(&rootfs);
        let fake_bwrap = temp.path().join("bwrap");
        create_fake_bwrap(&fake_bwrap);

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        std::env::set_var("BWRAP_COMMAND_TIMEOUT_SECS", "7");
        std::env::remove_var("BWRAP_RECREATE_LOCK_TIMEOUT_SECS");
        let manager = BwrapSandboxManager::new(SandboxScope::new(42, "default-lock-timeout"))
            .await
            .unwrap();
        assert_eq!(manager.config.lock_timeout, Duration::from_secs(12));

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        std::env::set_var("BWRAP_RECREATE_LOCK_TIMEOUT_SECS", "0");
        let zero_lock_timeout =
            BwrapSandboxManager::new(SandboxScope::new(42, "zero-lock-timeout"))
                .await
                .err()
                .expect("zero lock timeout should fail")
                .to_string();
        assert!(zero_lock_timeout.contains("BWRAP_RECREATE_LOCK_TIMEOUT_SECS"));
        assert!(zero_lock_timeout.contains("greater than zero"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bwrap_metadata_reports_manifest_path_package_manager_and_sha() {
        let _env_lock = crate::config::test_env_mutex()
            .lock()
            .expect("test env mutex poisoned");
        let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
        let temp = tempfile::tempdir().expect("temp dir");
        let fake_bwrap = temp.path().join("bwrap");
        create_fake_bwrap(&fake_bwrap);

        let image_dir = temp.path().join("images/debian-test");
        let rootfs = image_dir.join("rootfs");
        create_fake_rootfs(&rootfs);
        let manifest_path = image_dir.join("image.json");
        std::fs::write(
            &manifest_path,
            format!(
                r#"{{
  "schema_version": 1,
  "id": "debian-test",
  "arch": "{}",
  "package_manager": "apt",
  "rootfs": "rootfs",
  "default_shell": "/bin/sh",
  "default_workdir": "/workspace"
}}"#,
                host_arch()
            ),
        )
        .expect("image manifest");

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        std::env::remove_var("BWRAP_ROOTFS");
        std::env::set_var("BWRAP_IMAGE_STORE", temp.path().join("images"));
        std::env::set_var("BWRAP_IMAGE", "debian-test");

        let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "metadata-status"))
            .await
            .unwrap();
        manager.create_sandbox().await.unwrap();
        let record = manager.current_record().unwrap();

        assert_eq!(
            record.labels.get("agent.image_manifest_path"),
            Some(&manifest_path.display().to_string())
        );
        assert!(record
            .labels
            .get("agent.image_manifest_sha256")
            .is_some_and(|value| !value.is_empty()));
        assert_eq!(
            record.labels.get("agent.package_manager"),
            Some(&"apt".to_string())
        );
        let status = record.status.expect("status");
        assert!(status.contains("package_manager=apt"));
        assert!(status.contains(&format!("manifest={}", manifest_path.display())));
        assert!(status.contains(&format!("rootfs={}", rootfs.display())));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bwrap_image_bootstrap_download_creates_image() {
        let _env_lock = crate::config::test_env_mutex()
            .lock()
            .expect("test env mutex poisoned");
        let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
        let temp = tempfile::tempdir().expect("temp dir");
        let fake_bwrap = temp.path().join("bwrap");
        create_fake_bwrap(&fake_bwrap);
        let source_rootfs = temp.path().join("source-rootfs");
        create_fake_rootfs(&source_rootfs);
        let (tarball, sha256) = create_rootfs_tarball(temp.path(), &source_rootfs);

        configure_bwrap_image_bootstrap_env(
            temp.path(),
            &fake_bwrap,
            &tarball,
            &sha256,
            "bootstrap-test",
        );

        let manager = BwrapSandboxManager::new(SandboxScope::new(42, "bootstrap-download"))
            .await
            .unwrap();
        assert_eq!(manager.config.image_id, "bootstrap-test");
        let image_dir = temp.path().join("images/bootstrap-test");
        assert!(image_dir.join("image.json").is_file());
        assert!(image_dir.join("rootfs/bin/sh").is_file());
        let (manifest, _) = load_manifest(&image_dir.join("image.json")).unwrap();
        assert_eq!(manifest.package_manager.as_deref(), Some("apk"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bwrap_image_bootstrap_rejects_bad_sha256() {
        let _env_lock = crate::config::test_env_mutex()
            .lock()
            .expect("test env mutex poisoned");
        let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
        let temp = tempfile::tempdir().expect("temp dir");
        let fake_bwrap = temp.path().join("bwrap");
        create_fake_bwrap(&fake_bwrap);
        let source_rootfs = temp.path().join("source-rootfs");
        create_fake_rootfs(&source_rootfs);
        let (tarball, _) = create_rootfs_tarball(temp.path(), &source_rootfs);

        configure_bwrap_image_bootstrap_env(
            temp.path(),
            &fake_bwrap,
            &tarball,
            &"0".repeat(64),
            "bootstrap-bad-sha",
        );

        let error = BwrapSandboxManager::new(SandboxScope::new(42, "bootstrap-bad-sha"))
            .await
            .err()
            .expect("bad checksum should fail")
            .to_string();
        assert!(error.contains("Checksum mismatch"));
        assert!(!temp.path().join("images/bootstrap-bad-sha").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bwrap_image_bootstrap_noops_when_manifest_exists() {
        let _env_lock = crate::config::test_env_mutex()
            .lock()
            .expect("test env mutex poisoned");
        let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
        let temp = tempfile::tempdir().expect("temp dir");
        let fake_bwrap = temp.path().join("bwrap");
        create_fake_bwrap(&fake_bwrap);
        let image_dir = temp.path().join("images/existing-image");
        create_fake_rootfs(&image_dir.join("rootfs"));
        write_test_image_manifest(&image_dir, "existing-image", "apk");

        configure_fake_bwrap_env(temp.path(), &image_dir.join("rootfs"), &fake_bwrap);
        std::env::remove_var("BWRAP_ROOTFS");
        std::env::set_var("BWRAP_IMAGE", "existing-image");
        std::env::set_var("BWRAP_IMAGE_BOOTSTRAP", "download");
        std::env::set_var("BWRAP_IMAGE_STORE", temp.path().join("images"));
        std::env::remove_var("BWRAP_IMAGE_URL");
        std::env::remove_var("BWRAP_IMAGE_SHA256");

        let manager = BwrapSandboxManager::new(SandboxScope::new(42, "bootstrap-existing"))
            .await
            .unwrap();
        assert_eq!(manager.config.image_id, "existing-image");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bwrap_state_and_lock_dir_errors_name_config_keys() {
        let _env_lock = crate::config::test_env_mutex()
            .lock()
            .expect("test env mutex poisoned");
        let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
        let temp = tempfile::tempdir().expect("temp dir");
        let rootfs = temp.path().join("rootfs");
        create_fake_rootfs(&rootfs);
        let fake_bwrap = temp.path().join("bwrap");
        create_fake_bwrap(&fake_bwrap);

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        let state_file = temp.path().join("not-a-state-dir");
        std::fs::write(&state_file, b"file").expect("state file");
        std::env::set_var("BWRAP_STATE_DIR", &state_file);
        let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "bad-state-dir"))
            .await
            .unwrap();
        let state_error = manager
            .create_sandbox()
            .await
            .err()
            .expect("state file should fail")
            .to_string();
        assert!(state_error.contains("BWRAP_STATE_DIR"));
        assert!(state_error.contains("must be a directory"));

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        let state_target = temp.path().join("state-target");
        std::fs::create_dir(&state_target).expect("state target");
        let state_symlink = temp.path().join("state-link");
        symlink(&state_target, &state_symlink).expect("state symlink");
        std::env::set_var("BWRAP_STATE_DIR", &state_symlink);
        let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "bad-state-symlink"))
            .await
            .unwrap();
        let state_symlink_error = manager
            .create_sandbox()
            .await
            .err()
            .expect("state symlink should fail")
            .to_string();
        assert!(state_symlink_error.contains("BWRAP_STATE_DIR"));
        assert!(state_symlink_error.contains("must not be a symlink"));

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        let lock_file = temp.path().join("not-a-lock-dir");
        std::fs::write(&lock_file, b"file").expect("lock file");
        std::env::set_var("BWRAP_LOCK_DIR", &lock_file);
        let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "bad-lock-dir"))
            .await
            .unwrap();
        let lock_error = manager
            .create_sandbox()
            .await
            .err()
            .expect("lock file should fail")
            .to_string();
        assert!(lock_error.contains("BWRAP_LOCK_DIR"));
        assert!(lock_error.contains("must be a directory"));

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        let lock_target = temp.path().join("lock-target");
        std::fs::create_dir(&lock_target).expect("lock target");
        let lock_symlink = temp.path().join("lock-link");
        symlink(&lock_target, &lock_symlink).expect("lock symlink");
        std::env::set_var("BWRAP_LOCK_DIR", &lock_symlink);
        let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "bad-lock-symlink"))
            .await
            .unwrap();
        let lock_symlink_error = manager
            .create_sandbox()
            .await
            .err()
            .expect("lock symlink should fail")
            .to_string();
        assert!(lock_symlink_error.contains("BWRAP_LOCK_DIR"));
        assert!(lock_symlink_error.contains("must not be a symlink"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bwrap_root_upper_dir_override_is_per_scope_and_rejects_unsafe_paths() {
        let _env_lock = crate::config::test_env_mutex()
            .lock()
            .expect("test env mutex poisoned");
        let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
        let temp = tempfile::tempdir().expect("temp dir");
        let rootfs = temp.path().join("rootfs");
        create_fake_rootfs(&rootfs);
        let fake_bwrap = temp.path().join("bwrap");
        create_fake_bwrap(&fake_bwrap);

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        let root_upper_parent = temp.path().join("root-upper");
        std::env::set_var("BWRAP_ROOT_UPPER_DIR", &root_upper_parent);
        let scope = SandboxScope::new(42, "upper-override");
        let manager = BwrapSandboxManager::new(scope.clone()).await.unwrap();
        assert_eq!(
            manager.state.system_upper,
            root_upper_parent.join(scope.stable_name()).join("upper")
        );
        assert_eq!(
            manager.state.system_work,
            root_upper_parent.join(scope.stable_name()).join("work")
        );
        let work_dir = manager
            .prepare_overlay_workdir()
            .unwrap()
            .expect("overlay workdir");
        assert!(work_dir.starts_with(root_upper_parent.join(scope.stable_name()).join("work")));
        let mut manager = manager;
        manager.create_sandbox().await.unwrap();
        assert!(root_upper_parent.join(scope.stable_name()).exists());
        let changed_root_upper_parent = temp.path().join("changed-root-upper");
        std::env::set_var("BWRAP_ROOT_UPPER_DIR", &changed_root_upper_parent);
        let pinned_manager = BwrapSandboxManager::new(scope.clone()).await.unwrap();
        assert_eq!(
            pinned_manager.state.system_dir,
            root_upper_parent.join(scope.stable_name())
        );
        assert!(!changed_root_upper_parent.join(scope.stable_name()).exists());
        manager.destroy().await.unwrap();
        assert!(!root_upper_parent.join(scope.stable_name()).exists());
        assert!(root_upper_parent.exists());

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        std::env::set_var("BWRAP_ROOT_UPPER_DIR", &root_upper_parent);
        let delete_scope = SandboxScope::new(42, "upper-delete");
        let mut manager = BwrapSandboxManager::new(delete_scope.clone())
            .await
            .unwrap();
        manager.create_sandbox().await.unwrap();
        assert!(root_upper_parent.join(delete_scope.stable_name()).exists());
        assert!(
            BwrapSandboxManager::delete_sandbox_by_name(42, &delete_scope.stable_name())
                .await
                .unwrap()
        );
        assert!(!root_upper_parent.join(delete_scope.stable_name()).exists());

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        std::env::set_var("BWRAP_ROOT_UPPER_DIR", &root_upper_parent);
        let changed_delete_scope = SandboxScope::new(42, "upper-delete-after-env-change");
        let mut manager = BwrapSandboxManager::new(changed_delete_scope.clone())
            .await
            .unwrap();
        manager.create_sandbox().await.unwrap();
        std::env::set_var("BWRAP_ROOT_UPPER_DIR", &changed_root_upper_parent);
        assert!(BwrapSandboxManager::delete_sandbox_by_name(
            42,
            &changed_delete_scope.stable_name()
        )
        .await
        .unwrap());
        assert!(!root_upper_parent
            .join(changed_delete_scope.stable_name())
            .exists());
        assert!(!changed_root_upper_parent
            .join(changed_delete_scope.stable_name())
            .exists());

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        let file_upper = temp.path().join("file-upper");
        std::fs::write(&file_upper, b"file").expect("file upper");
        std::env::set_var("BWRAP_ROOT_UPPER_DIR", &file_upper);
        let file_error = BwrapSandboxManager::new(SandboxScope::new(42, "file-upper"))
            .await
            .err()
            .expect("file upper should fail")
            .to_string();
        assert!(file_error.contains("BWRAP_ROOT_UPPER_DIR"));
        assert!(file_error.contains("must be a directory"));

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        let symlink_target = temp.path().join("upper-target");
        std::fs::create_dir_all(&symlink_target).expect("upper target");
        let symlink_upper = temp.path().join("upper-symlink");
        symlink(&symlink_target, &symlink_upper).expect("upper symlink");
        std::env::set_var("BWRAP_ROOT_UPPER_DIR", &symlink_upper);
        let symlink_error = BwrapSandboxManager::new(SandboxScope::new(42, "symlink-upper"))
            .await
            .err()
            .expect("symlink upper should fail")
            .to_string();
        assert!(symlink_error.contains("BWRAP_ROOT_UPPER_DIR"));
        assert!(symlink_error.contains("must not be a symlink"));

        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        std::env::set_var("BWRAP_ROOT_UPPER_DIR", rootfs.join("unsafe-upper"));
        let rootfs_error = BwrapSandboxManager::new(SandboxScope::new(42, "rootfs-upper"))
            .await
            .err()
            .expect("rootfs upper should fail")
            .to_string();
        assert!(rootfs_error.contains("BWRAP_ROOT_UPPER_DIR"));
        assert!(rootfs_error.contains("must not be inside the bwrap rootfs image"));
    }

    #[cfg(unix)]
    #[test]
    fn bwrap_manifest_validation_rejects_unsafe_values() {
        let temp = tempfile::tempdir().expect("temp dir");
        let manifest_path = temp.path().join("image.json");

        std::fs::write(
            &manifest_path,
            format!(
                r#"{{
  "schema_version": 1,
  "id": "bad-rootfs",
  "arch": "{}",
  "rootfs": "/abs/rootfs",
  "default_shell": "/bin/sh",
  "default_workdir": "/workspace"
}}"#,
                host_arch()
            ),
        )
        .expect("manifest");
        let absolute_rootfs = load_manifest(&manifest_path).unwrap_err().to_string();
        assert!(absolute_rootfs.contains("rootfs must be relative"));

        std::fs::write(
            &manifest_path,
            format!(
                r#"{{
  "schema_version": 1,
  "id": "escaping-rootfs",
  "arch": "{}",
  "rootfs": "../rootfs",
  "default_shell": "/bin/sh",
  "default_workdir": "/workspace"
}}"#,
                host_arch()
            ),
        )
        .expect("manifest");
        let escaping_rootfs = load_manifest(&manifest_path).unwrap_err().to_string();
        assert!(escaping_rootfs.contains("rootfs must stay under the image directory"));

        std::fs::write(
            &manifest_path,
            format!(
                r#"{{
  "schema_version": 1,
  "id": "bad-workdir",
  "arch": "{}",
  "rootfs": "rootfs",
  "default_shell": "/bin/sh",
  "default_workdir": "/tmp"
}}"#,
                host_arch()
            ),
        )
        .expect("manifest");
        let bad_workdir = load_manifest(&manifest_path).unwrap_err().to_string();
        assert!(bad_workdir.contains("default_workdir must be /workspace"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bwrap_exec_preserves_nonzero_exit_truncates_output_and_times_out() {
        let _env_lock = crate::config::test_env_mutex()
            .lock()
            .expect("test env mutex poisoned");
        let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
        let temp = tempfile::tempdir().expect("temp dir");
        let rootfs = temp.path().join("rootfs");
        create_fake_rootfs(&rootfs);
        let fake_bwrap = temp.path().join("bwrap");
        create_fake_bwrap_script(
            &fake_bwrap,
            "#!/bin/sh\nprintf abcdefghijklmnop\nprintf qrstuvwxyz >&2\nexit 7\n",
        );
        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        std::env::set_var("BWRAP_MAX_OUTPUT_BYTES", "8");

        let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "exec-output"))
            .await
            .unwrap();
        let output = manager.exec_command("ignored", None).await.unwrap();
        assert_eq!(output.exit_code, 7);
        assert_eq!(output.stdout, "abcdefgh");
        assert!(output.stderr.contains("qrstuvwx"));
        assert!(output
            .stderr
            .contains("stdout truncated by BWRAP_MAX_OUTPUT_BYTES: captured 8 of 16 bytes"));
        assert!(output
            .stderr
            .contains("stderr truncated by BWRAP_MAX_OUTPUT_BYTES: captured 8 of 10 bytes"));

        let child_pid_file = temp.path().join("bwrap-child.pid");
        create_fake_bwrap_script(
            &fake_bwrap,
            &format!(
                "#!/bin/sh\nsleep 30 &\nprintf '%s' \"$!\" > {}\nwait\n",
                child_pid_file.display()
            ),
        );
        configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
        std::env::set_var("BWRAP_COMMAND_TIMEOUT_SECS", "1");
        let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "exec-timeout"))
            .await
            .unwrap();
        let error = manager
            .exec_command("ignored", None)
            .await
            .err()
            .expect("sleeping bwrap should time out")
            .to_string();
        assert!(error.contains("timed out after 1s"));
        assert!(error.contains("process group"));
        let child_pid = std::fs::read_to_string(child_pid_file)
            .expect("child pid")
            .trim()
            .to_string();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let child_alive = std::process::Command::new("kill")
            .arg("-0")
            .arg(&child_pid)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        assert!(!child_alive, "timeout left child process {child_pid} alive");
    }

    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "requires host bubblewrap support and BWRAP_ROOTFS pointing at a prepared rootfs"]
    async fn bwrap_smoke_exec_persists_workspace_and_overlay_rw() {
        let Some(rootfs) = std::env::var_os("BWRAP_ROOTFS").map(PathBuf::from) else {
            eprintln!("skipping ignored bwrap smoke: BWRAP_ROOTFS is not set");
            return;
        };
        if !rootfs.is_dir() {
            eprintln!(
                "skipping ignored bwrap smoke: BWRAP_ROOTFS is not a directory: {}",
                rootfs.display()
            );
            return;
        }
        let _env_lock = crate::config::test_env_mutex()
            .lock()
            .expect("test env mutex poisoned");
        let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
        let temp = tempfile::tempdir().expect("temp dir");
        configure_real_bwrap_env(temp.path(), &rootfs, BwrapRootMode::OverlayRw);

        let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "smoke-overlay-rw"))
            .await
            .unwrap();
        manager.create_sandbox().await.unwrap();
        let first = manager
            .exec_command(
                "pwd && printf persisted >/workspace/hello.txt && printf system >/etc/oxide-test && test ! -S /var/run/docker.sock && test ! -e /run/sandboxd",
                None,
            )
            .await
            .unwrap();
        assert_eq!(first.exit_code, 0, "stderr={}", first.stderr);
        assert_eq!(first.stdout.lines().next(), Some("/workspace"));

        let second = manager
            .exec_command(
                "cat /workspace/hello.txt && printf '\\n' && cat /etc/oxide-test",
                None,
            )
            .await
            .unwrap();
        assert_eq!(second.exit_code, 0, "stderr={}", second.stderr);
        assert_eq!(second.stdout, "persisted\nsystem");
    }

    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "requires host bubblewrap support and BWRAP_ROOTFS pointing at a prepared rootfs"]
    async fn bwrap_smoke_ro_root_rejects_system_writes() {
        let Some(rootfs) = std::env::var_os("BWRAP_ROOTFS").map(PathBuf::from) else {
            eprintln!("skipping ignored bwrap smoke: BWRAP_ROOTFS is not set");
            return;
        };
        if !rootfs.is_dir() {
            eprintln!(
                "skipping ignored bwrap smoke: BWRAP_ROOTFS is not a directory: {}",
                rootfs.display()
            );
            return;
        }
        let _env_lock = crate::config::test_env_mutex()
            .lock()
            .expect("test env mutex poisoned");
        let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
        let temp = tempfile::tempdir().expect("temp dir");
        configure_real_bwrap_env(temp.path(), &rootfs, BwrapRootMode::ReadOnly);

        let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "smoke-ro"))
            .await
            .unwrap();
        manager.create_sandbox().await.unwrap();
        let output = manager
            .exec_command(
                "printf workspace >/workspace/ok.txt && printf system >/etc/oxide-ro-test",
                None,
            )
            .await
            .unwrap();

        assert_ne!(output.exit_code, 0, "system write unexpectedly succeeded");
        assert_eq!(manager.read_file("ok.txt").await.unwrap(), b"workspace");
    }

    #[cfg(unix)]
    fn create_fake_rootfs(rootfs: &Path) {
        for directory in ["bin", "dev", "proc", "tmp", "workspace"] {
            std::fs::create_dir_all(rootfs.join(directory)).expect("fake rootfs dir");
        }
        std::fs::write(rootfs.join("bin/sh"), b"").expect("fake shell");
    }

    #[cfg(unix)]
    fn create_fake_bwrap(path: &PathBuf) {
        create_fake_bwrap_script(path, "#!/bin/sh\nprintf '%s\n' '--disable-userns'\n");
    }

    #[cfg(unix)]
    fn create_fake_bwrap_script(path: &PathBuf, script: &str) {
        std::fs::write(path, script).expect("fake bwrap");
        let mut permissions = std::fs::metadata(path)
            .expect("fake bwrap metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("fake bwrap permissions");
    }

    #[cfg(unix)]
    fn create_rootfs_tarball(temp: &Path, rootfs: &Path) -> (PathBuf, String) {
        let tarball = temp.join(format!("rootfs-{}.tar.gz", fastrand::u64(..)));
        let output = std::process::Command::new("tar")
            .arg("-czf")
            .arg(&tarball)
            .arg("-C")
            .arg(rootfs)
            .arg(".")
            .output()
            .expect("tar command should run");
        assert!(
            output.status.success(),
            "tar failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = std::fs::read(&tarball).expect("tarball bytes");
        (tarball, format!("{:x}", Sha256::digest(&bytes)))
    }

    #[cfg(unix)]
    fn write_test_image_manifest(image_dir: &Path, image_id: &str, package_manager: &str) {
        std::fs::create_dir_all(image_dir).expect("image dir");
        std::fs::write(
            image_dir.join("image.json"),
            format!(
                r#"{{
  "schema_version": 1,
  "id": "{image_id}",
  "arch": "{}",
  "package_manager": "{package_manager}",
  "rootfs": "rootfs",
  "default_shell": "/bin/sh",
  "default_workdir": "/workspace"
}}"#,
                host_arch()
            ),
        )
        .expect("image manifest");
    }

    #[cfg(unix)]
    fn configure_bwrap_image_bootstrap_env(
        temp: &Path,
        fake_bwrap: &Path,
        tarball: &Path,
        sha256: &str,
        image_id: &str,
    ) {
        let file_url = url::Url::from_file_path(tarball)
            .expect("tarball file URL")
            .to_string();
        std::env::set_var("BWRAP_ALLOW_OVERLAY", "true");
        std::env::set_var("BWRAP_BIN", fake_bwrap);
        std::env::set_var("BWRAP_COMMAND_TIMEOUT_SECS", "5");
        std::env::set_var("BWRAP_DISABLE_NESTED_USERNS", "false");
        std::env::set_var("BWRAP_IMAGE", image_id);
        std::env::set_var("BWRAP_IMAGE_BOOTSTRAP", "download");
        std::env::set_var("BWRAP_IMAGE_PACKAGE_MANAGER", "apk");
        std::env::set_var("BWRAP_IMAGE_SHA256", sha256);
        std::env::set_var("BWRAP_IMAGE_STORE", temp.join("images"));
        std::env::set_var("BWRAP_IMAGE_URL", file_url);
        std::env::set_var("BWRAP_LOCK_DIR", temp.join("locks"));
        std::env::set_var("BWRAP_MAX_OUTPUT_BYTES", "1024");
        std::env::set_var("BWRAP_MAX_READ_FILE_BYTES", "1024");
        std::env::set_var("BWRAP_NET", "none");
        std::env::set_var("BWRAP_RESOLV_CONF", "none");
        std::env::set_var("BWRAP_ROOT_MODE", "overlay-rw");
        std::env::remove_var("BWRAP_ROOTFS");
        std::env::set_var("BWRAP_STATE_DIR", temp.join("scopes"));
        std::env::remove_var("SANDBOX_EXEC_TIMEOUT_SECS");
    }

    #[cfg(unix)]
    fn configure_fake_bwrap_env(temp: &Path, rootfs: &Path, fake_bwrap: &Path) {
        std::env::set_var("BWRAP_ALLOW_OVERLAY", "true");
        std::env::set_var("BWRAP_BIN", fake_bwrap);
        std::env::set_var("BWRAP_COMMAND_TIMEOUT_SECS", "5");
        std::env::set_var("BWRAP_DISABLE_NESTED_USERNS", "false");
        std::env::set_var("BWRAP_IMAGE", "test-dev");
        std::env::set_var("BWRAP_LOCK_DIR", temp.join("locks"));
        std::env::set_var("BWRAP_MAX_OUTPUT_BYTES", "1024");
        std::env::set_var("BWRAP_MAX_READ_FILE_BYTES", "1024");
        std::env::set_var("BWRAP_NET", "none");
        std::env::set_var("BWRAP_RESOLV_CONF", "none");
        std::env::set_var("BWRAP_ROOT_MODE", "overlay-rw");
        std::env::set_var("BWRAP_ROOTFS", rootfs);
        std::env::set_var("BWRAP_STATE_DIR", temp.join("scopes"));
        std::env::remove_var("SANDBOX_EXEC_TIMEOUT_SECS");
    }

    #[cfg(unix)]
    fn configure_real_bwrap_env(temp: &Path, rootfs: &Path, root_mode: BwrapRootMode) {
        std::env::set_var("BWRAP_ALLOW_OVERLAY", "true");
        std::env::set_var("BWRAP_COMMAND_TIMEOUT_SECS", "15");
        std::env::set_var("BWRAP_IMAGE", "ignored-test-rootfs");
        std::env::set_var("BWRAP_LOCK_DIR", temp.join("locks"));
        std::env::set_var("BWRAP_MAX_OUTPUT_BYTES", "1048576");
        std::env::set_var("BWRAP_MAX_READ_FILE_BYTES", "1048576");
        std::env::set_var("BWRAP_NET", "host");
        std::env::set_var("BWRAP_RESOLV_CONF", "auto");
        std::env::set_var("BWRAP_ROOT_MODE", root_mode.to_string());
        std::env::set_var("BWRAP_ROOTFS", rootfs);
        std::env::set_var("BWRAP_STATE_DIR", temp.join("scopes"));
        std::env::remove_var("SANDBOX_EXEC_TIMEOUT_SECS");
    }

    #[cfg(unix)]
    fn args_to_strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[cfg(unix)]
    fn contains_arg_pair(args: &[String], first: &str, second: &str) -> bool {
        args.windows(2)
            .any(|window| window[0] == first && window[1] == second)
    }

    #[cfg(unix)]
    fn assert_args_do_not_bind_host_control_paths(args: &[String]) {
        let joined = args.join("\n");
        assert!(!joined.contains("/var/run/docker.sock"));
        assert!(!joined.contains("/run/sandboxd"));
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            let home = home.display().to_string();
            if !home.is_empty() && args.iter().all(|arg| !arg.starts_with(&home)) {
                assert!(!joined.contains(&home));
            }
        }
    }
}
