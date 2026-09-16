//! Summarising repeated timing samples: WP3.2 requires a median of 10 repetitions *and* the
//! spread ("gib neben dem Median auch die Streuung an") — a shared-runner wall-clock median alone
//! hides how noisy the measurement is (engine ADR-0010).

/// Median plus min/max spread of a batch of wall-clock timings, in microseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub median_us: f64,
    pub min_us: f64,
    pub max_us: f64,
    pub samples: usize,
}

impl Summary {
    /// Summarises `samples` (microseconds). Panics if `samples` is empty or contains a NaN — both
    /// are a bug in the caller's measurement loop, not a real outcome to report on.
    #[must_use]
    pub fn from_micros(mut samples: Vec<f64>) -> Self {
        assert!(!samples.is_empty(), "cannot summarize zero timing samples");
        samples.sort_by(|a, b| a.partial_cmp(b).expect("timings are never NaN"));
        let n = samples.len();
        let median_us = if n % 2 == 1 {
            samples[n / 2]
        } else {
            (samples[n / 2 - 1] + samples[n / 2]) / 2.0
        };
        Self {
            median_us,
            min_us: samples[0],
            max_us: samples[n - 1],
            samples: n,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Summary;

    #[test]
    fn odd_count_median_is_the_middle_sample() {
        let summary = Summary::from_micros(vec![5.0, 1.0, 3.0]);
        assert_eq!(summary.median_us, 3.0);
        assert_eq!(summary.min_us, 1.0);
        assert_eq!(summary.max_us, 5.0);
        assert_eq!(summary.samples, 3);
    }

    #[test]
    fn even_count_median_is_the_mean_of_the_middle_two() {
        let summary = Summary::from_micros(vec![10.0, 1.0, 2.0, 9.0]);
        assert_eq!(summary.median_us, 5.5);
    }

    #[test]
    #[should_panic(expected = "cannot summarize zero timing samples")]
    fn empty_samples_panics_instead_of_lying() {
        let _ = Summary::from_micros(Vec::new());
    }
}
