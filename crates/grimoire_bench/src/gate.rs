//! The regression gate (engine ADR-0010, Plan-0002 WP6.1 §4.3 / WP6.2): an exact-integer
//! comparison of a candidate measurement against an *accepted* basis (never the last push,
//! ADR-0010 §4.2), plus the ADR's sliding warning threshold.
//!
//! This is the same `gate_decision` proven in the WP6.1 noise spike (branch
//! `p1/wp6.1-bench-spike`, `spikes/bench-noise/src/lib.rs`) and already unit-tested there in two
//! green CI runs (ADR-0010, "Konsequenzen / Positiv"): unchanged here except for the crate it
//! lives in and the addition of [`warn_threshold_percent_x100`], which the spike left to its
//! caller as a bare parameter.

/// Outcome of comparing one candidate measurement against an accepted basis (ADR-0010 §4.3): the
/// exitcode a gate consumer should use, plus whether it is a warning-only crossing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateOutcome {
    /// Exitcode 0, no warning: candidate within the warning threshold of the basis, or an
    /// improvement.
    Green,
    /// Exitcode 0, with an annotation: `warn_percent < delta <= 10%`.
    Warn,
    /// Exitcode 3: `delta > 10%`, checked as an exact integer ratio so the boundary never hangs
    /// on floating-point rounding (ADR-0010 §4.3): `10 * candidate > 11 * baseline`.
    Red,
}

/// Measurement-error outcome (ADR-0010 §4.3, exitcode 2): a value of zero (or the basis missing,
/// which the caller represents by simply not calling this function) makes any ratio meaningless.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateError;

impl std::fmt::Display for GateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "measurement error: baseline or candidate is zero")
    }
}

impl std::error::Error for GateError {}

/// Compares `candidate` against `baseline` (both in the same unit — for the P0 gate,
/// Callgrind `Ir`). `warn_percent` is a whole-percent threshold (ADR-0010's `w`, e.g. 3), from
/// [`warn_threshold_percent_x100`].
///
/// # Errors
/// Returns [`GateError`] (exitcode 2) if either value is zero: a zero basis or measurement is
/// never a valid ratio (ADR-0010 §4.3, "fehlender Bench, Wert 0 oder NaN").
pub fn gate_decision(
    baseline: u64,
    candidate: u64,
    warn_percent: u32,
) -> Result<GateOutcome, GateError> {
    if baseline == 0 || candidate == 0 {
        return Err(GateError);
    }
    // Red: candidate / baseline > 1.10, as an exact integer comparison (ADR-0010 §4.3).
    if 10u128 * u128::from(candidate) > 11u128 * u128::from(baseline) {
        return Ok(GateOutcome::Red);
    }
    if candidate <= baseline {
        return Ok(GateOutcome::Green);
    }
    // Warn: (candidate - baseline) / baseline > warn_percent / 100, exact integer comparison.
    let delta = candidate - baseline;
    if 100u128 * u128::from(delta) > u128::from(warn_percent) * u128::from(baseline) {
        Ok(GateOutcome::Warn)
    } else {
        Ok(GateOutcome::Green)
    }
}

/// ADR-0010 §4.3's sliding warning threshold `w`, as an exact integer in hundredths of a percent
/// (so `300` means `3%`): `w = 3%` when the basis's own noise band `B <= 1%`, otherwise
/// `min(3*B, 9%)`. `noise_band_percent_x100` is `B`, also in hundredths of a percent.
///
/// For the Callgrind `Ir` gate this crate uses, ADR-0010 measured `B = 0.0%` in two independent
/// 10-run CI batches (job-to-job and run-to-run alike, "Rauschen ist... exakt null") — callers
/// gating on `Ir` should pass `0` here, which this function maps to the `B <= 1%` branch (`w =
/// 3%`). The general formula is implemented (not hardcoded to 3%) so a future non-deterministic
/// metric (e.g. an N-thread wall-clock trend) can supply its own measured `B`.
#[must_use]
pub fn warn_threshold_percent_x100(noise_band_percent_x100: u32) -> u32 {
    const ONE_PERCENT_X100: u32 = 100;
    const THREE_PERCENT_X100: u32 = 300;
    const NINE_PERCENT_X100: u32 = 900;
    if noise_band_percent_x100 <= ONE_PERCENT_X100 {
        THREE_PERCENT_X100
    } else {
        (3 * noise_band_percent_x100).min(NINE_PERCENT_X100)
    }
}

