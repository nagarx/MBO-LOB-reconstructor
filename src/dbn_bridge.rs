//! Bridge between Databento's DBN format and TLOB internal types.
//!
//! This module provides efficient conversion from `dbn::MboMsg` to our internal
//! `MboMessage` type. The conversion is designed to be:
//! - Zero-copy where possible
//! - Type-safe (compile-time guarantees)
//! - Handles edge cases gracefully
//! - Provides clear error messages
//!
//! # Example
//!
//! ```ignore
//! use dbn::MboMsg;
//! use mbo_lob_reconstructor::DbnBridge;
//!
//! // Assuming you have a dbn::MboMsg from the decoder
//! let dbn_msg: MboMsg = /* ... */;
//!
//! // Convert to our internal type
//! let mbo_msg = DbnBridge::convert(&dbn_msg)?;
//! ```

use crate::error::{Result, TlobError};
use crate::types::{Action, MboMessage, Side};

/// Bridge for converting DBN messages to TLOB types.
pub struct DbnBridge;

impl DbnBridge {
    /// Convert a DBN MboMsg to our internal MboMessage.
    ///
    /// # Arguments
    ///
    /// * `msg` - Reference to a `dbn::MboMsg`
    ///
    /// # Returns
    ///
    /// * `Ok(MboMessage)` - Successfully converted message
    /// * `Err(TlobError)` - Conversion failed (invalid action/side)
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mbo_msg = DbnBridge::convert(&dbn_msg)?;
    /// ```
    #[inline]
    pub fn convert(msg: &dbn::MboMsg) -> Result<MboMessage> {
        // Convert action (DBN uses i8, we convert to u8)
        let action = Self::convert_action(msg.action as u8)?;

        // Convert side (DBN uses i8, we convert to u8)
        let side = Self::convert_side(msg.side as u8)?;

        // Phase M M.A.6 (REV 3 F-023 closure) + M.A.9 (post-validation
        // F-010 ↔ F-023 cross-cascade fix). DBN stores `ts_event` as `u64`
        // nanoseconds. Three cases:
        //
        // 1. `ts_event > i64::MAX` (cast wraps to negative). Per hft-rules
        //    §2 (zero precision errors): always corrupt; fail-loud as
        //    `InvalidTimestamp`. This branch fires regardless of the action
        //    — overflow is genuine corruption.
        //
        // 2. `ts_event == 0` on an action for which the vendor legitimately
        //    omits a timestamp. Databento uses ts_event=0 as a sentinel for
        //    "no timestamp" on these records. Pre-M.A.9 the M.A.6 F-023 fix
        //    rejected ALL ts_event=0 as `InvalidTimestamp`, which silently
        //    shadowed the M.A.7 F-010 `system_messages_seen` counter at the
        //    typed iterator (those records flowed through the
        //    `BoundaryError::Convert` arm and inflated
        //    `rows_skipped_decode_or_convert` instead of counting as
        //    expected non-order events). Post-M.A.9 we yield them with
        //    `timestamp: None` so the iterator counts them correctly.
        //
        // 3. `ts_event == 0` on genuine order data. Per hft-rules §8 —
        //    corrupt feed; fail-loud as `InvalidTimestamp`. This is the core
        //    F-023 fail-loud surface that M.A.6 introduced.
        //
        // ⚠ THE DISCRIMINATOR IS THE DECODED **ACTION**, NOT THE FIELD SHAPE.
        //
        // This predicate used to be an inline longhand copy of
        // `MboMessage::is_system_message()` — `order_id == 0 || size == 0 ||
        // price <= 0`. That is a FIELD-SHAPE test standing in for a
        // SEMANTIC one, and it is wrong in both directions:
        //
        //   * It is venue-dependent, so it is not a sound proxy for
        //     "non-order-bearing" ACROSS venues. Measured on ARCX 2025-02-03,
        //     88,024 `T|N` trade prints carry `order_id != 0` — 19.8% of that
        //     day's `T` population — i.e. a whole fifth of that venue's trade
        //     prints are invisible to the field-shape test but correctly
        //     classified by the action test. (On XNAS.ITCH, 100% of `T` carries
        //     `order_id == 0`, which is exactly why the substitution went
        //     unnoticed: the two predicates agree on the one venue that was
        //     examined.)
        //
        //     ⚠ SCOPE, STATED PRECISELY: those 88,024 records are not
        //     misclassified *here*, because this branch only runs under
        //     `ts_event == 0` and ZERO ARCX records carry that. The ARCX
        //     divergence bites at the `is_system_message()` admission guards in
        //     `queue_position` / `order_lifecycle` / the extractor's
        //     `adapters.rs`, which are a DIFFERENT code path (resolved at
        //     L-ADMIT). The census is cited here as the reason to key on the
        //     ACTION rather than on field shape — not as a defect this
        //     predicate is repairing today.
        //   * It is a fourth, greppable-only-by-luck copy of a predicate the
        //     L-ADMIT layer deletes. No search for the function NAME finds
        //     it, so deleting the method would have left this copy alive
        //     under a local variable name and the deletion would have been
        //     cosmetic.
        //
        // The actions for which a missing timestamp is legitimate are the
        // NON-ORDER-BEARING ones: `TradeAggregate` (the aggressing order's
        // print), `Clear` (the session-boundary reset) and `None` (an
        // informational no-op that may carry only flags). For
        // `Add`/`Modify`/`Cancel`/`Fill` — all of which name a specific
        // resting order — a missing timestamp is corruption and fails loud.
        //
        // ⚠ DEVIATION, STATED: the specification's D-4 clause enumerates six
        // of the seven variants — `TradeAggregate`/`Clear` permissive and
        // `Add|Modify|Cancel|Fill` fail-loud — and is silent on
        // `Action::None`. An exhaustive match must place it. It is grouped
        // with the permissive set on two grounds: (a) the L-ADMIT layer's
        // specified `validate()` rewrite groups it exactly the same way
        // (`Action::None | Action::Clear => Ok(())`); and (b) fail-loud there
        // would be a NEW hard rejection of an unmeasured population that also
        // silently deflates `LobStats::noop_messages`, whereas the permissive
        // choice cannot lose a record. `N` measures ZERO records on both
        // pre-registered development days, so the choice is a no-op there
        // either way — it is stated because it is not derivable from the spec.
        //
        // ⚠ Measured NO-OP on the two pre-registered development days: the
        // divergence set between the old and new predicates is
        // {A,M,C,F with a zero field} ∪ {T,R without one}, and on
        // 2025-07-01 / 2025-07-02 / 2025-02-03 `price <= 0` matches 0 of
        // 34,573,499 records, `size == 0` matches only the daily `R`, and
        // `order_id == 0` matches only `T`. Both sets are therefore EMPTY
        // there. The change is a semantics repair, not a behaviour change.
        let ts_signed = msg.hd.ts_event as i64;
        if ts_signed < 0 {
            // Case 1: u64 overflow → always corrupt.
            return Err(TlobError::InvalidTimestamp(ts_signed));
        }
        let timestamp = if msg.hd.ts_event == 0 {
            // Exhaustive, no wildcard: a future `Action` variant must be a
            // compile error here, not a silently-admitted timestamp.
            match action {
                // Case 2: legitimate Databento sentinel → preserve as `None`
                // so downstream observability (F-010 counter) still sees it.
                Action::TradeAggregate | Action::Clear | Action::None => Option::None,
                // Case 3: genuine corrupt feed — an order-bearing action with
                // no timestamp violates the F-023 fail-loud surface.
                Action::Add | Action::Modify | Action::Cancel | Action::Fill => {
                    return Err(TlobError::InvalidTimestamp(0));
                }
            }
        } else {
            Some(ts_signed)
        };

        Ok(MboMessage {
            order_id: msg.order_id,
            action,
            side,
            price: msg.price,
            size: msg.size,
            timestamp,
        })
    }

