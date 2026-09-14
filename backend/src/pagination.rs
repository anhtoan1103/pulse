//! Shared `limit` query-param handling for list endpoints
//! (`docs/pulse-api-spec.md` #3's `checks` endpoint, #4's `incidents`).
//!
//! A single validated helper, rather than each handler reimplementing its
//! own bound: two independent copies had already drifted apart (one clamped
//! out-of-range values silently, the other returned a 422) before this was
//! factored out — the same input shape now behaves the same way everywhere.

use crate::error::ApiError;

pub const DEFAULT_LIMIT: i64 = 100;
pub const MAX_LIMIT: i64 = 1000;

/// `None` becomes [`DEFAULT_LIMIT`]; anything outside `1..=MAX_LIMIT` is a
/// 422, not a silent clamp — the caller asked for something nonsensical and
/// should be told, not have it quietly reinterpreted.
pub fn clamp_limit(limit: Option<i64>) -> Result<i64, ApiError> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    if (1..=MAX_LIMIT).contains(&limit) {
        Ok(limit)
    } else {
        Err(ApiError::validation(format!(
            "limit must be between 1 and {MAX_LIMIT}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_absent() {
        assert_eq!(clamp_limit(None).unwrap(), DEFAULT_LIMIT);
    }

    #[test]
    fn accepts_the_bounds() {
        assert_eq!(clamp_limit(Some(1)).unwrap(), 1);
        assert_eq!(clamp_limit(Some(MAX_LIMIT)).unwrap(), MAX_LIMIT);
    }

    #[test]
    fn rejects_out_of_range_instead_of_clamping() {
        assert!(clamp_limit(Some(0)).is_err());
        assert!(clamp_limit(Some(-1)).is_err());
        assert!(clamp_limit(Some(MAX_LIMIT + 1)).is_err());
    }
}
