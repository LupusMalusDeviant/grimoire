//! The stats overlay (plan 0002 WP6.4, PRD-0002 FR-12, contract §9.7): budget bars and digits of
//! the frame profile, drawn by the facade through the sprite pass on the debug layer.
//!
//! [`StatsOverlay::record`] collects every frame's [`FrameStats`] and [`FrameProfile`];
//! [`StatsOverlay::draw`] appends the overlay as [`SpriteInstance`]s to
//! `StageFrame::debug_sprites` (drawn last, over everything, `RenderLayer::DebugUi`). Numbers are
//! the mean over the last [`OVERLAY_WINDOW_FRAMES`] frames, so they stay readable at any frame
//! rate; until the first window completes, the overlay shows the frames seen so far.
//!
//! Layout, top left, one row per scope (`frame`, `sim`, the subsystems, `extract`, `render`,
//! `gpu`), glyph pixels scaled by [`overlay_scale`]:
//!
//! ```text
//! FPS 60.0 FT 16.67
//! FRAME   14.20 [=======|  ]      green: mean within budget
//! SIM      4.80 [========|==]     red:   mean over budget
//! ~GPU     6.50 [=====   |  ]     `~`, amber name: estimate (contract §13)
//! ```
//!
//! The bar puts the budget at three quarters of its width (the grey mark) and fills to the mean;
//! a one-pixel tick shows the window's peak, red when the peak exceeded the budget. A scope
//! without a budget shows only its number.

use std::fmt::Write as _;
use std::time::Duration;

use grimoire_debug::{FrameProfile, ScopeId};
use grimoire_platform::KeyCode;
use grimoire_render::{Camera2D, SpriteInstance};

use super::glyphs::{GLYPH_ADVANCE, GLYPH_HEIGHT, ScreenSpace};
use super::{LOOP_SCOPES, SCOPE_EXTRACT, SCOPE_FRAME, SCOPE_GPU, SCOPE_RENDER, SCOPE_SIM};
use crate::plugin::FrameStats;

/// Key that toggles the overlay unless [`crate::AppBuilder::overlay_key`] chose another (contract
/// §9.2).
pub const DEFAULT_OVERLAY_KEY: KeyCode = KeyCode::F3;

/// Frames averaged per displayed value.
pub const OVERLAY_WINDOW_FRAMES: u32 = 30;

/// Characters of the name column, including an estimate's `~`.
const NAME_CHARACTERS: u32 = 7;
/// Characters of the value column (milliseconds, right-aligned).
const VALUE_CHARACTERS: u32 = 6;
/// Bar width in glyph pixels; the budget sits at [`BAR_BUDGET`].
const BAR_WIDTH: u32 = 40;
/// Position of the budget mark in glyph pixels from the bar's left edge.
const BAR_BUDGET: u32 = 30;
/// Bar height in glyph pixels.
const BAR_HEIGHT: u32 = 5;
/// Padding inside the panel and gap between columns, in glyph pixels.
const PADDING: u32 = 2;
/// Distance of the panel from the viewport's top-left corner, in glyph pixels.
const MARGIN: u32 = 2;
/// Row height in glyph pixels: a glyph plus two rows of spacing.
const LINE_HEIGHT: u32 = GLYPH_HEIGHT + 2;

/// Panel background, linear RGBA.
const PANEL: [f32; 4] = [0.0, 0.0, 0.0, 0.7];
/// Text colour.
const TEXT: [f32; 4] = [0.92, 0.92, 0.92, 1.0];
/// Name colour of an estimated scope.
const ESTIMATE: [f32; 4] = [1.0, 0.62, 0.15, 1.0];
/// Bar background.
const BAR_BACK: [f32; 4] = [0.12, 0.12, 0.14, 1.0];
/// Bar fill within budget.
const WITHIN: [f32; 4] = [0.2, 0.75, 0.3, 1.0];
/// Bar fill and peak tick over budget.
const OVER: [f32; 4] = [0.95, 0.08, 0.06, 1.0];
/// Budget mark and a peak tick within budget.
const MARK: [f32; 4] = [0.75, 0.75, 0.75, 1.0];

