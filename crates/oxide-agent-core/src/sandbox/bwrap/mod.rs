//! Bubblewrap sandbox backend.

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::path::Path;
use std::time::{Duration, Instant};
use tracing::info;

mod bootstrap;
mod config;
mod env;
mod exec;
mod files;
mod host_fs;
mod image;
mod preflight;
mod process;
mod state;
mod types;
mod workspace;

use self::config::BwrapSandboxConfig;
use self::host_fs::{ensure_configured_dir, remove_dir_if_exists};
#[cfg(test)]
use self::image::{host_arch, load_manifest};
pub(crate) use self::preflight::{bootstrap_image_from_env, preflight_from_env};
use self::state::{BwrapScopeMetadata, BwrapScopeState};
use self::types::{BwrapNetworkMode, BwrapRootMode};
use super::{SandboxContainerRecord, SandboxScope};

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

pub(super) struct ScopeLock {
    file: File,
}

impl Drop for ScopeLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
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
}

#[cfg(test)]
mod tests;
