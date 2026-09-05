//! A log-bucketed histogram for approximate percentiles of line lengths.
//!
//! Every value below 16 bytes gets its own exact bucket. Above that, buckets
//! grow geometrically by 3% so a multi-gigabyte file's length distribution
//! fits in a few hundred buckets instead of one entry per line. `min`, `max`
//! and `mean` are tracked separately and are always exact; only `percentile`
//! carries the bucket's relative error.

const LINEAR_LIMIT: u64 = 16;
const GROWTH: f64 = 1.03;

/// A running histogram of `u64` values, built for line lengths but not
/// specific to them.
pub struct Histogram {
    count: u64,
    sum: u64,
    min: Option<u64>,
    max: Option<u64>,
    linear: [u64; LINEAR_LIMIT as usize],
    log_buckets: Vec<u64>,
}

impl Histogram {
    pub fn new() -> Self {
        Histogram {
            count: 0,
            sum: 0,
            min: None,
            max: None,
            linear: [0; LINEAR_LIMIT as usize],
            log_buckets: Vec::new(),
        }
    }

    pub fn add(&mut self, value: u64) {
        self.count += 1;
        self.sum += value;
        self.min = Some(self.min.map_or(value, |m| m.min(value)));
        self.max = Some(self.max.map_or(value, |m| m.max(value)));

        if value < LINEAR_LIMIT {
            self.linear[value as usize] += 1;
        } else {
            let idx = log_bucket_index(value);
            if idx >= self.log_buckets.len() {
                self.log_buckets.resize(idx + 1, 0);
            }
            self.log_buckets[idx] += 1;
        }
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn min(&self) -> u64 {
        self.min.unwrap_or(0)
    }

    pub fn max(&self) -> u64 {
        self.max.unwrap_or(0)
    }

    pub fn mean(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum as f64 / self.count as f64
        }
    }

    /// The approximate value at percentile `p` (0.0 to 1.0 inclusive).
    ///
    /// Below 16 bytes the result is exact. Above it, the returned value is
    /// the lower bound of the bucket the target rank fell into, so it is
    /// never more than ~3% below the true value.
    pub fn percentile(&self, p: f64) -> u64 {
        if self.count == 0 {
            return 0;
        }
        let p = p.clamp(0.0, 1.0);
        if p >= 1.0 {
            return self.max();
        }
        let target = ((self.count as f64) * p).ceil().max(1.0) as u64;

        let mut seen = 0u64;
        for (value, count) in self.linear.iter().enumerate() {
            if *count == 0 {
                continue;
            }
            seen += count;
            if seen >= target {
                return value as u64;
            }
        }
        for (idx, count) in self.log_buckets.iter().enumerate() {
            if *count == 0 {
                continue;
            }
            seen += count;
            if seen >= target {
                return bucket_lower_bound(idx);
            }
        }
        self.max()
    }
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

fn log_bucket_index(value: u64) -> usize {
    ((value as f64 / LINEAR_LIMIT as f64).ln() / GROWTH.ln()).floor() as usize
}

fn bucket_lower_bound(idx: usize) -> u64 {
    (LINEAR_LIMIT as f64 * GROWTH.powi(idx as i32)) as u64
}

#[cfg(test)]
mod tests {
    use super::Histogram;

    #[test]
    fn empty_histogram_reports_zero() {
        let h = Histogram::new();
        assert_eq!(h.count(), 0);
        assert_eq!(h.min(), 0);
        assert_eq!(h.max(), 0);
        assert_eq!(h.mean(), 0.0);
        assert_eq!(h.percentile(0.5), 0);
    }

    #[test]
    fn tracks_exact_min_max_mean() {
        let mut h = Histogram::new();
        for v in [5u64, 800, 12, 1, 400] {
            h.add(v);
        }
        assert_eq!(h.count(), 5);
        assert_eq!(h.min(), 1);
        assert_eq!(h.max(), 800);
        assert_eq!(h.mean(), (5 + 800 + 12 + 1 + 400) as f64 / 5.0);
    }

    #[test]
    fn small_values_are_exact() {
        let mut h = Histogram::new();
        for _ in 0..3 {
            h.add(4);
        }
        for _ in 0..7 {
            h.add(10);
        }
        // 10 values total: ranks 1-3 fall in the bucket for 4, ranks 4-10 in
        // the bucket for 10.
        assert_eq!(h.percentile(0.1), 4);
        assert_eq!(h.percentile(0.3), 4);
        assert_eq!(h.percentile(0.31), 10);
        assert_eq!(h.percentile(1.0), 10);
    }

    #[test]
    fn approximates_large_percentiles_within_error_bound() {
        let mut h = Histogram::new();
        for v in 1..=100_000u64 {
            h.add(v);
        }
        for &(p, exact) in &[(0.5, 50_000.0), (0.9, 90_000.0), (0.99, 99_000.0)] {
            let got = h.percentile(p) as f64;
            let relative_error = (exact - got).abs() / exact;
            assert!(
                relative_error <= 0.03,
                "p{p}: exact {exact}, got {got}, relative error {relative_error}"
            );
        }
    }

    #[test]
    fn max_is_reachable_at_the_top_percentile() {
        let mut h = Histogram::new();
        for v in [1u64, 2, 1_000_000] {
            h.add(v);
        }
        assert_eq!(h.percentile(1.0), h.max());
    }
}
