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
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, SystemTime};
use tokio::process::Command;
use tracing::{debug, info};
use uuid::Uuid;

use super::{ExecResult, SandboxContainerRecord, SandboxFileListing, SandboxScope};

const BWRAP_DEFAULT_BIN: &str = "bwrap";
const BWRAP_DEFAULT_IMAGE: &str = "debian-13-dev";
const BWRAP_DEFAULT_NET: BwrapNetworkMode = BwrapNetworkMode::Host;
const BWRAP_DEFAULT_ROOT_MODE: BwrapRootMode = BwrapRootMode::OverlayRw;
const BWRAP_DEFAULT_TIMEOUT_SECS: u64 = 60;
const BWRAP_DEFAULT_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const BWRAP_DEFAULT_MAX_READ_FILE_BYTES: u64 = 50 * 1024 * 1024;
const BWRAP_METADATA_SCHEMA_VERSION: u32 = 1;
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
    manifest_sha256: Option<String>,
    rootfs: PathBuf,
    state_dir: PathBuf,
    lock_dir: PathBuf,
    net: BwrapNetworkMode,
    root_mode: BwrapRootMode,
    command_timeout: Duration,
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

#[derive(Debug, Clone, Deserialize)]
struct BwrapImageManifest {
    schema_version: u32,
    id: String,
    arch: String,
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
    image_manifest_sha256: Option<String>,
    rootfs: String,
    workspace: String,
    root_mode: BwrapRootMode,
    network_mode: BwrapNetworkMode,
    created_at: i64,
    updated_at: i64,
    generation: u64,
}

impl BwrapSandboxManager {
    /// Create a bwrap manager for the provided scope.
    pub(crate) async fn new(scope: SandboxScope) -> Result<Self> {
        let mut config = BwrapSandboxConfig::from_env()?;
        let state = BwrapScopeState::new(&config, &scope);
        if let Some(metadata) = BwrapScopeMetadata::read(&state.metadata)? {
            config.apply_scope_pin(&metadata)?;
        }
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
        let args = self.bwrap_args(work_dir.as_deref(), cmd);
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
        remove_dir_if_exists(&scope_dir)?;
        Ok(true)
    }

    fn current_record(&self) -> Result<SandboxContainerRecord> {
        let metadata = BwrapScopeMetadata::read(&self.state.metadata)?
            .ok_or_else(|| anyhow!("bwrap sandbox metadata is missing after create"))?;
        Ok(metadata.to_record())
    }

