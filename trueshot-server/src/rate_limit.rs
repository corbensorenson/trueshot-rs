use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config::AppConfig;

#[derive(Debug, Clone, Copy)]
struct RateLimit {
    capacity: f64,
    refill_per_sec: f64,
}

#[derive(Debug, Clone)]
pub struct RateLimitDecision {
    pub allowed: bool,
    pub retry_after_seconds: Option<u64>,
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last_refill: Instant,
    last_seen: Instant,
}

#[derive(Debug)]
pub struct RateLimiter {
    ip_buckets: Mutex<HashMap<String, Bucket>>,
    user_buckets: Mutex<HashMap<String, Bucket>>,
    ip_limit: RateLimit,
    user_limit: RateLimit,
}

impl RateLimiter {
    pub fn from_config(config: &AppConfig, is_production: bool) -> Option<Self> {
        let enabled = config.server.rate_limit_enabled.unwrap_or(is_production);
        if !enabled {
            return None;
        }

        let ip_per_minute = config.server.rate_limit_ip_per_minute.unwrap_or(600);
        let ip_burst = config.server.rate_limit_ip_burst.unwrap_or(120);
        let user_per_minute = config.server.rate_limit_user_per_minute.unwrap_or(1200);
        let user_burst = config.server.rate_limit_user_burst.unwrap_or(240);

        Some(Self::new(
            ip_per_minute,
            ip_burst,
            user_per_minute,
            user_burst,
        ))
    }

    pub fn new(ip_per_minute: u32, ip_burst: u32, user_per_minute: u32, user_burst: u32) -> Self {
        let ip_limit = RateLimiter::limit_from_per_minute(ip_per_minute, ip_burst);
        let user_limit = RateLimiter::limit_from_per_minute(user_per_minute, user_burst);
        Self {
            ip_buckets: Mutex::new(HashMap::new()),
            user_buckets: Mutex::new(HashMap::new()),
            ip_limit,
            user_limit,
        }
    }

    pub fn check_ip(&self, key: &str) -> RateLimitDecision {
        self.check_bucket(&self.ip_buckets, key, self.ip_limit)
    }

    pub fn check_user(&self, key: &str) -> RateLimitDecision {
        self.check_bucket(&self.user_buckets, key, self.user_limit)
    }

    fn limit_from_per_minute(per_minute: u32, burst: u32) -> RateLimit {
        if per_minute == 0 || burst == 0 {
            return RateLimit {
                capacity: 0.0,
                refill_per_sec: 0.0,
            };
        }
        let capacity = burst.max(1) as f64;
        let refill_per_sec = per_minute as f64 / 60.0;
        RateLimit {
            capacity,
            refill_per_sec,
        }
    }

    fn check_bucket(
        &self,
        buckets: &Mutex<HashMap<String, Bucket>>,
        key: &str,
        limit: RateLimit,
    ) -> RateLimitDecision {
        if limit.capacity <= 0.0 || limit.refill_per_sec <= 0.0 {
            return RateLimitDecision {
                allowed: true,
                retry_after_seconds: None,
            };
        }

        let now = Instant::now();
        let Ok(mut buckets) = crate::sync_lock::lock(buckets, "rate_limit.buckets") else {
            return RateLimitDecision {
                allowed: false,
                retry_after_seconds: Some(60),
            };
        };
        if buckets.len() > 50_000 {
            purge_stale(&mut buckets, now, Duration::from_secs(3600));
        }
        let bucket = buckets.entry(key.to_string()).or_insert(Bucket {
            tokens: limit.capacity,
            last_refill: now,
            last_seen: now,
        });

        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * limit.refill_per_sec).min(limit.capacity);
        bucket.last_refill = now;
        bucket.last_seen = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            RateLimitDecision {
                allowed: true,
                retry_after_seconds: None,
            }
        } else {
            let needed = 1.0 - bucket.tokens;
            let retry_after = (needed / limit.refill_per_sec).ceil().max(1.0) as u64;
            RateLimitDecision {
                allowed: false,
                retry_after_seconds: Some(retry_after),
            }
        }
    }
}

fn purge_stale(buckets: &mut HashMap<String, Bucket>, now: Instant, max_age: Duration) {
    buckets.retain(|_, bucket| now.duration_since(bucket.last_seen) <= max_age);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn poisoned_rate_limit_state_fails_closed() {
        let limiter = Arc::new(RateLimiter::new(60, 10, 60, 10));
        let poisoned = limiter.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.ip_buckets.lock().expect("initial lock");
            panic!("poison rate limiter");
        })
        .join();

        let decision = limiter.check_ip("198.51.100.9");
        assert!(!decision.allowed);
        assert_eq!(decision.retry_after_seconds, Some(60));
    }
}
