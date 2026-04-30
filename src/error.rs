//! Error types for TLOB Rust core.
//!
//! Clean error handling using `thiserror` for ergonomic error definitions.

use thiserror::Error;

/// Result type alias for TLOB operations.
pub type Result<T> = std::result::Result<T, TlobError>;

/// Main error type for TLOB operations.
#[derive(Error, Debug, Clone)]
pub enum TlobError {
    /// Invalid order ID (e.g., zero or duplicate)
    #[error("Invalid order ID: {0}")]
    InvalidOrderId(u64),

    /// Order not found in LOB state
    #[error("Order not found: {0}")]
    OrderNotFound(u64),

    /// Invalid price (e.g., zero or negative)
    #[error("Invalid price: {0}")]
    InvalidPrice(i64),

    /// Invalid size (e.g., zero or negative)
    #[error("Invalid size: {0}")]
    InvalidSize(u32),

    /// Invalid timestamp (zero, negative, or u64→i64 overflow).
    ///
    /// Phase M M.A.6 (REV 3 F-023 closure): Databento DBN feeds occasionally
    /// emit `hd.ts_event = 0` as a sentinel for "no timestamp" (e.g., on
    /// session-control messages). Pre-M.A.6, [`crate::dbn_bridge::DbnBridge::convert`]
    /// silently coerced this to `Some(0)`, propagating the sentinel as if it
    /// were a real wall-clock timestamp. Post-M.A.6 the conversion fails-loud
    /// with this variant, which the `TypedMessageIterator` then wraps as
    /// [`crate::loader::BoundaryError::Convert`]. Per hft-rules §8 — never
    /// silently coerce; surface the anomaly so consumers can decide policy.
    #[error("Invalid timestamp: {0}")]
    InvalidTimestamp(i64),

    /// Invalid action type
    #[error("Invalid action: {0}")]
    InvalidAction(u8),

    /// Invalid side (must be Bid or Ask)
    #[error("Invalid side: {0}")]
    InvalidSide(u8),

    /// Symbol not found (for multi-symbol processor)
    #[error("Symbol not found: {0}")]
    SymbolNotFound(String),

    /// LOB state inconsistency detected
    #[error("LOB inconsistency: {0}")]
    InconsistentState(String),

    /// Crossed quote detected (bid >= ask, invalid market state)
    #[error("Crossed quote detected: best_bid={0} >= best_ask={1}")]
    CrossedQuote(i64, i64),

    /// Locked quote detected (bid == ask, unusual but can occur)
    #[error("Locked quote detected: best_bid={0} == best_ask={1}")]
    LockedQuote(i64, i64),

    /// Invalid configuration parameter
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    /// Generic error with context
    #[error("Error: {0}")]
    Generic(String),
}

impl TlobError {
    /// Create a generic error from any string-like type.
    pub fn generic(msg: impl Into<String>) -> Self {
        TlobError::Generic(msg.into())
    }
}

// Implement From for common error types for ergonomic error handling
impl From<std::io::Error> for TlobError {
    fn from(err: std::io::Error) -> Self {
        TlobError::Generic(format!("IO error: {err}"))
    }
}

impl From<String> for TlobError {
    fn from(err: String) -> Self {
        TlobError::Generic(err)
    }
}

impl From<&str> for TlobError {
    fn from(err: &str) -> Self {
        TlobError::Generic(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = TlobError::InvalidOrderId(12345);
        assert_eq!(err.to_string(), "Invalid order ID: 12345");
    }

    #[test]
    fn test_result_type() {
        let result: Result<i32> = Err(TlobError::InvalidPrice(-100));
        assert!(result.is_err());
    }
}
