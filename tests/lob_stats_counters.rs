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

/// BEHAVIOURAL POSITIVE CONTROL for the Fill oracle's `fill_side_mismatch`
/// alarm channel.
///
/// # Why this test had to exist
///
/// `fill_side_mismatch` has exactly ONE producer (`observe_resting_fill` in
/// `src/lob/reconstructor.rs`). Before this test its ONLY assertions were a
/// serde round-trip over a hand-written `LobStats` struct literal — which never
/// calls the producer. So **nothing exercised it**, and an adversary proved the
/// consequence: prefixing the producer's condition with `if false &&` left the
/// whole suite GREEN and every live-data counter BIT-IDENTICAL. Compounded with
/// a consistent bid/ask inversion, the entire acceptance channel matched the
/// correct fix on all nine counters, both days, **with the book mirrored**.
///
/// That is absence indistinguishable from agreement — exactly what hft-rules §1
/// forbids — sitting inside this commit's own new oracle.
///
/// # Why all four arms are load-bearing
///
/// * POSITIVE — a genuine side disagreement must increment by EXACTLY 1.
/// * NEGATIVE — a matching side must NOT increment, so the test cannot be
///   satisfied by a counter that simply always fires.
/// * `Side::None` — must NOT increment. Venues publish `N` on fills; that is an
///   absence of information, not a disagreement. This arm pins the
///   `msg.side != Side::None &&` half of the predicate, which a "simplifying"
///   edit would otherwise silently delete.
/// * `resting_fills_observed` is asserted in every arm. Without it, a `Fill`
///   that never reached the oracle at all would read as a clean zero — the same
///   absence-vs-agreement confusion one level up.
#[test]
fn test_fill_side_mismatch_increments_only_on_a_real_side_disagreement() {
    // ---- POSITIVE: resting BID, vendor asserts the fill was on the ASK. ----
    let mut lob = LobReconstructor::new(10);
    lob.process_message(&msg(1001, Action::Add, Side::Bid, 100.00, 500))
        .expect("seed add must succeed");
    lob.process_message(&msg(1001, Action::Fill, Side::Ask, 100.00, 50))
        .expect("a Fill must never error");

    let st = lob.stats();
    assert_eq!(
        st.resting_fills_observed, 1,
        "the oracle must have RUN; a zero here would make the mismatch assertion \
         below vacuous for the wrong reason"
    );
    assert_eq!(
        st.fill_side_mismatch, 1,
        "order 1001 rests on the BID and the vendor's Fill claims the ASK — that \
         is a side disagreement and must be counted exactly once"
    );
    assert_eq!(
        st.fill_price_differs_from_resting, 0,
        "SEPARATION: the price MATCHED here. Side and price are two counters \
         precisely so a benign price-difference rate (expected at a few percent, \
         because a fill's EXECUTION price may differ from the resting DISPLAY \
         price) cannot mask a must-be-zero wrong-side defect. A non-zero here \
         means the two were merged."
    );
    assert_eq!(
        st.fill_referenced_unknown_order, 0,
        "the order WAS resting; this is a side disagreement, not a lookup miss"
    );

    // ---- NEGATIVE: same book, matching side. Must NOT increment. ----
    let mut lob = LobReconstructor::new(10);
    lob.process_message(&msg(1001, Action::Add, Side::Bid, 100.00, 500))
        .expect("seed add must succeed");
    lob.process_message(&msg(1001, Action::Fill, Side::Bid, 100.00, 50))
        .expect("a Fill must never error");

    let st = lob.stats();
    assert_eq!(st.resting_fills_observed, 1, "the oracle must have RUN");
    assert_eq!(
        st.fill_side_mismatch, 0,
        "the sides AGREE, so the alarm must stay silent. If this fires, the \
         counter is unconditional and the positive arm above proves nothing."
    );

    // ---- Side::None: an absence of information, NOT a disagreement. ----
    let mut lob = LobReconstructor::new(10);
    lob.process_message(&msg(1001, Action::Add, Side::Bid, 100.00, 500))
        .expect("seed add must succeed");
    lob.process_message(&msg(1001, Action::Fill, Side::None, 100.00, 50))
        .expect("a Fill must never error");

    let st = lob.stats();
    assert_eq!(st.resting_fills_observed, 1, "the oracle must have RUN");
    assert_eq!(
        st.fill_side_mismatch, 0,
        "`Side::None` on a fill is an absence of information, not a \
         disagreement. Dropping the `msg.side != Side::None` guard would turn \
         every N-sided venue's fill population into a false alarm."
    );
}

