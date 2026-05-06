//! Per-key rolling baseline state.
//!
//! Each consumer/ip/global key owns one [`Baseline`] guarded by a `Mutex`.
//! The hot path acquires that mutex once per request, performs a small
//! constant-time update, releases it. No async work happens inside the
//! critical section.
//!
//! Memory shape: ~few hundred bytes per active key, dominated by the
//! `request_window` `VecDeque` and the `path_window` `HashSet`. Both are
//! bounded — we cap distinct paths at `MAX_PATH_ENTRIES` and discard old
//! request timestamps lazily when sampling.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

/// Hard cap on distinct path hashes we'll track in a single window before we
/// stop adding new ones. A misbehaving client that scans 10k unique URIs in
/// a window will still be caught by the diversity z-score; we just don't
/// pay unbounded memory for the privilege.
pub const MAX_PATH_ENTRIES: usize = 4096;

/// Below this stddev we don't compute a z-score — division would either
/// blow up or produce a meaningless answer for a near-constant stream.
pub const STD_EPSILON: f64 = 1e-6;

/// Online EWMA mean + variance.
///
/// Formulae:
/// ```text
/// delta   = x - mean_prev
/// mean_t  = mean_prev + α * delta
/// var_t   = (1 - α) * (var_prev + α * delta²)
/// std_t   = sqrt(var_t)
/// ```
///
/// This is a Welford-flavoured EWMA: variance tracks the running squared
/// deviation rather than recomputing from samples. `count` is informational
/// (used by callers to gate scoring during warmup).
#[derive(Debug, Clone)]
pub struct Ewma {
    pub mean: f64,
    pub var: f64,
    pub alpha: f64,
    pub count: u64,
}

impl Ewma {
    pub fn new(alpha: f64) -> Self {
        Self {
            mean: 0.0,
            var: 0.0,
            alpha,
            count: 0,
        }
    }

    /// Fold one observation into the running mean+variance. The first sample
    /// is taken as the seed mean, with zero variance — no update math runs
    /// until `count >= 2`.
    pub fn update(&mut self, x: f64) {
        self.count = self.count.saturating_add(1);
        if self.count == 1 {
            self.mean = x;
            self.var = 0.0;
            return;
        }
        let delta = x - self.mean;
        self.mean += self.alpha * delta;
        self.var = (1.0 - self.alpha) * (self.var + self.alpha * delta * delta);
    }

    pub fn std(&self) -> f64 {
        self.var.sqrt()
    }
}

/// Sliding window of distinct paths, identified by stable u64 hashes.
///
/// We don't bother removing individual paths — we just clear the set when
/// the window expires. That gives us approximate "distinct paths in last
/// W seconds" with at most a 1× window of staleness, which is plenty for
/// scoring traffic patterns.
#[derive(Debug)]
pub struct PathWindow {
    pub paths: HashSet<u64>,
    pub last_reset: Instant,
    pub window: Duration,
}

impl PathWindow {
    pub fn new(window_secs: u64) -> Self {
        Self {
            paths: HashSet::new(),
            last_reset: Instant::now(),
            window: Duration::from_secs(window_secs.max(1)),
        }
    }

    /// Record the current path and return the running count. If the window
    /// elapsed, reset first. Caps at `MAX_PATH_ENTRIES` distinct entries.
    pub fn observe(&mut self, path_hash: u64, now: Instant) -> usize {
        if now.duration_since(self.last_reset) >= self.window {
            self.paths.clear();
            self.last_reset = now;
        }
        if self.paths.len() < MAX_PATH_ENTRIES {
            self.paths.insert(path_hash);
        }
        self.paths.len()
    }
}

/// Rolling 4xx/5xx tally over a fixed window.
#[derive(Debug)]
pub struct ErrorWindow {
    pub errors: u64,
    pub total: u64,
    pub last_reset: Instant,
    pub window: Duration,
}

impl ErrorWindow {
    pub fn new(window_secs: u64) -> Self {
        Self {
            errors: 0,
            total: 0,
            last_reset: Instant::now(),
            window: Duration::from_secs(window_secs.max(1)),
        }
    }

    fn maybe_reset(&mut self, now: Instant) {
        if now.duration_since(self.last_reset) >= self.window {
            self.errors = 0;
            self.total = 0;
            self.last_reset = now;
        }
    }

    /// Record one observed status. `is_error` is true for 4xx/5xx.
    pub fn record(&mut self, is_error: bool, now: Instant) {
        self.maybe_reset(now);
        self.total = self.total.saturating_add(1);
        if is_error {
            self.errors = self.errors.saturating_add(1);
        }
    }

