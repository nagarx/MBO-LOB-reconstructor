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
//! CONSEQUENCE: `trade_order_not_found == 0` on any ARCX day is a **live-data W4
//! falsifier that needs no synthetic record and no new code**. An MBP-10 oracle
//! still cannot see W4 (the book is identical — see `active_orders` above), so the
//! oracle is not the second instrument; this counter is.
//!
//! ⚠️ The router's own comment claims the ARCX `T` population "is filtered by the
//! `side == Side::None` guard". That is WRONG: `reduce_or_remove_order` never
//! reads `msg.side` — Stage 2 branches on the **stored** order's side. What
//! actually saves the book on ARCX is Stage-1 namespace disjointness, which is
//! precisely why the counter moves while the book does not.
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
//! # ⚠️ STATUS: PENDING UNTIL COMMIT 2 LANDS — AND HOW THAT IS ENFORCED
//!
//! The two falsifiers below assert the POST-COMMIT-2 contract, which is not true
//! yet. They therefore cannot pass today. The question every such test has to
//! answer is: *what happens if someone forgets it exists?*
//!
//! They used to be `#[ignore]`d. Measured, on this branch:
//!
//! ```text
//!   cargo test --test carrier_routing_discriminator
//!     -> exit 0,   "1 passed; 0 failed; 2 ignored"      <- SILENT SKIP
//! ```
//!
//! and the branch has no CI (`.github/workflows/ci.yml` triggered only on
//! `push: [main, master]`), so nothing anywhere ran them. Forgetting to delete
//! two attributes was enough to make the only XNAS-side W4 detector vanish
//! without a sound.
//!
//! **THE MECHANISM NOW USED: INVERTED POLARITY.** Each falsifier runs on the
//! DEFAULT `cargo test` — no `#[ignore]`, no `--ignored`, no feature flag, no env
//! var, no CI job — with its assertion body wrapped in
//! [`pending_until_commit2`]. The wrapper asserts the assertion **still fails**:
//!
//! * **TODAY** the body panics with its carrier marker -> recorded PENDING -> green.
//! * **THE MOMENT COMMIT 2 LANDS** the body stops panicking -> the wrapper itself
//!   panics with a full instruction block -> **LOUD RED on the default test run.**
//! * **ANY OTHER CHANGE** (the body panics for a different reason) -> also RED,
//!   reporting the unexpected panic.
//!
//! Forgetting is therefore not a silent skip; it is the loudest failure in the
//! suite, and it fires at exactly the moment the work is being done. The
//! detection watches the ROUTER'S BEHAVIOUR, so it cannot drift out of sync with
//! a version constant somebody forgot to bump.
//!
//! **THIS IS NOT BUG-LOCKING.** A bug-locking test asserts the defect *as
//! correct*, so a fix reds it and the natural reaction is to revert the fix. Here
//! the permanent artifact is the assertion body — which asserts the CORRECT
//! post-COMMIT-2 contract — and the temporary artifact is the one-line wrapper.
//! Discharging costs exactly what deleting `#[ignore]` cost; forgetting costs the
//! opposite.
//!
//! ## COMMIT 2's definition of done, in this file:
//!   1. In each falsifier, delete the `pending_until_commit2(...)` wrapper and
//!      run the body directly. (The wrapper's own failure message spells this out.)
//!   2. UNCOMMENT the `=== COMMIT 2 ADDS ===` carrier-counter assertions.
//!   3. Delete [`pending_until_commit2`] and [`probe_carrier_mutates`] once both
//!      falsifiers are discharged — `dead_code` will remind you.
//! None of these is optional. Until (1) happens the falsifiers are recorded as
//! PENDING, not as passing.

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
// THE PENDING HARNESS — delete when COMMIT 2 lands.
// =============================================================================

/// Does `carrier` still mutate the book when handed a resolvable `order_id`?
///
/// Used ONLY to enrich [`pending_until_commit2`]'s failure message with the JOINT
/// state of both carriers, so a partial landing is diagnosed on the spot instead
/// of being read as "the fix is done".
fn probe_carrier_mutates(carrier: Action) -> bool {
    let mut lob = seeded_book();
    let before = fingerprint(&lob);
    // Bid side, resolvable id, partial size — same shape both falsifiers use.
    let _ = lob.process_message(&msg(1001, carrier, Side::Bid, 100.00, 100));
    fingerprint(&lob) != before
}

