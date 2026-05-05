//! Z-score evaluation against a single [`Ewma`] baseline.
//!
//! The math is trivial; the wrapper exists so `mod.rs` can call a single
//! function name per signal and so we have one place to enforce the
//! variance epsilon (avoiding division by ~zero on near-constant streams).

use super::baselines::{Ewma, STD_EPSILON};

#[derive(Debug, Clone, Copy)]
pub struct ScoreOutcome {
    pub observed: f64,
    pub mean: f64,
    pub std: f64,
    pub zscore: f64,
    pub triggered: bool,
}

/// Compute a one-sided z-score for `observed` against `ewma`. Only positive
/// excursions trigger — request volume, body size, error rate, and path
/// diversity are all interesting on the upside, never the downside.
///
/// Returns `None` if either:
/// - the EWMA hasn't seen enough samples to have a meaningful baseline
///   (`count < 2`), or
/// - the running stddev is below [`STD_EPSILON`] (effectively constant
///   stream — z-score isn't defined).
pub fn score(observed: f64, ewma: &Ewma, sigma_threshold: f64) -> Option<ScoreOutcome> {
    if ewma.count < 2 {
        return None;
    }
    let std = ewma.std();
    if std < STD_EPSILON {
        return None;
    }
    let z = (observed - ewma.mean) / std;
    Some(ScoreOutcome {
        observed,
        mean: ewma.mean,
        std,
        zscore: z,
        triggered: z > sigma_threshold,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn z_score_triggers_on_spike() {
        // Train EWMA on a stream around 10 with small noise.
        let mut e = Ewma::new(0.05);
        for i in 0..200 {
            // Tiny deterministic jitter so std > epsilon.
            let x = 10.0 + ((i % 5) as f64 - 2.0) * 0.1;
            e.update(x);
        }
        // 10x spike must trigger at sigma=4.
        let s = score(100.0, &e, 4.0).expect("scorable");
        assert!(s.triggered, "expected spike to trigger, got {s:?}");
        assert!(s.zscore > 4.0);
    }

    #[test]
    fn z_score_quiet_does_not_trigger() {
        let mut e = Ewma::new(0.05);
        for i in 0..200 {
            let x = 10.0 + ((i % 5) as f64 - 2.0) * 0.1;
            e.update(x);
        }
        let s = score(10.1, &e, 4.0).expect("scorable");
        assert!(!s.triggered);
    }

    #[test]
    fn z_score_constant_stream_returns_none() {
        let mut e = Ewma::new(0.1);
        for _ in 0..50 {
            e.update(5.0);
        }
        // Variance is ~0; we can't meaningfully score.
        assert!(score(50.0, &e, 4.0).is_none());
    }

    #[test]
    fn z_score_too_few_samples_returns_none() {
        let mut e = Ewma::new(0.1);
        e.update(1.0);
        assert!(score(100.0, &e, 4.0).is_none());
    }
}
