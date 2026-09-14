//! Exponential retry delays shared by background jobs (AI analysis, email
//! delivery).

use std::time::Duration;

/// `base × 2^(attempt-1)`, capped at `max`. Attempt numbers start at 1.
pub fn exponential(attempt: i32, base: Duration, max: Duration) -> Duration {
    let exponent = attempt.saturating_sub(1).clamp(0, 16) as u32;
    base.saturating_mul(2u32.saturating_pow(exponent)).min(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_and_caps() {
        let (base, max) = (Duration::from_secs(60), Duration::from_secs(1800));
        assert_eq!(exponential(0, base, max), base);
        assert_eq!(exponential(1, base, max), base);
        assert_eq!(exponential(2, base, max), Duration::from_secs(120));
        assert_eq!(exponential(4, base, max), Duration::from_secs(480));
        assert_eq!(exponential(10, base, max), max);
        assert_eq!(exponential(i32::MAX, base, max), max);
    }
}