    fn lock_scope(&self) -> Result<ScopeLock> {
        fs::create_dir_all(&self.config.lock_dir).with_context(|| {
            format!(
                "Failed to create bwrap lock directory {}",
                self.config.lock_dir.display()
            )
        })?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.state.lock)
            .with_context(|| format!("Failed to open bwrap lock {}", self.state.lock.display()))?;
        file.lock_exclusive()
            .with_context(|| format!("Failed to lock bwrap scope {}", self.state.scope_name))?;
        Ok(ScopeLock { file })
    }

    fn ensure_scope_dirs_locked(&self) -> Result<()> {
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
            image_manifest_sha256: self.config.manifest_sha256.clone(),
            rootfs: self.config.rootfs.display().to_string(),
            workspace: self.state.workspace.display().to_string(),
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

    fn bwrap_args(&self, work_dir: Option<&Path>, command: &str) -> Vec<OsString> {
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

        if let Some(resolv_conf) = self.resolv_conf_bind() {
            args.push("--ro-bind".into());
            args.push(resolv_conf.into_os_string());
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

    fn resolv_conf_bind(&self) -> Option<PathBuf> {
        if self.config.net != BwrapNetworkMode::Host {
            return None;
        }
        match &self.config.resolv_conf {
            BwrapResolvConf::None => None,
            BwrapResolvConf::Path(path) => Some(path.clone()),
            BwrapResolvConf::Auto => {
                let host = PathBuf::from("/etc/resolv.conf");
                let rootfs_target = self.config.rootfs.join("etc/resolv.conf");
                (host.is_file() && rootfs_target.exists()).then_some(host)
            }
        }
    }

    async fn run_bwrap(
        &self,
        args: Vec<OsString>,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<ExecResult> {
        let child = Command::new(&self.config.bwrap_bin)
            .args(args)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "Failed to spawn bwrap at {}. Install bubblewrap or set BWRAP_BIN=/path/to/bwrap.",
                    self.config.bwrap_bin.display()
                )
            })?;

        let timeout = tokio::time::sleep(self.config.command_timeout);
        tokio::pin!(timeout);

        let output = if let Some(token) = cancellation_token {
            tokio::select! {
                output = child.wait_with_output() => output.context("Failed to wait for bwrap command")?,
                () = &mut timeout => bail!("Bwrap command timed out after {}s and the process was killed.", self.config.command_timeout.as_secs()),
                () = token.cancelled() => bail!("Bwrap command cancelled by user and the process was killed."),
            }
        } else {
            tokio::select! {
                output = child.wait_with_output() => output.context("Failed to wait for bwrap command")?,
                () = &mut timeout => bail!("Bwrap command timed out after {}s and the process was killed.", self.config.command_timeout.as_secs()),
            }
        };

        let (stdout, stdout_truncated) =
            capture_output(output.stdout, self.config.max_output_bytes);
        let (mut stderr, stderr_truncated) =
            capture_output(output.stderr, self.config.max_output_bytes);
        if stdout_truncated {
            stderr.push_str(&format!(
                "\n[oxide-agent] stdout truncated to {} bytes by BWRAP_MAX_OUTPUT_BYTES",
                self.config.max_output_bytes
            ));
        }
        if stderr_truncated {
            stderr.push_str(&format!(
                "\n[oxide-agent] stderr truncated to {} bytes by BWRAP_MAX_OUTPUT_BYTES",
                self.config.max_output_bytes
            ));
        }

        Ok(ExecResult {
            stdout,
            stderr,
            exit_code: i64::from(output.status.code().unwrap_or(-1)),
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
        let max_output_bytes = env_usize("BWRAP_MAX_OUTPUT_BYTES", BWRAP_DEFAULT_MAX_OUTPUT_BYTES)?;
        let max_read_file_bytes = env_u64(
            "BWRAP_MAX_READ_FILE_BYTES",
            BWRAP_DEFAULT_MAX_READ_FILE_BYTES,
        )?;
        let disable_nested_userns = env_bool("BWRAP_DISABLE_NESTED_USERNS", true)?;
        let resolv_conf = parse_resolv_conf()?;

        let (manifest, manifest_sha256, rootfs) = if let Some(rootfs) = rootfs_override {
            let manifest_path = rootfs.parent().map(|parent| parent.join("image.json"));
            let (manifest, manifest_sha256) =
                match manifest_path.as_ref().filter(|path| path.is_file()) {
                    Some(path) => load_manifest(path)?,
                    None => (BwrapImageManifest::fallback(&image_id), None),
                };
            (manifest, manifest_sha256, rootfs)
        } else {
            let image_dir = image_store.join(&image_id);
            let manifest_path = image_dir.join("image.json");
            if !manifest_path.is_file() {
                bail!(
                    "Bwrap image manifest not found at {}. Run scripts/build-bwrap-rootfs-debian.sh or set BWRAP_ROOTFS.",
                    manifest_path.display()
                );
            }
            let (manifest, manifest_sha256) = load_manifest(&manifest_path)?;
            let rootfs = image_dir.join(&manifest.rootfs);
            (manifest, manifest_sha256, rootfs)
        };

        let mut default_env = default_manifest_env();
        default_env.extend(manifest.default_env.clone());

        Ok(Self {
            bwrap_bin,
            image_id: manifest.id.clone(),
            manifest_sha256,
            rootfs: absolute_maybe_existing_path(&rootfs)?,
            state_dir,
            lock_dir,
            net,
            root_mode,
            command_timeout,
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
        self.rootfs = absolute_existing_path(Path::new(&metadata.rootfs))?;
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
        Ok(())
    }
}

impl BwrapScopeState {
    fn new(config: &BwrapSandboxConfig, scope: &SandboxScope) -> Self {
        let scope_name = scope.stable_name();
        let scope_dir = config.state_dir.join(&scope_name);
        Self {
            workspace: scope_dir.join("workspace"),
            system_upper: scope_dir.join("system/upper"),
            system_work: scope_dir.join("system/work"),
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
        ]);
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
            status: Some(format!(
                "bwrap root_mode={} net={} rootfs={}",
                self.root_mode, self.network_mode, self.rootfs
            )),
            running: false,
            user_id: Some(self.owner_id),
            scope: Some(self.namespace.clone()),
            chat_id: self.chat_id,
            thread_id: self.thread_id,
            labels,
        }
    }
}

impl BwrapImageManifest {
    fn fallback(image_id: &str) -> Self {
        Self {
            schema_version: 1,
            id: image_id.to_string(),
            arch: host_arch().to_string(),
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
    if manifest.rootfs.starts_with('/') {
        bail!(
            "Invalid bwrap image manifest {}: rootfs must be relative",
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
            "Bwrap backend selected, but rootfs not found at {}. Run scripts/build-bwrap-rootfs-debian.sh or set BWRAP_ROOTFS.",
            config.rootfs.display()
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

fn bwrap_supports_disable_userns(bwrap_bin: &Path) -> Result<bool> {
    let output = std::process::Command::new(bwrap_bin)
        .arg("--help")
        .output()
        .with_context(|| {
            format!(
                "Bwrap backend selected, but BWRAP_BIN='{}' was not found or is not executable. Install bubblewrap or set BWRAP_BIN=/path/to/bwrap.",
                bwrap_bin.display()
            )
        })?;
    let help = String::from_utf8_lossy(&output.stdout);
    Ok(help.contains("--disable-userns"))
}

fn resolve_executable(value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    if path.components().count() > 1 || path.is_absolute() {
        if path.is_file() {
            return absolute_existing_path(&path);
        }
        bail!(
            "Bwrap backend selected, but BWRAP_BIN='{}' was not found or is not executable. Install bubblewrap or set BWRAP_BIN=/path/to/bwrap.",
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
        "Bwrap backend selected, but BWRAP_BIN='{value}' was not found or is not executable. Install bubblewrap or set BWRAP_BIN=/path/to/bwrap."
    )
}

fn env_string(key: &str, default: &str) -> Result<String> {
    Ok(std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string()))
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
        path => Ok(BwrapResolvConf::Path(absolute_existing_path(path)?)),
    }
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

fn capture_output(bytes: Vec<u8>, max_bytes: usize) -> (String, bool) {
    if bytes.len() <= max_bytes {
        return (String::from_utf8_lossy(&bytes).into_owned(), false);
    }
    (
        String::from_utf8_lossy(&bytes[..max_bytes]).into_owned(),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::{resolve_workspace_path, BwrapNetworkMode, BwrapRootMode};
    use std::path::Path;

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
}
