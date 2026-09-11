//! RUNG 4A (L-ADMIT, reconstructor half) — THE HALF-LANDING LOCK.
//!
//! # What rung 4A changes, and why it is two gates, not one
//!
//! `LobReconstructor::process_message_into` admits a message through TWO gates, in
//! order, under the DEFAULT `LobConfig` (`skip_system_messages: true`,
//! `validate_messages: true`):
//!
//! * **G1, the skip gate** — `skip_system_messages && msg.is_heartbeat()`. Before rung 4A
//!   it was `skip_system_messages && msg.is_system_message() && action != Clear`, which
//!   skipped every vendor `TradeAggregate` (`T`) carrying `order_id == 0` — **100% of the
//!   XNAS.ITCH `T` population** (375,643 / 375,643 records on 2025-07-01).
//! * **G2, the validation gate** — `validate_messages && msg.validate_admission()?`. Before
//!   rung 4A it was `validate_messages && action != Clear && msg.validate()?`, and
//!   `validate()` rejects `order_id == 0` as `Err(InvalidOrderId(0))`.
//!
//! **G1 and G2 are ONE indivisible change.** Relax G1 alone and every XNAS `T` stops being
//! skipped and starts being REJECTED: `process_message_into` returns
//! `Err(InvalidOrderId(0))`. Four production sites turn that into a silent skip — three
//! `.is_err()` sites (the xsec panel producer `continue`s; `fill_bracket_extract` and
//! `auction_book_extract` return from the per-message handler) plus one counted, WARN-logged
//! `Err` arm in this crate's `export_to_parquet` — so the carrier vanishes from `n_trades`,
//! `n_sweep_trades` and `hidden_fill_proxy` on a GREEN build with exit code 0. Relax G2
//! alone and nothing changes at all: `T` is still skipped at G1, and G-SIGN's red set is
//! byte-identical to "rung 4 not landed".
//!
//! # Why this is a NEW file (FINDING-177)
//!
//! Every pre-existing carrier test is STRUCTURALLY BLIND to L-ADMIT:
//! `tests/carrier_routing_discriminator.rs::seeded_book` pins
//! `.with_skip_system_messages(false)` and feeds `order_id != 0`, and the in-crate
//! `carrier_census_*` fixtures build `T` with `order_id != 0` (the ARCX shape). Before this
//! file, no RECONSTRUCTOR fixture built `Action::TradeAggregate` with `order_id == 0` — the
//! export tests and a `DbnBridge` test build that shape, but only through
//! `DbnBridge::convert` and the Parquet writers, never through `LobReconstructor` — and no
//! test asserted anything about it under the DEFAULT config, the only configuration
//! production runs. The real-data integration tests replay XNAS days but pre-filter every
//! record with the inline field-shape test (13 sites in `tests/integration_test.rs`), so no
//! trade print reaches the reconstructor there at all.
//!
//! # Rules this file follows
//!
//! * **DEFAULT `LobConfig`** — six of the nine tests drive the DEFAULT config through
//!   `process_message_into`; the other three are pure predicate truth tables, over every
//!   `Action` and all three sides. `default_lob()` asserts the two gate defaults before
//!   building, so a future change of default turns those six red instead of silently
//!   testing a different configuration.
//! * **The CONSUMER's predicate** — `process_message_into(..).is_err()` is what the three
//!   `.is_err()` sites evaluate, so it is what the headline assertion evaluates.
//! * **SIDE-ASYMMETRIC fixtures** — a symmetric ask/bid drive cannot see a side
//!   transposition, the single most likely hand-edit error in a carrier change.
//! * **LITERAL expectations** — every expected value is written out, never re-derived
//!   from the drive, so a co-migrated fixture cannot follow a defect.
//!
//! # Red proofs (hft-rules §1: an instrument that has never gone red is not an instrument)
//!
//! Driven red in a COPY of the tree before this file was trusted:
//!
//! | mutation | goes red |
//! |---|---|
//! | M1: full change, G2 reverted to `action != Clear && validate()` | `l_admit_relaxes_both_gates_or_the_carrier_is_rejected_not_merely_skipped` with `Err(InvalidOrderId(0))` |
//! | M2: full change, G1 reverted to `is_system_message() && action != Clear` | the same test, on the count: `aggregate_trades_observed` 0, `system_messages_skipped` 1 |
//! | M3: `is_heartbeat`'s `Clear` arm changed to `self.is_system_message()` | `clear_is_never_a_heartbeat` |
//! | M4: `is_system_message()` edited to exempt `TradeAggregate` | `design_b_is_system_message_truth_table_unchanged` |
//! | M5: `validate_admission`'s `Modify` arm returns `Ok(())` | `validate_admission_truth_table` |
//! | M6: the undefined-price clause deleted from `validate_fields` | `validate_admission_truth_table` (on an `Add` row) and `malformed_t_fails_closed_with_the_order_validation_error` (on the trade print): one shared definition, both paths |

