//! Permanent Life Mode bounded context.
//!
//! This crate owns life-mode domain contracts. It is intentionally separate
//! from core execution and transport crates so Web/Telegram can submit narrow
//! life inputs without owning memory semantics.

pub mod api;
pub mod config;
pub mod context;
pub mod curator;
pub mod domain;
pub mod engram;
pub mod errors;
pub mod gateway;
pub mod linking;
pub mod storage;
pub mod worker;

pub use config::LifeConfig;
pub use errors::{LifeDomainError, LifeResult};

#[cfg(test)]
mod tests {
    use crate::domain::{
        GenerationScoped, LifeIdentityProvider, LifeMemoryGeneration, MemoryGenerationId,
        MemoryGenerationStatus, MemoryScope, PrincipalUserId, TimestampMillis,
    };

    #[test]
    fn provider_wire_values_are_canonical() {
        assert_eq!(LifeIdentityProvider::Web.as_str(), "web");
        assert_eq!(LifeIdentityProvider::Telegram.as_str(), "telegram");
        assert_eq!(
            "web".parse::<LifeIdentityProvider>().expect("web provider"),
            LifeIdentityProvider::Web
        );
        assert!("forum".parse::<LifeIdentityProvider>().is_err());
    }

    #[test]
    fn principal_id_rejects_non_positive_values() {
        assert!(PrincipalUserId::new(1).is_ok());
        assert!(PrincipalUserId::new(0).is_err());
        assert!(PrincipalUserId::new(-1).is_err());
    }

    #[test]
    fn generation_scoped_records_require_active_scope() {
        let principal = PrincipalUserId::new(100500).expect("positive principal");
        let active_generation = MemoryGenerationId::new_v4();
        let stale_generation = MemoryGenerationId::new_v4();
        let active_scope = MemoryScope::new(principal, active_generation);
        let stale_generation_record = LifeMemoryGeneration {
            memory_generation_id: stale_generation,
            principal_user_id: principal,
            generation_number: 1,
            status: MemoryGenerationStatus::Archived,
            source_generation_id: Some(active_generation),
            build_reason: "test rebuild".to_owned(),
            build_policy: serde_json::json!({"policy": "old"}),
            source_scope: serde_json::json!({"turns": "all"}),
            comparison_report: serde_json::json!({}),
            activated_at: None,
            created_at: TimestampMillis::new(1),
            updated_at: TimestampMillis::new(2),
        };

        assert!(
            stale_generation_record
                .assert_in_scope(&active_scope)
                .is_err()
        );
    }
}
