//! Monitor placement and focus behaviour of the desktop window.
//!
//! The selection logic is free of winit so it can be tested without a display.

use winit::dpi::PhysicalPosition;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowAttributes;

use crate::window::{MonitorChoice, WindowConfig};

/// Geometry of one monitor in physical desktop coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MonitorRect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) scale_factor: f64,
}

/// Adds focus behaviour and monitor placement (including environment overrides) to `attributes`.
pub(crate) fn apply_placement(
    attributes: WindowAttributes,
    event_loop: &ActiveEventLoop,
    config: &WindowConfig,
) -> WindowAttributes {
    let config = config.clone().with_env_overrides();
    let mut attributes = attributes.with_active(config.focus_on_open);
    if config.monitor == MonitorChoice::Default {
        return attributes;
    }

    let handles: Vec<_> = event_loop.available_monitors().collect();
    let rects: Vec<MonitorRect> = handles
        .iter()
        .map(|handle| {
            let position = handle.position();
            let size = handle.size();
            MonitorRect {
                x: position.x,
                y: position.y,
                width: size.width,
                height: size.height,
                scale_factor: handle.scale_factor(),
            }
        })
        .collect();
    let primary = event_loop
        .primary_monitor()
        .and_then(|primary| handles.iter().position(|handle| *handle == primary));

    match select_monitor(config.monitor, &rects, primary) {
        Some(index) => {
            let (x, y) = centered_position(rects[index], config.width, config.height);
            attributes = attributes.with_position(PhysicalPosition::new(x, y));
        }
        None => log::warn!(
            "monitor {:?} is not available ({} monitors, primary {:?}); the OS places the window",
            config.monitor,
            rects.len(),
            primary
        ),
    }
    attributes
}

/// Index of the monitor matching `choice`, or `None` if the operating system should decide.
pub(crate) fn select_monitor(
    choice: MonitorChoice,
    monitors: &[MonitorRect],
    primary: Option<usize>,
) -> Option<usize> {
    match choice {
        MonitorChoice::Default => None,
        MonitorChoice::Primary => primary.filter(|&index| index < monitors.len()),
        MonitorChoice::Secondary => {
            let primary = primary.filter(|&index| index < monitors.len())?;
            (0..monitors.len()).find(|&index| index != primary)
        }
        MonitorChoice::Index(index) => (index < monitors.len()).then_some(index),
    }
}

/// Top-left position that centres a window of the given logical size on `monitor`. A window
/// larger than the monitor is aligned to the monitor's top-left corner.
pub(crate) fn centered_position(
    monitor: MonitorRect,
    logical_width: u32,
    logical_height: u32,
) -> (i32, i32) {
    let scale = if monitor.scale_factor.is_finite() && monitor.scale_factor > 0.0 {
        monitor.scale_factor
    } else {
        1.0
    };
    let width = (f64::from(logical_width) * scale).round();
    let height = (f64::from(logical_height) * scale).round();
    let free_x = (f64::from(monitor.width) - width).max(0.0);
    let free_y = (f64::from(monitor.height) - height).max(0.0);
    (
        monitor.x + (free_x / 2.0) as i32,
        monitor.y + (free_y / 2.0) as i32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: MonitorRect = MonitorRect {
        x: 0,
        y: 0,
        width: 2560,
        height: 1440,
        scale_factor: 1.0,
    };
    const RIGHT: MonitorRect = MonitorRect {
        x: 2560,
        y: 0,
        width: 2560,
        height: 1440,
        scale_factor: 1.0,
    };

    #[test]
    fn default_lets_the_os_decide() {
        assert_eq!(
            select_monitor(MonitorChoice::Default, &[LEFT, RIGHT], Some(0)),
            None
        );
    }

    #[test]
    fn secondary_is_the_first_non_primary_monitor() {
        assert_eq!(
            select_monitor(MonitorChoice::Secondary, &[LEFT, RIGHT], Some(0)),
            Some(1)
        );
        assert_eq!(
            select_monitor(MonitorChoice::Secondary, &[LEFT, RIGHT], Some(1)),
            Some(0)
        );
    }

    #[test]
    fn secondary_needs_a_second_monitor_and_a_known_primary() {
        assert_eq!(
            select_monitor(MonitorChoice::Secondary, &[LEFT], Some(0)),
            None
        );
        assert_eq!(
            select_monitor(MonitorChoice::Secondary, &[LEFT, RIGHT], None),
            None
        );
    }

    #[test]
    fn primary_and_index_respect_bounds() {
        assert_eq!(
            select_monitor(MonitorChoice::Primary, &[LEFT, RIGHT], Some(0)),
            Some(0)
        );
        assert_eq!(
            select_monitor(MonitorChoice::Primary, &[LEFT, RIGHT], None),
            None
        );
        assert_eq!(
            select_monitor(MonitorChoice::Index(1), &[LEFT, RIGHT], None),
            Some(1)
        );
        assert_eq!(
            select_monitor(MonitorChoice::Index(2), &[LEFT, RIGHT], None),
            None
        );
    }

    #[test]
    fn centres_on_the_selected_monitor() {
        assert_eq!(centered_position(RIGHT, 1280, 720), (3200, 360));
    }

    #[test]
    fn centring_uses_the_monitor_scale_factor() {
        let hidpi = MonitorRect {
            scale_factor: 2.0,
            ..RIGHT
        };
        assert_eq!(centered_position(hidpi, 1000, 500), (2840, 220));
    }

    #[test]
    fn oversized_windows_align_to_the_top_left() {
        assert_eq!(centered_position(RIGHT, 4000, 3000), (2560, 0));
        let broken_scale = MonitorRect {
            scale_factor: f64::NAN,
            ..LEFT
        };
        assert_eq!(centered_position(broken_scale, 1280, 720), (640, 360));
    }
}
