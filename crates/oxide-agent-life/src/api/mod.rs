//! API contract namespace for future `/api/life/*` routes.

use serde::{Deserialize, Serialize};

use crate::domain::{MemoryGenerationId, PrincipalUserId};

/// Query params for inspecting a principal generation.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifeInspectorScope {
    /// Principal to inspect.
    pub principal_user_id: PrincipalUserId,
    /// Optional generation. If absent, routes must resolve the active pointer.
    pub memory_generation_id: Option<MemoryGenerationId>,
}
