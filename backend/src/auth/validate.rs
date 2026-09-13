//! Input validation for credentials (pulse-security.md #4).

use crate::error::ApiError;

pub const PASSWORD_MIN_LEN: usize = 8;
/// Upper bound keeps attackers from making us hash megabyte-sized inputs.
pub const PASSWORD_MAX_LEN: usize = 128;
const EMAIL_MAX_LEN: usize = 254;

/// Trims + lowercases, then checks basic shape (`local@domain.tld`, no
/// whitespace). Real deliverability isn't checked — MVP has no email
/// verification flow.
pub fn normalize_email(raw: &str) -> Result<String, ApiError> {
    let email = raw.trim().to_lowercase();
    let valid = email.len() <= EMAIL_MAX_LEN
        && !email.chars().any(char::is_whitespace)
        && match email.split_once('@') {
            Some((local, domain)) => {
                !local.is_empty()
                    && !domain.contains('@')
                    && domain.contains('.')
                    && !domain.starts_with('.')
                    && !domain.ends_with('.')
            }
            None => false,
        };

    if valid {
        Ok(email)
    } else {
        Err(ApiError::validation("email is not a valid email address"))
    }
}

pub fn validate_password(password: &str) -> Result<(), ApiError> {
    let len = password.chars().count();
    if (PASSWORD_MIN_LEN..=PASSWORD_MAX_LEN).contains(&len) {
        Ok(())
    } else {
        Err(ApiError::validation(format!(
            "password must be between {PASSWORD_MIN_LEN} and {PASSWORD_MAX_LEN} characters"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_is_trimmed_and_lowercased() {
        assert_eq!(
            normalize_email("  Toan@Example.COM ").unwrap(),
            "toan@example.com"
        );
    }

    #[test]
    fn rejects_malformed_emails() {
        for bad in [
            "",
            "plainaddress",
            "@example.com",
            "a@b",
            "a@.com",
            "a@example.",
            "a@@example.com",
            "a b@example.com",
        ] {
            assert!(normalize_email(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(normalize_email(&format!("{}@example.com", "a".repeat(250))).is_err());
    }

    #[test]
    fn password_length_bounds() {
        assert!(validate_password(&"a".repeat(PASSWORD_MIN_LEN - 1)).is_err());
        assert!(validate_password(&"a".repeat(PASSWORD_MIN_LEN)).is_ok());
        assert!(validate_password(&"a".repeat(PASSWORD_MAX_LEN)).is_ok());
        assert!(validate_password(&"a".repeat(PASSWORD_MAX_LEN + 1)).is_err());
    }
}
