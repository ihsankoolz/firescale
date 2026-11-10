//! Core engine for firescale.
//!
//! In M0 this crate only exposes its own version, which is enough to prove the
//! Rust -> Node bridge works. File walking, hashing, the cache and the
//! scheduler arrive in M2.

/// The version of the firescale core, taken from the Cargo workspace version.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn version_is_semver_like() {
        let parts: Vec<&str> = version().split('.').collect();
        assert_eq!(
            parts.len(),
            3,
            "expected MAJOR.MINOR.PATCH, got {}",
            version()
        );
        for part in parts {
            assert!(
                part.chars().all(|c| c.is_ascii_digit()),
                "non-numeric part in {}",
                version()
            );
        }
    }
}
