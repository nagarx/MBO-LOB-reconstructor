//! THE DECODE-BOUNDARY CONTRACT — what `DbnBridge::convert` must reject, what it
//! must let through, and what it must carry.
//!
//! # Why this is a separate integration binary and not an inline `mod tests`
//!
//! `FINDING-177`: a literal-expectation test that lives in the same file as the code
//! it guards does **not** survive the mutation that actually happens — a whole-file
//! substitution moves the test along with the production line, and the suite stays
//! green while the behaviour inverts. Measured there: 19 occurrences swapped in one
//! file, 892 passed / 0 failed, three feature columns flat-lined. The protection is
//! **cross-file placement**, not the literal table. The W04 specification asked for
//! `#[test] fn convert_rejects_undef_price_on_order_bearing_action` *inside*
//! `src/dbn_bridge.rs`; that placement is the exact shape `FINDING-177` refutes, so
//! the contract lives here instead.
//!
//! For the same reason every sentinel below is written as a **literal**, never as
//! `dbn::UNDEF_PRICE`. Importing the constant the guard imports makes the test pass
//! by construction: if the guard ever compared against the wrong constant, an
//! importing test would follow it there and agree.
//!
//! # The aperture of the live measurement these tests encode
//!
//! Re-measured 2026-09-03 with the `dbn` CLI over **94,542,598 MBO records**,
//! 16 day-files, 2 venues (XNAS.ITCH, ARCX.PILLAR), 5 instruments
//! (NVDA, SNAP, CRSP, PEP + the ARCX NVDA book), spanning 2025-02-03 → 2026-01-07:
//!
//! ```text
//!   price == i64::MAX (dbn::UNDEF_PRICE)          20   100% on Action::Clear, 0 elsewhere
//!   size  == u32::MAX (dbn::UNDEF_ORDER_SIZE)      0
//!   size  == i32::MAX (the GLBX quantity form)     0
//!   price <= 0                                     0   the guard that already exists has never fired
//!   ts_event == 0 / == u64::MAX                    0
//!   Action::None population                        0   ZERO records, on every file
//! ```
//!
//! Every `Clear` in that population carries the sentinel (20 sentinels / 20 `R`
//! records) — 1 per XNAS day-file, 2 per ARCX day-file. **That is what makes the
//! Clear exemption load-bearing rather than defensive**, and it is the single most
//! likely way to get W04 wrong: a guard that keys on the FIELD instead of the ACTION
//! rejects every book reset in the corpus.

use mbo_lob_reconstructor::{Action, MboMessage, Side, TlobError};

/// `dbn::UNDEF_PRICE`, written out. See the module docs on why this is not imported.
const UNDEF_PRICE_LITERAL: i64 = 9_223_372_036_854_775_807;
/// `dbn::UNDEF_ORDER_SIZE`, written out.
const UNDEF_ORDER_SIZE_LITERAL: u32 = 4_294_967_295;

/// A plausible NVDA price in nanodollars, and a plausible round-lot size.
const GOOD_PRICE: i64 = 157_040_000_000;
const GOOD_SIZE: u32 = 200;

// ════════════════════════════════════════════════════════════════════════════
// THE HOT-PATH BUDGET
// ════════════════════════════════════════════════════════════════════════════

/// `MboMessage` is the per-record struct on the decode hot path — one instance per
/// vendor record, ~4.24e9 records per corpus pass. hft-rules §12 makes its footprint
/// a first-class property, and every doc in this crate that describes the conversion
/// as "we only copy the ~40 bytes of `MboMessage`" is asserting this number.
///
/// This test exists so the next field addition has to *see* the cost. It goes red on
/// any layout change, which is the point: a field that pushes the struct over a
/// padding boundary should be a decision, not a surprise.
///
/// Scope: 64-bit targets, where `u64`/`i64` align to 8. This crate has no 32-bit
/// consumer; if one ever appears, this assertion is where that shows up.
#[test]
fn mbo_message_fits_the_documented_forty_byte_hot_path_budget() {
    assert_eq!(
        std::mem::size_of::<MboMessage>(),
        40,
        "MboMessage grew. The decode path copies one of these per vendor record \
         (~4.24e9 per corpus pass) and this crate's docs quote 40 bytes. If the \
         growth is intended, change the number here and say why in the commit."
    );
}

