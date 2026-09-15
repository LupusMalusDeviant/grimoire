//! Debug-build check that parallel systems stay within their declared [`Access`] (engine
//! ADR-0006, building block 1). The whole module exists only with `debug_assertions`.
//!
//! While a parallel system runs, its context is on a thread-local stack. Data-parallel blocks
//! capture the context of the calling thread and enter it on whichever thread runs the block, so
//! reads inside blocks on pool workers are checked too. Without a context (exclusive systems,
//! code outside a schedule) every hook is a no-op.
//!
//! Blocks enter the absence of a context as well. A pool worker that waits inside a parallel
//! system may run a block of another caller, for example of a second world sharing the pool;
//! the pushed `None` hides the waiting system's context from that block.

use std::any::{TypeId, type_name};
use std::cell::RefCell;
use std::sync::Arc;

use crate::access::Access;
use crate::command::{CommandBuffer, CommandKind};
use crate::component::Component;
use crate::query::internal::QueryInternal;
use crate::resource::Resource;

/// Name and declaration of the parallel system being checked.
pub(crate) struct AccessContext {
    pub(crate) system: String,
    pub(crate) access: Access,
}

thread_local! {
    static CONTEXTS: RefCell<Vec<Option<Arc<AccessContext>>>> = const { RefCell::new(Vec::new()) };
}

/// Pops the context entered by [`enter`] when dropped, also while unwinding.
pub(crate) struct ContextGuard(());

impl Drop for ContextGuard {
    fn drop(&mut self) {
        // `try_with`: the guard may be dropped while thread-locals are torn down.
        let _ = CONTEXTS.try_with(|contexts| contexts.borrow_mut().pop());
    }
}

/// Makes `context` the current context of this thread until the guard is dropped; `None` hides
/// any context entered earlier on this thread.
pub(crate) fn enter(context: Option<Arc<AccessContext>>) -> ContextGuard {
    CONTEXTS.with(|contexts| contexts.borrow_mut().push(context));
    ContextGuard(())
}

/// The current context of this thread, if a parallel system or one of its blocks runs here.
pub(crate) fn current() -> Option<Arc<AccessContext>> {
    CONTEXTS.with(|contexts| contexts.borrow().last().cloned().flatten())
}

/// Every component element of query `Q` (`&T`, `Option<&T>`) must be declared with `read`.
pub(crate) fn check_query<Q: QueryInternal>(via: &str) {
    let Some(context) = current() else { return };
    Q::for_each_access(&mut |access| {
        if !context.access.has_component(access.type_id, false) {
            panic!(
                "system `{}` reads component `{}` without declaring it (Access::read, World::{via})",
                context.system, access.name
            );
        }
    });
}

/// Component `C` must be declared with `read`.
pub(crate) fn check_component_read<C: Component>(via: &str) {
    let Some(context) = current() else { return };
    if !context.access.has_component(TypeId::of::<C>(), false) {
        panic!(
            "system `{}` reads component `{}` without declaring it (Access::read, World::{via})",
            context.system,
            type_name::<C>()
        );
    }
}

/// Resource `R` must be declared with `read_resource`.
pub(crate) fn check_resource_read<R: Resource>() {
    let Some(context) = current() else { return };
    if !context.access.has_resource(TypeId::of::<R>(), false) {
        panic!(
            "system `{}` reads resource `{}` without declaring it (Access::read_resource)",
            context.system,
            type_name::<R>()
        );
    }
}

/// Reading the whole world is never covered by a declaration.
pub(crate) fn check_whole_world(via: &str) {
    if let Some(context) = current() {
        panic!(
            "system `{}` reads the whole world (World::{via})",
            context.system
        );
    }
}

/// Checks the recorded commands of a parallel system against its declaration, before any apply.
pub(crate) fn validate_commands(context: &AccessContext, commands: &CommandBuffer) {
    for kind in commands.kinds() {
        match *kind {
            CommandKind::Structural(method) if !context.access.structural => panic!(
                "system `{}` records CommandBuffer::{method} without declaring structural commands (Access::structural)",
                context.system
            ),
            CommandKind::Component { type_id, name }
                if !context.access.has_component(type_id, true) =>
            {
                panic!(
                    "system `{}` writes component `{name}` without declaring it (Access::write)",
                    context.system
                )
            }
            CommandKind::Resource { type_id, name }
                if !context.access.has_resource(type_id, true) =>
            {
                panic!(
                    "system `{}` writes resource `{name}` without declaring it (Access::write_resource)",
                    context.system
                )
            }
            _ => {}
        }
    }
}
