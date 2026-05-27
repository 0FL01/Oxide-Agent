use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PublicConfigResponse {
    pub registration_enabled: bool,
    pub bootstrap_required: bool,
    pub build_version: String,
}
