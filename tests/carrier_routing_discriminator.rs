//! THE DISCRIMINATING TEST — separates a real L-ROUTE fix from a no-op that
//! reproduces every named acceptance key.
//!
//! # Why this file exists
//!
//! COMMIT 2 (L-ROUTE) makes `Action::TradeAggregate` and `Action::Fill` each a
//! **book no-op**. Six wrong implementations reach the headline acceptance key
//! (`cancel_order_not_found -> 0`), and **two of them match EVERY named
//! acceptance key on both development days**. The worst is:
//!
//! > **W4 — "fix `Fill`, leave `TradeAggregate` book-mutating."**
//!
//! W4 is **bit-identical to the correct fix on every live-data counter**, because
//! 100% of the `TradeAggregate` population on the XNAS tape carries
//! `order_id == 0` (375,643/375,643 on 2025-07-01; 319,230/319,230 on
//! 2025-07-02) and is therefore dropped by `is_system_message()` at L-ADMIT
//! *before* it ever reaches the router. The T arm is **dormant on real data**.
//! Nothing in the corpus-derived acceptance set can observe it.
//!
//! This file constructs the one record shape that forces the two carriers down
//! separately observable paths: a `TradeAggregate` with `order_id != 0` and a
//! real `side`. That shape **does not occur on the XNAS tape**, so this test
//! costs nothing in production and cannot false-red on real data — but it is
//! the only construction under which W4 and the correct fix disagree.
//!
//! # THE LOAD-BEARING DESIGN DECISION: the book is PRE-POPULATED
//!
//! Read `LobReconstructor::reduce_or_remove_order` Stage 1. When the `order_id`
//! is **not** in `self.orders`, it increments a not-found counter and returns
//! `Ok(())` **leaving the book completely untouched**. So the same assertion
//! against an *empty* book would PASS under W4 and prove nothing.
//!
//! Every test below therefore seeds a resting order and targets it by
//! `order_id`, so a mutating router actually reaches Stage 4 and moves the book.
//!
//! # AND THE SIZE IS DELIBERATELY PARTIAL
//!
//! Stage 4 branches on `msg.size >= current_size`. A **partial** size takes the
//! `reduce_order` branch, which leaves the order in `self.orders` — so
//! `active_orders` is **unchanged even when the book IS mutated**. A count-only
//! assertion is structurally blind to this. The level-state assertion is what
//! does the work; `active_orders` is asserted alongside only to pin the
//! full-removal wrong-implementations as well.
//!
//! # STATUS: RED UNTIL COMMIT 2 LANDS
//!
//! The two falsifiers are `#[ignore]`d **solely** so the suite stays green for
//! agents working other tracks in parallel. They FAIL today, by design and by
//! demonstration — see the recorded run in the track report. A test that passed
//! today would be locking the bug.
//!
//! ## COMMIT 2's definition of done includes, in this file:
//!   1. DELETE the two `#[ignore = ...]` attributes.
//!   2. UNCOMMENT the `=== COMMIT 2 ADDS ===` carrier-counter assertions.
//! Neither is optional. Until (1) happens these falsifiers never run in CI.

use mbo_lob_reconstructor::{Action, LobConfig, LobReconstructor, MboMessage, Side};

// NOTE: do NOT `use mbo_lob_reconstructor::Fill` — that is the *struct* re-exported
// from `trade_aggregator` and it collides with the `Action::Fill` *variant* under
// test. Always write `Action::Fill` fully qualified.

/// Nanodollar fixed-point message constructor (matches `tests/lob_stats_counters.rs`).
fn msg(order_id: u64, action: Action, side: Side, price_dollars: f64, size: u32) -> MboMessage {
    MboMessage::new(order_id, action, side, (price_dollars * 1e9) as i64, size)
}

/// The BOOK, and only the book.
///
/// Deliberately excludes `triggering_action` / `triggering_side` / `sequence` /
/// `timestamp` / `delta_ns`: those are event annotations that SHOULD change when
/// any message is processed, including a no-op carrier. Comparing whole
/// `LobState`s would make this test fail for the wrong reason and would be
/// indistinguishable from the defect it exists to catch.
#[derive(Debug, PartialEq, Eq)]
struct BookFingerprint {
    bid_prices: [i64; 10],
    bid_sizes: [u32; 10],
    ask_prices: [i64; 10],
    ask_sizes: [u32; 10],
    best_bid: Option<i64>,
    best_ask: Option<i64>,
    active_orders: usize,
    bid_levels: usize,
    ask_levels: usize,
}