/// Glyph-pixel scale for a viewport of `viewport_height` physical pixels: one per 270 pixels
/// (1 at 270p and below, 2 at 720p, 4 at 1080p), at most 8.
#[must_use]
pub fn overlay_scale(viewport_height: f32) -> u32 {
    if !viewport_height.is_finite() || viewport_height < 270.0 {
        return 1;
    }
    // Truncation is the rounding wanted here; the value is finite and at least 1.
    ((viewport_height / 270.0) as u32).clamp(1, 8)
}

/// One scope's values in the current window.
#[derive(Debug, Clone)]
struct Accumulated {
    scope: ScopeId,
    name: String,
    sum: Duration,
    peak: Duration,
    budget: Option<Duration>,
    estimate: bool,
}

/// One displayed row.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    name: String,
    mean: Duration,
    peak: Duration,
    budget: Option<Duration>,
    estimate: bool,
}

/// The stats overlay of the main loop (see the module documentation).
///
/// Presentation state only: toggling it or drawing it never changes simulation state or a hash.
#[derive(Debug, Clone, Default)]
pub struct StatsOverlay {
    visible: bool,
    /// Frames recorded in the current window.
    frames: u32,
    frame_time_sum: Duration,
    fps: f64,
    scopes: Vec<Accumulated>,
    /// Whether a complete window was published yet.
    published: bool,
    shown_fps: f64,
    shown_frame_time: Duration,
    shown: Vec<Row>,
    scratch: String,
}

impl StatsOverlay {
    /// A hidden overlay with nothing recorded. Identical to [`StatsOverlay::default`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether [`StatsOverlay::draw`] draws anything.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Shows or hides the overlay.
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    /// Shows a hidden overlay and hides a visible one.
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Records one frame: `stats` for the header, `profile` (if the profiler runs) for the scope
    /// rows. Every [`OVERLAY_WINDOW_FRAMES`] frames the means and peaks of the window become the
    /// displayed values; before the first complete window, every frame updates them.
    pub fn record(&mut self, stats: &FrameStats, profile: Option<&FrameProfile>) {
        self.frames += 1;
        self.frame_time_sum = self.frame_time_sum.saturating_add(stats.frame_time);
        self.fps = stats.fps;
        if let Some(profile) = profile {
            for total in profile.scopes() {
                let entry = match self
                    .scopes
                    .iter_mut()
                    .position(|entry| entry.scope == total.scope)
                {
                    Some(index) => &mut self.scopes[index],
                    None => {
                        self.scopes.push(Accumulated {
                            scope: total.scope,
                            name: profile
                                .scope_name(total.scope)
                                .unwrap_or_default()
                                .to_owned(),
                            sum: Duration::ZERO,
                            peak: Duration::ZERO,
                            budget: None,
                            estimate: false,
                        });
                        let last = self.scopes.len() - 1;
                        &mut self.scopes[last]
                    }
                };
                entry.sum = entry.sum.saturating_add(total.total);
                entry.peak = entry.peak.max(total.total);
                entry.budget = total.budget;
                entry.estimate |= total.estimate;
            }
        }
        let complete = self.frames >= OVERLAY_WINDOW_FRAMES;
        if complete || !self.published {
            self.publish();
        }
        if complete {
            self.published = true;
            self.frames = 0;
            self.frame_time_sum = Duration::ZERO;
            for entry in &mut self.scopes {
                entry.sum = Duration::ZERO;
                entry.peak = Duration::ZERO;
                entry.estimate = false;
            }
        }
    }

    /// Turns the current window into the displayed values, rows in overlay order.
    fn publish(&mut self) {
        let frames = self.frames.max(1);
        self.shown_fps = self.fps;
        self.shown_frame_time = self.frame_time_sum / frames;
        self.shown.clear();
        let loop_rank = |name: &str| match name {
            SCOPE_FRAME => 0,
            SCOPE_SIM => 1,
            SCOPE_EXTRACT => 3,
            SCOPE_RENDER => 4,
            SCOPE_GPU => 5,
            // Subsystems sit between `sim` and `extract`, in the order they first appeared.
            _ => 2,
        };
        for rank in 0..=5 {
            for entry in &self.scopes {
                if loop_rank(&entry.name) == rank {
                    self.shown.push(Row {
                        name: entry.name.clone(),
                        mean: entry.sum / frames,
                        peak: entry.peak,
                        budget: entry.budget,
                        estimate: entry.estimate,
                    });
                }
            }
        }
        debug_assert!(LOOP_SCOPES.iter().all(|scope| loop_rank(scope) != 2));
    }