/// BEHAVIOURAL POSITIVE CONTROL for `fill_price_differs_from_resting`.
///
/// Sibling of the side-mismatch control above, and added for the SAME measured
/// reason: this counter also had exactly one producer and no assertion that
/// ever called it — only the serde round-trip. It is the benign half of the
/// pair, so its failure mode is the mirror image: if it silently stopped
/// counting, the *expected* few-percent signal would read as a suspiciously
/// perfect zero and be mistaken for conformance.
#[test]
fn test_fill_price_differs_from_resting_increments_only_on_a_real_price_difference() {
    // POSITIVE: side matches, execution price differs from the resting display price.
    let mut lob = LobReconstructor::new(10);
    lob.process_message(&msg(2002, Action::Add, Side::Ask, 100.05, 300))
        .expect("seed add must succeed");
    lob.process_message(&msg(2002, Action::Fill, Side::Ask, 100.06, 50))
        .expect("a Fill must never error");

    let st = lob.stats();
    assert_eq!(st.resting_fills_observed, 1, "the oracle must have RUN");
    assert_eq!(
        st.fill_price_differs_from_resting, 1,
        "the fill executed at $100.06 against an order resting at $100.05"
    );
    assert_eq!(
        st.fill_side_mismatch, 0,
        "SEPARATION: the side MATCHED. A non-zero here means a price difference \
         is leaking into the must-be-zero side-defect channel."
    );

    // NEGATIVE: identical price must NOT increment.
    let mut lob = LobReconstructor::new(10);
    lob.process_message(&msg(2002, Action::Add, Side::Ask, 100.05, 300))
        .expect("seed add must succeed");
    lob.process_message(&msg(2002, Action::Fill, Side::Ask, 100.05, 50))
        .expect("a Fill must never error");

    let st = lob.stats();
    assert_eq!(st.resting_fills_observed, 1, "the oracle must have RUN");
    assert_eq!(
        st.fill_price_differs_from_resting, 0,
        "the prices AGREE, so the counter must stay silent. If this fires, the \
         counter is unconditional and the positive arm above proves nothing."
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
    // Phase M M.A.5: the public const must remain pinned until the next
    // INTENTIONAL, coordinated bump. If a future commit SILENTLY bumps it,
    // this test fails. That trip-wire is undiminished by the update below.
    //
    // 2.0.0 -> 2.1.0 at L-ROUTE (COMMIT 2a): an intentional MINOR bump under
    // the policy documented on the constant itself ("MINOR: additive
    // non-breaking changes (e.g., new `LobStats` field)"). SIX fields were
    // added: `aggregate_trades_observed`, `resting_fills_observed`,
    // `fill_referenced_unknown_order`, `fill_size_exceeded_resting`,
    // `fill_side_mismatch`, `fill_price_differs_from_resting`.
    //
    // ⚠ WHY THE BUMP IS LOAD-BEARING AND NOT BOOKKEEPING. All six carry
    // `#[serde(default)]`, so a PRE-L-ROUTE stats file (where the keys are
    // absent) deserialises them to 0 — numerically identical to a POST-L-ROUTE
    // file in which the carrier genuinely observed zero records. Without the
    // version there is no way to tell "this reconstructor never counted" from
    // "this reconstructor counted nothing", which is exactly the
    // absence-indistinguishable-from-agreement failure hft-rules §1 forbids.
    // The envelope's `schema_version` is the only field that discriminates.
    assert_eq!(
        LOB_STATS_SCHEMA_VERSION, "2.1.0",
        "LOB_STATS_SCHEMA_VERSION must remain pinned at 2.1.0 until the next \
         intentional, coordinated bump"
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