use mbo_lob_reconstructor::{
    Action, LobConfig, LobReconstructor, LobState, MboMessage, Side, TlobError,
};

// Fixed-point prices, 1e-9 of the instrument's native unit (USD for these fixtures).
const PX_99_95: i64 = 99_950_000_000;
const PX_100_00: i64 = 100_000_000_000;
const PX_100_02: i64 = 100_020_000_000;
const PX_100_05: i64 = 100_050_000_000;
const PX_100_10: i64 = 100_100_000_000;

/// The seven `Action` variants, literally. `heartbeat_exempt` below is an EXHAUSTIVE match,
/// so a new variant fails to compile there first — add it to this list at the same time.
const ALL_ACTIONS: [Action; 7] = [
    Action::Add,
    Action::Modify,
    Action::Cancel,
    Action::TradeAggregate,
    Action::Fill,
    Action::Clear,
    Action::None,
];

/// The three `Side` variants, literally; the predicates under test must not read `side`.
const ALL_SIDES: [Side; 3] = [Side::Bid, Side::Ask, Side::None];

/// Which actions `is_heartbeat()` must NEVER report as heartbeats, whatever their fields.
/// Exhaustive, no wildcard: a future `Action` variant must be dispositioned here.
fn heartbeat_exempt(action: Action) -> bool {
    match action {
        Action::Add => false,
        Action::Modify => false,
        Action::Cancel => false,
        Action::TradeAggregate => true,
        Action::Fill => false,
        Action::Clear => true,
        Action::None => false,
    }
}

/// The field-shape grid `order_id ∈ {0, 7} × size ∈ {0, 5} × price ∈ {0, -1, 1, i64::MAX}`
/// with its LITERAL `is_system_message()` value. Only a record with a non-zero order id, a
/// non-zero size AND a strictly positive price is not a system message; `i64::MAX` is
/// positive, so the vendor's undefined-price sentinel is NOT a system-message shape on its
/// own (the `Clear` record is one because its `order_id` and `size` are 0).
const SHAPE_GRID: [(u64, u32, i64, bool); 16] = [
    (0, 0, 0, true),
    (0, 0, -1, true),
    (0, 0, 1, true),
    (0, 0, i64::MAX, true),
    (0, 5, 0, true),
    (0, 5, -1, true),
    (0, 5, 1, true),
    (0, 5, i64::MAX, true),
    (7, 0, 0, true),
    (7, 0, -1, true),
    (7, 0, 1, true),
    (7, 0, i64::MAX, true),
    (7, 5, 0, true),
    (7, 5, -1, true),
    (7, 5, 1, false),
    (7, 5, i64::MAX, false),
];

/// A reconstructor under the DEFAULT gates — the only configuration production runs.
fn default_lob() -> LobReconstructor {
    let config = LobConfig::default();
    assert!(
        config.skip_system_messages,
        "this lock is only meaningful under the DEFAULT LobConfig, and the default of \
         skip_system_messages is no longer true — re-derive every expectation in this file"
    );
    assert!(
        config.validate_messages,
        "this lock is only meaningful under the DEFAULT LobConfig, and the default of \
         validate_messages is no longer true — re-derive every expectation in this file"
    );
    LobReconstructor::with_config(config.with_logging(false))
}

fn trade_print(order_id: u64, side: Side, price: i64, size: u32) -> MboMessage {
    MboMessage::new(order_id, Action::TradeAggregate, side, price, size)
}

