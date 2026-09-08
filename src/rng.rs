//! A seedable random source and a reservoir sampler built on it. Used by the
//! `sample` command to draw a uniform sample from a stream whose length is
//! not known until the last line is read.

/// SplitMix64: the generator commonly used to seed xoshiro/xoroshiro
/// variants, but fine standalone here. It passes the standard statistical
/// test suites, needs no setup beyond a single `u64` seed, and the same seed
/// always produces the same sequence, which is what `--seed` promises.
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64 { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    /// A uniform value in `0..bound`, or 0 if `bound` is 0.
    ///
    /// Uses Lemire's method on the 128-bit product instead of `% bound`, so
    /// the result stays unbiased even when `bound` does not divide 2^64
    /// evenly.
    pub fn next_below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            return 0;
        }
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let r = self.next_u64();
            let product = (r as u128) * (bound as u128);
            if product as u64 >= threshold {
                return (product >> 64) as u64;
            }
        }
    }
}

/// Algorithm R reservoir sampling: keeps a uniform random sample of `capacity`
/// items from a stream of unknown length, in one pass, using memory
/// proportional to `capacity` rather than to the stream length.
///
/// Each kept item remembers the position it had in the original stream, so
/// callers that want the sample back in file order can ask for it with
/// [`Reservoir::into_sorted`].
pub struct Reservoir<T> {
    capacity: usize,
    seen: u64,
    rng: SplitMix64,
    items: Vec<(u64, T)>,
}

impl<T> Reservoir<T> {
    pub fn new(capacity: usize, seed: u64) -> Self {
        Reservoir {
            capacity,
            seen: 0,
            rng: SplitMix64::new(seed),
            items: Vec::with_capacity(capacity),
        }
    }

    /// Offers one more item from the stream. Once the stream ends, every item
    /// it saw has had exactly `capacity` in `seen` odds of surviving into the
    /// reservoir.
    pub fn consider(&mut self, item: T) {
        let index = self.seen;
        self.seen += 1;

        if self.items.len() < self.capacity {
            self.items.push((index, item));
            return;
        }
        if self.capacity == 0 {
            return;
        }
        let j = self.rng.next_below(self.seen);
        if (j as usize) < self.capacity {
            self.items[j as usize] = (index, item);
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The sampled items, restored to the order they appeared in the stream.
    pub fn into_sorted(mut self) -> Vec<T> {
        self.items.sort_by_key(|(index, _)| *index);
        self.items.into_iter().map(|(_, item)| item).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{Reservoir, SplitMix64};

    #[test]
    fn same_seed_gives_same_sequence() {
        let mut a = SplitMix64::new(42);
        let mut b = SplitMix64::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = SplitMix64::new(1);
        let mut b = SplitMix64::new(2);
        let seq_a: Vec<u64> = (0..20).map(|_| a.next_u64()).collect();
        let seq_b: Vec<u64> = (0..20).map(|_| b.next_u64()).collect();
        assert_ne!(seq_a, seq_b);
    }

    #[test]
    fn next_below_stays_in_range() {
        let mut rng = SplitMix64::new(7);
        for _ in 0..10_000 {
            assert!(rng.next_below(37) < 37);
        }
    }

    #[test]
    fn next_below_zero_is_zero() {
        let mut rng = SplitMix64::new(7);
        assert_eq!(rng.next_below(0), 0);
    }

    #[test]
    fn reservoir_keeps_everything_under_capacity() {
        let mut r = Reservoir::new(10, 1);
        for i in 0..5 {
            r.consider(i);
        }
        assert_eq!(r.len(), 5);
        assert_eq!(r.into_sorted(), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn reservoir_caps_at_capacity() {
        let mut r = Reservoir::new(3, 1);
        for i in 0..1000 {
            r.consider(i);
        }
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn reservoir_output_is_sorted_by_original_position() {
        let mut r = Reservoir::new(3, 99);
        for i in 0..1000 {
            r.consider(i);
        }
        let sample = r.into_sorted();
        let mut sorted = sample.clone();
        sorted.sort_unstable();
        assert_eq!(sample, sorted, "into_sorted should restore stream order");
    }

    #[test]
    fn zero_capacity_keeps_nothing() {
        let mut r: Reservoir<i32> = Reservoir::new(0, 1);
        for i in 0..100 {
            r.consider(i);
        }
        assert!(r.is_empty());
    }

    #[test]
    fn same_seed_gives_same_sample() {
        let items: Vec<i32> = (0..500).collect();
        let sample_a: Vec<i32> = {
            let mut r = Reservoir::new(20, 123);
            for &i in &items {
                r.consider(i);
            }
            r.into_sorted()
        };
        let sample_b: Vec<i32> = {
            let mut r = Reservoir::new(20, 123);
            for &i in &items {
                r.consider(i);
            }
            r.into_sorted()
        };
        assert_eq!(sample_a, sample_b);
    }

    #[test]
    fn sampling_is_roughly_uniform_over_many_trials() {
        // Each of 10 items should end up in a 3-slot reservoir with
        // probability 0.3. Over 20,000 independent reservoirs the observed
        // rate for any single item should land well within a generous
        // tolerance of that.
        let trials = 20_000;
        let n_items = 10u64;
        let capacity = 3;
        let mut hits = 0u64;

        for seed in 0..trials {
            let mut r = Reservoir::new(capacity, seed);
            for i in 0..n_items {
                r.consider(i);
            }
            if r.into_sorted().contains(&0) {
                hits += 1;
            }
        }

        let rate = hits as f64 / trials as f64;
        let expected = capacity as f64 / n_items as f64;
        assert!(
            (rate - expected).abs() < 0.02,
            "expected inclusion rate near {expected}, got {rate}"
        );
    }
}
