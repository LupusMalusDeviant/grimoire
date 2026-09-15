//! Deferred world mutations.

use std::fmt;

use crate::bundle::Bundle;
use crate::component::Component;
use crate::entity::Entity;
use crate::resource::Resource;
use crate::world::World;

type Deferred = Box<dyn FnOnce(&mut World) + Send>;

enum Command {
    Despawn(Entity),
    Deferred(Deferred),
}

/// Records world mutations to apply to a [`World`] later: spawns, despawns, inserts and removals
/// (structural commands), component replacements and resource changes.
///
/// Commands are applied in recorded order. Commands targeting entities that are not alive at
/// application time are skipped silently, exactly like the immediate [`World`] methods.
///
/// Systems of a parallel stage record into their own buffer, which the schedule applies after
/// the stage in list order (engine ADR-0006). Such a system declares
/// [`Access::structural`](crate::Access::structural) for `spawn`, `despawn`, `insert` and
/// `remove`, [`Access::write`](crate::Access::write) for [`CommandBuffer::set`] and
/// [`Access::write_resource`](crate::Access::write_resource) for the resource commands.
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
    ///
    /// # Panics
    ///
    /// If `bundle` contains the same component type more than once, like [`World::spawn`]. The
    /// check runs here, at recording time, so the buffer is unchanged when the panic is caught
    /// and [`CommandBuffer::apply`] never panics halfway through.
    pub fn spawn<B: Bundle>(&mut self, bundle: B) {
        crate::world::reject_duplicate_types::<B>();
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

    /// Records replacing the existing component `C` of `entity` with `value`.
    ///
    /// Skipped if `entity` is not alive or has no `C` at application time. Unlike
    /// [`CommandBuffer::insert`] it never adds a component and never moves the entity to another
    /// archetype, so it is not a structural command.
    pub fn set<C: Component>(&mut self, entity: Entity, value: C) {
        self.commands
            .push(Command::Deferred(Box::new(move |world: &mut World| {
                if let Some(slot) = world.get_mut::<C>(entity) {
                    *slot = value;
                }
            })));
    }

    /// Records inserting or replacing resource `R`, like [`World::insert_resource`].
    pub fn insert_resource<R: Resource>(&mut self, resource: R) {
        self.commands
            .push(Command::Deferred(Box::new(move |world: &mut World| {
                world.insert_resource(resource);
            })));
    }

    /// Records removing resource `R`, like [`World::remove_resource`]; the removed value is
    /// dropped and a missing resource is skipped.
    pub fn remove_resource<R: Resource>(&mut self) {
        self.commands
            .push(Command::Deferred(Box::new(|world: &mut World| {
                world.remove_resource::<R>();
            })));
    }

    /// Moves all commands of `other` to the end of this buffer, keeping their order; `other` is
    /// left empty.
    ///
    /// Useful to collect buffers recorded per data-parallel block in block order.
    pub fn append(&mut self, other: &mut CommandBuffer) {
        self.commands.append(&mut other.commands);
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
