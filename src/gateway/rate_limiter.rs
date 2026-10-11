use governor::{clock::Clock as _, clock::DefaultClock, state::keyed::DefaultKeyedStateStore, Quota, RateLimiter};
use std::num::NonZeroU32;
use std::time::Duration;

/// Two budgets per caller, because a client is not one kind of request.
///
/// A dashboard tab polls every 3 s and a TUI polls every 5 s: on a single
/// 60/minute budget the polling eats the quota before the first chat message
/// is even sent, and the client sees a 429 for something it never asked for.
/// Read-only traffic therefore gets its own, much larger budget, and the
/// default is sized for a real interactive session instead of a hostile one.
pub struct UserRateLimiter {
    write: RateLimiter<String, DefaultKeyedStateStore<String>, DefaultClock>,
    read: RateLimiter<String, DefaultKeyedStateStore<String>, DefaultClock>,
    write_per_minute: u32,
    read_per_minute: u32,
}

/// Sized for one interactive client plus a browser tab, not for a swarm.
pub const DEFAULT_WRITE_PER_MINUTE: u32 = 240;
pub const DEFAULT_READ_PER_MINUTE: u32 = 1200;

impl UserRateLimiter {
    pub fn new(requests_per_minute: u32) -> Self {
        Self::with_budgets(requests_per_minute, requests_per_minute.saturating_mul(5).max(1))
    }

    pub fn with_budgets(write_per_minute: u32, read_per_minute: u32) -> Self {
        Self {
            write: RateLimiter::keyed(Quota::per_minute(NonZeroU32::new(write_per_minute).unwrap())),
            read: RateLimiter::keyed(Quota::per_minute(NonZeroU32::new(read_per_minute).unwrap())),
            write_per_minute,
            read_per_minute,
        }
    }

    /// The write budget from `GATEWAY_RATE_LIMIT_PER_MINUTE` (default
    /// `DEFAULT_WRITE_PER_MINUTE`). Host-owned policy: clients see it as
    /// `Retry-After`, never as a reason to guess.
    pub fn per_minute_from_env() -> u32 {
        env_per_minute("GATEWAY_RATE_LIMIT_PER_MINUTE", DEFAULT_WRITE_PER_MINUTE)
    }

    /// The read/polling budget from `GATEWAY_RATE_LIMIT_READ_PER_MINUTE`
    /// (default `DEFAULT_READ_PER_MINUTE`).
    pub fn read_per_minute_from_env() -> u32 {
        env_per_minute("GATEWAY_RATE_LIMIT_READ_PER_MINUTE", DEFAULT_READ_PER_MINUTE)
    }

    pub fn budgets(&self) -> (u32, u32) {
        (self.write_per_minute, self.read_per_minute)
    }

    pub fn check(&self, user_id: &str, read_only: bool) -> Result<(), RateLimitError> {
        let limiter = if read_only { &self.read } else { &self.write };
        limiter
            .check_key(&user_id.to_string())
            .map_err(|not_until| RateLimitError::TooManyRequests {
                budget: if read_only { "read" } else { "write" },
                retry_after: not_until
                    .wait_time_from(DefaultClock::default().now())
                    .max(Duration::from_secs(1)),
            })
    }
}

fn env_per_minute(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .filter(|per_minute| *per_minute > 0)
        .unwrap_or(default)
}

#[derive(Debug)]
pub enum RateLimitError {
    /// The caller must wait this long before the next request succeeds.
    TooManyRequests { budget: &'static str, retry_after: Duration },
}

impl std::fmt::Display for RateLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyRequests { budget, .. } => write!(f, "Rate limit exceeded ({budget} budget)"),
        }
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn test_rate_limiter_allows() {
        let limiter = UserRateLimiter::new(10);
        assert!(limiter.check("user1", false).is_ok());
    }

    #[test]
    fn a_refusal_says_how_long_to_wait() {
        let limiter = UserRateLimiter::new(1);
        limiter.check("user", false).unwrap();
        let error = limiter.check("user", false).unwrap_err();
        let RateLimitError::TooManyRequests { retry_after, .. } = error;
        // `Retry-After` is the whole protocol for backing off: never zero.
        assert!(retry_after.as_secs() >= 1, "{retry_after:?}");
    }

    #[test]
    fn the_budget_is_operator_policy() {
        // An unset or invalid value falls back to the documented default.
        assert_eq!(UserRateLimiter::per_minute_from_env(), DEFAULT_WRITE_PER_MINUTE);
        assert_eq!(UserRateLimiter::read_per_minute_from_env(), DEFAULT_READ_PER_MINUTE);
    }

    #[test]
    fn test_rate_limiter_blocks() {
        let limiter = UserRateLimiter::new(1);
        assert!(limiter.check("user1", false).is_ok());
        assert!(limiter.check("user1", false).is_err());
    }

    #[test]
    fn polling_does_not_steal_the_chat_budget() {
        // A tab that polls every 3 s must not 429 the first chat message.
        let limiter = UserRateLimiter::with_budgets(2, 10);
        for _ in 0..10 {
            assert!(limiter.check("user", true).is_ok(), "polling must stay in the read budget");
        }
        assert!(limiter.check("user", false).is_ok(), "chat still has its own budget");
        assert!(limiter.check("user", false).is_ok());
        assert!(limiter.check("user", false).is_err());
    }

    #[test]
    fn the_default_is_big_enough_for_one_interactive_client() {
        // 5 s polling for a full minute is 12 read requests; the old default of
        // 60 total would have refused a normal session.
        let limiter = UserRateLimiter::with_budgets(DEFAULT_WRITE_PER_MINUTE, DEFAULT_READ_PER_MINUTE);
        for _ in 0..120 {
            assert!(limiter.check("user", true).is_ok());
        }
        for _ in 0..200 {
            assert!(limiter.check("user", false).is_ok());
        }
    }
}