/// The book-content projection of a reconstructor: every level price and size on both
/// sides, the best prices, and the order and level counts. Excludes the temporal fields
/// (`message_index`, `timestamp`, `triggering_*`), which legitimately advance on a no-op.
#[derive(Debug, PartialEq, Eq)]
struct BookImage {
    bid_prices: Vec<i64>,
    bid_sizes: Vec<u32>,
    ask_prices: Vec<i64>,
    ask_sizes: Vec<u32>,
    best_bid: Option<i64>,
    best_ask: Option<i64>,
    order_count: usize,
    bid_levels: usize,
    ask_levels: usize,
}

fn book_image(lob: &LobReconstructor) -> BookImage {
    let s = lob.get_lob_state();
    BookImage {
        bid_prices: s.bid_prices[..s.levels].to_vec(),
        bid_sizes: s.bid_sizes[..s.levels].to_vec(),
        ask_prices: s.ask_prices[..s.levels].to_vec(),
        ask_sizes: s.ask_sizes[..s.levels].to_vec(),
        best_bid: s.best_bid,
        best_ask: s.best_ask,
        order_count: lob.order_count(),
        bid_levels: lob.bid_levels(),
        ask_levels: lob.ask_levels(),
    }
}

/// (a) THE HEADLINE LOCK. The XNAS wire shape — `TradeAggregate`, `order_id == 0`, a real
/// side, a real price and size — must be COUNTED, and must not come back as an error.
#[test]
fn l_admit_relaxes_both_gates_or_the_carrier_is_rejected_not_merely_skipped() {
    let mut lob = default_lob();
    let mut state = LobState::new(10);
    let t = trade_print(0, Side::Ask, PX_100_00, 137);

    let result = lob.process_message_into(&t, &mut state);

    // The four silent consumers evaluate exactly this and `continue` when it is true.
    let consumer_drops_it = result.is_err();
    assert!(
        !consumer_drops_it,
        "HALF-LANDING: G1 admits the XNAS trade print (order_id == 0) but G2 still REJECTS \
         it: process_message_into returned {result:?}. The consumers that turn an Err into a \
         skip (three `.is_err()` sites, plus export_to_parquet's counted Err arm) now \
         silently zero n_trades / n_sweep_trades / hidden_fill_proxy on a green build. G2 must call validate_admission(), which \
         exempts TradeAggregate from the order_id clause only."
    );

    let s = lob.stats();
    assert_eq!(
        s.aggregate_trades_observed, 1,
        "the XNAS trade print was not counted: aggregate_trades_observed = {}, \
         system_messages_skipped = {}. A skipped count of 1 means G1 still classifies \
         TradeAggregate as a heartbeat (L-ADMIT not landed at the skip gate).",
        s.aggregate_trades_observed, s.system_messages_skipped
    );
    assert_eq!(
        s.aggregate_trades_observed_ask, 1,
        "T|A count (aggressor sold)"
    );
    assert_eq!(s.aggregate_trades_volume_ask, 137, "T|A volume, shares");
    assert_eq!(s.aggregate_trades_observed_bid, 0, "T|B count");
    assert_eq!(s.aggregate_trades_volume_bid, 0, "T|B volume");
    assert_eq!(s.aggregate_trades_observed_none, 0, "T|N count");
    assert_eq!(s.aggregate_trades_volume_none, 0, "T|N volume");
    assert_eq!(
        s.system_messages_skipped, 0,
        "a trade print is a counted book no-op, never a skipped heartbeat"
    );
    assert_eq!(
        s.messages_processed, 1,
        "the trade print is a processed message"
    );
    assert_eq!(
        s.resting_fills_observed, 0,
        "T must not touch the F carrier"
    );
}