/// Whole-percent threshold for [`gate_decision`], derived from [`warn_threshold_percent_x100`].
/// `gate_decision`'s `warn_percent` is a whole percent (matching ADR-0010's own `w = 3%`
/// example); this rounds the hundredths-of-a-percent value down to match, which only matters for
/// a future non-zero `B` (the `Ir` gate's own `B = 0` always yields exactly `w = 3%`, no
/// rounding).
#[must_use]
pub fn warn_threshold_whole_percent(noise_band_percent_x100: u32) -> u32 {
    warn_threshold_percent_x100(noise_band_percent_x100) / 100
}

#[cfg(test)]
mod tests {
    use super::*;

    // ADR-0010 / Plan-0002 WP6.1 §5, "Unit-Tests des Vergleichers" — proven in the WP6.1 spike
    // (two green CI runs); reused unchanged for grimoire_bench, plus the +15% case WP6.2 itself
    // must prove (task scope: "an injected +15% regression must yield red").
    #[test]
    fn plus_15_percent_is_red() {
        assert_eq!(gate_decision(100_000, 115_000, 3), Ok(GateOutcome::Red));
    }

    #[test]
    fn exactly_10_percent_is_green_with_warning() {
        // "> 10%" is the rule (ADR-0010 §4.3): exactly +10% must not trip the red gate.
        assert_eq!(gate_decision(100_000, 110_000, 3), Ok(GateOutcome::Warn));
    }

    #[test]
    fn just_over_10_percent_is_red() {
        assert_eq!(gate_decision(100_000, 110_010, 3), Ok(GateOutcome::Red));
    }

    #[test]
    fn plus_2_percent_under_warn_threshold_is_plain_green() {
        assert_eq!(gate_decision(100_000, 102_000, 3), Ok(GateOutcome::Green));
    }

    #[test]
    fn plus_4_percent_over_warn_threshold_warns() {
        assert_eq!(gate_decision(100_000, 104_000, 3), Ok(GateOutcome::Warn));
    }

    #[test]
    fn minus_20_percent_is_green() {
        assert_eq!(gate_decision(100_000, 80_000, 3), Ok(GateOutcome::Green));
    }

    #[test]
    fn zero_candidate_or_baseline_is_a_measurement_error() {
        assert_eq!(gate_decision(0, 100, 3), Err(GateError));
        assert_eq!(gate_decision(100, 0, 3), Err(GateError));
    }

    #[test]
    fn exact_integer_boundary_has_no_float_rounding_surprises() {
        // baseline = 10: candidate/baseline = 10/10 would be a repeating binary fraction for
        // some ratios, but 11 (exactly +10%, 11/10 = 1.1) and 12 (+20%) are exact in decimal too;
        // the point is that the *rule itself* (10*candidate > 11*baseline) is exact integer
        // arithmetic regardless of what the ratio looks like in floating point.
        let baseline = 10u64;
        assert_eq!(
            gate_decision(baseline, 11, 3),
            Ok(GateOutcome::Warn),
            "exactly +10% must warn, not gate red"
        );
        assert_eq!(
            gate_decision(baseline, 12, 3),
            Ok(GateOutcome::Red),
            "+20% must gate red"
        );
    }

    #[test]
    fn warn_threshold_is_three_percent_when_noise_band_at_or_under_one_percent() {
        assert_eq!(warn_threshold_percent_x100(0), 300);
        assert_eq!(warn_threshold_percent_x100(100), 300);
    }

    #[test]
    fn warn_threshold_scales_with_noise_band_above_one_percent() {
        // B = 2% -> 3*2% = 6%
        assert_eq!(warn_threshold_percent_x100(200), 600);
    }

    #[test]
    fn warn_threshold_caps_at_nine_percent() {
        // B = 10% -> 3*10% = 30%, capped at 9%
        assert_eq!(warn_threshold_percent_x100(1_000), 900);
        // Exactly at the cap boundary: B = 3% -> 3*3% = 9%, not yet reduced by the cap.
        assert_eq!(warn_threshold_percent_x100(300), 900);
    }

    #[test]
    fn ir_gate_default_noise_band_yields_three_percent_whole() {
        // ADR-0010 measured B = 0.0% for Callgrind Ir; this is the value every ir_probe-based
        // gate call in this crate passes.
        assert_eq!(warn_threshold_whole_percent(0), 3);
    }
}