    /// Convert DBN action character to our [`Action`] enum.
    ///
    /// DBN uses single-character codes for actions. This function is a thin, fail-loud wrapper
    /// over [`Action::from_byte`], which is the **canonical** vendor-byte map for this crate.
    ///
    /// # Why this delegates instead of carrying its own match
    ///
    /// Until 2026-08-16 there were **two** byte maps in this crate: `Action::from_byte`, which
    /// mapped `b'T' -> T` and `b'F' -> F` correctly and had zero production callers; and this
    /// function, which merged `b'T' | b'F'` into a single variant and decoded every byte the
    /// pipeline ever read. The correct decoder existed all along, tested and green, while the
    /// defective one shipped — the problem was never knowledge, it was **routing**. One map means
    /// the next byte-semantics question cannot be answered correctly in a dead copy and wrongly in
    /// the live one.
    ///
    /// # The two execution carriers, and why they must not be merged
    ///
    /// `T` and `F` are **not** two spellings of the same event. They are the two sides of one
    /// physical execution, and the vendor's `side` field means **opposite things** on them:
    ///
    /// * `T` — the aggressing order's **trade print**; `side` is the **AGGRESSOR's** side.
    /// * `F` — a fill against an **existing resting order**; `side` is the **RESTING order's** side.
    ///
    /// Because the populations are exact side-mirrors, merging them does not lose the sign — it
    /// **inverts it on the `F` half**, and the two halves annihilate. Measured, NVDA XNAS
    /// 2025-02-03: `T|A` 258,355 recs / 19,172,582 sh ≡ `F|B` 258,355 / 19,172,582, and
    /// `T|B` 215,055 / 19,497,864 ≡ `F|A` 215,055 / 19,497,864. Signed volume from `T` alone
    /// `+325,282` and from `F` alone `−325,282` — **combined exactly 0**. A directional feature
    /// built on the merged stream reads "no signal", indistinguishable from a genuine null.
    ///
    /// Both are also documented by the vendor as **book no-ops**. Databento's own MBP-10 contains
    /// **zero `F` records**, and treating `F` as a book no-op reproduces that vendor book
    /// bit-exactly on 100.000% of book-affecting records (2025-07-01, 4,214,602 RTH comparisons).
    ///
    /// # ⚠ WHAT IS AND IS NOT FIXED AT THIS COMMIT (L-DECODE only)
    ///
    /// This function now emits the two carriers as distinct variants, so the emitted **action
    /// byte** is correct and `Action::Fill` reaches the Parquet `action` column as `70` for the
    /// first time. **The router still merges them**: `reconstructor.rs`'s dispatch arm is
    /// `Action::TradeAggregate | Action::Fill => process_trade(...)`, so the **book is byte-for-byte
    /// unchanged** and `LobStats.cancel_order_not_found` / `trade_order_not_found` still carry
    /// their full mass (see `WARNINGS.md` §1). That is the L-ROUTE layer and it is a separate,
    /// sequenced commit — deliberately, so this diff stays reviewable in isolation.
    ///
    /// Note also that no part of this change recovers the `side == 'N'` prints — 21.27% of trade
    /// count but **50.33% of traded volume** — which die independently at consumer-side
    /// `order_id == 0` filters.
    #[inline]
    fn convert_action(action: u8) -> Result<Action> {
        // Single source of truth: `types::Action::from_byte`. Fail loud on an unknown byte —
        // an unrecognised action must never be coerced into a definite one.
        Action::from_byte(action).ok_or(TlobError::InvalidAction(action))
    }

