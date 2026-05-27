use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::config::BwrapSandboxConfig;
use super::types::{BwrapNetworkMode, BwrapRootMode};
use crate::sandbox::{SandboxContainerRecord, SandboxScope};

#[derive(Debug, Clone)]
pub(super) struct BwrapScopeState {
    pub(super) scope_name: String,
    pub(super) scope_dir: PathBuf,
    pub(super) workspace: PathBuf,
    pub(super) system_dir: PathBuf,
    pub(super) system_upper: PathBuf,
    pub(super) system_work: PathBuf,
    pub(super) tmp: PathBuf,
    pub(super) active: PathBuf,
    pub(super) metadata: PathBuf,
    pub(super) lock: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct BwrapScopeMetadata {
    pub(super) schema_version: u32,
    pub(super) backend: String,
    pub(super) scope_name: String,
    pub(super) owner_id: i64,
    pub(super) namespace: String,
    pub(super) chat_id: Option<i64>,
    pub(super) thread_id: Option<i64>,
    pub(super) image_id: String,
    #[serde(default)]
    pub(super) image_manifest_path: Option<String>,
    pub(super) image_manifest_sha256: Option<String>,
    #[serde(default)]
    pub(super) package_manager: Option<String>,
    pub(super) rootfs: String,
    pub(super) workspace: String,
    #[serde(default)]
    pub(super) system_dir: Option<String>,
    pub(super) root_mode: BwrapRootMode,
    pub(super) network_mode: BwrapNetworkMode,
    pub(super) created_at: i64,
    pub(super) updated_at: i64,
    pub(super) generation: u64,
}

impl BwrapScopeState {
    pub(super) fn new(config: &BwrapSandboxConfig, scope: &SandboxScope) -> Self {
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
    pub(super) fn read(path: &Path) -> Result<Option<Self>> {
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = fs::read(path)
            .with_context(|| format!("Failed to read bwrap metadata {}", path.display()))?;
        let metadata = serde_json::from_slice(&bytes)
            .with_context(|| format!("Invalid bwrap metadata JSON {}", path.display()))?;
        Ok(Some(metadata))
    }

    pub(super) fn to_record(&self) -> SandboxContainerRecord {
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
