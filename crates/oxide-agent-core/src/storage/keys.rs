use uuid::Uuid;

/// Generates a new random flow UUID (v4).
#[must_use]
pub fn generate_flow_id() -> String {
    Uuid::new_v4().to_string()
}