// ════════════════════════════════════════════════════════════════════════════
// W04 — `MboMessage::validate()`, the constructor-level invariant
// ════════════════════════════════════════════════════════════════════════════

/// `validate()` already rejects `price <= 0`. `i64::MAX` is **positive**, so the
/// undefined-price sentinel walks straight through it and out the other side as
/// `price_as_f64() == 9_223_372_036.854_776` — a finite, plausible-looking
/// $9.2-billion quote that passes every `is_finite()` check downstream. This is
/// hft-rules §2's named scar: "an unguarded divide neither crashes nor yields NaN".
///
/// The live reachability of this guard: `LobReconstructor::process_message_into`
/// calls `msg.validate_admission()?` under `config.validate_messages`, which
/// **defaults to `true`** — `validate()` for every order-bearing action and `None`,
/// and the same field clauses (this sentinel among them) for a `TradeAggregate` — so
/// it runs on every admitted non-`Clear` record of every day.
#[test]
fn validate_rejects_the_undef_price_sentinel_but_accepts_a_real_price() {
    let sentinel = MboMessage::new(1, Action::Add, Side::Bid, UNDEF_PRICE_LITERAL, GOOD_SIZE);
    match sentinel.validate() {
        Err(TlobError::InvalidPrice(p)) => assert_eq!(p, UNDEF_PRICE_LITERAL),
        other => panic!(
            "validate() admitted the undefined-price sentinel: {other:?}. \
             price_as_f64() would be {:.6}, which is finite and reads as a real quote.",
            sentinel.price_as_f64()
        ),
    }

    // POSITIVE CONTROL. Without this, a `validate()` that rejected everything would
    // satisfy the assertion above (`FINDING-172`).
    let good = MboMessage::new(1, Action::Add, Side::Bid, GOOD_PRICE, GOOD_SIZE);
    assert!(
        good.validate().is_ok(),
        "the guard rejects a normal record — it is over-broad, not protective"
    );
}

/// ⭐ **THE ASYMMETRY, LOCKED.** `u32::MAX` is a NULL ON THE WIRE and a LEGITIMATE
/// MAXIMUM IN THIS TYPE, so it is rejected at the vendor boundary and accepted by the
/// domain invariant. `i64::MAX` is neither legitimate nor representable-maximum-useful
/// as a price, so it is rejected in both places.
///
/// This test exists because that split looks like an inconsistency and the obvious
/// "tidy-up" is to make `validate()` reject both. That tidy-up was TRIED AND REFUTED
/// BY EXECUTION: it fails `tests/integration_test.rs::test_edge_case_large_sizes`, a
/// pre-existing overflow-boundary contract that builds a book at `size = u32::MAX`
/// via `MboMessage::new` and asserts the u32 -> u64 widening does not overflow. That
/// test uses the value as the largest representable size, never as a sentinel, and it
/// never touches the vendor path.
///
/// The vendor agrees: `data/DATABENTO_SCHEMA_REFERENCE.md` limits §4 says ordinary
/// trade, MBO, MBP, BBO and CBBO size fields "do not all document a field-specific
/// null rule", while §7 MANDATES the `i64::MAX` test for every fixed price.
#[test]
fn undef_order_size_is_a_wire_null_but_a_legitimate_size_for_the_domain_type() {
    let at_max = MboMessage::new(
        1,
        Action::Add,
        Side::Bid,
        GOOD_PRICE,
        UNDEF_ORDER_SIZE_LITERAL,
    );
    assert!(
        at_max.validate().is_ok(),
        "validate() must NOT reject the representational maximum — u32::MAX is a \
         vendor wire null, not an invalid quantity for this type, and \
         integration_test::test_edge_case_large_sizes depends on it being admitted"
    );
    // ...while the vendor boundary rejects the very same value: see
    // `vendor_decode::undef_order_size_is_rejected_on_every_price_bearing_action`.
    // Two guards, two different questions, deliberately not harmonised.
}

// ════════════════════════════════════════════════════════════════════════════
// W04 / W05 — the vendor decode boundary itself
// ════════════════════════════════════════════════════════════════════════════

#[cfg(feature = "databento")]
mod vendor_decode {
    use super::*;
    use mbo_lob_reconstructor::DbnBridge;

