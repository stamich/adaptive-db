//! Poisoning: once in-memory state may diverge from the log, refuse to continue.
use parking_lot::Mutex;

use crate::DbError;

/// Tracks whether the instance is still trustworthy.
#[derive(Debug, Default)]
pub struct EngineHealth {
    poisoned: Mutex<Option<String>>,
}

impl EngineHealth {
    /// Fails if the instance is poisoned.
    pub fn check(&self) -> Result<(), DbError> {
        match self.poisoned.lock().as_ref() {
            Some(reason) => Err(DbError::Poisoned(reason.clone())),
            None => Ok(()),
        }
    }

    /// Whether the instance is poisoned.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.lock().is_some()
    }

    /// Runs a step whose failure leaves state undefined; any error poisons the instance.
    pub fn guard<T>(&self, step: impl FnOnce() -> Result<T, DbError>) -> Result<T, String> {
        step().map_err(|error| {
            let reason = error.to_string();
            self.poisoned.lock().get_or_insert_with(|| reason.clone());
            reason
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_guarded_step_poisons_and_keeps_the_first_reason() {
        let health = EngineHealth::default();
        assert!(health.check().is_ok());
        assert_eq!(health.guard(|| Ok(5)).unwrap(), 5);
        assert!(health
            .guard::<()>(|| Err(DbError::InvalidArgument("disk full".into())))
            .is_err());
        assert!(health
            .guard::<()>(|| Err(DbError::InvalidArgument("second".into())))
            .is_err());
        assert!(health.is_poisoned());
        match health.check() {
            Err(DbError::Poisoned(reason)) => assert!(reason.contains("disk full")),
            other => panic!("unexpected {other:?}"),
        }
    }
}
