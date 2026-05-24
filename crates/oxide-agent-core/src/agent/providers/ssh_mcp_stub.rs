#![allow(missing_docs)]

//! SSH MCP support types used when the upstream MCP client is not compiled.

use crate::agent::memory::AgentMessage;
use crate::storage::{StorageProvider, TopicInfraAuthMode, TopicInfraConfigRecord};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use uuid::Uuid;

const APPROVAL_TTL_SECS: i64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretProbeKind {
    Opaque,
    SshPrivateKey,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SecretProbeReport {
    pub secret_ref: String,
    pub source: String,
    pub kind: SecretProbeKind,
    pub present: bool,
    pub usable: bool,
    pub status: String,
    pub fingerprint: Option<String>,
    pub key_type: Option<String>,
    pub comment: Option<String>,
    pub error: Option<String>,
}

impl SecretProbeReport {
    fn invalid(secret_ref: &str, source: &str, kind: SecretProbeKind, error: String) -> Self {
        Self {
            secret_ref: secret_ref.to_string(),
            source: source.to_string(),
            kind,
            present: false,
            usable: false,
            status: "invalid".to_string(),
            fingerprint: None,
            key_type: None,
            comment: None,
            error: Some(error),
        }
    }

    fn summary(&self) -> String {
        format!(
            "secret_ref '{}' from {} is unavailable: {}",
            self.secret_ref,
            self.source,
            self.error
                .as_deref()
                .unwrap_or("ssh mcp integration is not compiled")
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TopicInfraPreflightReport {
    pub topic_id: String,
    pub target_name: String,
    pub host: String,
    pub port: u16,
    pub remote_user: String,
    pub auth_mode: TopicInfraAuthMode,
    pub provider_enabled: bool,
    pub auth_secret: Option<SecretProbeReport>,
    pub sudo_secret: Option<SecretProbeReport>,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshApprovalRequestView {
    pub request_id: String,
    pub tool_name: String,
    pub topic_id: String,
    pub target_name: String,
    pub summary: String,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshApprovalGrant {
    pub request_id: String,
    pub approval_token: String,
    pub tool_name: String,
    pub topic_id: String,
    pub target_name: String,
    pub summary: String,
    pub expires_at: i64,
}

#[derive(Clone, Default)]
pub struct SshApprovalRegistry {
    inner: Arc<Mutex<HashMap<String, ApprovalEntry>>>,
}

#[derive(Clone)]
struct ApprovalEntry {
    view: SshApprovalRequestView,
    fingerprint: String,
    state: ApprovalState,
    announced: bool,
}

#[derive(Clone)]
enum ApprovalState {
    Pending,
    Approved { token: String, expires_at: i64 },
    Rejected,
    Consumed,
}

impl SshApprovalRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register(
        &self,
        tool_name: &str,
        topic_id: &str,
        target_name: &str,
        summary: String,
        fingerprint: String,
    ) -> SshApprovalRequestView {
        let now = now_unix_secs();
        let view = SshApprovalRequestView {
            request_id: Uuid::new_v4().to_string(),
            tool_name: tool_name.to_string(),
            topic_id: topic_id.to_string(),
            target_name: target_name.to_string(),
            summary,
            created_at: now,
            expires_at: now + APPROVAL_TTL_SECS,
        };
        let entry = ApprovalEntry {
            view: view.clone(),
            fingerprint,
            state: ApprovalState::Pending,
            announced: false,
        };
        let mut guard = self.inner.lock().await;
        purge_expired_entries(&mut guard, now);
        guard.insert(view.request_id.clone(), entry);
        view
    }

    pub async fn take_unannounced(&self) -> Vec<SshApprovalRequestView> {
        let now = now_unix_secs();
        let mut guard = self.inner.lock().await;
        purge_expired_entries(&mut guard, now);
        let mut pending = Vec::new();
        for entry in guard.values_mut() {
            if !matches!(entry.state, ApprovalState::Pending) || entry.announced {
                continue;
            }
            entry.announced = true;
            pending.push(entry.view.clone());
        }
        pending.sort_by(|left, right| left.created_at.cmp(&right.created_at));
        pending
    }

    pub async fn grant(&self, request_id: &str) -> Option<SshApprovalGrant> {
        let now = now_unix_secs();
        let mut guard = self.inner.lock().await;
        purge_expired_entries(&mut guard, now);
        let entry = guard.get_mut(request_id)?;
        if !matches!(entry.state, ApprovalState::Pending) {
            return None;
        }
        let token = Uuid::new_v4().to_string();
        let expires_at = now + APPROVAL_TTL_SECS;
        entry.state = ApprovalState::Approved {
            token: token.clone(),
            expires_at,
        };
        Some(SshApprovalGrant {
            request_id: entry.view.request_id.clone(),
            approval_token: token,
            tool_name: entry.view.tool_name.clone(),
            topic_id: entry.view.topic_id.clone(),
            target_name: entry.view.target_name.clone(),
            summary: entry.view.summary.clone(),
            expires_at,
        })
    }

    pub async fn reject(&self, request_id: &str) -> Option<SshApprovalRequestView> {
        let now = now_unix_secs();
        let mut guard = self.inner.lock().await;
        purge_expired_entries(&mut guard, now);
        let entry = guard.get_mut(request_id)?;
        if !matches!(
            entry.state,
            ApprovalState::Pending | ApprovalState::Approved { .. }
        ) {
            return None;
        }
        entry.state = ApprovalState::Rejected;
        Some(entry.view.clone())
    }

    pub async fn consume(
        &self,
        request_id: &str,
        approval_token: &str,
        fingerprint: &str,
    ) -> Result<()> {
        let now = now_unix_secs();
        let mut guard = self.inner.lock().await;
        purge_expired_entries(&mut guard, now);
        let entry = guard
            .get_mut(request_id)
            .ok_or_else(|| anyhow!("approval request not found or expired"))?;
        if entry.fingerprint != fingerprint {
            bail!("approval token does not match the original SSH action");
        }
        match &entry.state {
            ApprovalState::Approved { token, expires_at } => {
                if token != approval_token {
                    bail!("approval token is invalid");
                }
                if *expires_at < now {
                    bail!("approval token has expired");
                }
                entry.state = ApprovalState::Consumed;
                Ok(())
            }
            ApprovalState::Pending => bail!("approval has not been granted yet"),
            ApprovalState::Rejected => bail!("approval request was rejected"),
            ApprovalState::Consumed => bail!("approval token has already been used"),
        }
    }
}

pub async fn probe_secret_ref(
    _storage: &Arc<dyn StorageProvider>,
    _user_id: i64,
    secret_ref: &str,
    kind: SecretProbeKind,
) -> SecretProbeReport {
    SecretProbeReport::invalid(
        secret_ref,
        secret_source(secret_ref),
        kind,
        "ssh mcp integration is not compiled".to_string(),
    )
}

pub async fn inspect_topic_infra_config(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    topic_id: &str,
    config: &TopicInfraConfigRecord,
) -> TopicInfraPreflightReport {
    let auth_secret = match config.auth_mode {
        TopicInfraAuthMode::None => None,
        TopicInfraAuthMode::Password => {
            let secret_ref = config.secret_ref.as_deref().unwrap_or("<unset>");
            Some(probe_secret_ref(storage, user_id, secret_ref, SecretProbeKind::Opaque).await)
        }
        TopicInfraAuthMode::PrivateKey => {
            let secret_ref = config.secret_ref.as_deref().unwrap_or("<unset>");
            Some(
                probe_secret_ref(storage, user_id, secret_ref, SecretProbeKind::SshPrivateKey)
                    .await,
            )
        }
    };
    let sudo_secret = match config.sudo_secret_ref.as_deref() {
        Some(secret_ref) => {
            Some(probe_secret_ref(storage, user_id, secret_ref, SecretProbeKind::Opaque).await)
        }
        None => None,
    };
    let auth_summary = auth_secret
        .as_ref()
        .map(SecretProbeReport::summary)
        .unwrap_or_else(|| "ssh mcp integration is not compiled".to_string());
    let summary = format!(
        "SSH target '{}' for topic '{}' uses {}@{}:{} with auth mode {:?}. Auth check: {}. ssh_mcp tools are disabled because integration-ssh-mcp is not compiled.",
        config.target_name,
        topic_id,
        config.remote_user,
        config.host,
        config.port,
        config.auth_mode,
        auth_summary,
    );
    TopicInfraPreflightReport {
        topic_id: topic_id.to_string(),
        target_name: config.target_name.clone(),
        host: config.host.clone(),
        port: config.port,
        remote_user: config.remote_user.clone(),
        auth_mode: config.auth_mode,
        provider_enabled: false,
        auth_secret,
        sudo_secret,
        summary,
    }
}

pub fn inject_approval_credentials(
    arguments: &str,
    request_id: &str,
    approval_token: &str,
) -> Result<String> {
    let mut value = serde_json::from_str::<serde_json::Value>(arguments)
        .map_err(|err| anyhow!("invalid approval replay payload: {err}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("approval replay payload must be a JSON object"))?;
    object.insert(
        "approval_request_id".to_string(),
        serde_json::Value::String(request_id.to_string()),
    );
    object.insert(
        "approval_token".to_string(),
        serde_json::Value::String(approval_token.to_string()),
    );
    serde_json::to_string(&value).map_err(Into::into)
}

pub fn inject_ssh_approval_system_message(grant: &SshApprovalGrant) -> AgentMessage {
    AgentMessage::approval_replay(format!(
        "A human operator approved the pending SSH action for target '{}' in topic '{}'. Retry the exact same SSH tool call and include approval_request_id='{}' and approval_token='{}'. Do not change any other tool arguments.",
        grant.target_name, grant.topic_id, grant.request_id, grant.approval_token
    ))
}

pub fn inject_topic_infra_preflight_system_message(
    report: &TopicInfraPreflightReport,
) -> AgentMessage {
    AgentMessage::infra_status(format!(
        "Topic-scoped SSH preflight status: {} Never request, reveal, or print the underlying secret material.",
        report.summary
    ))
}

fn purge_expired_entries(entries: &mut HashMap<String, ApprovalEntry>, now: i64) {
    entries.retain(|_, entry| match entry.state {
        ApprovalState::Pending => entry.view.expires_at >= now,
        ApprovalState::Approved { expires_at, .. } => expires_at >= now,
        ApprovalState::Rejected | ApprovalState::Consumed => false,
    });
}

fn now_unix_secs() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(_) => 0,
    }
}

fn secret_source(secret_ref: &str) -> &'static str {
    if secret_ref.strip_prefix("env:").is_some() {
        "env"
    } else if secret_ref.strip_prefix("storage:").is_some() {
        "storage"
    } else {
        "unknown"
    }
}
