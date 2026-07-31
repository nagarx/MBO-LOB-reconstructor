use std::collections::BTreeSet;

use crate::{
    LobState, MboCausalInvalidationScopeV1, MboIngestDispositionV1, MboIngestOutcomeV1,
    Mbp10CompletedEndpointV1, Mbp10LevelV1, PublishedMboBookV1, RawMboRecordV1, RawMbp10RecordV1,
    SourceOrdinal, XnasBoundaryV1, XnasCompletedUpdateEnvelopeV1, XnasDailySourceQualificationV1,
    XnasEndpointMatchKeyV1, XnasIdentityV1, XnasMboStreamV1, XnasMbp10StreamV1, XnasSchemaV1,
    XnasSemanticsError, DBN_FLAG_BAD_TS_RECV, DBN_FLAG_LAST, DBN_FLAG_MAYBE_BAD_BOOK,
    DBN_FLAG_SNAPSHOT, DBN_RTYPE_MBO, DBN_RTYPE_MBP_10, DBN_UNDEF_PRICE, DBN_UNDEF_TIMESTAMP,
    XNAS_ITCH_PUBLISHER_ID,
};

const INSTRUMENT: u32 = 11_667;
const OTHER_INSTRUMENT: u32 = 22_001;

/// Keep the low-level semantic assertions concise while exercising the same
/// non-bypassable public transition used by authoritative callers.
trait XnasMboStreamTestExt {
    fn push(&mut self, record: RawMboRecordV1) -> MboIngestOutcomeV1;
}

impl XnasMboStreamTestExt for XnasMboStreamV1 {
    fn push(&mut self, record: RawMboRecordV1) -> MboIngestOutcomeV1 {
        self.push_causally(record)
            .expect("a semantic-only test cannot violate an un-emitted causal prefix")
    }
}

fn ordinal(value: u64) -> SourceOrdinal {
    SourceOrdinal::new(value).unwrap()
}

