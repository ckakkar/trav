//! Automatic upload limit.
//!
//! Uploading as fast as the connection allows fills the queue in front of a
//! home connection's uplink ("bufferbloat"). Every acknowledgement and block
//! request we send then waits behind that queue, so downloads crawl and the
//! rest of the network feels slow too. Like LEDBAT, the congestion control
//! behind µTP, the governor watches *queueing delay*: how far each
//! connection's smoothed round-trip time has risen above the lowest seen on
//! it. When that delay passes a target while we are uploading hard, it caps
//! uploads a little below their current rate; while the path stays clear and
//! the cap is what holds uploads back, it raises the cap again.

/// One connection's view of the path, from the kernel's TCP estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RttSample {
    /// Smoothed round-trip time now, in milliseconds.
    pub srtt_ms: u32,
    /// The lowest smoothed round-trip time seen on this connection.
    pub base_ms: u32,
}

#[derive(Debug, Default)]
pub struct UploadGovernor {
    /// Bytes per second; `None` means unlimited.
    cap: Option<u64>,
    /// Slowly decaying peak of the total upload rate: what the uplink has carried.
    peak: f64,
}

impl UploadGovernor {
    /// Added queueing delay we tolerate before backing off.
    pub const TARGET_MS: u32 = 100;
    /// Never throttle below this: peers reward uploaders (tit-for-tat).
    pub const MIN_CAP: u64 = 48 * 1024;
    /// Fewer samples than this is noise, not a measurement.
    const MIN_SAMPLES: usize = 3;

    pub fn cap(&self) -> Option<u64> {
        self.cap
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// One control step (run every couple of seconds). `upload_rate` is the
    /// session's total upload in bytes per second. Returns the new limit.
    pub fn step(&mut self, samples: &[RttSample], upload_rate: u64) -> Option<u64> {
        self.peak = (self.peak * 0.99).max(upload_rate as f64);
        if samples.len() < Self::MIN_SAMPLES {
            return self.cap;
        }
        let mut delays: Vec<u32> = samples
            .iter()
            .map(|s| s.srtt_ms.saturating_sub(s.base_ms))
            .collect();
        delays.sort_unstable();
        let delay = delays[delays.len() / 2];

        if delay > Self::TARGET_MS {
            // Only blame uploads when they are a large share of what the
            // uplink has carried; otherwise the delay comes from elsewhere
            // (a busy downlink, another device) and throttling would not help.
            let uploading_hard =
                upload_rate as f64 >= self.peak * 0.5 && upload_rate > Self::MIN_CAP;
            if uploading_hard {
                let below = (upload_rate as f64 * 0.8) as u64;
                let next = self.cap.map_or(below, |c| c.min(below)).max(Self::MIN_CAP);
                self.cap = Some(next);
            }
        } else if delay < Self::TARGET_MS / 2
            && let Some(c) = self.cap
            && upload_rate * 10 >= c * 9
        {
            // The cap is what holds uploads back and the path is clear: probe upward.
            let next = c + (c / 10).max(16 * 1024);
            // Far above anything the uplink has carried: lift the limit entirely.
            self.cap = (next as f64 <= self.peak * 4.0 || self.peak == 0.0).then_some(next);
        }
        self.cap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(delay: u32) -> Vec<RttSample> {
        (0..5)
            .map(|i| RttSample {
                srtt_ms: 30 + i + delay,
                base_ms: 30 + i,
            })
            .collect()
    }

    #[test]
    fn starts_unlimited_and_stays_so_on_a_clear_path() {
        let mut g = UploadGovernor::default();
        for _ in 0..20 {
            assert_eq!(g.step(&samples(10), 900_000), None);
        }
    }

    #[test]
    fn backs_off_when_our_uploads_build_a_queue() {
        let mut g = UploadGovernor::default();
        g.step(&samples(10), 1_000_000);
        let cap = g.step(&samples(800), 1_000_000).expect("capped");
        assert_eq!(cap, 800_000);
        // Still congested at the new rate: keep backing off.
        let cap2 = g.step(&samples(400), 800_000).unwrap();
        assert!(cap2 < cap);
    }

    #[test]
    fn leaves_uploads_alone_when_they_are_not_the_cause() {
        let mut g = UploadGovernor::default();
        g.step(&samples(10), 1_000_000);
        // Heavy delay but we're barely uploading: someone else's traffic.
        assert_eq!(g.step(&samples(800), 60_000), None);
    }

    #[test]
    fn never_throttles_below_the_floor() {
        let mut g = UploadGovernor::default();
        g.step(&samples(10), 100_000);
        for _ in 0..30 {
            g.step(&samples(900), 100_000);
        }
        assert!(g.cap().unwrap() >= UploadGovernor::MIN_CAP);
    }

    #[test]
    fn recovers_while_the_path_is_clear() {
        let mut g = UploadGovernor::default();
        g.step(&samples(10), 1_000_000);
        let low = g.step(&samples(800), 1_000_000).unwrap();
        let mut cap = low;
        for _ in 0..10 {
            cap = g.step(&samples(5), cap).unwrap_or(u64::MAX);
        }
        assert!(cap > low);
    }

    #[test]
    fn ignores_too_few_samples() {
        let mut g = UploadGovernor::default();
        g.step(&samples(10), 1_000_000);
        assert_eq!(g.step(&samples(800)[..2], 1_000_000), None);
    }
}
