//! Error type of the crate.

use std::fmt;

use crate::entity::Entity;

/// Errors reported by fallible [`World`](crate::World) operations.
///
/// `Display`/`Error` are implemented by hand with the same shape a `thiserror` derive would
/// produce, because `grimoire_ecs` does not depend on `thiserror` yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EcsError {
    /// The entity was never spawned, has been despawned or the handle has a stale generation.
    NoSuchEntity(Entity),
}

impl fmt::Display for EcsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchEntity(entity) => write!(f, "entity {entity} does not exist"),
        }
    }
}

impl std::error::Error for EcsError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_entity() {
        let error = EcsError::NoSuchEntity(Entity::from_bits((2 << 32) | 5));
        assert_eq!(error.to_string(), "entity 5v2 does not exist");
        let boxed: Box<dyn std::error::Error> = Box::new(error);
        assert!(boxed.source().is_none());
    }
}
