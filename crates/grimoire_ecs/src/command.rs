//! Deferred world mutations.

#[cfg(debug_assertions)]
use std::any::{TypeId, type_name};
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

/// Kind of a recorded command, checked against the declaration of a parallel system.
#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug)]
pub(crate) enum CommandKind {
    /// `spawn`, `despawn`, `insert` or `remove`.
    Structural(&'static str),
    /// `set` of a component type.
    Component { type_id: TypeId, name: &'static str },
    /// `insert_resource` or `remove_resource` of a resource type.
    Resource { type_id: TypeId, name: &'static str },
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
    /// One entry per command, only for the debug access check.
    #[cfg(debug_assertions)]
    kinds: Vec<CommandKind>,
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
        self.record("spawn");
        self.commands
            .push(Command::Deferred(Box::new(move |world: &mut World| {
                world.spawn(bundle);
            })));
    }

    /// Records despawning `entity`.
    pub fn despawn(&mut self, entity: Entity) {
        self.record("despawn");
        self.commands.push(Command::Despawn(entity));
    }

    /// Records inserting `component` into `entity`.
    pub fn insert<C: Component>(&mut self, entity: Entity, component: C) {
        self.record("insert");
        self.commands
            .push(Command::Deferred(Box::new(move |world: &mut World| {
                // A dead target is skipped by contract; the error carries no other information.
                let _ = world.insert(entity, component);
            })));
    }

    /// Records removing component `C` from `entity`; the removed value is dropped.
    pub fn remove<C: Component>(&mut self, entity: Entity) {
        self.record("remove");
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
        #[cfg(debug_assertions)]
        self.kinds.push(CommandKind::Component {
            type_id: TypeId::of::<C>(),
            name: type_name::<C>(),
        });
        self.commands
            .push(Command::Deferred(Box::new(move |world: &mut World| {
                if let Some(slot) = world.get_mut::<C>(entity) {
                    *slot = value;
                }
            })));
    }

    /// Records inserting or replacing resource `R`, like [`World::insert_resource`].
    pub fn insert_resource<R: Resource>(&mut self, resource: R) {
        self.record_resource::<R>();
        self.commands
            .push(Command::Deferred(Box::new(move |world: &mut World| {
                world.insert_resource(resource);
            })));
    }

    /// Records removing resource `R`, like [`World::remove_resource`]; the removed value is
    /// dropped and a missing resource is skipped.
    pub fn remove_resource<R: Resource>(&mut self) {
        self.record_resource::<R>();
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
        #[cfg(debug_assertions)]
        self.kinds.append(&mut other.kinds);
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

    /// Discards all recorded commands without applying them.
    pub(crate) fn clear(&mut self) {
        self.commands.clear();
        #[cfg(debug_assertions)]
        self.kinds.clear();
    }

    /// Kinds of the recorded commands, in recorded order.
    #[cfg(debug_assertions)]
    pub(crate) fn kinds(&self) -> &[CommandKind] {
        &self.kinds
    }

    /// Records the kind of a structural command (debug builds only).
    #[cfg_attr(not(debug_assertions), allow(clippy::unused_self))]
    fn record(&mut self, _method: &'static str) {
        #[cfg(debug_assertions)]
        self.kinds.push(CommandKind::Structural(_method));
    }

    /// Records the kind of a resource command (debug builds only).
    #[cfg_attr(not(debug_assertions), allow(clippy::unused_self))]
    fn record_resource<R: Resource>(&mut self) {
        #[cfg(debug_assertions)]
        self.kinds.push(CommandKind::Resource {
            type_id: TypeId::of::<R>(),
            name: type_name::<R>(),
        });
    }

    /// Applies all recorded commands in order and leaves the buffer empty for reuse.
    pub fn apply(&mut self, world: &mut World) {
        #[cfg(debug_assertions)]
        self.kinds.clear();
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
