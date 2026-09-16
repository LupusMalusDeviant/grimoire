//! The fixed stage pass graph (contract §6 "Ebenenreihenfolge", plan 0002 WP3.5, PRD-0003 FR-10).
//!
//! [`run`] is the one place that decides in which order the stage renderer executes its layers:
//! it walks [`RenderLayer::ORDER`] and nothing else, calls the caller's per-layer closure once per
//! slot, and records every slot it actually executed in a [`PassLog`]. A renderer never loops over
//! layers on its own, so the protocol it exposes (`WgpuRenderer::last_stage_pass_order`) is the
//! order its GPU passes were submitted in, not a second list that merely claims to match.
//!
//! The slots, earliest first (contract §6):
//!
//! | Slot | Draws in P1 |
//! |------|-------------|
//! | [`RenderLayer::World`] | layers 1–3: meshes (depth-tested, clears), then world sprites |
//! | [`RenderLayer::Vfx`] | nothing — empty slot, reserved for particles |
//! | [`RenderLayer::PostFxResolve`] | nothing — empty slot; every post effect resolves here, before layer 4 |
//! | [`RenderLayer::Telegraphy`] | nothing — layer 4, reserved, P1 has no channel |
//! | [`RenderLayer::Bullets`] | layer 6: the bullet pass, glow included |
//! | [`RenderLayer::PlayerMarker`] | layer 7: marker sprites |
//! | [`RenderLayer::DebugUi`] | debug and UI sprites |
//!
//! Because the post-processing resolve slot precedes telegraphy and bullets, no post effect can
//! tint, blur or occlude layers 4 and 6 (PRD-0003 rule 1): there is simply no slot after them that
//! could host one.

use crate::RenderLayer;

/// Number of slots in the pass graph, one per [`RenderLayer`].
const SLOT_COUNT: usize = RenderLayer::ORDER.len();

/// The slots [`run`] executed, in execution order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PassLog {
    layers: [RenderLayer; SLOT_COUNT],
    len: usize,
}

impl Default for PassLog {
    fn default() -> Self {
        Self {
            layers: [RenderLayer::World; SLOT_COUNT],
            len: 0,
        }
    }
}

impl PassLog {
    /// The executed slots, earliest first. Empty for a frame that skipped rendering (zero size).
    pub(crate) fn layers(&self) -> &[RenderLayer] {
        &self.layers[..self.len]
    }

    fn push(&mut self, layer: RenderLayer) {
        // `run` pushes at most once per entry of `RenderLayer::ORDER`, so this never overflows;
        // `get_mut` keeps even a violated invariant panic-free (contract §2 rule 6).
        if let Some(slot) = self.layers.get_mut(self.len) {
            *slot = layer;
            self.len += 1;
        }
    }
}

/// Executes the stage pass graph: calls `execute` once for every slot of [`RenderLayer::ORDER`],
/// in that order, and returns the log of executed slots.
///
/// An error from one slot stops the graph: later slots do not run, and the error is returned
/// unchanged (a failed GPU submission must not be followed by passes drawing on top of a
/// half-finished frame).
///
/// # Errors
/// The first error `execute` returns.
pub(crate) fn run<E>(mut execute: impl FnMut(RenderLayer) -> Result<(), E>) -> Result<PassLog, E> {
    let mut log = PassLog::default();
    for layer in RenderLayer::ORDER {
        execute(layer)?;
        log.push(layer);
    }
    Ok(log)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(log: &PassLog, layer: RenderLayer) -> usize {
        log.layers()
            .iter()
            .position(|&executed| executed == layer)
            .unwrap_or_else(|| panic!("{layer:?} never executed"))
    }

    #[test]
    fn executed_order_is_render_layer_order() {
        let mut seen = Vec::new();
        let log = run(|layer| {
            seen.push(layer);
            Ok::<(), ()>(())
        })
        .expect("no slot fails");
        assert_eq!(log.layers(), RenderLayer::ORDER.as_slice());
        assert_eq!(
            seen.as_slice(),
            log.layers(),
            "the log records exactly the calls made"
        );
    }

    #[test]
    fn telegraphy_and_bullets_run_after_the_post_fx_resolve_and_in_that_order() {
        // PRD-0003 FR-10 / rule 1, structurally: layer 4 (telegraphy) before layer 6 (bullets),
        // both after the (empty) post-processing resolve slot, and nothing post-processing-like
        // after them — only the player marker and debug/UI.
        let log = run(|_| Ok::<(), ()>(())).expect("no slot fails");
        let resolve = position(&log, RenderLayer::PostFxResolve);
        let telegraphy = position(&log, RenderLayer::Telegraphy);
        let bullets = position(&log, RenderLayer::Bullets);
        let marker = position(&log, RenderLayer::PlayerMarker);
        let debug = position(&log, RenderLayer::DebugUi);
        assert!(position(&log, RenderLayer::World) < position(&log, RenderLayer::Vfx));
        assert!(position(&log, RenderLayer::Vfx) < resolve);
        assert!(resolve < telegraphy, "post-FX resolves before layer 4");
        assert!(telegraphy < bullets, "layer 4 is drawn before layer 6");
        assert_eq!(bullets + 1, marker, "layer 7 follows layer 6 directly");
        assert_eq!(marker + 1, debug);
        assert_eq!(debug, SLOT_COUNT - 1, "debug/UI is the last slot");
    }

    #[test]
    fn a_failing_slot_stops_the_graph() {
        let mut seen = Vec::new();
        let result = run(|layer| {
            seen.push(layer);
            if layer == RenderLayer::Telegraphy {
                Err("telegraphy failed")
            } else {
                Ok(())
            }
        });
        assert_eq!(result, Err("telegraphy failed"));
        assert_eq!(
            seen.last(),
            Some(&RenderLayer::Telegraphy),
            "no slot after the failing one ran"
        );
        assert!(!seen.contains(&RenderLayer::Bullets));
    }

    #[test]
    fn default_log_is_empty() {
        assert!(PassLog::default().layers().is_empty());
    }
}
