use governor::{clock::Clock as _, clock::DefaultClock, state::keyed::DefaultKeyedStateStore, Quota, RateLimiter};
use std::num::NonZeroU32;
use std::time::Duration;

pub struct UserRateLimiter {
    limiter: RateLimiter<String, DefaultKeyedStateStore<String>, DefaultClock>,
}

impl UserRateLimiter {
    pub fn new(requests_per_minute: u32) -> Self {
        let quota = Quota::per_minute(NonZeroU32::new(requests_per_minute).unwrap());
        Self {
            limiter: RateLimiter::keyed(quota),
        }
    }

    /// The per-minute budget from `GATEWAY_RATE_LIMIT_PER_MINUTE` (default
    /// 60). Host-owned policy: clients see it as `Retry-After`, never as a
    /// reason to guess.
    pub fn per_minute_from_env() -> u32 {
        std::env::var("GATEWAY_RATE_LIMIT_PER_MINUTE")
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .filter(|per_minute| *per_minute > 0)
            .unwrap_or(60)
    }

    pub fn check(&self, user_id: &str) -> Result<(), RateLimitError> {
        self.limiter
            .check_key(&user_id.to_string())
            .map_err(|not_until| RateLimitError::TooManyRequests {
                retry_after: not_until
                    .wait_time_from(DefaultClock::default().now())
                    .max(Duration::from_secs(1)),
            })
    }
}

#[derive(Debug)]
pub enum RateLimitError {
    /// The caller must wait this long before the next request succeeds.
    TooManyRequests { retry_after: Duration },
}

impl std::fmt::Display for RateLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rate limit exceeded")
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn test_rate_limiter_allows() {
        let limiter = UserRateLimiter::new(10);
        assert!(limiter.check("user1").is_ok());
    }

    #[test]
    fn a_refusal_says_how_long_to_wait() {
        let limiter = UserRateLimiter::new(1);
        limiter.check("user").unwrap();
        let error = limiter.check("user").unwrap_err();
        let RateLimitError::TooManyRequests { retry_after } = error;
        // `Retry-After` is the whole protocol for backing off: never zero.
        assert!(retry_after.as_secs() >= 1, "{retry_after:?}");
    }

    #[test]
    fn the_budget_is_operator_policy() {
        // An unset or invalid value falls back to the documented default.
        assert_eq!(UserRateLimiter::per_minute_from_env(), 60);
    }

    #[test]
    fn test_rate_limiter_blocks() {
        let limiter = UserRateLimiter::new(1);
        assert!(limiter.check("user1").is_ok());
        assert!(limiter.check("user1").is_err());
    }
}
