//! Shared bootstrap identity and source-format models for Codex Meter.

pub mod telemetry;

/// Human-readable product name.
pub const PRODUCT_NAME: &str = "Codex Meter";

/// Installed executable name.
pub const BINARY_NAME: &str = "codex-meter";

#[cfg(test)]
mod tests {
    use super::{BINARY_NAME, PRODUCT_NAME};

    #[test]
    fn product_identity_is_stable() {
        assert_eq!(PRODUCT_NAME, "Codex Meter");
        assert_eq!(BINARY_NAME, "codex-meter");
    }
}