    /// Current error fraction in `[0, 1]`. Returns 0.0 if no requests yet.
    pub fn fraction(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.errors as f64 / self.total as f64
        }
    }
}

/// All per-key state. One `Baseline` per consumer/ip/global key.
#[derive(Debug)]
pub struct Baseline {
    /// Total requests we've ever observed for this key. Used to gate
    /// scoring during the warmup phase.
    pub seen: u64,
    /// Timestamps of recent requests. Trimmed lazily on each sample.
    pub request_window: VecDeque<Instant>,
    pub request_rate_ewma: Ewma,
    pub body_size_ewma: Ewma,
    pub path_diversity_ewma: Ewma,
    pub error_rate_ewma: Ewma,
    pub path_window: PathWindow,
    pub error_window: ErrorWindow,
    pub last_anomaly: Option<Instant>,
    /// Last time the `seen` counter advanced. Used by the pruner.
    pub last_activity: Instant,
    pub request_window_secs: u64,
}

impl Baseline {
    pub fn new(
        alpha: f64,
        request_window_secs: u64,
        path_window_secs: u64,
        error_window_secs: u64,
    ) -> Self {
        let now = Instant::now();
        Self {
            seen: 0,
            request_window: VecDeque::new(),
            request_rate_ewma: Ewma::new(alpha),
            body_size_ewma: Ewma::new(alpha),
            path_diversity_ewma: Ewma::new(alpha),
            error_rate_ewma: Ewma::new(alpha),
            path_window: PathWindow::new(path_window_secs),
            error_window: ErrorWindow::new(error_window_secs),
            last_anomaly: None,
            last_activity: now,
            request_window_secs: request_window_secs.max(1),
        }
    }

    /// Trim timestamps older than the request window and append the current
    /// one. Returns the resulting count (i.e. requests-in-window).
    pub fn push_request(&mut self, now: Instant) -> usize {
        let window = Duration::from_secs(self.request_window_secs);
        while let Some(&front) = self.request_window.front() {
            if now.duration_since(front) >= window {
                self.request_window.pop_front();
            } else {
                break;
            }
        }
        self.request_window.push_back(now);
        self.request_window.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn ewma_constant_stream_converges() {
        let mut e = Ewma::new(0.1);
        for _ in 0..500 {
            e.update(42.0);
        }
        assert!((e.mean - 42.0).abs() < 1e-6);
        assert!(e.var < 1e-6);
    }

    #[test]
    fn ewma_first_sample_seeds_mean() {
        let mut e = Ewma::new(0.1);
        e.update(7.5);
        assert_eq!(e.mean, 7.5);
        assert_eq!(e.var, 0.0);
        assert_eq!(e.count, 1);
    }

    #[test]
    fn path_window_resets_after_window() {
        let mut pw = PathWindow::new(1);
        pw.observe(1, Instant::now());
        pw.observe(2, Instant::now());
        assert_eq!(pw.paths.len(), 2);
        sleep(Duration::from_millis(1100));
        let count = pw.observe(3, Instant::now());
        assert_eq!(count, 1, "window should have reset");
    }

    #[test]
    fn path_window_caps_entries() {
        let mut pw = PathWindow::new(60);
        let now = Instant::now();
        for i in 0..(MAX_PATH_ENTRIES as u64 + 100) {
            pw.observe(i, now);
        }
        assert_eq!(pw.paths.len(), MAX_PATH_ENTRIES);
    }

    #[test]
    fn error_window_fraction_correct() {
        let mut ew = ErrorWindow::new(60);
        let now = Instant::now();
        for _ in 0..7 {
            ew.record(false, now);
        }
        for _ in 0..3 {
            ew.record(true, now);
        }
        assert!((ew.fraction() - 0.3).abs() < 1e-9);
    }

    #[test]
    fn error_window_resets() {
        let mut ew = ErrorWindow::new(1);
        ew.record(true, Instant::now());
        ew.record(false, Instant::now());
        sleep(Duration::from_millis(1100));
        ew.record(false, Instant::now());
        assert_eq!(ew.fraction(), 0.0);
        assert_eq!(ew.total, 1);
    }

    #[test]
    fn baseline_request_window_trims() {
        let mut b = Baseline::new(0.1, 1, 60, 60);
        b.push_request(Instant::now());
        b.push_request(Instant::now());
        sleep(Duration::from_millis(1100));
        let n = b.push_request(Instant::now());
        assert_eq!(n, 1, "older entries should be trimmed");
    }
}