    /// Appends the overlay to `out` (`StageFrame::debug_sprites`) for `camera` (`StageFrame::base`'s
    /// camera, which the sprite pass draws debug sprites with) on a viewport of `viewport` physical
    /// pixels. Draws nothing while hidden, for an empty viewport or a degenerate camera. Returns
    /// the number of sprites appended.
    pub fn draw(
        &mut self,
        camera: &Camera2D,
        viewport: [f32; 2],
        out: &mut Vec<SpriteInstance>,
    ) -> usize {
        if !self.visible {
            return 0;
        }
        let Some(space) = ScreenSpace::new(camera, viewport) else {
            return 0;
        };
        let before = out.len();
        let s = overlay_scale(viewport[1]);
        let rows = u32::try_from(self.shown.len()).unwrap_or(u32::MAX);
        let left = MARGIN * s;
        let top = MARGIN * s;
        let name_x = left + PADDING * s;
        let value_x = name_x + NAME_CHARACTERS * GLYPH_ADVANCE * s;
        let bar_x = value_x + VALUE_CHARACTERS * GLYPH_ADVANCE * s + PADDING * s;
        let panel_width = bar_x + (BAR_WIDTH + PADDING) * s - left;
        let panel_height = (PADDING * 2 + (rows.saturating_add(1)) * LINE_HEIGHT
            - (LINE_HEIGHT - GLYPH_HEIGHT))
            * s;
        out.push(space.rect(left, top, panel_width, panel_height, PANEL));

        let mut y = top + PADDING * s;
        self.scratch.clear();
        let _ = write!(
            self.scratch,
            "FPS {:.1} FT {:.2}",
            self.shown_fps.clamp(0.0, 9999.0),
            self.shown_frame_time
                .min(Duration::from_secs(99))
                .as_secs_f64()
                * 1000.0
        );
        space.push_text(out, name_x, y, s, &self.scratch, TEXT);

        for row in &self.shown {
            y += LINE_HEIGHT * s;
            self.scratch.clear();
            if row.estimate {
                self.scratch.push('~');
            }
            self.scratch.extend(
                row.name
                    .chars()
                    .take(NAME_CHARACTERS as usize - usize::from(row.estimate)),
            );
            let name_color = if row.estimate { ESTIMATE } else { TEXT };
            space.push_text(out, name_x, y, s, &self.scratch, name_color);
            self.scratch.clear();
            write_milliseconds(&mut self.scratch, row.mean);
            space.push_text(out, value_x, y, s, &self.scratch, TEXT);

            let Some(budget) = row.budget.filter(|budget| !budget.is_zero()) else {
                continue;
            };
            let bar_y = y + s;
            out.push(space.rect(bar_x, bar_y, BAR_WIDTH * s, BAR_HEIGHT * s, BAR_BACK));
            let fill = bar_pixels(row.mean, budget) * s;
            if fill > 0 {
                let color = if row.mean > budget { OVER } else { WITHIN };
                out.push(space.rect(bar_x, bar_y, fill, BAR_HEIGHT * s, color));
            }
            out.push(space.rect(bar_x + BAR_BUDGET * s, bar_y, s, BAR_HEIGHT * s, MARK));
            let peak = bar_pixels(row.peak, budget).min(BAR_WIDTH - 1) * s;
            let peak_color = if row.peak > budget { OVER } else { MARK };
            out.push(space.rect(bar_x + peak, bar_y, s, BAR_HEIGHT * s, peak_color));
        }
        out.len() - before
    }

    /// The displayed rows as `(name, mean, peak, budget, estimate)`, in overlay order.
    #[cfg(test)]
    fn rows(&self) -> Vec<(&str, Duration, Duration, Option<Duration>, bool)> {
        self.shown
            .iter()
            .map(|row| {
                (
                    row.name.as_str(),
                    row.mean,
                    row.peak,
                    row.budget,
                    row.estimate,
                )
            })
            .collect()
    }
}