/// (b) An asymmetric batch across every side and both order-id shapes the vendor sends.
#[test]
fn l_admit_counts_every_trade_print_shape_exactly_per_side() {
    let mut lob = default_lob();
    let mut state = LobState::new(10);

    // 5 T|Bid + 3 T|Ask + 2 T|None at order_id == 0 (the XNAS shapes; T|None is the
    // hidden-execution print behind FINDING-067's hidden_fill_proxy) + 1 T|None at
    // order_id != 0 (the ARCX shape, where that id is a trade identifier, not an order).
    // Interleaved, with distinct sizes, so a transposition or a dropped row moves a literal.
    let drive = [
        trade_print(0, Side::Bid, PX_100_00, 11),
        trade_print(0, Side::Ask, PX_100_05, 101),
        trade_print(0, Side::None, PX_100_02, 1_000),
        trade_print(0, Side::Bid, PX_100_00, 12),
        trade_print(0, Side::Ask, PX_100_05, 102),
        trade_print(0, Side::Bid, PX_100_00, 13),
        trade_print(777, Side::None, PX_100_02, 40_000),
        trade_print(0, Side::Bid, PX_100_00, 14),
        trade_print(0, Side::None, PX_100_02, 2_000),
        trade_print(0, Side::Ask, PX_100_05, 103),
        trade_print(0, Side::Bid, PX_100_00, 15),
    ];
    for (i, t) in drive.iter().enumerate() {
        let result = lob.process_message_into(t, &mut state);
        let consumer_drops_it = result.is_err();
        assert!(
            !consumer_drops_it,
            "trade print #{i} ({t:?}) was rejected: {result:?}"
        );
    }

    let s = lob.stats();
    assert_eq!(s.aggregate_trades_observed, 11, "carrier total");
    assert_eq!(s.aggregate_trades_observed_bid, 5, "T|B count");
    assert_eq!(
        s.aggregate_trades_volume_bid, 65,
        "T|B volume = 11+12+13+14+15"
    );
    assert_eq!(s.aggregate_trades_observed_ask, 3, "T|A count");
    assert_eq!(
        s.aggregate_trades_volume_ask, 306,
        "T|A volume = 101+102+103"
    );
    assert_eq!(
        s.aggregate_trades_observed_none, 3,
        "T|N count: BOTH order-id shapes, never folded into a directional row"
    );
    assert_eq!(
        s.aggregate_trades_volume_none, 43_000,
        "T|N volume = 1,000 + 2,000 + 40,000"
    );
    assert_eq!(
        s.system_messages_skipped, 0,
        "no trade print is a heartbeat"
    );
    assert_eq!(s.messages_processed, 11, "every trade print was processed");
    assert_eq!(
        s.resting_fills_observed, 0,
        "T must not touch the F carrier"
    );
    assert_eq!(
        s.cancel_order_not_found, 0,
        "T must not reach the reduction path"
    );
    assert_eq!(
        s.trade_order_not_found, 0,
        "the reduction path has no T entry"
    );
    assert_eq!(lob.order_count(), 0, "a trade print adds no order");
}

/// (c) `Clear` matches the field-shape test and MUST still reset the book. Rung 4A moved
/// the Phase O B.2a exemption from the call site (`&& msg.action != Action::Clear`) INTO
/// `is_heartbeat()`, where a later "simplification" back to the field-shape predicate would
/// silently re-swallow every Clear — the pre-B.2a defect in which each day inherits the
/// previous day's resting orders.
#[test]
fn clear_is_never_a_heartbeat() {
    // The record the vendor actually sends: order_id 0, size 0, price == UNDEF_PRICE.
    let clear = MboMessage::new(0, Action::Clear, Side::None, i64::MAX, 0);
    assert!(
        clear.is_system_message(),
        "the tape's Clear has the zero field shape; is_system_message() is field-shape only"
    );
    assert!(
        !clear.is_heartbeat(),
        "Clear is a book RESET, not a heartbeat — is_heartbeat() must exempt it by ACTION"
    );
    assert!(
        clear.validate_admission().is_ok(),
        "Clear names no order and no level; admission validation must pass it"
    );

    let mut lob = default_lob();
    let mut state = LobState::new(10);
    for add in [
        MboMessage::new(31, Action::Add, Side::Bid, PX_99_95, 300),
        MboMessage::new(32, Action::Add, Side::Bid, PX_100_00, 500),
        MboMessage::new(41, Action::Add, Side::Ask, PX_100_05, 400),
    ] {
        lob.process_message_into(&add, &mut state)
            .expect("seed add must succeed");
    }
    assert_eq!(lob.order_count(), 3, "seeded book: 3 orders");

    let result = lob.process_message_into(&clear, &mut state);
    assert!(
        result.is_ok(),
        "the tape's Clear must be admitted: {result:?}"
    );
    let s = lob.stats();
    assert_eq!(
        s.book_clears, 1,
        "Clear must reach the Clear arm under DEFAULT config (book_clears = {}, \
         system_messages_skipped = {})",
        s.book_clears, s.system_messages_skipped
    );
    assert_eq!(
        s.system_messages_skipped, 0,
        "Clear is never a skipped heartbeat"
    );
    assert_eq!(lob.order_count(), 0, "Clear must empty the book");
    assert_eq!(lob.bid_levels(), 0);
    assert_eq!(lob.ask_levels(), 0);
    assert!(lob.get_lob_state().best_bid.is_none());
    assert!(lob.get_lob_state().best_ask.is_none());
}