fn fingerprint(lob: &LobReconstructor) -> BookFingerprint {
    let s = lob.get_lob_state();
    let st = lob.stats();
    let mut bp = [0i64; 10];
    let mut bs = [0u32; 10];
    let mut ap = [0i64; 10];
    let mut as_ = [0u32; 10];
    bp.copy_from_slice(&s.bid_prices[..10]);
    bs.copy_from_slice(&s.bid_sizes[..10]);
    ap.copy_from_slice(&s.ask_prices[..10]);
    as_.copy_from_slice(&s.ask_sizes[..10]);
    BookFingerprint {
        bid_prices: bp,
        bid_sizes: bs,
        ask_prices: ap,
        ask_sizes: as_,
        best_bid: s.best_bid,
        best_ask: s.best_ask,
        active_orders: st.active_orders,
        bid_levels: st.bid_levels,
        ask_levels: st.ask_levels,
    }
}

/// Build a reconstructor with `skip_system_messages = false`.
///
/// This is REQUIRED by the test contract and is not cosmetic. The records under
/// test already carry `order_id != 0 && size != 0 && price > 0`, so
/// `is_system_message()` is false and L-ADMIT would not drop them either way —
/// but pinning the flag off makes this a **pure router test** that stays valid
/// when COMMIT 3 (L-ADMIT) changes the admission predicate underneath it.
fn seeded_book() -> LobReconstructor {
    let config = LobConfig::new(10)
        .with_skip_system_messages(false)
        .with_logging(false);
    let mut lob = LobReconstructor::with_config(config);

    // Resting BID: order 1001 @ $100.00 x 500
    lob.process_message(&msg(1001, Action::Add, Side::Bid, 100.00, 500))
        .expect("seed bid add must succeed");
    // Resting ASK: order 2002 @ $100.05 x 300
    lob.process_message(&msg(2002, Action::Add, Side::Ask, 100.05, 300))
        .expect("seed ask add must succeed");

    lob
}

/// Assert that no path through `reduce_or_remove_order` was taken.
///
/// Under COMMIT 2 neither carrier reaches that function at all, so every trade
/// anomaly counter must remain exactly zero. This catches a wrong implementation
/// that "fixes" the carriers by routing them at the *not-found* path instead of
/// making them true no-ops — that variant leaves the book unchanged and would
/// otherwise slip past the fingerprint assertion.
fn assert_reduction_path_untaken(lob: &LobReconstructor, carrier: &str) {
    let st = lob.stats();
    assert_eq!(
        (
            st.trade_order_not_found,
            st.trade_price_level_missing,
            st.trade_order_at_level_missing
        ),
        (0, 0, 0),
        "{carrier} must be a BOOK NO-OP, i.e. `reduce_or_remove_order` must never be \
         entered. Non-zero trade anomaly counters mean the carrier was still routed \
         into the reduction path (and merely missed), which is not a no-op. Got \
         not_found={} price_level_missing={} at_level_missing={}",
        st.trade_order_not_found,
        st.trade_price_level_missing,
        st.trade_order_at_level_missing
    );
}

// =============================================================================
// POSITIVE CONTROL — runs in CI, must ALWAYS be green.
// =============================================================================

/// The anti-false-green gate: prove the instrument can see a real mutation.
///
/// If `BookFingerprint` were blind — wrong field set, a stale snapshot, a
/// comparison that cannot fail — the two falsifiers below would go green after
/// COMMIT 2 for a reason that has nothing to do with COMMIT 2, and this whole
/// file would be decorative. `Cancel` is a legitimately book-MUTATING action, so
/// this asserts the fingerprint DOES change, with the same partial size the
/// falsifiers use.
///
/// This is not bug-locking: it pins behaviour that is correct today and must
/// remain correct after COMMIT 2 (L-ROUTE does not touch the `Cancel` arm).
#[test]
fn positive_control_fingerprint_detects_a_real_partial_book_mutation() {
    let mut lob = seeded_book();
    let before = fingerprint(&lob);

    // Partial CANCEL of the resting bid: 100 of 500.
    lob.process_message(&msg(1001, Action::Cancel, Side::Bid, 100.00, 100))
        .expect("cancel must succeed");

    let after = fingerprint(&lob);

    assert_ne!(
        before, after,
        "POSITIVE CONTROL FAILED: a partial Cancel demonstrably reduces the resting \
         bid, but BookFingerprint reported no change. The instrument is blind, so \
         every no-op assertion in this file is vacuous. Fix the fingerprint before \
         trusting anything else here."
    );
    assert_eq!(
        (before.bid_sizes[0], after.bid_sizes[0]),
        (500, 400),
        "POSITIVE CONTROL FAILED: partial reduction must move bid_sizes[0] 500 -> 400."
    );
    // ...and prove the partial path leaves the count blind, which is the whole
    // reason the falsifiers below cannot rely on `active_orders`.
    assert_eq!(
        (before.active_orders, after.active_orders),
        (2, 2),
        "POSITIVE CONTROL FAILED: a PARTIAL reduction must leave `active_orders` \
         unchanged. If this ever moves, the size chosen is no longer partial and the \
         falsifiers below silently lose their level-state discrimination."
    );
}

// =============================================================================
// FALSIFIER 1 — kills W4. RED until COMMIT 2 lands.
// =============================================================================