/// Bar length in glyph pixels for `value` against `budget`: the budget at [`BAR_BUDGET`], capped at
/// [`BAR_WIDTH`].
fn bar_pixels(value: Duration, budget: Duration) -> u32 {
    let ratio = value.as_secs_f64() / budget.as_secs_f64();
    let pixels = (ratio * f64::from(BAR_BUDGET)).round();
    if pixels.is_finite() {
        // Clamped before the cast, so truncation cannot wrap.
        pixels.clamp(0.0, f64::from(BAR_WIDTH)) as u32
    } else {
        BAR_WIDTH
    }
}

/// Appends `duration` in milliseconds, right-aligned in [`VALUE_CHARACTERS`] characters: two
/// decimals up to 999.99, whole milliseconds up to 99999, `>99999` beyond.
fn write_milliseconds(out: &mut String, duration: Duration) {
    let ms = duration.as_secs_f64() * 1000.0;
    let _ = if ms < 999.995 {
        write!(out, "{ms:>6.2}")
    } else if ms < 99_999.5 {
        write!(out, "{ms:>6.0}")
    } else {
        write!(out, ">99999")
    };
}

#[cfg(test)]
mod tests {
    use grimoire_render::RenderStats;

    use super::super::{Profiler, ProfilerBudgets};
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    fn stats(frame: u64, frame_time: Duration, fps: f64) -> FrameStats {
        FrameStats {
            frame,
            sim_tick: frame,
            ticks_this_frame: 1,
            alpha: 0.0,
            frame_time,
            fps,
            dropped_time: Duration::ZERO,
            render: RenderStats::default(),
        }
    }

    fn profile(profiler: &mut Profiler, frame: u64, sim: Duration, gpu_estimate: Duration) {
        profiler.begin_frame(frame);
        profiler.record("sigil", sim / 4);
        profiler.record(SCOPE_SIM, sim);
        profiler.record(SCOPE_EXTRACT, MS / 4);
        profiler.record(SCOPE_RENDER, 2 * MS);
        profiler.record_estimate(SCOPE_GPU, gpu_estimate);
        profiler.record(SCOPE_FRAME, sim + 3 * MS);
    }

    #[test]
    fn rows_follow_the_overlay_order_and_the_first_frames_show_immediately() {
        let mut overlay = StatsOverlay::new();
        let mut profiler = Profiler::new(ProfilerBudgets::default());
        profile(&mut profiler, 0, 2 * MS, 6 * MS);
        overlay.record(&stats(0, 16 * MS, 0.0), Some(profiler.profile()));
        let names: Vec<&str> = overlay.rows().iter().map(|row| row.0).collect();
        assert_eq!(names, ["frame", "sim", "sigil", "extract", "render", "gpu"]);
        assert_eq!(overlay.rows()[1].1, 2 * MS);
        assert!(overlay.rows()[5].4, "gpu is an estimate");

        profile(&mut profiler, 1, 4 * MS, 6 * MS);
        overlay.record(&stats(1, 18 * MS, 0.0), Some(profiler.profile()));
        assert_eq!(
            overlay.rows()[1].1,
            3 * MS,
            "running mean before a full window"
        );
        assert_eq!(overlay.rows()[1].2, 4 * MS, "peak");
        assert_eq!(overlay.shown_frame_time, 17 * MS);
    }

    #[test]
    fn a_full_window_publishes_means_and_peaks_and_then_holds_them() {
        let mut overlay = StatsOverlay::new();
        let mut profiler = Profiler::new(ProfilerBudgets::default());
        for frame in 0..u64::from(OVERLAY_WINDOW_FRAMES) {
            let sim = if frame == 7 { 10 * MS } else { MS };
            profile(&mut profiler, frame, sim, 6 * MS);
            overlay.record(&stats(frame, 16 * MS, 60.0), Some(profiler.profile()));
        }
        let window = OVERLAY_WINDOW_FRAMES;
        let expected_mean = (MS * (window - 1) + 10 * MS) / window;
        assert_eq!(overlay.rows()[1].1, expected_mean);
        assert_eq!(overlay.rows()[1].2, 10 * MS);
        assert_eq!(overlay.shown_fps, 60.0);

        // The next frames accumulate a new window but do not change what is shown.
        profile(&mut profiler, 30, 50 * MS, 6 * MS);
        overlay.record(&stats(30, 16 * MS, 61.0), Some(profiler.profile()));
        assert_eq!(overlay.rows()[1].1, expected_mean);
    }