    /// Build a `dbn::MboMsg` with a real timestamp, so nothing under test is
    /// entangled with the separate `ts_event == 0` dispatch.
    fn vendor_msg(
        action: u8,
        side: u8,
        order_id: u64,
        price: i64,
        size: u32,
        flags: u8,
    ) -> dbn::MboMsg {
        dbn::MboMsg {
            hd: dbn::RecordHeader::new::<dbn::MboMsg>(0, 2, 11667, 1_751_356_800_002_015_312),
            order_id,
            price,
            size,
            flags: dbn::FlagSet::new(flags),
            channel_id: 0,
            action: action as i8,
            side: side as i8,
            ts_recv: 1_751_356_800_002_015_312,
            ts_in_delta: 165_645,
            sequence: 287_176,
        }
    }

    /// The five actions for which a price is a meaningful quantity. `R` (Clear) and
    /// `N` (None) are deliberately absent — see the Clear test above and the guard
    /// site's comment for why the partition is what it is.
    const PRICE_BEARING_ACTIONS: [u8; 5] = [b'A', b'M', b'C', b'F', b'T'];

    #[test]
    fn undef_price_is_rejected_on_every_price_bearing_action() {
        for action in PRICE_BEARING_ACTIONS {
            let msg = vendor_msg(action, b'B', 12345, UNDEF_PRICE_LITERAL, GOOD_SIZE, 128);
            match DbnBridge::convert(&msg) {
                Err(TlobError::InvalidPrice(p)) => assert_eq!(p, UNDEF_PRICE_LITERAL),
                other => panic!(
                    "action {} admitted the undefined-price sentinel: {other:?}",
                    action as char
                ),
            }
        }
    }

    #[test]
    fn undef_order_size_is_rejected_on_every_price_bearing_action() {
        for action in PRICE_BEARING_ACTIONS {
            let msg = vendor_msg(
                action,
                b'B',
                12345,
                GOOD_PRICE,
                UNDEF_ORDER_SIZE_LITERAL,
                128,
            );
            match DbnBridge::convert(&msg) {
                Err(TlobError::InvalidSize(s)) => assert_eq!(s, UNDEF_ORDER_SIZE_LITERAL),
                other => panic!(
                    "action {} admitted the undefined-size sentinel: {other:?}",
                    action as char
                ),
            }
        }
    }

    /// POSITIVE CONTROL for both guards above, and for the two exempt actions.
    ///
    /// Every one of the seven vendor action bytes, carrying values that are ordinary
    /// in the live tape, must still convert. A guard that rejected everything — or
    /// that keyed on the field rather than the action — passes the two rejection
    /// tests above and fails here. `FINDING-172`: a falsifier with no positive
    /// control cannot distinguish "the mechanism fired" from "the mechanism ate
    /// everything".
    #[test]
    fn every_vendor_action_still_converts_on_an_ordinary_record() {
        for action in [b'A', b'M', b'C', b'F', b'T', b'R', b'N'] {
            let msg = vendor_msg(action, b'B', 12345, GOOD_PRICE, GOOD_SIZE, 128);
            assert!(
                DbnBridge::convert(&msg).is_ok(),
                "action {} no longer converts on an ordinary record — the guard is over-broad",
                action as char
            );
        }
    }

    // ════════════════════════════════════════════════════════════════════════
    // THE CLEAR EXEMPTION — the load-bearing non-regression
    // ════════════════════════════════════════════════════════════════════════

