//! Field rules for monitored endpoints. The schema doc (#2.2) leaves ranges
//! to the application layer; the concrete limits are decided here.

use crate::error::ApiError;

pub const NAME_MAX_LEN: usize = 100;
/// api-spec #7's example message: "between 10 and 3600".
pub const INTERVAL_MIN_SECONDS: i32 = 10;
pub const INTERVAL_MAX_SECONDS: i32 = 3600;
/// Capped at the Checker's request timeout (10s, pulse-security.md #1): a
/// slower response is a timeout — i.e. a failed check counted in the error
/// rate — so a higher latency threshold could never trigger.
pub const LATENCY_THRESHOLD_MIN_MS: i32 = 1;
pub const LATENCY_THRESHOLD_MAX_MS: i32 = 10_000;
pub const ERROR_RATE_THRESHOLD_MIN: f64 = 0.0;
pub const ERROR_RATE_THRESHOLD_MAX: f64 = 100.0;
/// Must match the CHECK constraint in the endpoints migration.
pub const METHODS: [&str; 7] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

pub fn name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX_LEN {
        return Err(ApiError::validation(format!(
            "name must be between 1 and {NAME_MAX_LEN} characters"
        )));
    }
    Ok(name.to_owned())
}

pub fn method(raw: &str) -> Result<String, ApiError> {
    let method = raw.trim().to_ascii_uppercase();
    if METHODS.contains(&method.as_str()) {
        Ok(method)
    } else {
        Err(ApiError::validation(format!(
            "method must be one of {}",
            METHODS.join(", ")
        )))
    }
}

pub fn check_interval_seconds(v: i32) -> Result<i32, ApiError> {
    in_range(
        "check_interval_seconds",
        v,
        INTERVAL_MIN_SECONDS,
        INTERVAL_MAX_SECONDS,
    )
}

pub fn latency_threshold_ms(v: i32) -> Result<i32, ApiError> {
    in_range(
        "latency_threshold_ms",
        v,
        LATENCY_THRESHOLD_MIN_MS,
        LATENCY_THRESHOLD_MAX_MS,
    )
}

/// Rounded to 2 decimals to match the `NUMERIC(5,2)` column.
pub fn error_rate_threshold_percent(v: f64) -> Result<f64, ApiError> {
    let rounded = (v * 100.0).round() / 100.0;
    if rounded.is_finite()
        && (ERROR_RATE_THRESHOLD_MIN..=ERROR_RATE_THRESHOLD_MAX).contains(&rounded)
    {
        Ok(rounded)
    } else {
        Err(ApiError::validation(format!(
            "error_rate_threshold_percent must be between {ERROR_RATE_THRESHOLD_MIN} and {ERROR_RATE_THRESHOLD_MAX}"
        )))
    }
}

fn in_range(field: &str, v: i32, min: i32, max: i32) -> Result<i32, ApiError> {
    if (min..=max).contains(&v) {
        Ok(v)
    } else {
        Err(ApiError::validation(format!(
            "{field} must be between {min} and {max}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_trimmed_and_bounded() {
        assert_eq!(name("  Orders API ").unwrap(), "Orders API");
        assert!(name("   ").is_err());
        assert!(name(&"x".repeat(NAME_MAX_LEN)).is_ok());
        assert!(name(&"x".repeat(NAME_MAX_LEN + 1)).is_err());
    }

    #[test]
    fn method_is_uppercased_and_restricted() {
        assert_eq!(method("post").unwrap(), "POST");
        assert!(method("CONNECT").is_err());
        assert!(method("TRACE").is_err());
    }

    #[test]
    fn interval_bounds() {
        assert!(check_interval_seconds(9).is_err());
        assert!(check_interval_seconds(10).is_ok());
        assert!(check_interval_seconds(3600).is_ok());
        assert!(check_interval_seconds(3601).is_err());
    }

    #[test]
    fn latency_bounds() {
        assert!(latency_threshold_ms(0).is_err());
        assert!(latency_threshold_ms(1).is_ok());
        assert!(latency_threshold_ms(10_000).is_ok());
        assert!(latency_threshold_ms(10_001).is_err());
    }

    #[test]
    fn error_rate_bounds_and_rounding() {
        assert_eq!(error_rate_threshold_percent(5.004).unwrap(), 5.0);
        assert_eq!(error_rate_threshold_percent(0.0).unwrap(), 0.0);
        assert_eq!(error_rate_threshold_percent(100.0).unwrap(), 100.0);
        assert!(error_rate_threshold_percent(-0.01).is_err());
        assert!(error_rate_threshold_percent(100.01).is_err());
        assert!(error_rate_threshold_percent(f64::NAN).is_err());
    }
}
