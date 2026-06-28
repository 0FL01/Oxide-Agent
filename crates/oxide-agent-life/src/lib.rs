//! Permanent Life Mode bounded context.
//!
//! This crate owns life-mode domain contracts. It is intentionally separate
//! from core execution and transport crates so Web/Telegram can submit narrow
//! life inputs without owning memory semantics.

pub mod config;
pub mod domain;
pub mod errors;
pub mod gateway;
pub mod runtime;
pub mod storage;
pub mod worker;

pub use config::LifeConfig;
pub use errors::{LifeDomainError, LifeResult};

#[cfg(test)]
mod tests {
    use crate::domain::{LifeIdentityProvider, PrincipalUserId};

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
}
