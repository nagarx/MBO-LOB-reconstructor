//! Lossless projection from pinned DBN MBO records into the canonical event contract.
//!
//! This module performs no action, side, flag, sentinel, clock, or book
//! interpretation. Validation and disposition remain owned by
//! `hft-mbo-event-contract`; reconstruction consumes only its typed outputs.

use dbn::{MboMsg, Record};
use hft_mbo_event_contract::{RawMboEventV1, Sha256DigestV1};
use std::num::NonZeroU64;
use thiserror::Error;

/// Stateless one-to-one projection of a successfully decoded DBN MBO record.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct CanonicalDbnBridgeV1;

impl CanonicalDbnBridgeV1 {
    /// Preserve every public DBN MBO scalar plus stable source-row identity.
    pub fn project(
        message: &MboMsg,
        source_object_sha256: Sha256DigestV1,
        raw_ordinal: NonZeroU64,
    ) -> Result<RawMboEventV1, CanonicalProjectionErrorV1> {
        let record_size_bytes = u16::try_from(message.record_size())
            .map_err(|_| CanonicalProjectionErrorV1::RecordSizeOverflow(message.record_size()))?;

        Ok(RawMboEventV1 {
            source_object_sha256,
            raw_ordinal: raw_ordinal.get(),
            subordinal: 0,
            rtype: message.hd.rtype,
            record_size_bytes,
            publisher_id: message.hd.publisher_id,
            instrument_id: message.hd.instrument_id,
            ts_event: message.hd.ts_event,
            ts_recv: message.ts_recv,
            ts_in_delta: message.ts_in_delta,
            channel_id: message.channel_id,
            sequence: message.sequence,
            order_id: message.order_id,
            price_raw: message.price,
            size_raw: message.size,
            flags_raw: message.flags.raw(),
            action_raw: message.action as u8,
            side_raw: message.side as u8,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbn::{flags, RecordHeader};

    #[test]
    fn projection_preserves_every_dbn_mbo_field_without_interpretation() {
        let source = Sha256DigestV1::from_bytes([0x5A; 32]);
        let msg = MboMsg {
            hd: RecordHeader::new::<MboMsg>(0xA0, 2, 0x1020_3040, 0x0102_0304_0506_0708),
            order_id: 0x1112_1314_1516_1718,
            price: -0x0102_0304_0506_0708,
            size: 0x2122_2324,
            flags: flags::LAST
                .wrapping_add(flags::SNAPSHOT)
                .wrapping_add(flags::PUBLISHER_SPECIFIC)
                .into(),
            channel_id: 0x31,
            action: b'F' as _,
            side: b'A' as _,
            ts_recv: 0x4142_4344_4546_4748,
            ts_in_delta: -0x0102_0304,
            sequence: 0x5152_5354,
        };

        let ordinal = NonZeroU64::new(0x6162_6364_6566_6768).unwrap();
        let event = CanonicalDbnBridgeV1::project(&msg, source, ordinal)
            .expect("MboMsg has a representable DBN record size");

        assert_eq!(event.source_object_sha256, source);
        assert_eq!(event.raw_ordinal, 0x6162_6364_6566_6768);
        assert_eq!(event.subordinal, 0);
        assert_eq!(event.rtype, 0xA0);
        assert_eq!(event.record_size_bytes, 56);
        assert_eq!(event.publisher_id, 2);
        assert_eq!(event.instrument_id, 0x1020_3040);
        assert_eq!(event.ts_event, 0x0102_0304_0506_0708);
        assert_eq!(event.ts_recv, 0x4142_4344_4546_4748);
        assert_eq!(event.ts_in_delta, -0x0102_0304);
        assert_eq!(event.channel_id, 0x31);
        assert_eq!(event.sequence, 0x5152_5354);
        assert_eq!(event.order_id, 0x1112_1314_1516_1718);
        assert_eq!(event.price_raw, -0x0102_0304_0506_0708);
        assert_eq!(event.size_raw, 0x2122_2324);
        assert_eq!(
            event.flags_raw,
            flags::LAST + flags::SNAPSHOT + flags::PUBLISHER_SPECIFIC
        );
        assert_eq!(event.action_raw, b'F');
        assert_eq!(event.side_raw, b'A');
    }

    #[test]
    fn projection_preserves_true_sentinels_and_all_flag_bits() {
        let source = Sha256DigestV1::from_bytes([0xA5; 32]);
        for flag_byte in u8::MIN..=u8::MAX {
            let msg = MboMsg {
                hd: RecordHeader::new::<MboMsg>(0xA0, 1, 2, u64::MAX),
                order_id: 0,
                price: i64::MAX,
                size: u32::MAX,
                flags: flag_byte.into(),
                channel_id: u8::MAX,
                action: b'N' as _,
                side: b'N' as _,
                ts_recv: u64::MAX,
                ts_in_delta: i32::MIN,
                sequence: u32::MAX,
            };
            let ordinal = NonZeroU64::new(u64::from(flag_byte) + 1).unwrap();
            let event = CanonicalDbnBridgeV1::project(&msg, source, ordinal).unwrap();
            assert_eq!(event.flags_raw, flag_byte);
            assert_eq!(event.price_raw, i64::MAX);
            assert_eq!(event.size_raw, u32::MAX);
            assert_eq!(event.ts_event, u64::MAX);
            assert_eq!(event.ts_recv, u64::MAX);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CanonicalProjectionErrorV1 {
    #[error("decoded DBN record size {0} cannot be represented as canonical u16")]
    RecordSizeOverflow(usize),
}