    /// **THE ONE TEST THAT SEPARATES A CORRECT W04 FROM THE OBVIOUS WRONG ONE.**
    ///
    /// The record decoded here is the exact shape the vendor sends, copied from the
    /// first record of `xnas-itch-20250701.mbo.dbn.zst`:
    ///
    /// ```text
    ///   action R | side N | price 9223372036854775807 | size 0 | order_id 0 | flags 8
    /// ```
    ///
    /// 100% of the corpus's `UNDEF_PRICE` population lives on records of this shape
    /// (20 sentinels against exactly 20 `R` records in a 94,542,598-record census),
    /// and each one is a session-boundary **book reset**. A guard keyed on the FIELD
    /// rather than the ACTION rejects every one of them. Nothing errors: the book
    /// simply never resets, and each day inherits the previous day's resting orders.
    ///
    /// ⚠ **THIS TEST TRAVERSES `convert()` AND *THEN* THE BOOK, AND THE FIRST HALF IS
    /// NOT DECORATION.** An earlier draft built the `Clear` with `MboMessage::new`
    /// and pushed it straight into `LobReconstructor`. It passed — and it went on
    /// passing when `Action::Clear` was moved into the guarded arm of `convert()`,
    /// because it never called `convert()` at all. It was measuring the
    /// *reconstructor's* Clear exemption while claiming to protect the *decoder's*
    /// (`FINDING-172`: the falsifier's subject has to be observable BY the falsifier).
    /// Under the same negative control this version fails. Both layers, one path.
    #[test]
    fn the_vendor_clear_survives_the_decoder_and_still_resets_the_book() {
        use mbo_lob_reconstructor::LobReconstructor;

        let mut lob = LobReconstructor::new(10);

        // Seed a book, so "reset" has something observable to do. A Clear against an
        // empty book empties an empty book, which is indistinguishable from a Clear
        // that was dropped (`FINDING-174` — the fixture must occupy the live
        // position in the predicate's input space).
        for (id, side, px) in [(1u64, b'B', GOOD_PRICE), (2, b'A', GOOD_PRICE + 10_000_000)] {
            let add = DbnBridge::convert(&vendor_msg(b'A', side, id, px, 100, 128))
                .expect("seed add must decode");
            lob.process_message(&add)
                .expect("seed add must be admitted");
        }
        assert_eq!(
            lob.order_count(),
            2,
            "the book must be non-empty before the Clear"
        );
        assert_eq!(lob.stats().book_clears, 0);

        // The vendor's own Clear, decoded exactly as the loader would decode it.
        let clear = DbnBridge::convert(&vendor_msg(b'R', b'N', 0, UNDEF_PRICE_LITERAL, 0, 8))
            .expect("the vendor sends exactly this record 1-2x per day per venue");
        assert_eq!(clear.action, Action::Clear);
        assert_eq!(
            clear.price, UNDEF_PRICE_LITERAL,
            "the sentinel must survive verbatim"
        );

        lob.process_message(&clear)
            .expect("the vendor's Clear must be admitted, sentinel price and all");

        assert_eq!(
            lob.stats().book_clears,
            1,
            "the Clear did not reach the reset arm — a sentinel guard swallowed the \
             session boundary"
        );
        assert_eq!(
            lob.order_count(),
            0,
            "the Clear was counted but the book was not emptied"
        );
    }

    /// The other exempt action, on a ZERO-RECORD population. `N` appears 0 times in
    /// all 94,542,598 records measured, so the exemption is a judgement rather than a
    /// measurement — which is exactly why it needs a behavioural test: live data
    /// cannot qualify it in either direction (`FINDING-155`).
    #[test]
    fn the_exempt_actions_admit_the_sentinels_that_the_guarded_ones_reject() {
        for action in [b'R', b'N'] {
            assert!(
                DbnBridge::convert(&vendor_msg(action, b'N', 0, UNDEF_PRICE_LITERAL, 0, 0)).is_ok(),
                "action {} must stay exempt from the price guard",
                action as char
            );
            assert!(
                DbnBridge::convert(&vendor_msg(
                    action,
                    b'N',
                    0,
                    GOOD_PRICE,
                    UNDEF_ORDER_SIZE_LITERAL,
                    0
                ))
                .is_ok(),
                "action {} must stay exempt from the size guard",
                action as char
            );
        }
    }

    // ════════════════════════════════════════════════════════════════════════
    // W05 — THE VENDOR FLAG BYTE
    // ════════════════════════════════════════════════════════════════════════

