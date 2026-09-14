//! Systems and the ordered, single-threaded schedule (engine ADR-0003).

use std::fmt;

use crate::world::World;

/// A unit of simulation logic run by a [`Schedule`].
pub trait System {
    /// Human-readable name for diagnostics and ordering checks.
    fn name(&self) -> &str;
    /// Runs the system once against `world`.
    fn run(&mut self, world: &mut World);
}

struct FnSystem<F> {
    name: &'static str,
    function: F,
}

impl<F: FnMut(&mut World) + Send + 'static> System for FnSystem<F> {
    fn name(&self) -> &str {
        self.name
    }

    fn run(&mut self, world: &mut World) {
        (self.function)(world);
    }
}

/// Wraps a closure as a named [`System`].
pub fn system_fn<F: FnMut(&mut World) + Send + 'static>(name: &'static str, f: F) -> impl System {
    FnSystem { name, function: f }
}

/// Ordered list of systems executed one after another on the calling thread.
///
/// Systems run exactly in the order they were added; there is no implicit reordering.
#[derive(Default)]
pub struct Schedule {
    systems: Vec<Box<dyn System>>,
}

impl fmt::Debug for Schedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Schedule")
            .field("systems", &self.system_names())
            .finish()
    }
}

impl Schedule {
    /// Creates an empty schedule.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `system` after all previously added systems.
    pub fn add_system(&mut self, system: impl System + 'static) -> &mut Self {
        self.systems.push(Box::new(system));
        self
    }

    /// Runs every system once, in insertion order.
    pub fn run(&mut self, world: &mut World) {
        for system in &mut self.systems {
            system.run(world);
        }
    }

    /// Names of all systems in execution order.
    #[must_use]
    pub fn system_names(&self) -> Vec<&str> {
        self.systems.iter().map(|system| system.name()).collect()
    }

    /// Number of systems.
    #[must_use]
    pub fn len(&self) -> usize {
        self.systems.len()
    }

    /// Whether the schedule contains no systems.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }
}
