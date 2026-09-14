//! Notification Service (docs/pulse-architecture.md #2.6): an outbox table
//! filled by event producers, delivered by email from the worker.

pub mod digest_email;
pub mod email;
pub mod incident_email;
pub mod service;

/// Collapses control chars/whitespace (including newlines) to single spaces
/// so untrusted text (an endpoint name, AI-generated text) can't inject
/// extra lines into a plaintext email. Shared by every email renderer in
/// this module — previously duplicated per-renderer, which is exactly the
/// kind of defense that's easy to strengthen in one copy and forget in
/// another.
pub(crate) fn one_line(text: &str) -> String {
    text.split(|c: char| c.is_control() || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_newlines_and_control_chars_to_single_spaces() {
        assert_eq!(one_line("a\nb\r\nc\td"), "a b c d");
        assert_eq!(one_line("  leading and trailing  "), "leading and trailing");
        assert_eq!(one_line(""), "");
    }
}
