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
    use crate::domain::{
        LifeTransportId, PrincipalUserId, TELEGRAM_TRANSPORT_ID, WEB_TRANSPORT_ID,
    };

    #[test]
    fn transport_ids_are_open_but_non_empty() {
        assert_eq!(
            LifeTransportId::new(WEB_TRANSPORT_ID)
                .expect("web transport")
                .as_str(),
            "web"
        );
        assert_eq!(
            LifeTransportId::new(TELEGRAM_TRANSPORT_ID)
                .expect("telegram transport")
                .as_str(),
            "telegram"
        );
        assert_eq!(
            LifeTransportId::new("linux")
                .expect("future transport")
                .as_str(),
            "linux"
        );
        assert!(LifeTransportId::new(" ").is_err());
    }

    #[test]
    fn principal_id_rejects_non_positive_values() {
        assert!(PrincipalUserId::new(1).is_ok());
        assert!(PrincipalUserId::new(0).is_err());
        assert!(PrincipalUserId::new(-1).is_err());
    }
}
