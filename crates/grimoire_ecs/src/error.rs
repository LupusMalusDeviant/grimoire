//! Error type of the crate.

use crate::entity::Entity;

/// Errors reported by fallible [`World`](crate::World) operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EcsError {
    /// The entity was never spawned, has been despawned or the handle has a stale generation.
    #[error("entity {0} does not exist")]
    NoSuchEntity(Entity),
}

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
