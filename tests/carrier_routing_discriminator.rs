//! THE DISCRIMINATING TEST — separates a real L-ROUTE fix from a no-op that
//! reproduces every named acceptance key.
//!
//! # Why this file exists
//!
//! COMMIT 2 (L-ROUTE) makes `Action::TradeAggregate` and `Action::Fill` each a
//! **book no-op**. Six wrong implementations reach the headline acceptance key
//! (`cancel_order_not_found -> 0`), and **two of them match EVERY named
//! acceptance key on the XNAS development days**. The worst is:
//!
//! > **W4 — "fix `Fill`, leave `TradeAggregate` book-mutating."**
//!
//! On the XNAS tape W4 is **bit-identical to the correct fix on every counter**,
//! because 100% of the `TradeAggregate` population there carries `order_id == 0`
//! (375,643/375,643 on 2025-07-01; 319,230/319,230 on 2025-07-02) and is dropped
//! by `is_system_message()` at L-ADMIT *before* it reaches the router. The T arm
//! is **dormant on XNAS data**. Nothing in the XNAS acceptance set can see it.
//!
//! Measured 2026-08-16, three built arms (baseline / W4 / correct), same source
//! except the two router arms, full-day replay:
//!
//! ```text
//!   XNAS NVDA 2025-07-01     W4                       correct
//!     trade_order_not_found      0                        0
//!     cancel_order_not_found     0                        0
//!     active_orders           5910                     5910     <- INDISTINGUISHABLE
//! ```
//!
//! This file constructs the one record shape that forces the two carriers down
//! separately observable paths: a carrier with `order_id != 0` that resolves to a
//! resting order. That shape **does not occur on the XNAS tape**, so this test
//! costs nothing in production and cannot false-red on real data — but on XNAS it
//! is the only construction under which W4 and the correct fix disagree.
//!
//! # ⭐ THIS FILE IS NOT THE ONLY INSTRUMENT — ARCX SEES W4 ON LIVE DATA
//!
//! Do not treat this test as a single point of failure. Same three arms, same
//! binary, replayed over an ARCX day that is already on disk:
//!
//! ```text
//!   ARCX NVDA 2025-07-01     baseline      W4        correct
//!     trade_order_not_found     60,376     49,788          0   <- DISCRIMINATES
//!     cancel_order_not_found   157,493          0          0
//!     active_orders             29,090     29,091     29,091   <- book identical
//!
//!   ARCX NVDA 2025-07-02                   40,206          0   <- DISCRIMINATES
//! ```
//!
//! MECHANISM: ARCX carries a `T` population with `order_id != 0` (49,788 records
//! on 2025-07-01, 40,206 on 2025-07-02 — all `side = N`). Those order_ids are in a
//! **disjoint namespace** from the `Add` order_ids (measured
//! `INTERSECT(T_oids, A_oids) = 0` against 2,296,183 unique Add ids), so under W4
//! they enter `reduce_or_remove_order`, miss at Stage 1, and increment
//! `trade_order_not_found` — **without ever touching the book**. Counter-visible,
//! book-invisible. Under the correct fix they never enter, and the counter is 0.
//!
//! ⚠️ THE TABLE ABOVE WAS MEASURED ON A PRE-COLLAPSE BUILD, AND THE COUNTER IT
//! NAMES IS NOW DEAD. Those three arms were built while `OrderReductionOp` still
//! carried a `Trade` variant, which is where W4's misses landed. COMMIT 2a
//! collapsed that enum to `Cancel` alone, and **`trade_order_not_found` now has
//! zero increment sites in `src/`** — it reads 0 under every implementation,
//! including W4. Do NOT use it as a falsifier; an agent following the old
//! instruction would certify the impostor as correct.
//!
//! CONSEQUENCE: the live-data discriminator is **`cancel_order_not_found`**:
//! 157,493 -> 0 (ARCX NVDA 2025-07-01) and 127,527 -> 0 (2025-07-02). It is the
//! right instrument for a structural reason, not just an empirical one — it has
//! exactly ONE producer (the Stage-1 miss branch of `reduce_or_remove_order`)
//! and `Cancel` is the only `OrderReductionOp` variant, so ANY carrier routed
//! into the reduction path and missing must increment it. On ARCX the `T`
//! population that makes W4 observable is precisely a population that misses
//! (disjoint namespace, above), so a W4 impostor written against the current
//! enum cannot reach 0 there. An MBP-10 oracle still cannot see W4 (the book is
//! identical — see `active_orders` above), so the oracle is not the second
//! instrument; this counter is.
//!
//! ⚠️ The router's comment USED TO claim the ARCX `T` population "is filtered by
//! the `side == Side::None` guard". That was WRONG and has been corrected in the
//! same commit that discharged this harness: `reduce_or_remove_order` never reads
//! `msg.side` — Stage 2 branches on the **stored** order's side. What actually
//! saved the book on ARCX was Stage-1 namespace disjointness (`INTERSECT` of the
//! `T` order_ids with the `Add` order_ids measured exactly 0), which is precisely
//! why the counter moved while the book did not. Namespace disjointness is a
//! STRUCTURAL property, and therefore a stronger guarantee than a guard that a
//! later commit could "wake".
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
//! # ✅ STATUS: DISCHARGED — COMMIT 2a (L-ROUTE) HAS LANDED
//!
//! These two falsifiers assert the POST-L-ROUTE contract. Until the router was
//! split they could not pass, so they ran on the default `cargo test` with their
//! bodies wrapped in an inverted-polarity `pending_until_commit2(...)` harness
//! that asserted they *still failed* — making "forgetting them" the loudest
//! failure in the suite rather than a silent `#[ignore]` skip.
//!
//! **That harness has served its purpose and is deleted.** All three steps of its
//! own definition of done were carried out in the same commit that split the
//! router: the wrappers were removed, the `=== COMMIT 2 ADDS ===` carrier-counter
//! assertions were uncommented, and `pending_until_commit2` /
//! `probe_carrier_mutates` were deleted. The falsifiers now run directly and are
//! ordinary green tests that go red if either carrier ever mutates the book again.
//!
//! Each falsifier additionally runs the **paired `Cancel`** after its no-op
//! assertion. That matters: asserting only "the carrier changed nothing" is
//! equally satisfied by a router that drops the record on the floor. Running the
//! real vendor `F`→`C` pair proves the level is reduced **exactly once** — not
//! twice (the double-decrement) and not zero times.
//!
//! MEASURED WHEN DISCHARGED (XNAS NVDA, full-day replay, same binary):
//!
//! ```text
//!                              before        after
//!   cancel_order_not_found     261,386  ->   0        2025-07-01
//!   cancel_order_not_found     207,959  ->   0        2025-07-02
//!   trade_order_not_found       18,061  ->   0        (path no longer entered)
//!   active_orders                5,909  ->   5,910
//! ```

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
/// This exists to catch the ONE impostor the fingerprint is structurally blind
/// to: an implementation that "fixes" the carriers by routing them into the
/// reduction path where they *miss* at Stage 1. Stage 1 returns `Ok(())`
/// leaving the book untouched, so `before == after` and the fingerprint passes.
/// Only a counter can see it.
///
/// # Which counter — and the measurement behind the choice
///
/// `cancel_order_not_found` is the LIVE assertion. Measured on this tree:
/// * It has exactly ONE producer, the Stage-1 miss branch of
///   `reduce_or_remove_order`, and `OrderReductionOp` has exactly ONE variant
///   (`Cancel`). So *any* carrier routed into that function and missing MUST
///   land here — there is nowhere else for it to go.
/// * Directly confirmed: with `Action::Fill` patched to call
///   `reduce_or_remove_order(msg, OrderReductionOp::Cancel)`, a carrier naming
///   an order the book does not hold drove it 0 -> 1, while the correct build
///   leaves it at 0.
///
/// ⚠ WHICH CALLER MAKES IT LIVE — measured, because the obvious answer is wrong.
/// In the two headline falsifiers the seeded order IS resolvable by design, so a
/// wrongly-routed carrier *finds* it, mutates the book, and the FINGERPRINT
/// fires first. Confirmed by running that exact impostor: the failure came from
/// the `before == after` assertion, never from here. Those two calls are
/// therefore belt-and-braces, not the discriminator.
/// [`carriers_naming_an_unresting_order_must_not_enter_the_reduction_path`] is
/// the caller that makes this counter load-bearing: there the book is unchanged
/// under BOTH the correct implementation and the impostor, so the fingerprint is
/// structurally blind and only this counter can tell them apart.
///
/// ⚠ The three `trade_*` counters below are asserted as a FORWARD tripwire, and
/// deliberately not as today's discriminator. Measured: post-L-ROUTE they have
/// **zero increment sites in `src/`**, because the `Trade` variant they were
/// written for no longer exists — so on this tree they cannot fail under any
/// implementation. They re-arm the moment someone re-introduces a reducing
/// variant, which is exactly when this guard is needed again. Keeping them is
/// cheap; relying on them alone was the defect (they read as protection while
/// asserting nothing).
///
/// Every caller invokes this BEFORE processing any `Cancel` of its own, so a
/// correct implementation is at 0 on every counter here.
fn assert_reduction_path_untaken(lob: &LobReconstructor, carrier: &str) {
    let st = lob.stats();
    assert_eq!(
        st.cancel_order_not_found, 0,
        "{carrier} must be a BOOK NO-OP, i.e. `reduce_or_remove_order` must never be \
         entered. Post-L-ROUTE `Cancel` is the ONLY `OrderReductionOp` variant, so a \
         carrier wrongly routed into the reduction path and missing at Stage 1 \
         increments THIS counter — and leaves the book untouched, so the fingerprint \
         assertion cannot see it. No `Cancel` has been processed at this point, so \
         anything but 0 means a carrier was routed into the reduction path. Got {}",
        st.cancel_order_not_found
    );
    assert_eq!(
        (
            st.trade_order_not_found,
            st.trade_price_level_missing,
            st.trade_order_at_level_missing
        ),
        (0, 0, 0),
        "{carrier}: a `trade_*` anomaly counter moved. These have no increment site \
         on this tree, so this can only fire if a reducing `OrderReductionOp` variant \
         was re-introduced and a carrier was routed through it. Got \
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
// FALSIFIER 0 — the MISS shape, which the fingerprint cannot see.
// =============================================================================

/// A carrier naming an order the book does NOT hold must not enter the
/// reduction path.
///
/// # Why this shape needs its own test
///
/// The two falsifiers below seed a RESOLVABLE order, so a wrongly-routed carrier
/// reaches Stage 4 and moves the book — the fingerprint catches it. This test is
/// the complement: with an unresting `order_id`, `reduce_or_remove_order` misses
/// at Stage 1 and returns `Ok(())` **leaving the book completely untouched**.
/// `before == after` holds under the correct implementation AND under the
/// impostor, so the fingerprint is structurally blind and a counter is the only
/// possible instrument. Without this test
/// [`assert_reduction_path_untaken`] is never reached on a shape that can make
/// it fail, i.e. it reads as protection while asserting nothing.
///
/// This is the ARCX mechanism in miniature: there the `T` population carries
/// `order_id != 0` drawn from a namespace DISJOINT from the `Add` ids
/// (`INTERSECT` measured exactly 0), so under W4 those records miss at Stage 1
/// and move a counter **without ever touching the book** — counter-visible,
/// book-invisible.
#[test]
fn carriers_naming_an_unresting_order_must_not_enter_the_reduction_path() {
    // ---- Action::Fill naming an order that is not resting. ----
    let mut lob = seeded_book();
    let before = fingerprint(&lob);

    lob.process_message(&msg(9999, Action::Fill, Side::Ask, 100.05, 50))
        .expect("Fill must not error");

    let after = fingerprint(&lob);
    assert_eq!(
        before, after,
        "the book must be untouched — but note this assertion is WEAK here by \
         construction: it also holds under a router that sends the carrier into \
         the reduction path and merely misses. That is exactly why the counter \
         assertion below is the load-bearing one."
    );
    assert_reduction_path_untaken(&lob, "Action::Fill (unresting order_id)");

    // The observation is REPOINTED, not lost. A zero here would mean the record
    // was silently dropped, which is a different defect wearing the same mask.
    let st = lob.stats();
    assert_eq!(
        st.resting_fills_observed, 1,
        "the Fill oracle must have RUN on this record"
    );
    assert_eq!(
        st.fill_referenced_unknown_order, 1,
        "the vendor asserted a resting order the book does not hold; that is the \
         signal `trade_order_not_found` used to carry, and it must be REPORTED on \
         the carrier's own counter rather than silently absorbed"
    );

    // ---- Action::TradeAggregate naming an order that is not resting. ----
    let mut lob = seeded_book();
    let before = fingerprint(&lob);

    lob.process_message(&msg(8888, Action::TradeAggregate, Side::Bid, 100.00, 100))
        .expect("TradeAggregate must not error");

    assert_eq!(before, fingerprint(&lob), "the book must be untouched");
    assert_reduction_path_untaken(&lob, "Action::TradeAggregate (unresting order_id)");

    let st = lob.stats();
    assert_eq!(
        st.aggregate_trades_observed, 1,
        "the TradeAggregate carrier counter must observe exactly this one record"
    );
    assert_eq!(
        st.resting_fills_observed, 0,
        "a TradeAggregate must NEVER be counted as a resting fill — the two \
         populations are DISJOINT"
    );
}

// =============================================================================
// FALSIFIER 1 — kills W4. PENDING (inverted polarity) until COMMIT 2 lands.
// =============================================================================

/// `Action::TradeAggregate` must be a BOOK NO-OP, even when it carries a
/// resolvable `order_id`.
///
/// **This is the only test in the repository that can distinguish the correct
/// seven-arm router from W4** ("fix `Fill`, leave `TradeAggregate`
/// book-mutating") — and, on XNAS data, the only instrument of any kind. On ARCX
/// the `trade_order_not_found` counter is a second, live-data falsifier; see the
/// module header.
///
/// PRE-L-ROUTE (`Action::TradeAggregate | Action::Fill => self.process_trade(msg)?`)
/// order 1001 was found, Stage 4 took the partial branch and `bid_sizes[0]` moved
/// 500 -> 400, so this body FAILED. That it could fail is the proof it is a real
/// falsifier and not a fixture that locks the bug.
#[test]
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
         the reduction path. Every XNAS live-data counter is bit-identical to the correct \
         fix because 100% of the real XNAS T population carries order_id == 0 and is \
         dropped at L-ADMIT — so on XNAS ONLY this test can see it. (On ARCX the \
         live-data instrument is `cancel_order_not_found`: 157,493 -> 0 on 2025-07-01 \
         and 127,527 -> 0 on 2025-07-02. NOT `trade_order_not_found` — that counter \
         has no increment site post-COMMIT-2a and reads 0 under W4 too.)\n\
         before = {before:#?}\n\
         after  = {after:#?}\n"
    );
    assert_reduction_path_untaken(&lob, "Action::TradeAggregate");

    // Discharged from `pending_until_commit2` when L-ROUTE landed.
    let st = lob.stats();
    assert_eq!(
        st.aggregate_trades_observed, 1,
        "the TradeAggregate carrier counter must observe exactly this one record"
    );
    assert_eq!(
        st.resting_fills_observed, 0,
        "a TradeAggregate must NEVER be counted as a resting fill — the two \
         populations are DISJOINT"
    );
}

// =============================================================================
// FALSIFIER 2 — the mirror. PENDING (inverted polarity) until COMMIT 2 lands.
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
/// PRE-L-ROUTE (`Action::TradeAggregate | Action::Fill => self.process_trade(msg)?`)
/// order 2002 was found, Stage 4 took the partial branch and `ask_sizes[0]` moved
/// 300 -> 250, so this body FAILED. That it could fail is the proof it is a real
/// falsifier and not a fixture that locks the bug. It passes today.
#[test]
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

    // Discharged from `pending_until_commit2` when L-ROUTE landed.
    let st = lob.stats();
    assert_eq!(
        st.resting_fills_observed, 1,
        "the Fill carrier counter must observe exactly this one record"
    );
    assert_eq!(
        st.aggregate_trades_observed, 0,
        "a Fill must NEVER be counted as an aggregate trade — the two \
         populations are DISJOINT"
    );

    // ...and the PAIRED CANCEL is what performs the removal. Asserting only the
    // no-op would also pass if `Fill` were dropped on the floor; running the real
    // F->C pair proves the level is reduced EXACTLY ONCE.
    lob.process_message(&msg(2002, Action::Cancel, Side::Ask, 100.05, 50))
        .expect("the paired Cancel must not error");

    assert_eq!(
        fingerprint(&lob).ask_sizes[0],
        250,
        "the F->C pair must take the resting ask 300 -> 250 exactly once. \
         Pre-L-ROUTE it went to 200 (the double-decrement)."
    );
    assert_eq!(
        lob.stats().cancel_order_not_found,
        0,
        "THE HEADLINE FALSIFIER IN MINIATURE: the paired Cancel must FIND its order. \
         Pre-L-ROUTE the Fill had already consumed it, which is the whole of \
         261,386 -> 0 on XNAS NVDA 2025-07-01."
    );
}