    /// `dbn::MboMsg.flags` is documented by the vendor as "a bit field indicating
    /// event end, message characteristics, and **data quality**". Before this
    /// contract the decoder never read it, so the byte died at `convert()` and no
    /// consumer downstream could recover it.
    ///
    /// The raw values below are not invented. They are the COMPLETE set observed in
    /// a 94,542,598-record live census (2026-09-03, 16 day-files, 2 venues, 5
    /// instruments), plus three synthetic values that the census proves are absent —
    /// which is exactly why they must be in the test: a carrier that only ever sees
    /// `{0, 8, 128, 130}` in production would never exercise the other five bits, so
    /// live data cannot qualify it (`FINDING-155`). Only a behavioural test can.
    ///
    /// ```text
    ///   observed live      0   (no flags)
    ///                      8   BAD_TS_RECV  — exactly 1 record per file, 16/16 files
    ///                    128   LAST         — 52.29%-85.17% of records
    ///                    130   LAST|PUBLISHER_SPECIFIC
    ///   never observed     4   MAYBE_BAD_BOOK   0 / 94,542,598
    ///                     32   SNAPSHOT         0 / 94,542,598
    ///                    255   all bits set
    /// ```
    ///
    /// ⚠ The encoding is NOT stable over the corpus. `PUBLISHER_SPECIFIC` went from
    /// 52.197% to 0.000% between `xnas-itch-20250801` and `xnas-itch-20250804`, and
    /// on ARCX from 84.228% to 0.000% on the same two dates — reproduced here to
    /// three decimals, and extended to SNAP (70.585 -> 0), CRSP (46.563 -> 0) and
    /// PEP (55.354 -> 0). The 233-day flagship corpus straddles that boundary and
    /// nothing in either repo could detect it. That is the reason the carrier is the
    /// raw byte and not a fixed set of per-bit counters: a counter set chosen today
    /// forecloses whichever bit turns out to matter next.
    #[test]
    fn the_vendor_flag_byte_survives_the_decoder_bit_for_bit() {
        for raw in [0u8, 8, 128, 130, 4, 32, 255] {
            let msg = vendor_msg(b'A', b'B', 12345, GOOD_PRICE, GOOD_SIZE, raw);
            let converted = DbnBridge::convert(&msg).expect("ordinary record must convert");
            assert_eq!(
                converted.flags, raw,
                "the decoder dropped or rewrote the vendor flag byte {raw:#010b}"
            );
        }
    }

    /// The carrier must not be a constant. A `flags` field populated with a literal
    /// `0` satisfies "the field exists" and every consumer reading it gets the same
    /// answer forever — the shape this arc has now hit six times, a mechanism that is
    /// correct and never reaches the thing it names. This asserts the field VARIES
    /// with the input, which a hardcoded population cannot do.
    #[test]
    fn the_flag_carrier_is_populated_from_the_record_and_not_from_a_constant() {
        let a = DbnBridge::convert(&vendor_msg(b'A', b'B', 1, GOOD_PRICE, GOOD_SIZE, 0))
            .expect("must convert");
        let b = DbnBridge::convert(&vendor_msg(b'A', b'B', 1, GOOD_PRICE, GOOD_SIZE, 130))
            .expect("must convert");
        assert_ne!(
            a.flags, b.flags,
            "two records with different vendor flags decoded to the same flag byte \
             — the field is not being read from the record"
        );
    }

    // ════════════════════════════════════════════════════════════════════════
    // THE TWO SPELLINGS OF ONE CONSTANT
    // ════════════════════════════════════════════════════════════════════════

    /// `src/types.rs::MboMessage::validate` compares against `i64::MAX` / `u32::MAX`
    /// spelled out, because `types.rs` must compile without the `databento` feature
    /// and therefore cannot name `dbn::UNDEF_PRICE` / `dbn::UNDEF_ORDER_SIZE`.
    /// `src/dbn_bridge.rs::convert` names the vendor constants directly.
    ///
    /// So the crate now holds the sentinel in TWO places. That is precisely the shape
    /// that produced this crate's worst decode defect: two byte maps, the correct one
    /// with no callers and the defective one decoding everything. This test is the
    /// only thing standing between that and a repeat. It goes red if a `dbn` version
    /// bump ever redefines either constant — which is a real trigger: the pin has
    /// already moved v0.20.0 -> v0.64.0 once.
    #[test]
    fn vendor_sentinel_constants_still_have_the_values_this_crate_hardcodes() {
        assert_eq!(
            dbn::UNDEF_PRICE,
            i64::MAX,
            "dbn::UNDEF_PRICE moved; src/types.rs::validate still compares against i64::MAX"
        );
        assert_eq!(
            dbn::UNDEF_ORDER_SIZE,
            u32::MAX,
            "dbn::UNDEF_ORDER_SIZE moved; src/types.rs::validate still compares against u32::MAX"
        );
        // And the literals this file asserts with are those same values.
        assert_eq!(UNDEF_PRICE_LITERAL, dbn::UNDEF_PRICE);
        assert_eq!(UNDEF_ORDER_SIZE_LITERAL, dbn::UNDEF_ORDER_SIZE);
    }
}
