//! Global token-bucket rate limiter shared by every peer connection.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

pub struct RateLimiter {
    /// Bytes per second; 0 = unlimited.
    rate: AtomicU64,
    state: Mutex<(f64, Instant)>,
}

impl RateLimiter {
    pub fn new(rate: u64) -> Self {
        Self {
            rate: AtomicU64::new(rate),
            state: Mutex::new((0.0, Instant::now())),
        }
    }

    pub fn set_rate(&self, rate: u64) {
        self.rate.store(rate, Ordering::Relaxed);
    }

    pub fn rate(&self) -> u64 {
        self.rate.load(Ordering::Relaxed)
    }

    /// Consume `n` bytes of budget, sleeping if the bucket is in debt.
    /// Debt-based accounting means large blocks never starve under tiny limits.
    pub async fn acquire(&self, n: usize) {
        let mut rate = self.rate();
        if rate == 0 {
            return;
        }
        let wait = {
            let mut st = self.state.lock();
            let now = Instant::now();
            let burst = (rate as f64 / 4.0).max(16_384.0);
            st.0 = (st.0 + now.duration_since(st.1).as_secs_f64() * rate as f64).min(burst);
            st.1 = now;
            st.0 -= n as f64;
            if st.0 < 0.0 {
                Duration::from_secs_f64(-st.0 / rate as f64)
            } else {
                Duration::ZERO
            }
        };
        // Sleep in short slices so a raised or removed limit (including the
        // automatic upload limit) takes effect at once instead of after the
        // whole debt has been waited out.
        let mut left = wait;
        while !left.is_zero() {
            let slice = left.min(Duration::from_millis(100));
            tokio::time::sleep(slice).await;
            left -= slice;
            let now = self.rate();
            if now == 0 {
                return;
            }
            // Rescale once per raise: what is left was measured at `rate`.
            if now > rate {
                left = left.mul_f64(rate as f64 / now as f64);
                rate = now;
            }
        }
    }
}

/// Exponentially smoothed byte rate, sampled once per second.
#[derive(Debug, Default, Clone)]
pub struct RateMeter {
    total: u64,
    last_total: u64,
    rate: f64,
}

impl RateMeter {
    #[inline]
    pub fn add(&mut self, n: u64) {
        self.total += n;
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    /// Call once per `dt` seconds.
    pub fn tick(&mut self, dt: f64) {
        let inst = (self.total - self.last_total) as f64 / dt.max(1e-3);
        self.last_total = self.total;
        // α≈0.4 balances responsiveness with a calm readout.
        self.rate = if self.rate == 0.0 {
            inst
        } else {
            self.rate * 0.6 + inst * 0.4
        };
        if self.rate < 1.0 {
            self.rate = 0.0;
        }
    }

    pub fn rate(&self) -> u64 {
        self.rate as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn throttles() {
        let l = RateLimiter::new(100_000);
        let t = Instant::now();
        for _ in 0..10 {
            l.acquire(25_000).await;
        }
        // 250 KB at 100 KB/s minus the initial burst ≈ 2.25 s.
        assert!(
            t.elapsed() >= Duration::from_millis(2000),
            "{:?}",
            t.elapsed()
        );
    }

    #[tokio::test]
    async fn lifting_the_limit_wakes_waiters() {
        let l = std::sync::Arc::new(RateLimiter::new(10_000));
        l.acquire(10_000).await;
        let t = Instant::now();
        let waiter = tokio::spawn({
            let l = l.clone();
            // ~5 s of debt at 10 KB/s.
            async move { l.acquire(50_000).await }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        l.set_rate(0);
        waiter.await.unwrap();
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    }
}
