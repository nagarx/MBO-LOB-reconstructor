//! Regression-lock tests for `LobStats` counter wire-ups (Phase M REV 3 — M.A.8).
//!
//! Cross-repo integration coverage for the producer-side observability surface
//! that closes F-007 + F-013 + F-034 + envelope schema bump. These tests live
//! at the integration tier (vs the inline `src/lob/reconstructor.rs::tests`
//! which test internals) so they exercise the public `pub use lob::*` surface
//! exposed at the crate root + the on-disk JSON wire format.

use mbo_lob_reconstructor::{
    Action, LobReconstructor, LobStats, LobStatsExportEnvelope, MboMessage, Side,
    LOB_STATS_SCHEMA_VERSION,
};

/// Helper — construct a test message via the public `MboMessage::new` API.
/// Price in nanodollars (i64 fixed-point). System messages are detected by the
/// loader/lob via `is_system_message()` (order_id == 0 || size == 0 || price <= 0).
fn msg(order_id: u64, action: Action, side: Side, price_dollars: f64, size: u32) -> MboMessage {
    MboMessage::new(order_id, action, side, (price_dollars * 1e9) as i64, size)
}

#[test]
fn test_modify_order_not_found_increments_on_silent_fall_through() {
    // Phase M M.A.4 (REV 3 F-013 closure): pre-M.A.4, `modify_order` on a
    // missing order_id silently fell through to `add_order(msg)` creating a
    // NEW order at the modify message's price. Post-M.A.4 the recovery path
    // is preserved bit-for-bit, but the
    // `LobStats::modify_order_not_found` counter now increments BEFORE the
    // fall-through so operators can audit modify-of-missing rates.
    let mut lob = LobReconstructor::new(10);

    // Modify an order that was never added → silent fall-through to add_order.
    // The modify SHOULD succeed (recovery semantic) but the counter must increment.
    let modify_msg = msg(99999, Action::Modify, Side::Bid, 100.0, 100);
    let _ = lob.process_message(&modify_msg);

    let stats = lob.stats();
    assert!(
        stats.modify_order_not_found >= 1,
        "modify-of-missing must increment counter; got: {}",
        stats.modify_order_not_found
    );
}

#[test]
fn test_add_order_id_collision_increments_on_silent_fall_through() {
    // Phase M M.A.4 (REV 3 F-013 sibling closure): pre-M.A.4,
    // `add_order(msg)` on an order_id that already existed silently fell
    // through to `modify_order(msg)`. Post-M.A.4 the recovery path is
    // preserved but the `LobStats::add_order_id_collision` counter now
    // increments before fall-through.
    let mut lob = LobReconstructor::new(10);

    // First add — clean.
    let add1 = msg(12345, Action::Add, Side::Bid, 100.0, 100);
    let _ = lob.process_message(&add1);

    // Second add with same order_id — silent fall-through to modify_order.
    let add2 = msg(12345, Action::Add, Side::Bid, 101.0, 200);
    let _ = lob.process_message(&add2);

    let stats = lob.stats();
    assert!(
        stats.add_order_id_collision >= 1,
        "add-of-existing-id must increment counter; got: {}",
        stats.add_order_id_collision
    );
}

#[test]
fn test_lobstats_default_initializes_all_counters_to_zero() {
    // Phase M M.A.4 + M.A.7: locks the field surface against silent
    // refactor-deletion. If a future commit deletes any of these fields
    // this test will fail to compile.
    let stats = LobStats::default();
    assert_eq!(stats.messages_processed, 0);
    assert_eq!(stats.system_messages_skipped, 0);
    assert_eq!(stats.cancel_order_not_found, 0);
    assert_eq!(stats.cancel_price_level_missing, 0);
    assert_eq!(stats.cancel_order_at_level_missing, 0);
    assert_eq!(stats.trade_order_not_found, 0);
    assert_eq!(stats.trade_price_level_missing, 0);
    assert_eq!(stats.trade_order_at_level_missing, 0);
    assert_eq!(stats.modify_order_not_found, 0);
    assert_eq!(stats.add_order_id_collision, 0);
    assert_eq!(stats.book_clears, 0);
    assert_eq!(stats.noop_messages, 0);
    assert_eq!(stats.crossed_quotes, 0);
    assert_eq!(stats.locked_quotes, 0);
    assert_eq!(stats.last_timestamp, None);
    assert!(!stats.has_warnings(), "default state must have no warnings");
    assert_eq!(stats.total_warnings(), 0);
}