    /// Convert DBN side character to our Side enum.
    ///
    /// DBN uses single-character codes for sides.
    #[inline]
    fn convert_side(side: u8) -> Result<Side> {
        match side {
            b'B' => Ok(Side::Bid),
            b'A' | b'S' => Ok(Side::Ask), // 'S' = sell, treat as ask
            b'N' => Ok(Side::None),
            _ => Err(TlobError::InvalidSide(side)),
        }
    }

    /// Batch convert multiple DBN messages.
    ///
    /// This is more efficient than calling `convert()` in a loop
    /// because it pre-allocates the output vector.
    ///
    /// # Arguments
    ///
    /// * `msgs` - Slice of `dbn::MboMsg` references
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<MboMessage>)` - All messages successfully converted
    /// * `Err(TlobError)` - First conversion error encountered
    pub fn convert_batch(msgs: &[dbn::MboMsg]) -> Result<Vec<MboMessage>> {
        let mut result = Vec::with_capacity(msgs.len());

        for msg in msgs {
            result.push(Self::convert(msg)?);
        }

        Ok(result)
    }

    /// Convert with error recovery.
    ///
    /// Unlike `convert()`, this method doesn't fail on invalid messages.
    /// Instead, it returns `None` for invalid messages and logs a warning.
    ///
    /// # Arguments
    ///
    /// * `msg` - Reference to a `dbn::MboMsg`
    ///
    /// # Returns
    ///
    /// * `Some(MboMessage)` - Successfully converted
    /// * `None` - Conversion failed (message logged)
    #[inline]
    pub fn convert_or_skip(msg: &dbn::MboMsg) -> Option<MboMessage> {
        match Self::convert(msg) {
            Ok(mbo_msg) => Some(mbo_msg),
            Err(e) => {
                log::warn!(
                    "Skipping invalid MBO message (order_id={}): {}",
                    msg.order_id,
                    e
                );
                None
            }
        }
    }

    /// Batch convert with error recovery.
    ///
    /// Returns only the successfully converted messages,
    /// skipping any that fail validation.
    ///
    /// # Arguments
    ///
    /// * `msgs` - Slice of `dbn::MboMsg` references
    ///
    /// # Returns
    ///
    /// * `Vec<MboMessage>` - All successfully converted messages
    /// * Note: The returned vector may be shorter than the input
    pub fn convert_batch_or_skip(msgs: &[dbn::MboMsg]) -> Vec<MboMessage> {
        msgs.iter().filter_map(Self::convert_or_skip).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to create a test MboMsg
    fn create_test_dbn_msg() -> dbn::MboMsg {
        dbn::MboMsg {
            hd: dbn::RecordHeader::new::<dbn::MboMsg>(
                0,                      // rtype
                0,                      // publisher_id
                0,                      // instrument_id
                1234567890_000_000_000, // ts_event
            ),
            order_id: 12345,
            price: 100_000_000_000, // $100.00 in fixed-point
            size: 100,
            flags: dbn::FlagSet::empty(),
            channel_id: 0,
            action: b'A' as i8,
            side: b'B' as i8,
            ts_recv: 1234567890_000_000_000,
            ts_in_delta: 0,
            sequence: 0,
        }
    }

    #[test]
    fn test_convert_action() {
        assert_eq!(DbnBridge::convert_action(b'A').unwrap(), Action::Add);
        assert_eq!(DbnBridge::convert_action(b'M').unwrap(), Action::Modify);
        assert_eq!(DbnBridge::convert_action(b'C').unwrap(), Action::Cancel);
        assert_eq!(DbnBridge::convert_action(b'R').unwrap(), Action::Clear);
        assert_eq!(
            DbnBridge::convert_action(b'T').unwrap(),
            Action::TradeAggregate
        );
        // ⭐ THE T/F SPLIT, ASSERTED. This line previously read `Action::Trade` under a comment
        // declaring that it locked a known bug. It is now the lock on the FIX: `b'F'` is a fill
        // against a resting order and carries the RESTING side, the opposite convention from
        // `b'T'`. Re-merging them here re-annihilates signed order flow.
        assert_eq!(DbnBridge::convert_action(b'F').unwrap(), Action::Fill);
        assert_ne!(
            DbnBridge::convert_action(b'F').unwrap(),
            DbnBridge::convert_action(b'T').unwrap(),
            "the two execution carriers MUST decode to distinct variants"
        );
        assert_eq!(DbnBridge::convert_action(b'N').unwrap(), Action::None);

        // Invalid action
        assert!(DbnBridge::convert_action(b'X').is_err());
    }

    /// The decoder has exactly ONE byte map, and it is `types::Action::from_byte`.
    ///
    /// Two maps is how `b'F'` came to decode as a trade: the correct map existed, tested and
    /// green, with zero production callers, while the defective one decoded every byte the
    /// pipeline ever read. This test fails if the two ever diverge again.
    #[test]
    fn test_convert_action_delegates_to_canonical_map() {
        for byte in [b'A', b'M', b'C', b'T', b'F', b'R', b'N'] {
            assert_eq!(
                DbnBridge::convert_action(byte).unwrap(),
                Action::from_byte(byte).unwrap(),
                "convert_action diverged from Action::from_byte on wire byte {byte}"
            );
        }
        // And the unknown byte fails loud on both paths rather than being coerced.
        assert!(Action::from_byte(b'X').is_none());
        assert!(DbnBridge::convert_action(b'X').is_err());
    }

    #[test]
    fn test_convert_side() {
        assert_eq!(DbnBridge::convert_side(b'B').unwrap(), Side::Bid);
        assert_eq!(DbnBridge::convert_side(b'A').unwrap(), Side::Ask);
        assert_eq!(DbnBridge::convert_side(b'S').unwrap(), Side::Ask);
        assert_eq!(DbnBridge::convert_side(b'N').unwrap(), Side::None);

        // Invalid side
        assert!(DbnBridge::convert_side(b'X').is_err());
    }

    #[test]
    fn test_convert() {
        let dbn_msg = create_test_dbn_msg();
        let mbo_msg = DbnBridge::convert(&dbn_msg).unwrap();

        assert_eq!(mbo_msg.order_id, 12345);
        assert_eq!(mbo_msg.action, Action::Add);
        assert_eq!(mbo_msg.side, Side::Bid);
        assert_eq!(mbo_msg.price, 100_000_000_000);
        assert_eq!(mbo_msg.size, 100);
        assert_eq!(mbo_msg.timestamp, Some(1234567890_000_000_000));
    }

    #[test]
    fn test_convert_or_skip_valid() {
        let dbn_msg = create_test_dbn_msg();
        let mbo_msg = DbnBridge::convert_or_skip(&dbn_msg);

        assert!(mbo_msg.is_some());
        let msg = mbo_msg.unwrap();
        assert_eq!(msg.order_id, 12345);
    }

    #[test]
    fn test_convert_or_skip_invalid() {
        let mut dbn_msg = create_test_dbn_msg();
        dbn_msg.action = b'X' as i8; // Invalid action

        let mbo_msg = DbnBridge::convert_or_skip(&dbn_msg);
        assert!(mbo_msg.is_none());
    }

    #[test]
    fn test_convert_batch() {
        let mut msg1 = create_test_dbn_msg();
        msg1.order_id = 1;

        let mut msg2 = create_test_dbn_msg();
        msg2.order_id = 2;
        msg2.action = b'M' as i8;

        let msgs = vec![msg1, msg2];
        let result = DbnBridge::convert_batch(&msgs).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].order_id, 1);
        assert_eq!(result[1].order_id, 2);
        assert_eq!(result[1].action, Action::Modify);
    }

    #[test]
    fn test_convert_batch_or_skip() {
        let mut msg1 = create_test_dbn_msg();
        msg1.order_id = 1;

        let mut msg2 = create_test_dbn_msg();
        msg2.order_id = 2;
        msg2.action = b'X' as i8; // Invalid

        let mut msg3 = create_test_dbn_msg();
        msg3.order_id = 3;

        let msgs = vec![msg1, msg2, msg3];
        let result = DbnBridge::convert_batch_or_skip(&msgs);

        // Should skip the invalid message
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].order_id, 1);
        assert_eq!(result[1].order_id, 3);
    }

    #[test]
    fn test_convert_rejects_zero_timestamp() {
        // Phase M M.A.6 (REV 3 F-023 closure): ts_event == 0 is the Databento
        // sentinel for "no timestamp" on session-control / metadata messages.
        // Pre-M.A.6 this silently coerced to `Some(0)`. Post-M.A.6 it must
        // fail-loud as `TlobError::InvalidTimestamp(0)`.
        let dbn_msg = dbn::MboMsg {
            hd: dbn::RecordHeader::new::<dbn::MboMsg>(0, 0, 0, 0), // ts_event = 0
            order_id: 12345,
            price: 100_000_000_000,
            size: 100,
            flags: dbn::FlagSet::empty(),
            channel_id: 0,
            action: b'A' as i8,
            side: b'B' as i8,
            ts_recv: 0,
            ts_in_delta: 0,
            sequence: 0,
        };

        let result = DbnBridge::convert(&dbn_msg);
        assert!(
            matches!(result, Err(TlobError::InvalidTimestamp(0))),
            "ts_event == 0 must fail-loud per F-023; got: {result:?}"
        );
    }

    #[test]
    fn test_convert_rejects_overflow_timestamp() {
        // Phase M M.A.6 (REV 3 F-023 closure): u64 ts_event > i64::MAX wraps
        // negative on `as i64` cast — silent precision loss per hft-rules §2.
        // Post-M.A.6 the negative-cast result is rejected as InvalidTimestamp.
        let overflow_value = (i64::MAX as u64) + 1; // First u64 that wraps to negative
        let dbn_msg = dbn::MboMsg {
            hd: dbn::RecordHeader::new::<dbn::MboMsg>(0, 0, 0, overflow_value),
            order_id: 12345,
            price: 100_000_000_000,
            size: 100,
            flags: dbn::FlagSet::empty(),
            channel_id: 0,
            action: b'A' as i8,
            side: b'B' as i8,
            ts_recv: 0,
            ts_in_delta: 0,
            sequence: 0,
        };

        let result = DbnBridge::convert(&dbn_msg);
        assert!(
            matches!(result, Err(TlobError::InvalidTimestamp(t)) if t < 0),
            "u64 ts_event overflow must fail-loud per F-023; got: {result:?}"
        );
    }

    #[test]
    fn test_convert_accepts_system_message_with_zero_timestamp() {
        // Phase M M.A.9 (post-validation F-010 ↔ F-023 cross-cascade fix):
        // Databento omits `ts_event` (sentinel 0) on records that do not name
        // a resting order. Pre-M.A.9, M.A.6 F-023 rejected ALL ts_event=0 as
        // `InvalidTimestamp`, which silently shadowed the M.A.7 F-010
        // `system_messages_seen` counter at the typed iterator (these messages
        // flowed through `BoundaryError::Convert` and inflated
        // `rows_skipped_decode_or_convert` instead of counting as expected
        // non-order events). Post-M.A.9 they convert cleanly with
        // `timestamp: None` so the counter fires.
        //
        // ⚠ FIXTURE CHANGED WITH D-4 (`b'A'` -> `b'T'`), and the change is the
        // point. The old fixture was an ADD carrying `order_id == 0` — i.e. a
        // record that does not exist on any tape censused, constructed purely
        // to satisfy the field-shape predicate this commit deletes. The real
        // population it stands for is the aggressing-order TRADE PRINT: on
        // XNAS 2025-07-01, 375,643/375,643 `T` records carry `order_id == 0`,
        // and they are 100% of the `is_system_message()` mass apart from the
        // single daily `R`. The record is now what it always described.
        let dbn_msg = dbn::MboMsg {
            hd: dbn::RecordHeader::new::<dbn::MboMsg>(0, 0, 0, 0), // ts_event = 0
            order_id: 0, // the vendor's trade print carries no order id
            price: 100_000_000_000,
            size: 100,
            flags: dbn::FlagSet::empty(),
            channel_id: 0,
            action: b'T' as i8, // aggressing-order TRADE PRINT
            side: b'B' as i8,
            ts_recv: 0,
            ts_in_delta: 0,
            sequence: 0,
        };

        let mbo_msg = DbnBridge::convert(&dbn_msg)
            .expect("trade print with ts_event=0 must convert cleanly per M.A.9");
        assert_eq!(mbo_msg.action, Action::TradeAggregate);
        // timestamp must be None — Databento sentinel preserved as no-timestamp.
        assert_eq!(
            mbo_msg.timestamp, None,
            "trade print with ts_event=0 must yield timestamp=None"
        );
        // Resulting MboMessage MUST self-classify as a system message via
        // is_system_message(), so the typed iterator's F-010 counter
        // increments for it. (That predicate is deleted by the L-ADMIT layer;
        // this assertion moves to the action-keyed successor at that commit.)
        assert!(
            mbo_msg.is_system_message(),
            "converted message must self-identify as system message for F-010 counter to fire"
        );
    }

    /// D-4 lock: the ts_event==0 discriminator is the decoded ACTION, not the field shape.
    ///
    /// The deleted predicate (`order_id == 0 || size == 0 || price <= 0`) agreed with the action
    /// test on XNAS.ITCH — 100% of `T` there carries `order_id == 0` — which is exactly why the
    /// substitution survived review. It does NOT agree on ARCX, where 88,024 `T|N` prints a day
    /// carry `order_id != 0`. This test pins the action-keyed behaviour on both sides.
    #[test]
    fn test_zero_timestamp_dispatch_is_action_keyed_not_field_shaped() {
        let zero_ts = |action: u8, order_id: u64| dbn::MboMsg {
            hd: dbn::RecordHeader::new::<dbn::MboMsg>(0, 0, 0, 0), // ts_event = 0
            order_id,
            price: 100_000_000_000, // > 0  — passes the old field-shape test
            size: 100,              // > 0  — passes the old field-shape test
            flags: dbn::FlagSet::empty(),
            channel_id: 0,
            action: action as i8,
            side: b'N' as i8,
            ts_recv: 0,
            ts_in_delta: 0,
            sequence: 0,
        };

        // An ARCX-shaped trade print: `order_id != 0`, so the DELETED field-shape predicate
        // would have called it "not a system message" and rejected it as corrupt. Action-keyed,
        // it is admitted with no timestamp — the correct treatment for a vendor trade print.
        let arcx_trade = DbnBridge::convert(&zero_ts(b'T', 88_024))
            .expect("a trade print with order_id != 0 (ARCX shape) must still be admitted");
        assert_eq!(arcx_trade.action, Action::TradeAggregate);
        assert_eq!(arcx_trade.timestamp, None);

        // A Fill names a specific resting order. A missing timestamp on it is corruption,
        // whatever its field shape — including `order_id == 0`, which the deleted predicate
        // would have waved through as a "heartbeat".
        assert!(
            matches!(
                DbnBridge::convert(&zero_ts(b'F', 0)),
                Err(TlobError::InvalidTimestamp(0))
            ),
            "a Fill with ts_event == 0 must fail loud even when order_id == 0"
        );
        assert!(matches!(
            DbnBridge::convert(&zero_ts(b'F', 12345)),
            Err(TlobError::InvalidTimestamp(0))
        ));

        // Clear is the session-boundary reset and legitimately carries no timestamp.
        let clear = DbnBridge::convert(&zero_ts(b'R', 0)).expect("Clear must be admitted");
        assert_eq!(clear.action, Action::Clear);
        assert_eq!(clear.timestamp, None);

        // Add/Modify/Cancel all name a resting order: fail loud.
        for action in [b'A', b'M', b'C'] {
            assert!(
                matches!(
                    DbnBridge::convert(&zero_ts(action, 0)),
                    Err(TlobError::InvalidTimestamp(0))
                ),
                "order-bearing action {action} with ts_event == 0 must fail loud"
            );
        }
    }

    #[test]
    fn test_convert_rejects_zero_timestamp_for_non_system_message() {
        // Phase M M.A.9 (post-validation F-010 ↔ F-023 cross-cascade fix):
        // The post-M.A.9 policy reserves `InvalidTimestamp(0)` for the
        // genuine corruption case — order data (non-system-message) with
        // ts_event=0. The earlier `test_convert_rejects_zero_timestamp`
        // exercises this same surface (order_id=12345, non-system); this
        // test makes the policy rationale explicit.
        let dbn_msg = dbn::MboMsg {
            hd: dbn::RecordHeader::new::<dbn::MboMsg>(0, 0, 0, 0), // ts_event = 0
            order_id: 99999,                                       // NOT a system message
            price: 100_000_000_000,
            size: 100,
            flags: dbn::FlagSet::empty(),
            channel_id: 0,
            action: b'A' as i8,
            side: b'B' as i8,
            ts_recv: 0,
            ts_in_delta: 0,
            sequence: 0,
        };

        let result = DbnBridge::convert(&dbn_msg);
        assert!(
            matches!(result, Err(TlobError::InvalidTimestamp(0))),
            "non-system-message with ts_event=0 must fail-loud per F-023; got: {result:?}"
        );
    }

    #[test]
    fn test_convert_accepts_minimum_valid_timestamp() {
        // Boundary check: ts_event == 1 is the minimum valid (non-sentinel)
        // value. Both this and i64::MAX should round-trip cleanly.
        let dbn_msg = dbn::MboMsg {
            hd: dbn::RecordHeader::new::<dbn::MboMsg>(0, 0, 0, 1), // ts_event = 1
            order_id: 12345,
            price: 100_000_000_000,
            size: 100,
            flags: dbn::FlagSet::empty(),
            channel_id: 0,
            action: b'A' as i8,
            side: b'B' as i8,
            ts_recv: 0,
            ts_in_delta: 0,
            sequence: 0,
        };

        let mbo_msg = DbnBridge::convert(&dbn_msg).expect("ts_event=1 must convert cleanly");
        assert_eq!(mbo_msg.timestamp, Some(1));
    }
}
