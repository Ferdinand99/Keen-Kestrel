//! Token buckets in memory: one per DSN key (its `rate_limit_per_minute`)
//! and one per client IP address. Nothing here is ever written to disk.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Buckets kept before idle ones are dropped.
const MAX_BUCKETS: usize = 50_000;

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Token buckets by key. A bucket holds a minute's worth of requests and
/// refills continuously.
#[derive(Debug, Default)]
pub struct Buckets<K> {
    map: Mutex<HashMap<K, Bucket>>,
}

impl<K: Hash + Eq + Clone> Buckets<K> {
    pub fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
        }
    }

    /// Takes one token from `key`'s bucket (`per_minute` capacity). `Err`
    /// holds how long until a token is free.
    pub fn take(&self, key: &K, per_minute: u32, now: Instant) -> Result<(), Duration> {
        let capacity = f64::from(per_minute.max(1));
        let rate = capacity / 60.0;
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= MAX_BUCKETS && !map.contains_key(key) {
            // Full buckets are the same as no bucket.
            map.retain(|_, b| {
                now.duration_since(b.last).as_secs_f64() * rate + b.tokens < capacity
            });
            if map.len() >= MAX_BUCKETS {
                map.clear();
            }
        }
        let bucket = map.entry(key.clone()).or_insert(Bucket {
            tokens: capacity,
            last: now,
        });
        let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * rate).min(capacity);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - bucket.tokens) / rate))
        }
    }
}

/// Whole seconds to wait, at least one (for `Retry-After`).
pub fn retry_secs(wait: Duration) -> u64 {
    wait.as_secs_f64().ceil().max(1.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bucket_empties_and_refills() {
        let b = Buckets::new();
        let t0 = Instant::now();
        for _ in 0..60 {
            assert!(b.take(&1, 60, t0).is_ok());
        }
        let wait = b.take(&1, 60, t0).unwrap_err();
        assert_eq!(retry_secs(wait), 1);
        assert!(b.take(&2, 60, t0).is_ok(), "buckets are per key");
        assert!(b.take(&1, 60, t0 + Duration::from_secs(1)).is_ok());
        assert!(b.take(&1, 60, t0 + Duration::from_secs(1)).is_err());
    }

    #[test]
    fn slow_buckets_say_how_long() {
        let b = Buckets::new();
        let t0 = Instant::now();
        b.take(&"k", 2, t0).unwrap();
        b.take(&"k", 2, t0).unwrap();
        assert_eq!(retry_secs(b.take(&"k", 2, t0).unwrap_err()), 30);
    }
}
