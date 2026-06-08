#![allow(missing_docs)]

//! SSH MCP support types used when the upstream MCP client is not compiled.

use crate::agent::memory::AgentMessage;
use crate::storage::{StorageProvider, TopicInfraAuthMode, TopicInfraConfigRecord};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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

pub fn inject_topic_infra_preflight_system_message(
    report: &TopicInfraPreflightReport,
) -> AgentMessage {
    AgentMessage::infra_status(format!(
        "Topic-scoped SSH preflight status: {} Never request, reveal, or print the underlying secret material.",
        report.summary
    ))
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