/// Run `body`, and require that it STILL FAILS with `marker` in its panic.
///
/// See the module header (`STATUS: PENDING UNTIL COMMIT 2 LANDS`) for why the
/// polarity is inverted rather than `#[ignore]`d. Three outcomes:
///
/// * body panics with `marker`   -> pre-COMMIT-2, recorded PENDING, test passes.
/// * body does not panic         -> COMMIT 2 landed -> **panic with instructions**.
/// * body panics without `marker`-> something else changed -> panic, reporting it.
///
/// The inner panic is printed by the default hook and captured by libtest; on a
/// PENDING pass it is discarded, so it is only visible under `--nocapture` or on
/// a real failure.
fn pending_until_commit2<F>(carrier: &str, marker: &str, body: F)
where
    F: FnOnce(),
{
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));

    let payload = match outcome {
        Err(p) => p,
        Ok(()) => {
            let t = probe_carrier_mutates(Action::TradeAggregate);
            let f = probe_carrier_mutates(Action::Fill);
            let joint = match (t, f) {
                (false, false) => {
                    "BOTH carriers are now book no-ops. This is the shape COMMIT 2 is \
                     supposed to have. DISCHARGE THIS HARNESS."
                }
                (true, false) => {
                    "*** ONLY `Fill` CHANGED — `TradeAggregate` STILL MUTATES THE BOOK. ***\n\
                     THIS IS THE W4 SIGNATURE, THE EXACT WRONG IMPLEMENTATION THIS FILE \
                     EXISTS TO CATCH. Do NOT discharge the harness to make this green: fix \
                     the router so `Action::TradeAggregate` is a book no-op too. \
                     Cross-check on live data: replay any ARCX day and require \
                     `trade_order_not_found == 0` (W4 leaves 49,788 on 2025-07-01)."
                }
                (false, true) => {
                    "*** ONLY `TradeAggregate` CHANGED — `Fill` STILL MUTATES THE BOOK. ***\n\
                     This is W4's mirror. `Fill` is the half that is LIVE on every venue, so \
                     this variant is also caught by `cancel_order_not_found`, which will not \
                     reach 0. Fix the router; do not discharge the harness."
                }
                (true, true) => {
                    "Neither carrier changed, yet the assertion body stopped failing. That \
                     is contradictory — suspect the fingerprint, the seed, or a change to \
                     `seeded_book()`. Do not discharge until this is explained."
                }
            };
            panic!(
                "\n\n\
                 ================================================================\n\
                 PENDING TEST IS NOW PASSING: {carrier}\n\
                 ================================================================\n\
                 The assertion body below `pending_until_commit2(...)` no longer fails.\n\
                 It asserts the POST-COMMIT-2 (L-ROUTE) contract, so this means the\n\
                 routing behaviour has CHANGED. This failure is the intended alarm, not\n\
                 a regression — it is how this file refuses to be forgotten.\n\
                 \n\
                 JOINT CARRIER STATE, probed just now:\n\
                   Action::TradeAggregate mutates the book = {t}\n\
                   Action::Fill           mutates the book = {f}\n\
                 {joint}\n\
                 \n\
                 TO DISCHARGE (only when BOTH are false):\n\
                   1. In this test, delete the `pending_until_commit2(...)` wrapper and\n\
                      run the body directly.\n\
                   2. Do the same for the sibling falsifier.\n\
                   3. Uncomment the `=== COMMIT 2 ADDS ===` carrier-counter assertions.\n\
                   4. Delete `pending_until_commit2` and `probe_carrier_mutates`.\n\
                 \n\
                 EXPECT COMPANY: 7 library tests fail identically under W4 AND under a\n\
                 correct fix, because they assert the OLD book-mutating contract —\n\
                 test_over_trade_removes_order, test_partial_trade_size_reduction,\n\
                 test_price_level_cache_consistency_complex, test_trade_full_fill,\n\
                 test_trade_partial_fill, test_trade_unknown_order_is_ok,\n\
                 test_warning_stats_accumulate. They CANNOT discriminate W4 and must not\n\
                 be read as 'the fix broke something'.\n\
                 ================================================================\n"
            );
        }
    };

    let text = if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    };

    assert!(
        text.contains(marker),
        "\n\n\
         ================================================================\n\
         PENDING TEST FAILED FOR AN UNEXPECTED REASON: {carrier}\n\
         ================================================================\n\
         Expected the body to still fail with the marker {marker:?} (the pre-COMMIT-2\n\
         state). It failed with something else, so the assertion is no longer\n\
         measuring what this file thinks it measures. Read the panic below before\n\
         touching anything; do NOT delete the harness to make this green.\n\
         \n\
         ACTUAL PANIC:\n{text}\n\
         ================================================================\n"
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
/// TODAY (`Action::TradeAggregate | Action::Fill => self.process_trade(msg)?`):
/// order 1001 is found, Stage 4 takes the partial branch, `bid_sizes[0]` moves
/// 500 -> 400, and the body FAILS. That failure is the proof it is a real
/// falsifier and not a fixture that locks the bug — and it is what
/// `pending_until_commit2` records as PENDING.
#[test]
fn trade_aggregate_with_resolvable_order_id_must_not_mutate_the_book() {
    // COMMIT 2: delete this wrapper and run the body directly.
    pending_until_commit2(
        "Action::TradeAggregate",
        "THE T ARM STILL MUTATES THE BOOK",
        || {
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
                 `process_trade`. Every XNAS live-data counter is bit-identical to the correct \
                 fix because 100% of the real XNAS T population carries order_id == 0 and is \
                 dropped at L-ADMIT — so on XNAS ONLY this test can see it. (On ARCX, \
                 `trade_order_not_found` also sees it: 49,788 vs 0 on 2025-07-01.)\n\
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
        },
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
/// TODAY the body FAILS: `ask_sizes[0]` moves 300 -> 250.
#[test]
fn fill_with_resolvable_order_id_must_not_mutate_the_book() {
    // COMMIT 2: delete this wrapper and run the body directly.
    pending_until_commit2("Action::Fill", "THE F ARM STILL MUTATES THE BOOK", || {
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
    });
}