/// (d) Admitting `T` must not make it a book mutation. Seed a side-asymmetric book, image
/// it, feed trade prints of every side and order-id shape — including two that name a
/// RESTING order id, the sharpest probe for a `T` routed by `order_id` — and require the
/// book to be bit-identical afterwards. Then cancel every seeded order at its exact size: a
/// miss on any stage would mean a trade print removed or moved it.
#[test]
fn t_is_a_book_noop() {
    let mut lob = default_lob();
    let mut state = LobState::new(10);
    let seeds = [
        MboMessage::new(11, Action::Add, Side::Bid, PX_99_95, 300),
        MboMessage::new(12, Action::Add, Side::Bid, PX_100_00, 500),
        MboMessage::new(13, Action::Add, Side::Bid, PX_100_00, 250),
        MboMessage::new(21, Action::Add, Side::Ask, PX_100_05, 400),
        MboMessage::new(22, Action::Add, Side::Ask, PX_100_10, 150),
    ];
    for add in &seeds {
        lob.process_message_into(add, &mut state)
            .expect("seed add must succeed");
    }
    let before = book_image(&lob);
    assert_eq!(
        before.order_count, 5,
        "seeded book: 3 bid orders on 2 levels, 2 ask orders on 2 levels"
    );
    assert_eq!(before.bid_levels, 2);
    assert_eq!(before.ask_levels, 2);

    let prints = [
        trade_print(0, Side::Ask, PX_100_00, 200), // XNAS: aggressor sold into the bid
        trade_print(0, Side::Bid, PX_100_05, 100), // XNAS: aggressor bought the ask
        trade_print(0, Side::None, PX_100_02, 75), // XNAS hidden execution
        trade_print(7_001, Side::None, PX_100_02, 60), // ARCX shape
        trade_print(12, Side::Bid, PX_100_00, 100), // names RESTING bid 12 (partial size)
        trade_print(21, Side::Ask, PX_100_05, 400), // names RESTING ask 21 (its FULL size)
    ];
    for (i, t) in prints.iter().enumerate() {
        let result = lob.process_message_into(t, &mut state);
        assert!(
            result.is_ok(),
            "trade print #{i} ({t:?}) must be admitted: {result:?}"
        );
    }

    let after = book_image(&lob);
    assert_eq!(
        before, after,
        "A TRADE PRINT MUTATED THE BOOK. TradeAggregate is a vendor book no-op: admitting it \
         to the router must not change a single level, size or order."
    );
    let s = lob.stats();
    assert_eq!(
        s.aggregate_trades_observed, 6,
        "all six prints were counted"
    );
    assert_eq!(s.messages_processed, 11, "5 seed adds + 6 trade prints");
    assert_eq!(s.system_messages_skipped, 0);

    // Per-order identity: every seeded order is still resting at its side, price and full
    // size, because an exact-size cancel finds it at every lookup stage.
    for add in &seeds {
        let cancel = MboMessage::new(add.order_id, Action::Cancel, add.side, add.price, add.size);
        lob.process_message_into(&cancel, &mut state)
            .expect("cancel of a seeded order must succeed");
    }
    let s = lob.stats();
    assert_eq!(s.cancel_order_not_found, 0, "a seeded order vanished");
    assert_eq!(s.cancel_price_level_missing, 0, "a seeded level vanished");
    assert_eq!(
        s.cancel_order_at_level_missing, 0,
        "a seeded order moved level"
    );
    assert_eq!(
        lob.order_count(),
        0,
        "exact-size cancels must empty the book"
    );
    assert_eq!(lob.bid_levels(), 0);
    assert_eq!(lob.ask_levels(), 0);
}

