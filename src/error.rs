//! Error types for TLOB Rust core.
//!
//! Clean error handling using `thiserror` for ergonomic error definitions.

use thiserror::Error;

/// Result type alias for TLOB operations.
pub type Result<T> = std::result::Result<T, TlobError>;

/// Main error type for TLOB operations.
///
/// Phase M M.A.10 (REV 3 polish, post-validation V4 HIGH + A7 §6 closure):
/// `#[non_exhaustive]` discipline applied so future variant additions (M.A.6
/// `InvalidTimestamp` was technically a Rust SemVer break for any external
/// exhaustive match) become non-breaking. External crates pattern-matching on
/// `TlobError` MUST include a wildcard arm. Verified zero live exhaustive
/// matches in feature-extractor / mbo-statistical-profiler / opra-statistical-profiler
/// by Agent V1 cumulative ground-truth audit (2026-04-30).
#[derive(Error, Debug, Clone)]
#[non_exhaustive]
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

    /// A statistics counter would overflow `u64`.
    ///
    /// COMMIT 2b (the per-side carrier census). The carrier counters are
    /// incremented with `checked_add` rather than `+=` because they are the
    /// ACCEPTANCE SUBJECT of the per-carrier conjunction gate
    /// (`scripts/ci/check_carrier_sign.py`): a silently wrapped counter would
    /// be graded against the vendor census and read as a DECODE defect, which
    /// is the most expensive wrong answer this pipeline can produce.
    ///
    /// ⚠ WHY NOT `.expect()`, AND WHY NOT A SATURATING ADD. Overflow here is
    /// unreachable in practice — `u64::MAX` is ~1.8e19 against ~7e5 carrier
    /// records/day — so an `.expect()` would be dead code that can never be
    /// exercised, and `saturating_add` would report a WRONG count as if it
    /// were right, the silent-corruption class hft-rules §8 forbids. Returning
    /// an error keeps the failure path both real and REACHABLE: it is driven
    /// red by `carrier_census_overflow_is_fail_loud`, per hft-rules §1
    /// ("an instrument that cannot go red is not an instrument").
    ///
    /// ⚠ SCOPE OF "FAIL-LOUD", STATED HONESTLY (hft-rules §8 — fail-open vs
    /// fail-closed is a DECISION, stated at the site). This variant is loud in
    /// the LIBRARY. Its sole production consumer is NOT: `export_to_parquet`'s
    /// per-message loop absorbs an `Err(other)` into `rows_skipped_other` with
    /// a `log::warn!`, and the day still writes a stats file. Worse, the `?`
    /// fires BEFORE `stats.messages_processed += 1`, so the shortfall would
    /// surface downstream as G-SIGN's tier-1 `G2-processed-complements-skipped`
    /// — an arithmetic abort diagnosed as an ADMISSION defect, one level up
    /// from the mis-diagnosis `LobStats::accumulate`'s compute-then-commit
    /// exists to prevent. That is a doc-overclaim, not a live bug: overflow
    /// needs ~2.7e13 days at the observed ~6.8e5 carrier records/day. Changing
    /// the exporter's error policy is a separate decision and is recorded as
    /// owed rather than made here.
    ///
    /// The payload names the carrier, not the individual counter: the three
    /// slots of one carrier advance together (see `LobStats::accumulate`), so
    /// the carrier is the smallest unit that can fail.
    #[error("Statistics counter would overflow u64 for carrier: {0}")]
    CounterOverflow(&'static str),

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
