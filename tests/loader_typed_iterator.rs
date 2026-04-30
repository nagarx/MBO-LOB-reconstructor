//! Regression-lock tests for the typed iterator + LoaderStats observability
//! surface (Phase M REV 3 — M.A.8).
//!
//! These tests exercise the public crate-root re-exports via
//! `mbo_lob_reconstructor::*` so they catch any future refactor that
//! accidentally hides Phase M public API.
//!
//! Live DBN-fixture tests (covering decode/convert/mid-record-EOF on a real
//! synthetic stream) live in the inline `src/loader/mod.rs::tests` module
//! since they require crate-internal `DecodeRecord` mock infrastructure.
//! These tests focus on the LoaderStats public-field surface + the
//! crate-root re-exports.

#![cfg(feature = "databento")]

use mbo_lob_reconstructor::{BoundaryError, LoaderStats, TlobError};

#[test]
fn test_loader_stats_default_initializes_phase_m_fields_to_zero() {
    // Phase M cumulative: locks the LoaderStats public-field surface.
    // Any future refactor that silently deletes one of these fields will
    // fail this test (compile-error or assertion).
    let stats = LoaderStats::default();

    // M.A.2 (F-008 closure)
    assert_eq!(stats.bytes_read, 0);
    assert_eq!(stats.file_size, 0);

    // M.A.3 Decision 5b (mid-record EOF observability)
    assert_eq!(stats.mid_record_eof, 0);
    assert!(stats.is_clean_eof());

    // M.A.7 (F-010 producer-side system-message counter)
    assert_eq!(stats.system_messages_seen, 0);

    // Pre-Phase-M baseline (still guarded against silent deletion)
    assert_eq!(stats.messages_read, 0);
    assert_eq!(stats.messages_skipped, 0);
}

#[test]
fn test_loader_stats_is_clean_eof_distinguishes_torn_from_clean() {
    // Phase M M.A.3 Decision 5c: caller-decides abort/warn discipline.
    // The `is_clean_eof()` helper is the load-bearing query operators
    // use to detect torn DBN streams. Locks the boolean semantic.
    let mut stats = LoaderStats::default();
    assert!(stats.is_clean_eof(), "default state must be clean EOF");

    stats.mid_record_eof = 1;
    assert!(!stats.is_clean_eof(), "any torn record means NOT clean EOF");

    stats.mid_record_eof = 100;
    assert!(
        !stats.is_clean_eof(),
        "many torn records means NOT clean EOF"
    );

    stats.mid_record_eof = 0;
    assert!(
        stats.is_clean_eof(),
        "reset to zero must mean clean EOF again"
    );
}

#[test]
fn test_loader_stats_phase_m_counters_are_independent() {
    // Phase M cumulative: each counter tracks an independent anomaly class.
    // Locks that counters don't share storage / aliasing.
    let mut stats = LoaderStats::default();

    stats.bytes_read = 1_000_000;
    stats.messages_read = 50_000;
    stats.messages_skipped = 100;
    stats.system_messages_seen = 5_000;
    stats.mid_record_eof = 1;

    assert_eq!(stats.bytes_read, 1_000_000);
    assert_eq!(stats.messages_read, 50_000);
    assert_eq!(stats.messages_skipped, 100);
    assert_eq!(stats.system_messages_seen, 5_000);
    assert_eq!(stats.mid_record_eof, 1);
    assert!(!stats.is_clean_eof());
}

#[test]
fn test_boundary_error_decode_round_trip() {
    // Phase M M.A.1: locks the BoundaryError::Decode wire format. The
    // internal String message is the load-bearing payload that consumers
    // use to populate diagnostics dashboards.
    let err = BoundaryError::Decode("DBN parse failure: bad sync byte".to_string());
    let msg = err.to_string();
    assert!(
        msg.contains("DBN parse failure"),
        "Display impl must surface the inner message; got: {msg}"
    );
}

#[test]
fn test_boundary_error_convert_wraps_tlob_error() {
    // Phase M M.A.1: locks the BoundaryError::Convert wire format. The
    // wrapped TlobError preserves the variant (e.g., InvalidTimestamp,
    // InvalidPrice) so consumers can dispatch policy on the inner type.
    let err = BoundaryError::Convert(TlobError::InvalidTimestamp(0));
    let msg = err.to_string();
    assert!(
        msg.contains("Invalid timestamp"),
        "Display must surface the inner TlobError; got: {msg}"
    );
}

#[test]
fn test_boundary_error_clone() {
    // Phase M M.A.1 Decision 3 (REV 3): BoundaryError is Clone-derived to
    // mirror TlobError, so iterator combinators (Peekable / bench harness
    // clone-then-compare) work. Locks the derive against silent removal.
    let err1 = BoundaryError::Decode("test".to_string());
    let err2 = err1.clone();
    assert_eq!(err1.to_string(), err2.to_string());
}

#[test]
fn test_tlob_error_invalid_timestamp_variant() {
    // Phase M M.A.6 (F-023 closure): locks the new InvalidTimestamp variant.
    // External consumers checking for this anomaly class via Display must
    // see "Invalid timestamp:" prefix.
    let err = TlobError::InvalidTimestamp(0);
    assert!(err.to_string().contains("Invalid timestamp"));

    let err_neg = TlobError::InvalidTimestamp(-12345);
    assert!(err_neg.to_string().contains("-12345"));
}
