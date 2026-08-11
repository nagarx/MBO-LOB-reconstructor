//! Bridge between Databento's DBN format and TLOB internal types.
//!
//! This module provides efficient conversion from `dbn::MboMsg` to our internal
//! `MboMessage` type. The conversion is designed to be:
//! - Zero-copy where possible
//! - Type-safe (compile-time guarantees)
//! - Rejects unsupported values explicitly
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

        // DBN timestamps are unsigned nanoseconds. Zero is an ordinary present
        // value, not the undefined sentinel. This legacy message type cannot
        // represent the upper half of u64, so fail before conversion rather
        // than wrapping it into a plausible negative clock.
        let ts_signed = i64::try_from(msg.hd.ts_event)
            .map_err(|_| TlobError::TimestampOutOfRange(msg.hd.ts_event))?;
        let timestamp = Some(ts_signed);

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
    ///
    /// `T` and `F` are deliberately distinct. `T` is an aggregate economic
    /// execution whose side is the aggressor; `F` is resting-order execution
    /// evidence whose side is the resting side. Neither action mutates the
    /// order book. Collapsing them destroys their opposite side conventions
    /// and makes paired `F`/`C` messages double-decrement resting quantity.
    #[inline]
    fn convert_action(action: u8) -> Result<Action> {
        match action {
            b'A' => Ok(Action::Add),
            b'M' => Ok(Action::Modify),
            b'C' => Ok(Action::Cancel),
            b'R' => Ok(Action::Clear),
            b'T' => Ok(Action::TradeAggregate),
            b'F' => Ok(Action::Fill),
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
            b'A' => Ok(Side::Ask),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to create a test MboMsg
    fn create_test_dbn_msg() -> dbn::MboMsg {
        dbn::MboMsg {
            hd: dbn::RecordHeader::new::<dbn::MboMsg>(
                0,                         // rtype
                0,                         // publisher_id
                0,                         // instrument_id
                1_234_567_890_000_000_000, // ts_event
            ),
            order_id: 12345,
            price: 100_000_000_000, // $100.00 in fixed-point
            size: 100,
            flags: dbn::FlagSet::empty(),
            channel_id: 0,
            action: b'A' as i8,
            side: b'B' as i8,
            ts_recv: 1_234_567_890_000_000_000,
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
        assert_eq!(DbnBridge::convert_action(b'F').unwrap(), Action::Fill);
        assert_eq!(DbnBridge::convert_action(b'N').unwrap(), Action::None);

        // Invalid action
        assert!(DbnBridge::convert_action(b'X').is_err());
    }

    #[test]
    fn test_convert_side() {
        assert_eq!(DbnBridge::convert_side(b'B').unwrap(), Side::Bid);
        assert_eq!(DbnBridge::convert_side(b'A').unwrap(), Side::Ask);
        assert!(DbnBridge::convert_side(b'S').is_err());
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
        assert_eq!(mbo_msg.timestamp, Some(1_234_567_890_000_000_000));
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
    fn test_convert_preserves_zero_timestamp() {
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

        let converted = DbnBridge::convert(&dbn_msg).unwrap();
        assert_eq!(converted.timestamp, Some(0));
    }

    #[test]
    fn test_convert_rejects_overflow_timestamp() {
        let overflow_value = (i64::MAX as u64) + 1;
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
            matches!(result, Err(TlobError::TimestampOutOfRange(t)) if t == overflow_value),
            "u64 ts_event overflow must preserve the rejected value; got: {result:?}"
        );
    }

    #[test]
    fn test_aggregate_trade_order_id_zero_is_not_a_system_message() {
        let dbn_msg = dbn::MboMsg {
            hd: dbn::RecordHeader::new::<dbn::MboMsg>(0, 0, 0, 0),
            order_id: 0,
            price: 100_000_000_000,
            size: 100,
            flags: dbn::FlagSet::empty(),
            channel_id: 0,
            action: b'T' as i8,
            side: b'N' as i8,
            ts_recv: 0,
            ts_in_delta: 0,
            sequence: 0,
        };

        let mbo_msg = DbnBridge::convert(&dbn_msg).unwrap();
        assert_eq!(mbo_msg.action, Action::TradeAggregate);
        assert_eq!(mbo_msg.timestamp, Some(0));
        assert!(!mbo_msg.is_noop_control());
        mbo_msg.validate().unwrap();
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