fn qualification(schema: XnasSchemaV1, identities: &[u32]) -> XnasDailySourceQualificationV1 {
    let identities = identities
        .iter()
        .map(|instrument_id| XnasIdentityV1::new(XNAS_ITCH_PUBLISHER_ID, *instrument_id))
        .collect::<BTreeSet<_>>();
    XnasDailySourceQualificationV1::from_verified_images(
        schema,
        identities,
        "verified-source-image".to_owned(),
        "0".repeat(64),
        "verified-manifest-image".to_owned(),
        "1".repeat(64),
    )
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
fn mbo(
    ord: u64,
    instrument_id: u32,
    sequence: u32,
    ts_event: u64,
    ts_recv: u64,
    action: u8,
    side: u8,
    order_id: u64,
    price: i64,
    size: u32,
    flags: u8,
) -> RawMboRecordV1 {
    RawMboRecordV1 {
        source_ordinal: ordinal(ord),
        rtype: DBN_RTYPE_MBO,
        publisher_id: XNAS_ITCH_PUBLISHER_ID,
        instrument_id,
        ts_event,
        order_id,
        price,
        size,
        flags,
        channel_id: 0,
        action,
        side,
        ts_recv,
        ts_in_delta: 10,
        sequence,
    }
}

fn control(ord: u64, instrument_id: u32) -> RawMboRecordV1 {
    let mut record = mbo(
        ord,
        instrument_id,
        0,
        123,
        456,
        b'R',
        b'N',
        0,
        DBN_UNDEF_PRICE,
        0,
        0x08,
    );
    record.ts_in_delta = 0;
    record
}

fn levels(bid_size: u32) -> [Mbp10LevelV1; 10] {
    std::array::from_fn(|idx| Mbp10LevelV1 {
        bid_px: 100_000_000_000 - idx as i64 * 1_000_000,
        ask_px: 100_010_000_000 + idx as i64 * 1_000_000,
        bid_sz: if idx == 0 { bid_size } else { 10 + idx as u32 },
        ask_sz: 20 + idx as u32,
        bid_ct: 1 + idx as u32,
        ask_ct: 2 + idx as u32,
    })
}

fn mbp(
    ord: u64,
    sequence: u32,
    ts_recv: u64,
    action: u8,
    flags: u8,
    endpoint_levels: [Mbp10LevelV1; 10],
) -> RawMbp10RecordV1 {
    mbp_for(
        ord,
        INSTRUMENT,
        sequence,
        ts_recv,
        action,
        flags,
        endpoint_levels,
    )
}

fn mbp_for(
    ord: u64,
    instrument_id: u32,
    sequence: u32,
    ts_recv: u64,
    action: u8,
    flags: u8,
    endpoint_levels: [Mbp10LevelV1; 10],
) -> RawMbp10RecordV1 {
    RawMbp10RecordV1 {
        source_ordinal: ordinal(ord),
        rtype: DBN_RTYPE_MBP_10,
        publisher_id: XNAS_ITCH_PUBLISHER_ID,
        instrument_id,
        ts_event: ts_recv - 10,
        price: 100_000_000_000,
        size: 7,
        action,
        side: match action {
            b'R' => b'N',
            b'T' => b'A',
            _ => b'B',
        },
        flags,
        depth: 0,
        ts_recv,
        ts_in_delta: 10,
        sequence,
        levels: endpoint_levels,
    }
}

fn expect_publication(disposition: MboIngestDispositionV1) -> PublishedMboBookV1 {
    match disposition {
        MboIngestDispositionV1::Published(value) => *value,
        other => panic!("expected publication, got {other:?}"),
    }
}

fn synthetic_publication(
    effective_available_ns: u64,
    endpoint_ns: u64,
    witness_ordinal: u64,
    bid_px: i64,
    ask_px: i64,
) -> PublishedMboBookV1 {
    let identity = XnasIdentityV1::new(XNAS_ITCH_PUBLISHER_ID, INSTRUMENT);
    let mut endpoint_levels = [Mbp10LevelV1::default(); 10];
    endpoint_levels[0] = Mbp10LevelV1 {
        bid_px,
        ask_px,
        bid_sz: 1,
        ask_sz: 1,
        bid_ct: 1,
        ask_ct: 1,
    };
    PublishedMboBookV1 {
        envelope: XnasCompletedUpdateEnvelopeV1 {
            schema: "xnas_completed_update_envelope_v1".to_owned(),
            identity,
            channel_id: 0,
            ordered_distinct_sequence_vector: vec![u32::try_from(witness_ordinal).unwrap()],
            terminal_sequence: u32::try_from(witness_ordinal).unwrap(),
            records: Vec::new(),
            terminal_source_ordinal: ordinal(witness_ordinal),
            witness_source_ordinal: ordinal(witness_ordinal),
            endpoint_ns,
            witness_ts_recv: effective_available_ns,
            effective_available_ns,
            closure_confirmation_delay_ns: effective_available_ns - endpoint_ns,
            venue_sequence_block_count: 1,
            execution_sequence_block_count: 0,
            execution_carrier_count: 0,
            execution_envelope: false,
            last_execution_price: None,
            execution_price_change_proxy_v1: None,
        },
        state: LobState::new(10),
        levels: endpoint_levels,
    }
}

fn invalidate_mbo_boundary(
    stream: &mut XnasMboStreamV1,
    boundary: XnasBoundaryV1,
) -> XnasSemanticsError {
    stream.invalidate_boundary_causally(boundary)
}

#[cfg(feature = "databento")]
#[test]
fn lossless_dbn_mbo_conversion_preserves_every_scalar_and_fill() {
    let msg = dbn::MboMsg {
        hd: dbn::RecordHeader::new::<dbn::MboMsg>(
            DBN_RTYPE_MBO,
            XNAS_ITCH_PUBLISHER_ID,
            INSTRUMENT,
            u64::MAX - 10,
        ),
        order_id: u64::MAX - 20,
        price: i64::MIN + 20,
        size: u32::MAX - 1,
        flags: dbn::FlagSet::new(0x82),
        channel_id: 9,
        action: b'F' as i8,
        side: b'B' as i8,
        ts_recv: u64::MAX - 30,
        ts_in_delta: -123,
        sequence: u32::MAX - 2,
    };
    let raw = RawMboRecordV1::from_dbn(ordinal(1), &msg);
    assert_eq!(raw.source_ordinal.get(), 1);
    assert_eq!(raw.rtype, msg.hd.rtype);
    assert_eq!(raw.publisher_id, XNAS_ITCH_PUBLISHER_ID);
    assert_eq!(raw.instrument_id, INSTRUMENT);
    assert_eq!(raw.ts_event, u64::MAX - 10);
    assert_eq!(raw.order_id, u64::MAX - 20);
    assert_eq!(raw.price, i64::MIN + 20);
    assert_eq!(raw.size, u32::MAX - 1);
    assert_eq!(raw.flags, 0x82);
    assert_eq!(raw.channel_id, 9);
    assert_eq!(raw.action, b'F');
    assert_ne!(raw.action, b'T');
    assert_eq!(raw.side, b'B');
    assert_eq!(raw.ts_recv, u64::MAX - 30);
    assert_eq!(raw.ts_in_delta, -123);
    assert_eq!(raw.sequence, u32::MAX - 2);
}

#[cfg(feature = "databento")]
#[test]
fn lossless_dbn_mbp_conversion_preserves_all_ten_levels_and_sentinels() {
    let dbn_levels = std::array::from_fn(|idx| dbn::BidAskPair {
        bid_px: if idx == 9 {
            dbn::UNDEF_PRICE
        } else {
            idx as i64
        },
        ask_px: -(idx as i64),
        bid_sz: idx as u32,
        ask_sz: 100 + idx as u32,
        bid_ct: 200 + idx as u32,
        ask_ct: 300 + idx as u32,
    });
    let msg = dbn::Mbp10Msg {
        hd: dbn::RecordHeader::new::<dbn::Mbp10Msg>(
            DBN_RTYPE_MBP_10,
            XNAS_ITCH_PUBLISHER_ID,
            INSTRUMENT,
            100,
        ),
        price: 50,
        size: 60,
        action: b'C' as i8,
        side: b'B' as i8,
        flags: dbn::FlagSet::new(0x82),
        depth: 9,
        ts_recv: 110,
        ts_in_delta: -2,
        sequence: 77,
        levels: dbn_levels,
    };
    let raw = RawMbp10RecordV1::from_dbn(ordinal(1), &msg);
    assert_eq!(raw.levels.len(), 10);
    assert_eq!(raw.levels[0].ask_ct, 300);
    assert_eq!(raw.levels[8].bid_ct, 208);
    assert_eq!(raw.levels[9].bid_px, dbn::UNDEF_PRICE);
    assert_eq!(raw.action, b'C');
    assert_eq!(raw.flags, 0x82);
}

#[cfg(feature = "databento")]
#[test]
fn local_dbn_constants_match_the_pinned_dependency() {
    assert_eq!(DBN_RTYPE_MBO, dbn::rtype::MBO);
    assert_eq!(DBN_RTYPE_MBP_10, dbn::rtype::MBP_10);
    assert_eq!(DBN_FLAG_LAST, dbn::flags::LAST);
    assert_eq!(DBN_FLAG_SNAPSHOT, dbn::flags::SNAPSHOT);
    assert_eq!(DBN_FLAG_BAD_TS_RECV, dbn::flags::BAD_TS_RECV);
    assert_eq!(DBN_FLAG_MAYBE_BAD_BOOK, dbn::flags::MAYBE_BAD_BOOK);
    assert_eq!(DBN_UNDEF_PRICE, dbn::UNDEF_PRICE);
    assert_eq!(DBN_UNDEF_TIMESTAMP, dbn::UNDEF_TIMESTAMP);
}

#[test]
fn stream_rejects_a_qualification_for_the_wrong_schema() {
    let mbp_token = qualification(XnasSchemaV1::Mbp10, &[INSTRUMENT]);
    let mut stream = XnasMboStreamV1::new(mbp_token);
    assert_eq!(
        stream.push(control(1, INSTRUMENT)).unwrap_err(),
        XnasSemanticsError::SourceNotQualified
    );
}

#[test]
fn exact_initial_clear_is_retained_control_only_and_excludes_timestamps() {
    let mut stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    let mut initial = control(1, INSTRUMENT);
    initial.ts_event = u64::MAX;
    initial.ts_recv = u64::MAX;
    let expected = initial.clone();
    let retained = match stream.push(initial).unwrap() {
        MboIngestDispositionV1::InitialClearControl(value) => value,
        other => panic!("expected initial control, got {other:?}"),
    };
    assert_eq!(retained.record, expected);
    assert_eq!(stream.global_watermark(), None);
    assert_eq!(stream.counts().raw_record_count, 1);
    assert_eq!(stream.counts().initial_xnas_clear_control_count, 1);
    assert_eq!(stream.counts().private_book_reset_count, 1);
    assert_eq!(stream.counts().completed_update_envelope_count, 0);
    assert_eq!(stream.counts().published_book_state_count, 0);
}

#[test]
fn later_exact_initial_clear_is_quarantined_and_requires_authoritative_recovery() {
    let mut stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    stream.push(control(1, INSTRUMENT)).unwrap();
    assert_eq!(
        stream.push(control(2, INSTRUMENT)).unwrap_err(),
        XnasSemanticsError::LaterInitialClear
    );
    assert_eq!(stream.counts().raw_record_count, 2);
    assert_eq!(stream.counts().initial_xnas_clear_control_count, 1);
    assert_eq!(stream.counts().quarantined_record_count, 1);
    assert_eq!(
        stream.counts().quarantined_by_reason["LATER_INITIAL_CLEAR"].record_count,
        1
    );
    assert!(stream.population_reconciles());

    assert_eq!(
        stream
            .push(mbo(
                3,
                INSTRUMENT,
                10,
                990,
                1_000,
                b'A',
                b'B',
                1,
                100,
                1,
                DBN_FLAG_LAST,
            ))
            .unwrap_err(),
        XnasSemanticsError::InvalidState
    );
    stream
        .push(mbo(
            4,
            INSTRUMENT,
            20,
            1_090,
            1_100,
            b'R',
            b'N',
            0,
            DBN_UNDEF_PRICE,
            0,
            DBN_FLAG_LAST,
        ))
        .unwrap();
    let recovered = expect_publication(
        stream
            .push(mbo(
                5,
                INSTRUMENT,
                30,
                1_190,
                1_200,
                b'A',
                b'B',
                2,
                99,
                1,
                DBN_FLAG_LAST,
            ))
            .unwrap(),
    );
    assert_eq!(recovered.envelope.records[0].action, b'R');
    assert!(stream.population_reconciles());
}

#[test]
fn initial_control_timestamp_mutation_does_not_change_publication() {
    let mut first = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    let mut second = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    let mut first_control = control(1, INSTRUMENT);
    first_control.ts_event = 0;
    first_control.ts_recv = 0;
    let mut second_control = control(1, INSTRUMENT);
    second_control.ts_event = u64::MAX;
    second_control.ts_recv = u64::MAX;
    first.push(first_control).unwrap();
    second.push(second_control).unwrap();

    let bid = mbo(
        2,
        INSTRUMENT,
        10,
        990,
        1_000,
        b'A',
        b'B',
        1,
        100,
        1,
        DBN_FLAG_LAST,
    );
    let witness = mbo(
        3,
        INSTRUMENT,
        20,
        1_090,
        1_100,
        b'A',
        b'A',
        2,
        110,
        1,
        DBN_FLAG_LAST,
    );
    first.push(bid.clone()).unwrap();
    second.push(bid).unwrap();
    let first_publication = expect_publication(first.push(witness.clone()).unwrap());
    let second_publication = expect_publication(second.push(witness).unwrap());
    assert_eq!(first_publication, second_publication);
    assert_eq!(first.global_watermark(), second.global_watermark());
}

#[test]
fn initial_clear_signature_deviation_and_control_only_eof_fail_closed() {
    let mut bad_stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    let mut bad = control(1, INSTRUMENT);
    bad.flags = 0x88;
    assert_eq!(
        bad_stream.push(bad).unwrap_err(),
        XnasSemanticsError::InitialClearSignatureMismatch
    );

    let mut control_only = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    control_only.push(control(1, INSTRUMENT)).unwrap();
    let report = control_only.finish_report();
    assert_eq!(
        report.terminal_error,
        Some(XnasSemanticsError::InitializationIncompleteAtEof)
    );
    assert_eq!(report.counts.raw_record_count, 1);
    assert_eq!(report.counts.initial_xnas_clear_control_count, 1);
    assert_eq!(report.counts.pending_record_count, 0);
    assert!(report.counts.population_reconciles());
}

#[test]
fn empty_private_book_is_proven_by_first_cancel_anomaly() {
    let mut stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    stream.push(control(1, INSTRUMENT)).unwrap();
    stream
        .push(mbo(
            2, INSTRUMENT, 10, 90, 100, b'C', b'B', 999, 100, 1, 0x80,
        ))
        .unwrap();
    assert_eq!(
        stream
            .push(mbo(
                3, INSTRUMENT, 20, 190, 200, b'A', b'B', 1, 100, 1, 0x80,
            ))
            .unwrap_err(),
        XnasSemanticsError::BookMutationAnomaly
    );
    assert_eq!(stream.counts().completed_update_envelope_count, 0);
    assert_eq!(stream.counts().published_book_state_count, 0);
}

#[test]
fn repeated_last_tfc_is_one_execution_envelope_and_post_cancel_book() {
    let mut stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    stream.push(control(1, INSTRUMENT)).unwrap();
    stream
        .push(mbo(
            2,
            INSTRUMENT,
            100,
            900,
            1_000,
            b'A',
            b'B',
            42,
            100_000_000_000,
            100,
            0x80,
        ))
        .unwrap();

    let first = expect_publication(
        stream
            .push(mbo(
                3,
                INSTRUMENT,
                200,
                1_090,
                1_100,
                b'T',
                b'A',
                0,
                100_000_000_000,
                40,
                0x82,
            ))
            .unwrap(),
    );
    assert_eq!(first.levels[0].bid_sz, 100);
    assert_eq!(first.levels[0].bid_ct, 1);

    stream
        .push(mbo(
            4,
            INSTRUMENT,
            200,
            1_090,
            1_100,
            b'F',
            b'B',
            42,
            100_000_000_000,
            40,
            0x82,
        ))
        .unwrap();
    stream
        .push(mbo(
            5,
            INSTRUMENT,
            200,
            1_090,
            1_100,
            b'C',
            b'B',
            42,
            100_000_000_000,
            40,
            0x82,
        ))
        .unwrap();
    let publication = expect_publication(
        stream
            .push(mbo(
                6,
                INSTRUMENT,
                900,
                1_190,
                1_200,
                b'A',
                b'A',
                99,
                101_000_000_000,
                10,
                0x80,
            ))
            .unwrap(),
    );
    let tfc = &publication.envelope;
    assert_eq!(tfc.records.len(), 3);
    assert_eq!(tfc.venue_sequence_block_count, 1);
    assert_eq!(tfc.execution_sequence_block_count, 1);
    assert_eq!(tfc.execution_carrier_count, 2);
    assert!(tfc.execution_envelope);
    assert_eq!(tfc.last_execution_price, Some(100_000_000_000));
    assert_eq!(tfc.execution_price_change_proxy_v1, Some(0));
    assert_eq!(tfc.ordered_distinct_sequence_vector, vec![200]);
    assert_eq!(tfc.terminal_sequence, 200);
    assert_eq!(tfc.witness_source_ordinal.get(), 6);
    assert_eq!(publication.levels[0].bid_sz, 60);
    assert_eq!(publication.levels[0].bid_ct, 1);
    assert_eq!(stream.counts().completed_update_envelope_count, 2);
    assert_eq!(stream.counts().published_book_state_count, 2);
    assert_eq!(stream.counts().execution_envelope_count, 1);
}

#[test]
fn conventional_nonlast_tf_then_last_cancel_is_one_full_removal_envelope() {
    let mut stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    stream.push(control(1, INSTRUMENT)).unwrap();
    stream
        .push(mbo(
            2,
            INSTRUMENT,
            100,
            900,
            1_000,
            b'A',
            b'B',
            42,
            100_000_000_000,
            100,
            DBN_FLAG_LAST,
        ))
        .unwrap();
    stream
        .push(mbo(
            3,
            INSTRUMENT,
            200,
            1_090,
            1_100,
            b'A',
            b'A',
            99,
            101_000_000_000,
            10,
            DBN_FLAG_LAST,
        ))
        .unwrap();
    stream
        .push(mbo(
            4,
            INSTRUMENT,
            300,
            1_190,
            1_200,
            b'T',
            b'A',
            0,
            100_000_000_000,
            40,
            0,
        ))
        .unwrap();
    stream
        .push(mbo(
            5,
            INSTRUMENT,
            300,
            1_190,
            1_200,
            b'F',
            b'B',
            42,
            100_000_000_000,
            40,
            0,
        ))
        .unwrap();
    stream
        .push(mbo(
            6,
            INSTRUMENT,
            300,
            1_190,
            1_200,
            b'C',
            b'B',
            42,
            100_000_000_000,
            100,
            DBN_FLAG_LAST,
        ))
        .unwrap();
    let publication = expect_publication(
        stream
            .push(mbo(
                7,
                INSTRUMENT,
                400,
                1_290,
                1_300,
                b'A',
                b'B',
                77,
                99_000_000_000,
                1,
                DBN_FLAG_LAST,
            ))
            .unwrap(),
    );
    let envelope = &publication.envelope;
    assert_eq!(
        envelope
            .records
            .iter()
            .map(|record| record.action)
            .collect::<Vec<_>>(),
        vec![b'T', b'F', b'C']
    );
    assert_eq!(envelope.execution_sequence_block_count, 1);
    assert_eq!(envelope.execution_carrier_count, 2);
    assert!(envelope.execution_envelope);
    assert_eq!(publication.levels[0].bid_px, DBN_UNDEF_PRICE);
    assert_eq!(publication.levels[0].bid_sz, 0);
    assert_eq!(publication.levels[0].bid_ct, 0);
    assert_eq!(publication.levels[0].ask_px, 101_000_000_000);
}

#[test]
fn conventional_nonlast_execution_then_last_cancel_is_exact_for_t_and_f() {
    for execution_action in [b'T', b'F'] {
        let mut stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
        stream.push(control(1, INSTRUMENT)).unwrap();
        stream
            .push(mbo(
                2,
                INSTRUMENT,
                100,
                900,
                1_000,
                b'A',
                b'B',
                42,
                100_000_000_000,
                100,
                DBN_FLAG_LAST,
            ))
            .unwrap();
        stream
            .push(mbo(
                3,
                INSTRUMENT,
                200,
                1_090,
                1_100,
                b'A',
                b'A',
                99,
                101_000_000_000,
                10,
                DBN_FLAG_LAST,
            ))
            .unwrap();

        let execution = mbo(
            4,
            INSTRUMENT,
            300,
            1_190,
            1_200,
            execution_action,
            if execution_action == b'T' { b'A' } else { b'B' },
            if execution_action == b'T' { 0 } else { 42 },
            100_000_000_000,
            40,
            0,
        );
        stream.push(execution.clone()).unwrap();
        stream
            .push(mbo(
                5,
                INSTRUMENT,
                300,
                1_190,
                1_200,
                b'C',
                b'B',
                42,
                100_000_000_000,
                40,
                DBN_FLAG_LAST,
            ))
            .unwrap();
        let publication = expect_publication(
            stream
                .push(mbo(
                    6,
                    INSTRUMENT,
                    400,
                    1_290,
                    1_300,
                    b'A',
                    b'B',
                    77,
                    99_000_000_000,
                    1,
                    DBN_FLAG_LAST,
                ))
                .unwrap(),
        );

        assert_eq!(publication.envelope.records.len(), 2);
        assert_eq!(publication.envelope.records[0], execution);
        assert_eq!(publication.envelope.records[1].action, b'C');
        assert_eq!(publication.envelope.execution_sequence_block_count, 1);
        assert_eq!(publication.envelope.execution_carrier_count, 1);
        assert!(publication.envelope.execution_envelope);
        assert_eq!(publication.levels[0].bid_px, 100_000_000_000);
        assert_eq!(publication.levels[0].bid_sz, 60);
        assert_eq!(publication.levels[0].bid_ct, 1);
        assert_eq!(publication.levels[0].ask_px, 101_000_000_000);
        assert_eq!(stream.counts().pending_record_count, 1);
        assert!(stream.population_reconciles());
    }
}

#[test]
fn witness_payload_is_excluded_then_applied_exactly_once_to_next_envelope() {
    let mut stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    stream.push(control(1, INSTRUMENT)).unwrap();
    stream
        .push(mbo(
            2,
            INSTRUMENT,
            10,
            90,
            100,
            b'A',
            b'B',
            1,
            100,
            1,
            DBN_FLAG_LAST,
        ))
        .unwrap();
    let prior = expect_publication(
        stream
            .push(mbo(
                3,
                INSTRUMENT,
                20,
                190,
                200,
                b'A',
                b'A',
                2,
                102,
                1,
                DBN_FLAG_LAST,
            ))
            .unwrap(),
    );
    assert_eq!(
        prior
            .envelope
            .records
            .iter()
            .map(|record| record.source_ordinal.get())
            .collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(prior.envelope.witness_source_ordinal.get(), 3);
    assert_eq!(prior.levels[0].bid_px, 100);
    assert_eq!(prior.levels[0].ask_px, DBN_UNDEF_PRICE);

    let next = expect_publication(
        stream
            .push(mbo(
                4,
                INSTRUMENT,
                30,
                290,
                300,
                b'A',
                b'B',
                3,
                99,
                1,
                DBN_FLAG_LAST,
            ))
            .unwrap(),
    );
    assert_eq!(
        next.envelope
            .records
            .iter()
            .map(|record| record.source_ordinal.get())
            .collect::<Vec<_>>(),
        vec![3]
    );
    assert_eq!(next.envelope.witness_source_ordinal.get(), 4);
    assert_eq!(next.levels[0].bid_px, 100);
    assert_eq!(next.levels[0].ask_px, 102);
}

#[test]
fn modify_removes_old_level_and_installs_exact_new_price_and_size() {
    let mut stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    stream.push(control(1, INSTRUMENT)).unwrap();
    stream
        .push(mbo(
            2,
            INSTRUMENT,
            10,
            990,
            1_000,
            b'A',
            b'B',
            1,
            100,
            10,
            DBN_FLAG_LAST,
        ))
        .unwrap();
    stream
        .push(mbo(
            3,
            INSTRUMENT,
            20,
            1_090,
            1_100,
            b'A',
            b'A',
            2,
            110,
            5,
            DBN_FLAG_LAST,
        ))
        .unwrap();
    stream
        .push(mbo(
            4,
            INSTRUMENT,
            30,
            1_190,
            1_200,
            b'M',
            b'B',
            1,
            101,
            7,
            DBN_FLAG_LAST,
        ))
        .unwrap();
    let publication = expect_publication(
        stream
            .push(mbo(
                5,
                INSTRUMENT,
                40,
                1_290,
                1_300,
                b'A',
                b'B',
                3,
                99,
                1,
                DBN_FLAG_LAST,
            ))
            .unwrap(),
    );

    assert_eq!(publication.envelope.records.len(), 1);
    assert_eq!(publication.envelope.records[0].action, b'M');
    assert_eq!(publication.levels[0].bid_px, 101);
    assert_eq!(publication.levels[0].bid_sz, 7);
    assert_eq!(publication.levels[0].bid_ct, 1);
    assert_eq!(publication.levels[0].ask_px, 110);
    assert!(!publication.levels.iter().any(|level| level.bid_px == 100));
    assert!(stream.population_reconciles());
}

#[test]
fn execution_price_change_proxy_is_completed_envelope_local_and_resets() {
    let mut stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    stream.push(control(1, INSTRUMENT)).unwrap();
    stream
        .push(mbo(2, INSTRUMENT, 10, 90, 100, b'A', b'B', 1, 90, 10, 0x80))
        .unwrap();
    let first_nonexecution = expect_publication(
        stream
            .push(mbo(
                3, INSTRUMENT, 20, 190, 200, b'A', b'A', 2, 110, 10, 0x80,
            ))
            .unwrap(),
    );
    assert_eq!(
        first_nonexecution.envelope.execution_price_change_proxy_v1,
        None
    );

    let second_nonexecution = expect_publication(
        stream
            .push(mbo(
                4, INSTRUMENT, 30, 290, 300, b'T', b'A', 0, 100, 1, 0x80,
            ))
            .unwrap(),
    );
    assert_eq!(
        second_nonexecution.envelope.execution_price_change_proxy_v1,
        None
    );

    let first_execution = expect_publication(
        stream
            .push(mbo(
                5, INSTRUMENT, 40, 390, 400, b'T', b'A', 0, 100, 1, 0x80,
            ))
            .unwrap(),
    );
    assert_eq!(first_execution.envelope.last_execution_price, Some(100));
    assert_eq!(
        first_execution.envelope.execution_price_change_proxy_v1,
        Some(0)
    );

    let same_execution = expect_publication(
        stream
            .push(mbo(
                6, INSTRUMENT, 50, 490, 500, b'T', b'A', 0, 101, 1, 0x80,
            ))
            .unwrap(),
    );
    assert_eq!(same_execution.envelope.last_execution_price, Some(100));
    assert_eq!(
        same_execution.envelope.execution_price_change_proxy_v1,
        Some(0)
    );

    let upward_change = expect_publication(
        stream
            .push(mbo(7, INSTRUMENT, 60, 590, 600, b'F', b'B', 0, 99, 1, 0x80))
            .unwrap(),
    );
    assert_eq!(upward_change.envelope.last_execution_price, Some(101));
    assert_eq!(
        upward_change.envelope.execution_price_change_proxy_v1,
        Some(1)
    );
    stream
        .push(mbo(8, INSTRUMENT, 60, 590, 600, b'T', b'A', 0, 98, 1, 0x80))
        .unwrap();
    let downward_mixed = expect_publication(
        stream
            .push(mbo(9, INSTRUMENT, 70, 690, 700, b'A', b'B', 3, 89, 1, 0x80))
            .unwrap(),
    );
    assert_eq!(downward_mixed.envelope.last_execution_price, Some(98));
    assert_eq!(
        downward_mixed.envelope.execution_price_change_proxy_v1,
        Some(1)
    );

    let nonexecution = expect_publication(
        stream
            .push(mbo(
                10, INSTRUMENT, 80, 790, 800, b'A', b'B', 4, 88, 1, 0x80,
            ))
            .unwrap(),
    );
    assert_eq!(nonexecution.envelope.last_execution_price, None);
    assert_eq!(nonexecution.envelope.execution_price_change_proxy_v1, None);

    stream
        .push(mbo(
            11,
            INSTRUMENT,
            90,
            890,
            900,
            b'R',
            b'N',
            0,
            DBN_UNDEF_PRICE,
            0,
            0x80,
        ))
        .unwrap();
    let reset_envelope = expect_publication(
        stream
            .push(mbo(
                12, INSTRUMENT, 100, 990, 1_000, b'T', b'A', 0, 77, 1, 0x80,
            ))
            .unwrap(),
    );
    assert_eq!(reset_envelope.envelope.last_execution_price, None);
    let first_after_reset = expect_publication(
        stream
            .push(mbo(
                13, INSTRUMENT, 110, 1_090, 1_100, b'A', b'B', 5, 87, 1, 0x80,
            ))
            .unwrap(),
    );
    assert_eq!(first_after_reset.envelope.last_execution_price, Some(77));
    assert_eq!(
        first_after_reset.envelope.execution_price_change_proxy_v1,
        Some(0)
    );
    assert!(stream.population_reconciles());
}

#[test]
fn every_external_boundary_resets_execution_price_change_baseline() {
    for boundary in [
        XnasBoundaryV1::SourceGap,
        XnasBoundaryV1::DecodeGap,
        XnasBoundaryV1::SessionBoundary,
    ] {
        let mut stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
        stream.push(control(1, INSTRUMENT)).unwrap();
        stream
            .push(mbo(
                2,
                INSTRUMENT,
                10,
                90,
                100,
                b'A',
                b'B',
                1,
                90,
                10,
                DBN_FLAG_LAST,
            ))
            .unwrap();
        stream
            .push(mbo(
                3,
                INSTRUMENT,
                20,
                190,
                200,
                b'T',
                b'A',
                0,
                100,
                1,
                DBN_FLAG_LAST,
            ))
            .unwrap();
        let before_boundary = expect_publication(
            stream
                .push(mbo(
                    4,
                    INSTRUMENT,
                    30,
                    290,
                    300,
                    b'A',
                    b'A',
                    2,
                    110,
                    10,
                    DBN_FLAG_LAST,
                ))
                .unwrap(),
        );
        assert_eq!(
            before_boundary.envelope.execution_price_change_proxy_v1,
            Some(0)
        );

        invalidate_mbo_boundary(&mut stream, boundary);
        stream
            .push(mbo(
                5,
                INSTRUMENT,
                40,
                390,
                400,
                b'R',
                b'N',
                0,
                DBN_UNDEF_PRICE,
                0,
                DBN_FLAG_LAST,
            ))
            .unwrap();
        stream
            .push(mbo(
                6,
                INSTRUMENT,
                50,
                490,
                500,
                b'T',
                b'A',
                0,
                100,
                1,
                DBN_FLAG_LAST,
            ))
            .unwrap();
        let after_boundary = expect_publication(
            stream
                .push(mbo(
                    7,
                    INSTRUMENT,
                    60,
                    590,
                    600,
                    b'A',
                    b'B',
                    3,
                    89,
                    1,
                    DBN_FLAG_LAST,
                ))
                .unwrap(),
        );
        assert_eq!(
            after_boundary.envelope.execution_price_change_proxy_v1,
            Some(0)
        );
        assert!(stream.population_reconciles());
    }
}

#[test]
fn undefined_execution_price_fails_closed_before_proxy_state_changes() {
    let mut stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    stream.push(control(1, INSTRUMENT)).unwrap();
    assert_eq!(
        stream
            .push(mbo(
                2,
                INSTRUMENT,
                10,
                90,
                100,
                b'T',
                b'A',
                0,
                DBN_UNDEF_PRICE,
                1,
                0x80,
            ))
            .unwrap_err(),
        XnasSemanticsError::UndefinedExecutionPrice
    );
    assert!(stream.population_reconciles());
    assert_eq!(stream.counts().quarantined_record_count, 1);
    assert_eq!(
        stream.counts().quarantined_by_reason["UNDEFINED_EXECUTION_PRICE"].open_candidate_count,
        0
    );
}

#[test]
fn failed_multi_mutation_envelope_commits_neither_partial_book_nor_counts() {
    let mut stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    stream.push(control(1, INSTRUMENT)).unwrap();
    stream
        .push(mbo(
            2, INSTRUMENT, 10, 100, 100, b'A', b'B', 1, 100, 10, 0x80,
        ))
        .unwrap();
    expect_publication(
        stream
            .push(mbo(3, INSTRUMENT, 20, 200, 200, b'A', b'A', 77, 110, 10, 0))
            .unwrap(),
    );
    stream
        .push(mbo(
            4, INSTRUMENT, 20, 200, 200, b'C', b'B', 999, 100, 1, 0x80,
        ))
        .unwrap();
    assert_eq!(
        stream
            .push(mbo(5, INSTRUMENT, 30, 300, 300, b'A', b'B', 3, 99, 1, 0x80,))
            .unwrap_err(),
        XnasSemanticsError::BookMutationAnomaly
    );
    assert_eq!(stream.counts().completed_update_envelope_count, 1);
    assert_eq!(stream.counts().published_book_state_count, 1);
    assert_eq!(stream.counts().raw_record_count, 5);
    assert_eq!(stream.counts().initial_xnas_clear_control_count, 1);
    assert_eq!(stream.counts().completed_member_record_count, 1);
    assert_eq!(stream.counts().pending_record_count, 0);
    assert_eq!(stream.counts().quarantined_record_count, 3);
    assert_eq!(
        stream.counts().quarantined_by_reason["BOOK_MUTATION_ANOMALY"].open_candidate_count,
        1
    );
    assert_eq!(
        stream.counts().quarantined_by_reason["BOOK_MUTATION_ANOMALY"].record_count,
        3
    );
    assert!(stream.population_reconciles());

    stream
        .push(mbo(
            6,
            INSTRUMENT,
            40,
            400,
            400,
            b'R',
            b'N',
            0,
            DBN_UNDEF_PRICE,
            0,
            0x80,
        ))
        .unwrap();
    let recovered = expect_publication(
        stream
            .push(mbo(7, INSTRUMENT, 50, 500, 500, b'A', b'B', 4, 98, 1, 0x80))
            .unwrap(),
    );
    assert_eq!(recovered.levels[0].bid_px, DBN_UNDEF_PRICE);
    assert_eq!(stream.counts().completed_update_envelope_count, 2);
    assert_eq!(stream.counts().private_book_reset_count, 2);
}

#[test]
fn multisequence_nonterminal_prefix_needs_terminal_block_and_later_witness() {
    let mut stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    stream.push(control(1, INSTRUMENT)).unwrap();
    stream
        .push(mbo(
            2, INSTRUMENT, 10, 100, 1_000, b'A', b'B', 1, 100, 10, 0,
        ))
        .unwrap();
    stream
        .push(mbo(
            3, INSTRUMENT, 50, 101, 1_000, b'M', b'B', 1, 101, 11, 0,
        ))
        .unwrap();
    stream
        .push(mbo(
            4, INSTRUMENT, 90, 102, 1_000, b'C', b'B', 1, 101, 1, 0x80,
        ))
        .unwrap();
    let publication = expect_publication(
        stream
            .push(mbo(
                5, INSTRUMENT, 500, 200, 1_010, b'A', b'A', 2, 200, 5, 0x80,
            ))
            .unwrap(),
    );
    let completed = publication.envelope;
    assert_eq!(completed.ordered_distinct_sequence_vector, vec![10, 50, 90]);
    assert_eq!(completed.venue_sequence_block_count, 3);
    assert_eq!(completed.endpoint_ns, 1_000);
    assert_eq!(completed.witness_ts_recv, 1_010);
    assert_eq!(completed.effective_available_ns, 1_010);
    assert_eq!(completed.closure_confirmation_delay_ns, 10);
}

#[test]
fn cross_identity_record_neither_closes_group_nor_lowers_global_watermark() {
    let mut stream = XnasMboStreamV1::new(qualification(
        XnasSchemaV1::Mbo,
        &[INSTRUMENT, OTHER_INSTRUMENT],
    ));
    stream.push(control(1, INSTRUMENT)).unwrap();
    stream.push(control(2, OTHER_INSTRUMENT)).unwrap();
    stream
        .push(mbo(
            3, INSTRUMENT, 10, 90, 100, b'A', b'B', 1, 100, 10, 0x80,
        ))
        .unwrap();
    stream
        .push(mbo(
            4,
            OTHER_INSTRUMENT,
            20,
            900,
            1_000,
            b'A',
            b'B',
            2,
            90,
            10,
            0x80,
        ))
        .unwrap();
    let publication = expect_publication(
        stream
            .push(mbo(
                5, INSTRUMENT, 500, 190, 200, b'A', b'A', 3, 110, 10, 0x80,
            ))
            .unwrap(),
    );
    assert_eq!(publication.envelope.witness_ts_recv, 200);
    assert_eq!(publication.envelope.effective_available_ns, 1_000);
    assert_eq!(publication.envelope.closure_confirmation_delay_ns, 900);
    assert!(!publication.envelope.is_observable_at(999));
    assert!(publication.envelope.is_observable_at(1_000));
}

#[test]
fn rejected_finite_clocks_still_lift_the_global_mbo_watermark() {
    let mut stream = XnasMboStreamV1::new(qualification(
        XnasSchemaV1::Mbo,
        &[INSTRUMENT, OTHER_INSTRUMENT],
    ));
    stream.push(control(1, INSTRUMENT)).unwrap();
    stream.push(control(2, OTHER_INSTRUMENT)).unwrap();
    stream
        .push(mbo(3, INSTRUMENT, 10, 90, 100, b'A', b'B', 1, 100, 1, 0x80))
        .unwrap();
    assert_eq!(
        stream
            .push(mbo(
                4,
                OTHER_INSTRUMENT,
                20,
                990,
                1_000,
                b'A',
                b'B',
                2,
                90,
                1,
                DBN_FLAG_MAYBE_BAD_BOOK,
            ))
            .unwrap_err(),
        XnasSemanticsError::MaybeBadBook
    );
    let first = expect_publication(
        stream
            .push(mbo(
                5, INSTRUMENT, 30, 190, 200, b'A', b'A', 3, 110, 1, 0x80,
            ))
            .unwrap(),
    );
    assert_eq!(first.envelope.effective_available_ns, 1_000);

    assert_eq!(
        stream
            .push(mbo(
                6,
                OTHER_INSTRUMENT,
                40,
                1_090,
                1_100,
                b'A',
                b'B',
                4,
                89,
                1,
                0x80,
            ))
            .unwrap_err(),
        XnasSemanticsError::InvalidState
    );
    let second = expect_publication(
        stream
            .push(mbo(7, INSTRUMENT, 50, 290, 300, b'A', b'B', 5, 99, 1, 0x80))
            .unwrap(),
    );
    assert_eq!(second.envelope.effective_available_ns, 1_100);
    assert!(stream.population_reconciles());
}

#[test]
fn undefined_or_bad_receive_clocks_do_not_lift_the_mbo_watermark() {
    for (ts_recv, extra_flags, expected_error) in [
        (
            DBN_UNDEF_TIMESTAMP,
            0_u8,
            XnasSemanticsError::UndefinedTsRecv,
        ),
        (
            9_000_u64,
            DBN_FLAG_BAD_TS_RECV,
            XnasSemanticsError::BadTsRecv,
        ),
    ] {
        let mut stream = XnasMboStreamV1::new(qualification(
            XnasSchemaV1::Mbo,
            &[INSTRUMENT, OTHER_INSTRUMENT],
        ));
        stream.push(control(1, INSTRUMENT)).unwrap();
        stream.push(control(2, OTHER_INSTRUMENT)).unwrap();
        stream
            .push(mbo(3, INSTRUMENT, 10, 90, 100, b'A', b'B', 1, 100, 1, 0x80))
            .unwrap();
        assert_eq!(
            stream
                .push(mbo(
                    4,
                    OTHER_INSTRUMENT,
                    20,
                    990,
                    ts_recv,
                    b'A',
                    b'B',
                    2,
                    90,
                    1,
                    extra_flags,
                ))
                .unwrap_err(),
            expected_error
        );
        let publication = expect_publication(
            stream
                .push(mbo(
                    5, INSTRUMENT, 30, 190, 200, b'A', b'A', 3, 110, 1, 0x80,
                ))
                .unwrap(),
        );
        assert_eq!(publication.envelope.effective_available_ns, 200);
        assert!(stream.population_reconciles());
    }
}

#[test]
fn same_key_last_to_nonlast_and_exact_duplicate_fail_closed() {
    let mut last_stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    last_stream.push(control(1, INSTRUMENT)).unwrap();
    last_stream
        .push(mbo(2, INSTRUMENT, 10, 90, 100, b'T', b'A', 0, 100, 1, 0x80))
        .unwrap();
    assert_eq!(
        last_stream
            .push(mbo(3, INSTRUMENT, 10, 90, 100, b'F', b'B', 1, 100, 1, 0,))
            .unwrap_err(),
        XnasSemanticsError::LastToNonLast
    );

    let mut duplicate_stream =
        XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    duplicate_stream.push(control(1, INSTRUMENT)).unwrap();
    let first = mbo(2, INSTRUMENT, 10, 90, 100, b'T', b'A', 0, 100, 1, 0x80);
    duplicate_stream.push(first.clone()).unwrap();
    let mut distinct = first.clone();
    distinct.source_ordinal = ordinal(3);
    distinct.action = b'F';
    distinct.side = b'B';
    distinct.order_id = 7;
    duplicate_stream.push(distinct).unwrap();
    let mut duplicate = first;
    duplicate.source_ordinal = ordinal(4);
    assert_eq!(
        duplicate_stream.push(duplicate).unwrap_err(),
        XnasSemanticsError::ExactDuplicate
    );
}

#[test]
fn eof_tails_after_valid_g1_are_quarantined_without_publication() {
    let mut terminal = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    terminal.push(control(1, INSTRUMENT)).unwrap();
    terminal
        .push(mbo(2, INSTRUMENT, 10, 90, 100, b'A', b'B', 1, 100, 1, 0x80))
        .unwrap();
    expect_publication(
        terminal
            .push(mbo(
                3, INSTRUMENT, 20, 190, 200, b'A', b'A', 2, 110, 1, 0x80,
            ))
            .unwrap(),
    );
    let counts = terminal.finish().unwrap();
    assert_eq!(counts.raw_record_count, 3);
    assert_eq!(counts.initial_xnas_clear_control_count, 1);
    assert_eq!(counts.completed_member_record_count, 1);
    assert_eq!(counts.pending_record_count, 0);
    assert_eq!(counts.quarantined_record_count, 1);
    assert!(counts.population_reconciles());
    assert_eq!(counts.completed_update_envelope_count, 1);
    assert_eq!(
        counts.quarantined_by_reason["TERMINAL_AT_EOF"].record_count,
        1
    );

    let mut open = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    open.push(control(1, INSTRUMENT)).unwrap();
    open.push(mbo(2, INSTRUMENT, 10, 90, 100, b'A', b'B', 1, 100, 1, 0x80))
        .unwrap();
    expect_publication(
        open.push(mbo(3, INSTRUMENT, 20, 190, 200, b'A', b'A', 2, 110, 1, 0))
            .unwrap(),
    );
    let counts = open.finish().unwrap();
    assert_eq!(counts.quarantined_by_reason["OPEN_AT_EOF"].record_count, 1);
}

#[test]
fn missing_expected_identity_fails_at_eof() {
    let mut stream = XnasMboStreamV1::new(qualification(
        XnasSchemaV1::Mbo,
        &[INSTRUMENT, OTHER_INSTRUMENT],
    ));
    stream.push(control(1, INSTRUMENT)).unwrap();
    stream
        .push(mbo(2, INSTRUMENT, 10, 90, 100, b'A', b'B', 1, 100, 1, 0x80))
        .unwrap();
    expect_publication(
        stream
            .push(mbo(
                3, INSTRUMENT, 20, 190, 200, b'A', b'A', 2, 110, 1, 0x80,
            ))
            .unwrap(),
    );
    assert_eq!(
        stream.finish().unwrap_err(),
        XnasSemanticsError::MissingExpectedIdentity
    );
}

#[test]
fn snapshot_invalidates_and_only_witnessed_authoritative_reset_recovers() {
    let mut stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    stream.push(control(1, INSTRUMENT)).unwrap();
    stream
        .push(mbo(2, INSTRUMENT, 10, 90, 100, b'A', b'B', 1, 100, 1, 0x80))
        .unwrap();
    expect_publication(
        stream
            .push(mbo(
                3, INSTRUMENT, 20, 190, 200, b'A', b'A', 2, 110, 1, 0x80,
            ))
            .unwrap(),
    );

    assert_eq!(
        stream
            .push(mbo(
                4,
                INSTRUMENT,
                30,
                290,
                300,
                b'A',
                b'B',
                3,
                99,
                1,
                DBN_FLAG_SNAPSHOT,
            ))
            .unwrap_err(),
        XnasSemanticsError::SnapshotBoundary
    );
    assert_eq!(
        stream
            .push(mbo(5, INSTRUMENT, 40, 390, 400, b'A', b'B', 4, 98, 1, 0x80,))
            .unwrap_err(),
        XnasSemanticsError::InvalidState
    );
    stream
        .push(mbo(
            6,
            INSTRUMENT,
            50,
            490,
            500,
            b'R',
            b'N',
            0,
            DBN_UNDEF_PRICE,
            0,
            0x80,
        ))
        .unwrap();
    let recovered = expect_publication(
        stream
            .push(mbo(7, INSTRUMENT, 60, 590, 600, b'A', b'B', 5, 97, 1, 0x80))
            .unwrap(),
    );
    assert_eq!(recovered.envelope.records.len(), 1);
    assert_eq!(recovered.envelope.records[0].action, b'R');
    assert_eq!(stream.counts().private_book_reset_count, 2);
}

#[test]
fn boundary_before_first_mbo_record_cannot_be_forgotten() {
    let mut stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    invalidate_mbo_boundary(&mut stream, XnasBoundaryV1::SourceGap);
    assert_eq!(
        stream.push(control(1, INSTRUMENT)).unwrap_err(),
        XnasSemanticsError::InvalidState
    );
    let reset = stream
        .push(mbo(
            2,
            INSTRUMENT,
            10,
            90,
            100,
            b'R',
            b'N',
            0,
            DBN_UNDEF_PRICE,
            0,
            0x80,
        ))
        .unwrap();
    assert!(matches!(
        reset,
        MboIngestDispositionV1::AuthoritativeReset(_)
    ));
    let recovered = expect_publication(
        stream
            .push(mbo(
                3, INSTRUMENT, 20, 190, 200, b'A', b'B', 1, 100, 1, 0x80,
            ))
            .unwrap(),
    );
    assert_eq!(recovered.envelope.records[0].action, b'R');
    assert!(stream.population_reconciles());
}

#[test]
fn boundary_materialization_does_not_fake_identity_observation() {
    let mut stream = XnasMboStreamV1::new(qualification(
        XnasSchemaV1::Mbo,
        &[INSTRUMENT, OTHER_INSTRUMENT],
    ));
    invalidate_mbo_boundary(&mut stream, XnasBoundaryV1::DecodeGap);
    stream
        .push(mbo(
            1,
            INSTRUMENT,
            10,
            90,
            100,
            b'R',
            b'N',
            0,
            DBN_UNDEF_PRICE,
            0,
            0x80,
        ))
        .unwrap();
    expect_publication(
        stream
            .push(mbo(
                2, INSTRUMENT, 20, 190, 200, b'A', b'B', 1, 100, 1, 0x80,
            ))
            .unwrap(),
    );
    assert_eq!(
        stream.finish().unwrap_err(),
        XnasSemanticsError::MissingExpectedIdentity
    );
}

#[test]
fn reset_is_never_a_witness_for_the_preceding_candidate() {
    let mut stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    stream.push(control(1, INSTRUMENT)).unwrap();
    stream
        .push(mbo(2, INSTRUMENT, 10, 90, 100, b'A', b'B', 1, 100, 1, 0x80))
        .unwrap();
    assert!(matches!(
        stream
            .push(mbo(
                3,
                INSTRUMENT,
                20,
                190,
                200,
                b'R',
                b'N',
                0,
                DBN_UNDEF_PRICE,
                0,
                0x80,
            ))
            .unwrap(),
        MboIngestDispositionV1::AuthoritativeReset(_)
    ));
    assert_eq!(stream.counts().completed_update_envelope_count, 0);
    let recovered = expect_publication(
        stream
            .push(mbo(4, INSTRUMENT, 30, 290, 300, b'A', b'B', 2, 99, 1, 0x80))
            .unwrap(),
    );
    assert_eq!(recovered.envelope.records[0].action, b'R');
    assert_eq!(
        stream.counts().quarantined_by_reason["RESET_BOUNDARY"].record_count,
        1
    );
}

#[test]
fn reset_and_multi_identity_boundary_transfer_exact_pending_populations() {
    let mut reset = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    reset.push(control(1, INSTRUMENT)).unwrap();
    reset
        .push(mbo(2, INSTRUMENT, 10, 90, 100, b'A', b'B', 1, 100, 2, 0))
        .unwrap();
    reset
        .push(mbo(3, INSTRUMENT, 10, 90, 100, b'M', b'B', 1, 101, 2, 0x80))
        .unwrap();
    reset
        .push(mbo(
            4,
            INSTRUMENT,
            20,
            190,
            200,
            b'R',
            b'N',
            0,
            DBN_UNDEF_PRICE,
            0,
            0x80,
        ))
        .unwrap();
    let counts = reset.counts();
    assert_eq!(counts.raw_record_count, 4);
    assert_eq!(counts.initial_xnas_clear_control_count, 1);
    assert_eq!(counts.pending_record_count, 1);
    assert_eq!(counts.completed_member_record_count, 0);
    assert_eq!(counts.quarantined_record_count, 2);
    let reset_population = &counts.quarantined_by_reason["RESET_BOUNDARY"];
    assert_eq!(reset_population.incident_count, 1);
    assert_eq!(reset_population.open_candidate_count, 1);
    assert_eq!(reset_population.record_count, 2);
    assert!(reset.population_reconciles());

    let mut boundary = XnasMboStreamV1::new(qualification(
        XnasSchemaV1::Mbo,
        &[INSTRUMENT, OTHER_INSTRUMENT],
    ));
    boundary.push(control(1, INSTRUMENT)).unwrap();
    boundary.push(control(2, OTHER_INSTRUMENT)).unwrap();
    boundary
        .push(mbo(3, INSTRUMENT, 10, 90, 100, b'A', b'B', 1, 100, 2, 0))
        .unwrap();
    boundary
        .push(mbo(4, INSTRUMENT, 10, 90, 100, b'M', b'B', 1, 101, 2, 0x80))
        .unwrap();
    boundary
        .push(mbo(
            5,
            OTHER_INSTRUMENT,
            20,
            190,
            200,
            b'A',
            b'B',
            2,
            90,
            3,
            0,
        ))
        .unwrap();
    boundary
        .push(mbo(
            6,
            OTHER_INSTRUMENT,
            20,
            190,
            200,
            b'M',
            b'B',
            2,
            91,
            3,
            0,
        ))
        .unwrap();
    boundary
        .push(mbo(
            7,
            OTHER_INSTRUMENT,
            20,
            190,
            200,
            b'C',
            b'B',
            2,
            91,
            1,
            0x80,
        ))
        .unwrap();
    invalidate_mbo_boundary(&mut boundary, XnasBoundaryV1::DecodeGap);
    let counts = boundary.counts();
    assert_eq!(counts.raw_record_count, 7);
    assert_eq!(counts.initial_xnas_clear_control_count, 2);
    assert_eq!(counts.pending_record_count, 0);
    assert_eq!(counts.quarantined_record_count, 5);
    let boundary_population = &counts.quarantined_by_reason["DECODE_GAP"];
    assert_eq!(boundary_population.incident_count, 1);
    assert_eq!(boundary_population.open_candidate_count, 2);
    assert_eq!(boundary_population.record_count, 5);
    assert!(boundary.population_reconciles());
}

#[test]
fn channel_change_and_receive_time_change_before_terminal_fail_closed() {
    let mut channel = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    channel.push(control(1, INSTRUMENT)).unwrap();
    channel
        .push(mbo(2, INSTRUMENT, 10, 90, 100, b'A', b'B', 1, 100, 1, 0))
        .unwrap();
    let mut changed = mbo(3, INSTRUMENT, 20, 91, 100, b'C', b'B', 1, 100, 1, 0x80);
    changed.channel_id = 1;
    assert_eq!(
        channel.push(changed).unwrap_err(),
        XnasSemanticsError::ChannelChange
    );

    let mut time = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    time.push(control(1, INSTRUMENT)).unwrap();
    time.push(mbo(2, INSTRUMENT, 10, 90, 100, b'A', b'B', 1, 100, 1, 0))
        .unwrap();
    assert_eq!(
        time.push(mbo(3, INSTRUMENT, 20, 91, 101, b'C', b'B', 1, 100, 1, 0x80,))
            .unwrap_err(),
        XnasSemanticsError::ReceiveTimeChangedBeforeTerminal
    );
}

#[test]
fn cutoff_equality_passes_and_one_nanosecond_early_fails() {
    let mut stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    stream.push(control(1, INSTRUMENT)).unwrap();
    stream
        .push(mbo(2, INSTRUMENT, 10, 90, 100, b'A', b'B', 1, 100, 1, 0x80))
        .unwrap();
    let completed = expect_publication(
        stream
            .push(mbo(
                3, INSTRUMENT, 100, 190, 200, b'A', b'A', 2, 110, 1, 0x80,
            ))
            .unwrap(),
    )
    .envelope;
    assert!(completed.is_observable_at(200));
    assert!(!completed.is_observable_at(199));
}

#[test]
fn mbp_repeated_last_endpoint_uses_terminal_cancel_levels_and_exact_key() {
    let mut stream = XnasMbp10StreamV1::new(qualification(XnasSchemaV1::Mbp10, &[INSTRUMENT]));
    stream
        .push(mbp(1, 348_451, 1_000, b'T', 0x82, levels(100)))
        .unwrap();
    stream
        .push(mbp(2, 348_451, 1_000, b'C', 0x82, levels(93)))
        .unwrap();
    let endpoint = stream
        .push(mbp(3, 348_803, 1_100, b'A', 0x82, levels(94)))
        .unwrap()
        .expect("later sequence is the witness");
    assert_eq!(endpoint.levels[0].bid_sz, 93);
    assert_eq!(endpoint.terminal_source_ordinal.get(), 2);
    assert_eq!(endpoint.witness_source_ordinal.get(), 3);
    assert_eq!(endpoint.ordered_distinct_sequence_vector, vec![348_451]);
    assert_eq!(stream.counts().raw_record_count, 3);
    assert_eq!(stream.counts().completed_member_record_count, 2);
    assert_eq!(stream.counts().pending_record_count, 1);
    assert_eq!(stream.counts().quarantined_record_count, 0);
    assert!(stream.population_reconciles());
    let key = XnasEndpointMatchKeyV1::from_mbp("2025-07-03", &endpoint);
    assert_eq!(key.terminal_sequence, 348_451);
    assert_eq!(key.endpoint_ns, 1_000);
    assert_eq!(key.session, "2025-07-03");
}

#[test]
fn exact_endpoint_match_key_is_identical_across_mbo_and_mbp() {
    let publication = synthetic_publication(1_100_000_000, 1_000_000_000, 348_451, 100, 110);
    let envelope = &publication.envelope;
    let endpoint = Mbp10CompletedEndpointV1 {
        identity: envelope.identity,
        ordered_distinct_sequence_vector: envelope.ordered_distinct_sequence_vector.clone(),
        terminal_sequence: envelope.terminal_sequence,
        terminal_source_ordinal: envelope.terminal_source_ordinal,
        witness_source_ordinal: envelope.witness_source_ordinal,
        endpoint_ns: envelope.endpoint_ns,
        witness_ts_recv: envelope.witness_ts_recv,
        effective_available_ns: envelope.effective_available_ns,
        closure_confirmation_delay_ns: envelope.closure_confirmation_delay_ns,
        levels: publication.levels,
    };
    assert_eq!(
        XnasEndpointMatchKeyV1::from_mbo("2025-07-03", envelope),
        XnasEndpointMatchKeyV1::from_mbp("2025-07-03", &endpoint)
    );
}

#[test]
fn mbp_endpoint_is_last_book_bearing_record_in_terminal_block() {
    let mut stream = XnasMbp10StreamV1::new(qualification(XnasSchemaV1::Mbp10, &[INSTRUMENT]));
    stream
        .push(mbp(1, 10, 1_000, b'A', 0, levels(100)))
        .unwrap();
    stream
        .push(mbp(2, 20, 1_000, b'C', 0x80, levels(90)))
        .unwrap();
    stream
        .push(mbp(3, 20, 1_000, b'T', 0x80, levels(999)))
        .unwrap();
    let endpoint = stream
        .push(mbp(4, 50, 1_100, b'A', 0x80, levels(91)))
        .unwrap()
        .unwrap();
    assert_eq!(endpoint.terminal_source_ordinal.get(), 2);
    assert_eq!(endpoint.levels[0].bid_sz, 90);

    let mut no_book = XnasMbp10StreamV1::new(qualification(XnasSchemaV1::Mbp10, &[INSTRUMENT]));
    no_book
        .push(mbp(1, 10, 1_000, b'A', 0, levels(100)))
        .unwrap();
    no_book
        .push(mbp(2, 20, 1_000, b'T', 0x80, levels(999)))
        .unwrap();
    assert_eq!(
        no_book
            .push(mbp(3, 50, 1_100, b'A', 0x80, levels(91)))
            .unwrap_err(),
        XnasSemanticsError::NoBookBearingTerminalRecord
    );
}

#[test]
fn mbp_snapshot_is_quarantined_and_schema_is_evidence_bound() {
    let mut snapshot = XnasMbp10StreamV1::new(qualification(XnasSchemaV1::Mbp10, &[INSTRUMENT]));
    assert_eq!(
        snapshot
            .push(mbp(1, 10, 1_000, b'A', DBN_FLAG_SNAPSHOT, levels(100),))
            .unwrap_err(),
        XnasSemanticsError::SnapshotBoundary
    );

    let mut wrong = XnasMbp10StreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    assert_eq!(
        wrong
            .push(mbp(1, 10, 1_000, b'A', 0x80, levels(100)))
            .unwrap_err(),
        XnasSemanticsError::SourceNotQualified
    );
}

#[test]
fn first_invalid_mbp_record_persists_until_witnessed_reset() {
    let mut stream = XnasMbp10StreamV1::new(qualification(XnasSchemaV1::Mbp10, &[INSTRUMENT]));
    assert_eq!(
        stream
            .push(mbp(1, 10, 1_000, b'A', DBN_FLAG_SNAPSHOT, levels(100),))
            .unwrap_err(),
        XnasSemanticsError::SnapshotBoundary
    );
    assert_eq!(
        stream
            .push(mbp(2, 20, 1_100, b'A', 0x80, levels(101)))
            .unwrap_err(),
        XnasSemanticsError::InvalidState
    );
    stream
        .push(mbp(3, 30, 1_200, b'R', 0x80, levels(0)))
        .unwrap();
    let endpoint = stream
        .push(mbp(4, 40, 1_300, b'A', 0x80, levels(102)))
        .unwrap()
        .expect("clean witness completes reset recovery");
    assert_eq!(endpoint.terminal_source_ordinal.get(), 3);
    assert_eq!(endpoint.witness_source_ordinal.get(), 4);
    assert_eq!(endpoint.levels, levels(0));
    assert!(stream.population_reconciles());
}

#[test]
fn unwitnessed_mbp_reset_remains_recovering_at_eof() {
    let mut stream = XnasMbp10StreamV1::new(qualification(XnasSchemaV1::Mbp10, &[INSTRUMENT]));
    stream
        .push(mbp(1, 10, 1_000, b'R', 0x80, levels(0)))
        .unwrap();
    let report = stream.finish_report();
    assert_eq!(
        report.terminal_error,
        Some(XnasSemanticsError::InitializationIncompleteAtEof)
    );
    assert_eq!(report.counts.raw_record_count, 1);
    assert_eq!(report.counts.pending_record_count, 0);
    assert_eq!(report.counts.quarantined_record_count, 1);
    assert!(report.counts.population_reconciles());
}

#[test]
fn rejected_finite_clocks_lift_the_global_mbp_watermark() {
    let mut stream = XnasMbp10StreamV1::new(qualification(
        XnasSchemaV1::Mbp10,
        &[INSTRUMENT, OTHER_INSTRUMENT],
    ));
    stream
        .push(mbp_for(1, INSTRUMENT, 10, 100, b'A', 0x80, levels(100)))
        .unwrap();
    assert_eq!(
        stream
            .push(mbp_for(
                2,
                OTHER_INSTRUMENT,
                20,
                1_000,
                b'A',
                DBN_FLAG_MAYBE_BAD_BOOK,
                levels(90),
            ))
            .unwrap_err(),
        XnasSemanticsError::MaybeBadBook
    );
    let first = stream
        .push(mbp_for(3, INSTRUMENT, 30, 200, b'A', 0x80, levels(101)))
        .unwrap()
        .unwrap();
    assert_eq!(first.effective_available_ns, 1_000);

    assert_eq!(
        stream
            .push(mbp_for(
                4,
                OTHER_INSTRUMENT,
                40,
                1_100,
                b'A',
                0x80,
                levels(91),
            ))
            .unwrap_err(),
        XnasSemanticsError::InvalidState
    );
    let second = stream
        .push(mbp_for(5, INSTRUMENT, 50, 300, b'A', 0x80, levels(102)))
        .unwrap()
        .unwrap();
    assert_eq!(second.effective_available_ns, 1_100);
    assert!(stream.population_reconciles());
}

#[test]
fn boundary_before_first_mbp_record_requires_witnessed_reset() {
    let mut stream = XnasMbp10StreamV1::new(qualification(XnasSchemaV1::Mbp10, &[INSTRUMENT]));
    stream.invalidate_boundary(XnasBoundaryV1::SessionBoundary);
    assert_eq!(
        stream
            .push(mbp(1, 10, 1_000, b'A', 0x80, levels(100)))
            .unwrap_err(),
        XnasSemanticsError::InvalidState
    );
    stream
        .push(mbp(2, 20, 1_100, b'R', 0x80, levels(0)))
        .unwrap();
    assert!(stream
        .push(mbp(3, 30, 1_200, b'A', 0x80, levels(101)))
        .unwrap()
        .is_some());
    assert!(stream.population_reconciles());
}

#[test]
fn ordinal_mismatch_record_is_admitted_and_quarantined_exactly_once() {
    let mut mbo_stream = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    mbo_stream.push(control(1, INSTRUMENT)).unwrap();
    assert_eq!(
        mbo_stream
            .push(mbo(
                3, INSTRUMENT, 10, 990, 1_000, b'A', b'B', 1, 100, 1, 0x80,
            ))
            .unwrap_err(),
        XnasSemanticsError::SourceOrdinalMismatch {
            expected: 2,
            observed: 3,
        }
    );
    assert!(mbo_stream.population_reconciles());
    assert_eq!(mbo_stream.counts().raw_record_count, 2);
    assert_eq!(mbo_stream.counts().initial_xnas_clear_control_count, 1);
    assert_eq!(mbo_stream.counts().quarantined_record_count, 1);
    assert_eq!(
        mbo_stream.counts().quarantined_by_reason["SOURCE_ORDINAL_MISMATCH"].record_count,
        1
    );

    let mut mbp_stream = XnasMbp10StreamV1::new(qualification(XnasSchemaV1::Mbp10, &[INSTRUMENT]));
    assert_eq!(
        mbp_stream
            .push(mbp(2, 10, 1_000, b'A', 0x80, levels(100)))
            .unwrap_err(),
        XnasSemanticsError::SourceOrdinalMismatch {
            expected: 1,
            observed: 2,
        }
    );
    assert!(mbp_stream.population_reconciles());
    assert_eq!(mbp_stream.counts().raw_record_count, 1);
    assert_eq!(mbp_stream.counts().quarantined_record_count, 1);
    assert_eq!(
        mbp_stream.counts().quarantined_by_reason["SOURCE_ORDINAL_MISMATCH"].record_count,
        1
    );
}

#[test]
fn prewatermark_initialization_and_rtype_failures_are_globally_terminal() {
    let mut mbo_stream = XnasMboStreamV1::new(qualification(
        XnasSchemaV1::Mbo,
        &[INSTRUMENT, OTHER_INSTRUMENT],
    ));
    mbo_stream.push(control(1, INSTRUMENT)).unwrap();
    mbo_stream
        .push(mbo(2, INSTRUMENT, 10, 90, 100, b'A', b'B', 1, 100, 1, 0x80))
        .unwrap();
    let initialization_failure = mbo_stream.push(mbo(
        3,
        OTHER_INSTRUMENT,
        20,
        990,
        1_000,
        b'A',
        b'B',
        2,
        101,
        1,
        0x80,
    ));
    assert!(matches!(
        &initialization_failure,
        MboIngestOutcomeV1::Rejected(rejection)
            if rejection.error == XnasSemanticsError::InitialClearSignatureMismatch
                && rejection.invalidation_scope == MboCausalInvalidationScopeV1::All
    ));
    assert_eq!(mbo_stream.global_watermark(), Some(100));
    assert_eq!(
        mbo_stream
            .push(mbo(
                4, INSTRUMENT, 30, 190, 200, b'A', b'A', 3, 102, 1, 0x80,
            ))
            .unwrap_err(),
        XnasSemanticsError::InitialClearSignatureMismatch
    );
    assert_eq!(mbo_stream.counts().raw_record_count, 3);
    assert!(mbo_stream.population_reconciles());

    let mut wrong_mbo = XnasMboStreamV1::new(qualification(XnasSchemaV1::Mbo, &[INSTRUMENT]));
    wrong_mbo.push(control(1, INSTRUMENT)).unwrap();
    let mut wrong_mbo_record = mbo(2, INSTRUMENT, 10, 90, 100, b'A', b'B', 1, 100, 1, 0x80);
    wrong_mbo_record.rtype = DBN_RTYPE_MBP_10;
    assert_eq!(
        wrong_mbo.push(wrong_mbo_record).unwrap_err(),
        XnasSemanticsError::WrongRecordType {
            expected: DBN_RTYPE_MBO,
            observed: DBN_RTYPE_MBP_10,
        }
    );
    assert!(wrong_mbo.population_reconciles());

    let mut wrong_mbp = XnasMbp10StreamV1::new(qualification(XnasSchemaV1::Mbp10, &[INSTRUMENT]));
    let mut wrong_mbp_record = mbp(1, 10, 1_000, b'A', 0x80, levels(100));
    wrong_mbp_record.rtype = DBN_RTYPE_MBO;
    assert_eq!(
        wrong_mbp.push(wrong_mbp_record).unwrap_err(),
        XnasSemanticsError::WrongRecordType {
            expected: DBN_RTYPE_MBP_10,
            observed: DBN_RTYPE_MBO,
        }
    );
    assert!(wrong_mbp.population_reconciles());
}
