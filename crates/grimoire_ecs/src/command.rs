//! Deferred world mutations.

use std::fmt;

use crate::bundle::Bundle;
use crate::component::Component;
use crate::entity::Entity;
use crate::world::World;

type Deferred = Box<dyn FnOnce(&mut World) + Send>;

enum Command {
    Despawn(Entity),
    Deferred(Deferred),
}

/// Records spawns, despawns, inserts and removals to apply to a [`World`] later.
///
/// Commands are applied in recorded order. Commands targeting entities that are not alive at
/// application time are skipped silently, exactly like the immediate [`World`] methods.
#[derive(Default)]
pub struct CommandBuffer {
    commands: Vec<Command>,
}

impl fmt::Debug for CommandBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandBuffer")
            .field("commands", &self.commands.len())
            .finish()
    }
}

impl CommandBuffer {
    /// Creates an empty buffer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records spawning an entity with `bundle`.
    pub fn spawn<B: Bundle>(&mut self, bundle: B) {
        self.commands
            .push(Command::Deferred(Box::new(move |world: &mut World| {
                world.spawn(bundle);
            })));
    }

    /// Records despawning `entity`.
    pub fn despawn(&mut self, entity: Entity) {
        self.commands.push(Command::Despawn(entity));
    }

    /// Records inserting `component` into `entity`.
    pub fn insert<C: Component>(&mut self, entity: Entity, component: C) {
        self.commands
            .push(Command::Deferred(Box::new(move |world: &mut World| {
                // A dead target is skipped by contract; the error carries no other information.
                let _ = world.insert(entity, component);
            })));
    }

    /// Records removing component `C` from `entity`; the removed value is dropped.
    pub fn remove<C: Component>(&mut self, entity: Entity) {
        self.commands
            .push(Command::Deferred(Box::new(move |world: &mut World| {
                world.remove::<C>(entity);
            })));
    }

    /// Number of recorded commands.
    #[must_use]
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Whether no commands are recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Applies all recorded commands in order and leaves the buffer empty for reuse.
    pub fn apply(&mut self, world: &mut World) {
        for command in self.commands.drain(..) {
            match command {
                Command::Despawn(entity) => {
                    world.despawn(entity);
                }
                Command::Deferred(apply) => apply(world),
            }
        }
    }
}
