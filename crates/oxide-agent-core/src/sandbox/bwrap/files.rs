use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::workspace::{
    cleanup_old_files, dir_size, ensure_no_symlink_escape, list_workspace_entries,
    resolve_workspace_path,
};
use super::BwrapSandboxManager;
use crate::sandbox::traits::apply_sandbox_file_edit;
use crate::sandbox::{
    SandboxApplyFileEditResult, SandboxEditReadGuard, SandboxFileEdit, SandboxFileListing,
};

impl BwrapSandboxManager {
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