/// (e) The relaxation is for TWO actions only. A genuine heartbeat is still skipped — and
/// so is a malformed record of every OTHER action, including the other carrier (`Fill`).
#[test]
fn heartbeat_still_skipped() {
    let mut lob = default_lob();
    let mut state = LobState::new(10);

    let probes = [
        // A pure heartbeat: the no-op action with every field zero.
        MboMessage::new(0, Action::None, Side::None, 0, 0),
        // An Add with no order id: not an order, so not admitted to the book.
        MboMessage::new(0, Action::Add, Side::Bid, PX_100_00, 100),
        // A Fill with no order id: the OTHER carrier is NOT exempt — its order_id is a real
        // resting-order reference (100% of vendor F carry order_id != 0).
        MboMessage::new(0, Action::Fill, Side::Ask, PX_100_05, 25),
    ];
    for (i, m) in probes.iter().enumerate() {
        assert!(m.is_heartbeat(), "probe #{i} ({m:?}) must be a heartbeat");
        let result = lob.process_message_into(m, &mut state);
        assert!(result.is_ok(), "a skipped heartbeat returns Ok: {result:?}");
    }

    let s = lob.stats();
    assert_eq!(
        s.system_messages_skipped, 3,
        "all three probes were skipped"
    );
    assert_eq!(s.messages_processed, 0, "no probe was processed");
    assert_eq!(
        s.noop_messages, 0,
        "the zero-field None never reached the router"
    );
    assert_eq!(
        s.resting_fills_observed, 0,
        "the order-less Fill never reached the router"
    );
    assert_eq!(s.aggregate_trades_observed, 0);
    assert_eq!(
        lob.order_count(),
        0,
        "the order-less Add never reached the book"
    );
}