#[test]
fn test_lobstats_export_envelope_round_trip() {
    // Phase M M.A.5: end-to-end envelope round-trip via `pub use lob::*`
    // crate-root re-exports. Locks both serialization shape AND wire-format
    // backwards-compat (legacy reads + envelope writes).
    let mut lob = LobReconstructor::new(10);
    let _ = lob.process_message(&msg(1, Action::Add, Side::Bid, 100.0, 100));
    let _ = lob.process_message(&msg(2, Action::Add, Side::Ask, 101.0, 200));
    let _ = lob.process_message(&msg(99999, Action::Modify, Side::Bid, 100.5, 50)); // increments counter
    let stats_before = lob.stats().clone();
    assert!(stats_before.modify_order_not_found >= 1);

    let dir = std::env::temp_dir().join("lobstats_envelope_round_trip_test_M_A_8");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("envelope.json");

    stats_before.export_to_file(&path).unwrap();

    // Verify on-disk envelope shape via crate-root LobStatsExportEnvelope re-export.
    let json = std::fs::read_to_string(&path).unwrap();
    let envelope: LobStatsExportEnvelope = serde_json::from_str(&json).unwrap();
    assert_eq!(envelope.schema_version, LOB_STATS_SCHEMA_VERSION);
    assert_eq!(
        envelope.stats.modify_order_not_found,
        stats_before.modify_order_not_found
    );

    // Round-trip via load_from_file (envelope branch).
    let stats_after = LobStats::load_from_file(&path).unwrap();
    assert_eq!(
        stats_after.modify_order_not_found,
        stats_before.modify_order_not_found
    );
    assert_eq!(
        stats_after.add_order_id_collision,
        stats_before.add_order_id_collision
    );
    assert_eq!(stats_after.last_timestamp, stats_before.last_timestamp);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn test_lobstats_schema_version_constant_is_pinned() {
    // Phase M M.A.5: the public const must remain pinned at "2.0.0" until
    // the next intentional MAJOR bump. If a future commit silently bumps
    // this without a coordinated cycle, this test will fail.
    assert_eq!(
        LOB_STATS_SCHEMA_VERSION, "2.0.0",
        "LOB_STATS_SCHEMA_VERSION must remain pinned at 2.0.0 until next intentional MAJOR bump"
    );
}

#[test]
fn test_lobstats_envelope_load_rejects_malformed_envelope() {
    // Phase M M.A.5 hardening (post-validation Agent 3 MEDIUM): an envelope-
    // claimed JSON (top-level `schema_version` present) but missing the
    // `stats` wrapper MUST fail-loud per hft-rules §5. Pre-hardening the
    // `#[serde(untagged)]` enum silently routed this through the legacy
    // arm dropping `schema_version`. Post-hardening the explicit Value-peek
    // dispatch raises `std::io::Error::other`.
    let dir = std::env::temp_dir().join("lobstats_malformed_envelope_external_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("malformed.json");

    let malformed = r#"{
        "schema_version": "2.0.0",
        "messages_processed": 99,
        "system_messages_skipped": 0,
        "active_orders": 0,
        "bid_levels": 0,
        "ask_levels": 0,
        "crossed_quotes": 0,
        "locked_quotes": 0,
        "last_timestamp": null,
        "cancel_order_not_found": 0,
        "cancel_price_level_missing": 0,
        "cancel_order_at_level_missing": 0,
        "trade_order_not_found": 0,
        "trade_price_level_missing": 0,
        "trade_order_at_level_missing": 0,
        "book_clears": 0,
        "noop_messages": 0
    }"#;
    std::fs::write(&path, malformed).unwrap();

    let result = LobStats::load_from_file(&path);
    assert!(
        result.is_err(),
        "malformed envelope (top-level schema_version, missing stats wrapper) MUST fail-loud"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn test_lobstats_has_warnings_includes_phase_m_counters() {
    // Phase M M.A.4 (REV 3 F-013 closure): the new
    // `modify_order_not_found` + `add_order_id_collision` counters MUST be
    // included in the `has_warnings()` + `total_warnings()` aggregations
    // so existing operator dashboards that surface "any warning" detect
    // these new anomaly classes too. Locks the integration into
    // pre-existing observability surface.
    let mut stats = LobStats::default();
    assert!(!stats.has_warnings());
    assert_eq!(stats.total_warnings(), 0);

    stats.modify_order_not_found = 5;
    assert!(stats.has_warnings());
    assert_eq!(stats.total_warnings(), 5);

    stats.add_order_id_collision = 3;
    assert!(stats.has_warnings());
    assert_eq!(stats.total_warnings(), 8);
}

#[test]
fn test_export_envelope_load_round_trip_preserves_default_state() {
    // Boundary case: a fresh LobReconstructor (no messages processed) exports
    // a default-state envelope; reload preserves the all-zero state.
    let lob = LobReconstructor::new(10);
    let stats_before = lob.stats().clone();

    let dir = std::env::temp_dir().join("lobstats_default_round_trip_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("default.json");

    stats_before.export_to_file(&path).unwrap();
    let stats_after = LobStats::load_from_file(&path).unwrap();

    assert_eq!(stats_after.messages_processed, 0);
    assert_eq!(stats_after.modify_order_not_found, 0);
    assert_eq!(stats_after.add_order_id_collision, 0);
    assert_eq!(stats_after.last_timestamp, None);
    assert!(!stats_after.has_warnings());

    std::fs::remove_dir_all(&dir).ok();
}
