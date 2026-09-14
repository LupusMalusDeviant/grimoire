//! Native window contract consumed by the GPU layer.

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

/// Size of a drawable area in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PhysicalSize {
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
}

impl PhysicalSize {
    /// Creates a size from width and height.
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Returns `true` if either dimension is zero (e.g. a minimised window).
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// Environment variable that overrides [`WindowConfig::monitor`] for every window opened by
/// [`crate::run_desktop`]: `default`, `primary`, `secondary` or a zero-based monitor index.
/// Invalid values are ignored.
pub const ENV_WINDOW_MONITOR: &str = "GRIMOIRE_WINDOW_MONITOR";

/// Environment variable that overrides [`WindowConfig::focus_on_open`]: `1`/`true` or
/// `0`/`false`. Invalid values are ignored.
pub const ENV_WINDOW_FOCUS: &str = "GRIMOIRE_WINDOW_FOCUS";

/// Monitor a window opens on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MonitorChoice {
    /// Let the operating system place the window.
    #[default]
    Default,
    /// The primary monitor.
    Primary,
    /// The first monitor that is not the primary one. Falls back to [`MonitorChoice::Default`]
    /// when there is none or the platform cannot tell which monitor is primary (Wayland).
    Secondary,
    /// A monitor by its zero-based index in the platform's monitor list. Out-of-range indices
    /// fall back to [`MonitorChoice::Default`].
    Index(usize),
}

impl MonitorChoice {
    /// Parses `default`, `primary`, `secondary` (case-insensitive) or a zero-based index.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        match value.as_str() {
            "default" => Some(Self::Default),
            "primary" => Some(Self::Primary),
            "secondary" => Some(Self::Secondary),
            other => other.parse::<usize>().ok().map(Self::Index),
        }
    }
}

/// Parameters for creating the main window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowConfig {
    /// Window title.
    pub title: String,
    /// Initial inner width in logical pixels.
    pub width: u32,
    /// Initial inner height in logical pixels.
    pub height: u32,
    /// Whether the user may resize the window.
    pub resizable: bool,
    /// Monitor the window opens on, centred. Ignored where the platform does not allow
    /// positioning windows (Wayland).
    pub monitor: MonitorChoice,
    /// Whether the window takes keyboard focus when it opens. `false` keeps the currently
    /// focused application in front, e.g. a game running full screen on another monitor.
    pub focus_on_open: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: String::from("Grimoire"),
            width: 1280,
            height: 720,
            resizable: true,
            monitor: MonitorChoice::Default,
            focus_on_open: true,
        }
    }
}

impl WindowConfig {
    /// Applies [`ENV_WINDOW_MONITOR`] and [`ENV_WINDOW_FOCUS`] from the process environment.
    #[must_use]
    pub fn with_env_overrides(self) -> Self {
        let monitor = std::env::var(ENV_WINDOW_MONITOR).ok();
        let focus = std::env::var(ENV_WINDOW_FOCUS).ok();
        self.with_overrides(monitor.as_deref(), focus.as_deref())
    }

    /// Applies override values as they would come from the environment. `None` and invalid
    /// values keep the configured setting.
    #[must_use]
    pub fn with_overrides(mut self, monitor: Option<&str>, focus: Option<&str>) -> Self {
        if let Some(choice) = monitor.and_then(MonitorChoice::parse) {
            self.monitor = choice;
        }
        match focus
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("1" | "true") => self.focus_on_open = true,
            Some("0" | "false") => self.focus_on_open = false,
            _ => {}
        }
        self
    }
}

/// A native window a GPU surface can be created for.
///
/// Contract:
/// - Implementations are shared as `Arc<dyn PlatformWindow>` and must be thread-safe.
/// - The GPU layer relies only on the raw handles and [`PlatformWindow::inner_size`];
///   everything else is presentation glue.
/// - The handles stay valid for as long as any `Arc` to the window is alive.
///
/// Platform notes for the desktop implementation:
/// - **macOS:** the raw handles are only available on the main thread (other threads get
///   [`raw_window_handle::HandleError::Unavailable`]), so GPU surfaces must be created inside
///   the [`crate::AppHandler`] callbacks. Calls from other threads are forwarded to the main
///   thread and can block while it is busy.
/// - **Wayland:** redraws are not throttled to the compositor's frame callbacks; frame pacing
///   comes from presenting with vsync (FIFO).
pub trait PlatformWindow: HasWindowHandle + HasDisplayHandle + Send + Sync {
    /// Current drawable size in physical pixels. May be empty while minimised.
    fn inner_size(&self) -> PhysicalSize;

    /// Ratio of physical to logical pixels.
    fn scale_factor(&self) -> f64;

    /// Replaces the window title.
    fn set_title(&self, title: &str);

    /// Asks the platform to schedule another frame.
    fn request_redraw(&self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_monitor_choices() {
        assert_eq!(
            MonitorChoice::parse("secondary"),
            Some(MonitorChoice::Secondary)
        );
        assert_eq!(
            MonitorChoice::parse(" Primary "),
            Some(MonitorChoice::Primary)
        );
        assert_eq!(
            MonitorChoice::parse("DEFAULT"),
            Some(MonitorChoice::Default)
        );
        assert_eq!(MonitorChoice::parse("1"), Some(MonitorChoice::Index(1)));
        assert_eq!(MonitorChoice::parse(""), None);
        assert_eq!(MonitorChoice::parse("left"), None);
        assert_eq!(MonitorChoice::parse("-1"), None);
    }

    #[test]
    fn overrides_replace_only_valid_values() {
        let config = WindowConfig::default().with_overrides(Some("secondary"), Some("0"));
        assert_eq!(config.monitor, MonitorChoice::Secondary);
        assert!(!config.focus_on_open);

        let unchanged = config
            .clone()
            .with_overrides(Some("nonsense"), Some("maybe"));
        assert_eq!(unchanged, config);

        let restored = config.with_overrides(None, Some("TRUE"));
        assert_eq!(restored.monitor, MonitorChoice::Secondary);
        assert!(restored.focus_on_open);
    }

    #[test]
    fn default_config_keeps_os_placement_and_focus() {
        let config = WindowConfig::default();
        assert_eq!(config.monitor, MonitorChoice::Default);
        assert!(config.focus_on_open);
    }
}
