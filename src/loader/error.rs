//! Boundary errors for the loader module.
//!
//! Phase M (REV 3 boundary discipline cycle): `BoundaryError` is a peer enum
//! to [`crate::error::TlobError`], NOT a variant. Decode + Convert errors
//! propagate through the typed iterator
//! (`Iterator<Item = Result<MboMessage, BoundaryError>>`) at compile-time so
//! consumers must explicitly handle boundary failures via `?` propagation or
//! pattern matching.
//!
//! **Mid-record EOF is NOT a variant**: it lives in `LoaderStats::mid_record_eof`
//! counter (added in M.A.3) and surfaces via `LoaderStats::is_clean_eof()`
//! post-iteration (Decision 5b — observability-tier, not type-domain).

use thiserror::Error;

use crate::error::TlobError;

/// Boundary errors at the DBN-loader iterator surface.
///
/// Distinct from [`TlobError`] (book-state invariants). The typed iterator
/// `iter_messages_typed()` yields `Result<MboMessage, BoundaryError>` so
/// callers must explicitly handle boundary failures via `?` propagation or
/// pattern matching.
///
/// # Variants
///
/// - [`Decode`](BoundaryError::Decode): DBN decoder rejected a record
///   (corrupt frame, invalid `rtype`, etc.). The source is stored as
///   `String` to preserve `Clone` semantics (per Phase M Decision 3 —
///   `dbn::Error` does not derive Clone); fine-grained variant info is
///   preserved via the Display string.
/// - [`Convert`](BoundaryError::Convert): `DbnBridge::convert` failed
///   (out-of-spec field). Carries the underlying [`TlobError`] for
///   downstream pattern-matching on `InvalidAction`, `InvalidSide`, etc.
///
/// # Why no `MidRecordEof` variant
///
/// Mid-record EOF is operational, not type-domain. It's counted in
/// `LoaderStats::mid_record_eof` (added in M.A.3) and surfaced post-iteration
/// via `LoaderStats::is_clean_eof()`. The iterator returns `None` on both
/// clean EOF and mid-record EOF; callers distinguish at finalize() time.
///
/// # Why no `From<TlobError>`
///
/// Per Phase M Decision 3: `BoundaryError` and `TlobError` are separate enums
/// by design. Convert error sites use `BoundaryError::Convert(source)`
/// explicitly, NOT `?` propagation across the enum boundary. This preserves
/// the type-system signal "I/O domain vs book-state domain" at the iterator
/// surface.
///
/// # Examples
///
/// ```ignore
/// use mbo_lob_reconstructor::{BoundaryError, DbnLoader};
///
/// let loader = DbnLoader::new("data.dbn.zst")?;
/// let mut iter = loader.iter_messages_typed()?;
/// for msg_result in &mut iter {
///     let msg = msg_result?;  // BoundaryError::Decode/Convert propagates
///     // ... process msg ...
/// }
/// let stats = iter.finalize();
/// if !stats.is_clean_eof() {
///     // mid-record EOF detected; counter at stats.mid_record_eof
/// }
/// ```
#[derive(Error, Debug, Clone)]
#[non_exhaustive]
pub enum BoundaryError {
    /// DBN decoder rejected a record (corrupt frame, invalid `rtype`, etc.).
    ///
    /// Returned when `dbn::DynDecoder::decode_record::<dbn::MboMsg>()` returns
    /// `Err(_)` and `skip_invalid=false`. Source string is the
    /// `dbn::Error::to_string()` output so variant context is preserved.
    ///
    /// With `skip_invalid=true`, decode errors are logged and counted in
    /// `LoaderStats::messages_skipped` without being surfaced.
    #[error("DBN decoder rejected record: {0}")]
    Decode(String),

    /// `DbnBridge::convert` failed (out-of-spec field).
    ///
    /// Carries the underlying [`TlobError`] (e.g. `InvalidAction(u8)`,
    /// `InvalidSide(u8)`, `Generic(String)`) so downstream consumers can
    /// pattern-match on the specific failure mode.
    #[error("DBN→MboMessage conversion failed: {0}")]
    Convert(TlobError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_error_decode_display() {
        let err = BoundaryError::Decode("invalid rtype: 99".to_string());
        assert_eq!(
            err.to_string(),
            "DBN decoder rejected record: invalid rtype: 99"
        );
    }

    #[test]
    fn boundary_error_convert_carries_tlob_variant() {
        let inner = TlobError::InvalidAction(99);
        let err = BoundaryError::Convert(inner.clone());
        assert_eq!(
            err.to_string(),
            "DBN→MboMessage conversion failed: Invalid action: 99"
        );
        // Verify variant pattern-matching works
        match err {
            BoundaryError::Convert(TlobError::InvalidAction(code)) => {
                assert_eq!(code, 99);
            }
            _ => panic!("expected Convert(InvalidAction)"),
        }
    }

    #[test]
    fn boundary_error_is_clone() {
        // Decision 3: BoundaryError must derive Clone for iterator
        // combinator support (Peekable, bench harness clone-then-compare).
        let err = BoundaryError::Decode("test".to_string());
        let cloned = err.clone();
        assert_eq!(err.to_string(), cloned.to_string());

        let err2 = BoundaryError::Convert(TlobError::InvalidSide(42));
        let cloned2 = err2.clone();
        assert_eq!(err2.to_string(), cloned2.to_string());
    }
}