/// (f) Admitting `T` relaxes ONE clause — `order_id == 0` — and nothing else. A malformed
/// trade print FAILS CLOSED (hft-rules §8): it is rejected with exactly the error
/// `validate()` gives an order-bearing record with the same fields.
///
/// ⚠ That is NOT "treated like a malformed order" end to end, and the difference is the
/// decision: under the DEFAULT config a malformed `Add` (`size == 0` or `price <= 0`) is a
/// heartbeat and is SKIPPED with `Ok`, while a trade print is never a heartbeat, so its
/// malformation surfaces as an `Err`. Before rung 4A such a print was skipped too. The
/// measured population is 0 malformed `T` of 202,054,096 (the COMMIT A review's vendor
/// census; relayed), so this moves nothing on disk.
#[test]
fn malformed_t_fails_closed_with_the_order_validation_error() {
    let mut lob = default_lob();
    let mut state = LobState::new(10);

    let size_zero = trade_print(0, Side::Bid, PX_100_00, 0);
    let result = lob.process_message_into(&size_zero, &mut state);
    assert!(
        matches!(result, Err(TlobError::InvalidSize(0))),
        "T with size 0 must be Err(InvalidSize(0)), got {result:?}"
    );

    let undef_price = trade_print(0, Side::Ask, i64::MAX, 50);
    let result = lob.process_message_into(&undef_price, &mut state);
    assert!(
        matches!(result, Err(TlobError::InvalidPrice(p)) if p == i64::MAX),
        "T at the vendor's undefined-price sentinel must be Err(InvalidPrice(i64::MAX)), \
         got {result:?}"
    );

    let price_zero = trade_print(0, Side::None, 0, 50);
    let result = lob.process_message_into(&price_zero, &mut state);
    assert!(
        matches!(result, Err(TlobError::InvalidPrice(0))),
        "T with price 0 must be Err(InvalidPrice(0)), got {result:?}"
    );

    let price_negative = trade_print(0, Side::Bid, -1, 50);
    let result = lob.process_message_into(&price_negative, &mut state);
    assert!(
        matches!(result, Err(TlobError::InvalidPrice(-1))),
        "T with price -1 must be Err(InvalidPrice(-1)), got {result:?}"
    );

    let s = lob.stats();
    assert_eq!(
        s.aggregate_trades_observed, 0,
        "a rejected trade print is not counted"
    );
    assert_eq!(
        s.system_messages_skipped, 0,
        "a malformed trade print is not a heartbeat"
    );
    assert_eq!(
        s.messages_processed, 0,
        "a rejected message is not processed"
    );

    // The same ERROR as validate() on an order-bearing record with the same fields
    // (order_id 5, so the Add has a real order reference): the field clauses have ONE
    // definition, shared by validate() and the trade-print arm of validate_admission().
    for (price, size) in [(PX_100_00, 0_u32), (i64::MAX, 50), (0, 50), (-1, 50)] {
        let t = trade_print(0, Side::Bid, price, size);
        let add = MboMessage::new(5, Action::Add, Side::Bid, price, size);
        let t_err = format!("{:?}", t.validate_admission());
        let add_err = format!("{:?}", add.validate());
        assert_eq!(
            t_err, add_err,
            "price {price} / size {size}: the trade print's admission error must equal \
             validate()'s error for an order-bearing record with the same fields"
        );
        assert!(
            t_err.starts_with("Err("),
            "price {price} / size {size} must be rejected"
        );
    }

    // And the exemption is exactly the order_id clause: a well-formed XNAS trade print
    // passes admission, while validate() — unchanged public API — still rejects it.
    let well_formed = trade_print(0, Side::Ask, PX_100_00, 10);
    assert!(well_formed.validate_admission().is_ok());
    assert!(
        matches!(well_formed.validate(), Err(TlobError::InvalidOrderId(0))),
        "validate() itself is unchanged: order_id == 0 is still InvalidOrderId(0) there"
    );

    // THE DELIBERATE ASYMMETRY, locked: under the DEFAULT config the same malformation on
    // an order-bearing record is a heartbeat and is skipped with Ok — not rejected.
    let mut lob = default_lob();
    let mut state = LobState::new(10);
    let malformed_add = MboMessage::new(5, Action::Add, Side::Bid, PX_100_00, 0);
    let result = lob.process_message_into(&malformed_add, &mut state);
    assert!(
        result.is_ok(),
        "a size-0 Add is a heartbeat under the DEFAULT config and must be skipped with Ok, \
         got {result:?}"
    );
    assert_eq!(
        lob.stats().system_messages_skipped,
        1,
        "the size-0 Add was skipped"
    );
    let malformed_t = trade_print(0, Side::Bid, PX_100_00, 0);
    let result = lob.process_message_into(&malformed_t, &mut state);
    assert!(
        matches!(result, Err(TlobError::InvalidSize(0))),
        "the same malformation on a trade print must fail closed, got {result:?}"
    );
    assert_eq!(
        lob.stats().system_messages_skipped,
        1,
        "the malformed trade print was rejected, not skipped"
    );
}

/// (g) DESIGN B. `is_system_message()` stays BYTE-IDENTICAL: three consumers
/// (`feature-extractor-MBO-LOB`, `mbo-statistical-profiler`,
/// `xsec_equity_discovery/extractor`) are linked to this crate BY PATH, so any edit that
/// made it exempt `TradeAggregate` would admit `T` in the extractor with no extractor edit
/// and silently re-phase its exported rows. The predicate is ACTION-BLIND and SIDE-BLIND:
/// the same literal truth table for every variant on every side.
#[test]
fn design_b_is_system_message_truth_table_unchanged() {
    for action in ALL_ACTIONS {
        for side in ALL_SIDES {
            for (order_id, size, price, expected) in SHAPE_GRID {
                let m = MboMessage::new(order_id, action, side, price, size);
                assert_eq!(
                    m.is_system_message(),
                    expected,
                    "DESIGN B VIOLATED: is_system_message() for {action:?}/{side:?} at \
                     order_id {order_id} / size {size} / price {price} must be {expected} — \
                     it is exactly `order_id == 0 || size == 0 || price <= 0`, for every \
                     action. Exempt T in is_heartbeat(), never here."
                );
            }
        }
    }
}

/// (h) `is_heartbeat()` IS the field-shape test for every action except the two exempt ones,
/// and is `false` for those two on EVERY shape and side.
#[test]
fn is_heartbeat_is_field_shape_except_for_clear_and_trade_aggregate() {
    for action in ALL_ACTIONS {
        for side in ALL_SIDES {
            for (order_id, size, price, shape_is_system) in SHAPE_GRID {
                let m = MboMessage::new(order_id, action, side, price, size);
                let expected = if heartbeat_exempt(action) {
                    false
                } else {
                    shape_is_system
                };
                assert_eq!(
                    m.is_heartbeat(),
                    expected,
                    "is_heartbeat() for {action:?}/{side:?} at order_id {order_id} / size \
                     {size} / price {price} must be {expected}"
                );
            }
        }
    }
}