    #[test]
    fn hidden_draws_nothing_and_toggle_shows_it() {
        let mut overlay = StatsOverlay::new();
        let mut out = Vec::new();
        assert_eq!(
            overlay.draw(&Camera2D::default(), [160.0, 90.0], &mut out),
            0
        );
        overlay.toggle();
        assert!(overlay.is_visible());
        let drawn = overlay.draw(&Camera2D::default(), [160.0, 90.0], &mut out);
        assert!(drawn > 1, "panel and header text");
        assert_eq!(out.len(), drawn);
        assert_eq!(overlay.draw(&Camera2D::default(), [0.0, 90.0], &mut out), 0);
        overlay.set_visible(false);
        assert!(!overlay.is_visible());
    }

    #[test]
    fn over_budget_rows_are_red_and_rows_without_budget_have_no_bar() {
        let mut overlay = StatsOverlay::new();
        overlay.set_visible(true);
        let mut profiler = Profiler::new(ProfilerBudgets::default());
        profiler.begin_frame(0);
        profiler.record(SCOPE_SIM, 6 * MS); // budget 4 ms: over
        profiler.record(SCOPE_RENDER, MS); // budget 3 ms: within
        profiler.record("app", 5 * MS); // no budget
        overlay.record(&stats(0, 16 * MS, 0.0), Some(profiler.profile()));
        let mut out = Vec::new();
        overlay.draw(&Camera2D::default(), [160.0, 90.0], &mut out);
        let red = out.iter().filter(|sprite| sprite.color == OVER).count();
        let green = out.iter().filter(|sprite| sprite.color == WITHIN).count();
        let backs = out.iter().filter(|sprite| sprite.color == BAR_BACK).count();
        assert_eq!(red, 2, "sim's fill and peak tick");
        assert_eq!(green, 1, "render's fill");
        assert_eq!(backs, 2, "bars only for the two scopes with a budget");
    }

    #[test]
    fn the_panel_fits_a_160_by_90_viewport_at_scale_one() {
        let mut overlay = StatsOverlay::new();
        overlay.set_visible(true);
        let mut profiler = Profiler::new(ProfilerBudgets::default());
        profile(&mut profiler, 0, MS, MS);
        profiler.record("collide", MS);
        profiler.record("app", MS);
        overlay.record(&stats(0, 16 * MS, 0.0), Some(profiler.profile()));
        assert_eq!(overlay.rows().len(), 8);
        let mut out = Vec::new();
        overlay.draw(&Camera2D::default(), [160.0, 90.0], &mut out);
        let panel = out[0];
        let world_per_pixel = Camera2D::default().world_height / 90.0;
        let right_px = (panel.position[0] + panel.half_size[0]) / world_per_pixel + 80.0;
        let bottom_px = 45.0 - (panel.position[1] - panel.half_size[1]) / world_per_pixel;
        assert!(right_px <= 160.0 + 1e-3, "panel right edge at {right_px}");
        assert!(bottom_px <= 90.0 + 1e-3, "panel bottom edge at {bottom_px}");
    }

    #[test]
    fn helpers_scale_format_and_clamp() {
        assert_eq!(overlay_scale(90.0), 1);
        assert_eq!(overlay_scale(720.0), 2);
        assert_eq!(overlay_scale(1080.0), 4);
        assert_eq!(overlay_scale(100_000.0), 8);
        assert_eq!(overlay_scale(f32::NAN), 1);
        let format = |duration| {
            let mut text = String::new();
            write_milliseconds(&mut text, duration);
            text
        };
        assert_eq!(format(Duration::from_micros(1_250)), "  1.25");
        assert_eq!(format(Duration::from_millis(1_500)), "  1500");
        assert_eq!(format(Duration::from_secs(500)), ">99999");
        assert_eq!(format(Duration::from_micros(999_996)), "  1000");
        assert_eq!(bar_pixels(4 * MS, 4 * MS), BAR_BUDGET);
        assert_eq!(bar_pixels(100 * MS, 4 * MS), BAR_WIDTH);
        assert_eq!(bar_pixels(Duration::ZERO, 4 * MS), 0);
    }
}