/// `Action::TradeAggregate` must be a BOOK NO-OP, even when it carries a
/// resolvable `order_id`.
///
/// **This is the only test in the repository that can distinguish the correct
/// seven-arm router from W4** ("fix `Fill`, leave `TradeAggregate`
/// book-mutating"), because it is the only one that presents a
/// `TradeAggregate` the L-ADMIT filter does not eat.
///
/// TODAY (`Action::TradeAggregate | Action::Fill => self.process_trade(msg)?`):
/// order 1001 is found, Stage 4 takes the partial branch, `bid_sizes[0]` moves
/// 500 -> 400, and this test FAILS. That failure is the proof it is a real
/// falsifier and not a fixture that locks the bug.
#[test]
#[ignore = "RED BY DESIGN until COMMIT 2 (L-ROUTE) lands. Run with `--ignored`. \
            COMMIT 2 MUST delete this attribute."]
fn trade_aggregate_with_resolvable_order_id_must_not_mutate_the_book() {
    let mut lob = seeded_book();
    let before = fingerprint(&lob);

    // THE DISCRIMINATING RECORD: a TradeAggregate that is NOT a system message.
    //   order_id = 1001  -> resolvable; forces Stage 1 to succeed
    //   side     = Bid   -> a real side, not Side::None
    //   size     = 100   -> PARTIAL (< the resting 500), so `active_orders` cannot
    //                       detect the mutation and the level state must
    // This shape does not occur on the XNAS tape; it exists to make the dormant
    // T arm observable.
    lob.process_message(&msg(1001, Action::TradeAggregate, Side::Bid, 100.00, 100))
        .expect("TradeAggregate must not error");

    let after = fingerprint(&lob);

    assert_eq!(
        before, after,
        "\n\n*** THE T ARM STILL MUTATES THE BOOK ***\n\
         `Action::TradeAggregate` is a documented vendor BOOK NO-OP, but processing one \
         with a resolvable order_id changed the book.\n\
         This is the W4 signature: `Fill` fixed, `TradeAggregate` left routed into \
         `process_trade`. Every live-data counter is bit-identical to the correct fix \
         because 100% of the real T population carries order_id == 0 and is dropped at \
         L-ADMIT — so ONLY this test can see it.\n\
         before = {before:#?}\n\
         after  = {after:#?}\n"
    );
    assert_reduction_path_untaken(&lob, "Action::TradeAggregate");

    // === COMMIT 2 ADDS === (fields do not exist yet; uncomment when L-ROUTE lands)
    // let st = lob.stats();
    // assert_eq!(st.aggregate_trades_observed, 1,
    //     "the TradeAggregate carrier counter must observe exactly this one record");
    // assert_eq!(st.resting_fills_observed, 0,
    //     "a TradeAggregate must NEVER be counted as a resting fill — the two \
    //      populations are DISJOINT");
}

// =============================================================================
// FALSIFIER 2 — the mirror. RED until COMMIT 2 lands.
// =============================================================================

/// `Action::Fill` must be a BOOK NO-OP too.
///
/// The removal is performed by the paired `Cancel`, which follows the `Fill` as
/// the literal next record (F->C pairing measured at 1.00000000 over 6 days /
/// 1,808,570 records, including inside both auction crosses). A `Fill` that also
/// reduces the book is the double-decrement half of the original defect.
///
/// Targets the ASK side so the pair of falsifiers covers both sides of the book.
///
/// TODAY this FAILS: `ask_sizes[0]` moves 300 -> 250.
#[test]
#[ignore = "RED BY DESIGN until COMMIT 2 (L-ROUTE) lands. Run with `--ignored`. \
            COMMIT 2 MUST delete this attribute."]
fn fill_with_resolvable_order_id_must_not_mutate_the_book() {
    let mut lob = seeded_book();
    let before = fingerprint(&lob);

    // order_id = 2002 -> the resting ASK; size 50 of 300 -> partial.
    lob.process_message(&msg(2002, Action::Fill, Side::Ask, 100.05, 50))
        .expect("Fill must not error");

    let after = fingerprint(&lob);

    assert_eq!(
        before, after,
        "\n\n*** THE F ARM STILL MUTATES THE BOOK ***\n\
         `Action::Fill` is a documented vendor BOOK NO-OP; the paired `Cancel` performs \
         the removal. A Fill that also reduces the level is the double-decrement half of \
         the T/F carrier-merge defect.\n\
         before = {before:#?}\n\
         after  = {after:#?}\n"
    );
    assert_reduction_path_untaken(&lob, "Action::Fill");

    // === COMMIT 2 ADDS === (fields do not exist yet; uncomment when L-ROUTE lands)
    // let st = lob.stats();
    // assert_eq!(st.resting_fills_observed, 1,
    //     "the Fill carrier counter must observe exactly this one record");
    // assert_eq!(st.aggregate_trades_observed, 0,
    //     "a Fill must NEVER be counted as an aggregate trade — the two \
    //      populations are DISJOINT");
}