/// A literal `validate_admission()` outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Admit {
    Ok,
    OrderId,
    Price(i64),
    Size,
}

/// `SHAPE_GRID` with its LITERAL `validate_admission()` outcome for the two validated
/// groups: `(order_id, size, price, order-bearing and None => validate(), TradeAggregate =>
/// field clauses only)`. `validate()`'s clause order is `order_id == 0`, then `price <= 0`,
/// then the undefined-price sentinel `i64::MAX`, then `size == 0`; the first failing clause
/// decides the error. `Clear` is `Ok` on every shape.
const ADMISSION_GRID: [(u64, u32, i64, Admit, Admit); 16] = [
    (0, 0, 0, Admit::OrderId, Admit::Price(0)),
    (0, 0, -1, Admit::OrderId, Admit::Price(-1)),
    (0, 0, 1, Admit::OrderId, Admit::Size),
    (0, 0, i64::MAX, Admit::OrderId, Admit::Price(i64::MAX)),
    (0, 5, 0, Admit::OrderId, Admit::Price(0)),
    (0, 5, -1, Admit::OrderId, Admit::Price(-1)),
    (0, 5, 1, Admit::OrderId, Admit::Ok),
    (0, 5, i64::MAX, Admit::OrderId, Admit::Price(i64::MAX)),
    (7, 0, 0, Admit::Price(0), Admit::Price(0)),
    (7, 0, -1, Admit::Price(-1), Admit::Price(-1)),
    (7, 0, 1, Admit::Size, Admit::Size),
    (
        7,
        0,
        i64::MAX,
        Admit::Price(i64::MAX),
        Admit::Price(i64::MAX),
    ),
    (7, 5, 0, Admit::Price(0), Admit::Price(0)),
    (7, 5, -1, Admit::Price(-1), Admit::Price(-1)),
    (7, 5, 1, Admit::Ok, Admit::Ok),
    (
        7,
        5,
        i64::MAX,
        Admit::Price(i64::MAX),
        Admit::Price(i64::MAX),
    ),
];

/// Map a result onto the literal table; any other error variant maps to `None` and fails.
fn admit_outcome(result: &mbo_lob_reconstructor::Result<()>) -> Option<Admit> {
    match result {
        Ok(()) => Some(Admit::Ok),
        Err(TlobError::InvalidOrderId(0)) => Some(Admit::OrderId),
        Err(TlobError::InvalidPrice(price)) => Some(Admit::Price(*price)),
        Err(TlobError::InvalidSize(0)) => Some(Admit::Size),
        Err(_) => None,
    }
}

/// (i) EVERY arm of `validate_admission()` — every `Action` × every shape × all three sides —
/// against a literal table. Before this test only the `TradeAggregate` and `Add` arms had a
/// test expecting a rejection, so a `Modify`, `Cancel`, `Fill` or `None` arm returning `Ok`
/// unconditionally would have stayed green.
#[test]
fn validate_admission_truth_table() {
    for action in ALL_ACTIONS {
        for side in ALL_SIDES {
            for (order_id, size, price, order_bearing, trade_print) in ADMISSION_GRID {
                // Exhaustive, no wildcard: a future `Action` variant must be dispositioned.
                let expected = match action {
                    Action::Add => order_bearing,
                    Action::Modify => order_bearing,
                    Action::Cancel => order_bearing,
                    Action::TradeAggregate => trade_print,
                    Action::Fill => order_bearing,
                    Action::Clear => Admit::Ok,
                    Action::None => order_bearing,
                };
                let m = MboMessage::new(order_id, action, side, price, size);
                let result = m.validate_admission();
                assert_eq!(
                    admit_outcome(&result),
                    Some(expected),
                    "validate_admission() for {action:?}/{side:?} at order_id {order_id} / \
                     size {size} / price {price}: got {result:?}, expected {expected:?}"
                );
            }
        }
    }
}
