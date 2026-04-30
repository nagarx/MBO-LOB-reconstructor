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
        //    `InvalidTimestamp`. This branch fires regardless of system-
        //    message status — overflow is genuine corruption.
        //
        // 2. `ts_event == 0` AND message IS a system message (heartbeat /
        //    metadata / session-control with `order_id == 0`, `size == 0`,
        //    or `price <= 0`). Databento uses ts_event=0 as a sentinel for
        //    "no timestamp" on these messages. Pre-M.A.9 the M.A.6 F-023
        //    fix rejected ALL ts_event=0 as `InvalidTimestamp`, which
        //    silently shadowed the M.A.7 F-010 `system_messages_seen`
        //    counter at the typed iterator (system messages with ts_event=0
        //    flowed through the `BoundaryError::Convert` arm and inflated
        //    `rows_skipped_decode_or_convert` instead of counting as
        //    expected heartbeats). Post-M.A.9 we yield these system
        //    messages with `timestamp: None` so they reach the iterator's
        //    is_system_message() check and increment `system_messages_seen`
        //    correctly.
        //
        // 3. `ts_event == 0` AND message is NOT a system message (genuine
        //    real-order-data with no timestamp). Per hft-rules §8 — corrupt
        //    feed; fail-loud as `InvalidTimestamp`. This is the core F-023
        //    fail-loud surface that M.A.6 introduced.
        let ts_signed = msg.hd.ts_event as i64;
        if ts_signed < 0 {
            // Case 1: u64 overflow → always corrupt.
            return Err(TlobError::InvalidTimestamp(ts_signed));
        }
        let timestamp = if msg.hd.ts_event == 0 {
            // Determine system-message status from raw fields (price > 0 is
            // a non-system-message signal). Mirrors `MboMessage::is_system_message`.
            let is_system = msg.order_id == 0 || msg.size == 0 || msg.price <= 0;
            if is_system {
                // Case 2: legitimate Databento sentinel on a heartbeat /
                // metadata message → preserve as `None` so downstream
                // observability (F-010 counter) sees the system message.
                None
            } else {
                // Case 3: genuine corrupt feed — order data with no
                // timestamp violates the F-023 fail-loud surface.
                return Err(TlobError::InvalidTimestamp(0));
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

    /// Convert DBN action character to our Action enum.
    ///
    /// DBN uses single-character codes for actions.
    /// We map them to our internal enum representation.
    #[inline]
    fn convert_action(action: u8) -> Result<Action> {
        match action {
            b'A' => Ok(Action::Add),
            b'M' => Ok(Action::Modify),
            b'C' => Ok(Action::Cancel),
            b'R' => Ok(Action::Clear),
            b'T' | b'F' => Ok(Action::Trade), // 'F' = fill, treat as trade
            b'N' => Ok(Action::None),
            _ => Err(TlobError::InvalidAction(action)),
        }
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
        assert_eq!(DbnBridge::convert_action(b'T').unwrap(), Action::Trade);
        assert_eq!(DbnBridge::convert_action(b'F').unwrap(), Action::Trade);
        assert_eq!(DbnBridge::convert_action(b'N').unwrap(), Action::None);

        // Invalid action
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
        // Databento heartbeat / metadata / session-control messages carry
        // BOTH `order_id == 0` (system-message marker) AND `ts_event == 0`
        // (no-timestamp sentinel). Pre-M.A.9, M.A.6 F-023 rejected ALL
        // ts_event=0 as `InvalidTimestamp`, which silently shadowed the
        // M.A.7 F-010 `system_messages_seen` counter at the typed iterator
        // (these messages flowed through `BoundaryError::Convert` and
        // inflated `rows_skipped_decode_or_convert` instead of counting
        // as expected heartbeats). Post-M.A.9 they convert cleanly with
        // `timestamp: None` so they reach `is_system_message()` correctly.
        let dbn_msg = dbn::MboMsg {
            hd: dbn::RecordHeader::new::<dbn::MboMsg>(0, 0, 0, 0), // ts_event = 0
            order_id: 0,                                           // system-message marker
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

        let mbo_msg = DbnBridge::convert(&dbn_msg)
            .expect("system message with ts_event=0 must convert cleanly per M.A.9");
        // timestamp must be None — Databento sentinel preserved as no-timestamp.
        assert_eq!(
            mbo_msg.timestamp, None,
            "system message with ts_event=0 must yield timestamp=None"
        );
        // Resulting MboMessage MUST self-classify as a system message via
        // is_system_message(), so the typed iterator's F-010 counter
        // increments for it.
        assert!(
            mbo_msg.is_system_message(),
            "converted message must self-identify as system message for F-010 counter to fire"
        );
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
