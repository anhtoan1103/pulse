//! reqwest is built with `rustls-no-provider` (its default aws-lc backend
//! needs cmake/NASM to build on Windows), so a rustls crypto provider must be
//! installed before any HTTP client is built.

use std::sync::Once;

/// Installs the `ring` provider once per process. Idempotent.
pub fn install_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // Err only if a provider is already installed — fine either way.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}
