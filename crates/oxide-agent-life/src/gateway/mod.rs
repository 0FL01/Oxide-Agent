//! Transport-neutral life gateway contracts.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{
    InputId, LifeIdentityProvider, MemoryScope, PrincipalUserId, ProviderSubject, RunId, TurnId,
};

/// Narrow submit contract used by Web/Telegram transports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifeInputSubmission {
    /// Provider namespace.
    pub provider: LifeIdentityProvider,
    /// Provider-local subject.
    pub provider_subject: ProviderSubject,
    /// User content.
    pub content: String,
    /// Attachment references.
    pub attachments: Value,
    /// Transport metadata.
    pub metadata: Value,
}

/// Submit result returned after canonical turn/input creation.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitLifeInputResult {
    /// Resolved principal.
    pub principal_user_id: PrincipalUserId,
    /// Active memory scope used for queue/run decisions.
    pub memory_scope: MemoryScope,
    /// Canonical user turn id.
    pub turn_id: TurnId,
    /// Queued input id.
    pub input_id: InputId,
    /// Attached or created run id.
    pub run_id: Option<RunId>,
}
