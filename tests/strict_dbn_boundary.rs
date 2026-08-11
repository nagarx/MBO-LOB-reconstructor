#![cfg(feature = "databento")]

mod support;

use dbn::encode::{DbnEncoder, DynWriter, EncodeRecord};
use dbn::{
    flags, Compression, MappingInterval, MboMsg, Metadata, RecordHeader, SType, Schema,
    SymbolMapping, TradeMsg,
};
use hft_mbo_event_contract::{
    EventDispositionV1, LogicalSourceV1, PublisherPolicyIdV1, Sha256DigestV1, ValidationReasonV1,
    CANONICAL_MBO_EVENT_CONTRACT_SHA256,
};
use mbo_lob_reconstructor::{
    CanonicalSourceExpectationV1, StrictBoundaryErrorV1, StrictDbnLoaderV1,
    VerifiedRejectionStageV1, XnasDailyMetadataExpectationV1, XnasExpectedInstrumentIdentityV1,
};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use tempfile::tempdir;

fn metadata(dataset: &str, schema: Option<Schema>, version: u8, ts_out: bool) -> Metadata {
    const START_NS: u64 = 1_751_328_000_000_000_000;
    const END_NS: u64 = START_NS + 86_400_000_000_000;
    let mut metadata = Metadata::builder()
        .version(version)
        .dataset(dataset)
        .schema(schema)
        .start(START_NS)
        .end(std::num::NonZeroU64::new(END_NS))
        .stype_in(Some(SType::RawSymbol))
        .stype_out(SType::InstrumentId)
        .ts_out(ts_out)
        .build();
    let start_date = metadata.start().date();
    metadata.symbols = vec!["TEST".into()];
    metadata.mappings = vec![SymbolMapping {
        raw_symbol: "TEST".into(),
        intervals: vec![MappingInterval {
            start_date,
            end_date: start_date.next_day().unwrap(),
            symbol: "101".into(),
        }],
    }];
    metadata
}

fn metadata_for_instruments(instruments: &[(&str, u32)]) -> Metadata {
    let mut value = metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false);
    let start_date = value.start().date();
    value.symbols = instruments
        .iter()
        .map(|(symbol, _)| (*symbol).to_owned())
        .collect();
    value.mappings = instruments
        .iter()
        .map(|(symbol, instrument_id)| SymbolMapping {
            raw_symbol: (*symbol).to_owned(),
            intervals: vec![MappingInterval {
                start_date,
                end_date: start_date.next_day().unwrap(),
                symbol: instrument_id.to_string(),
            }],
        })
        .collect();
    value
}

fn message(action: u8, side: u8, order_id: u64, size: u32, sequence: u32) -> MboMsg {
    MboMsg {
        hd: RecordHeader::new::<MboMsg>(0xA0, 2, 101, 1_750_000_000_000_000_000 + sequence as u64),
        order_id,
        price: 123_456_000_000,
        size,
        flags: flags::LAST.into(),
        channel_id: 3,
        action: action as _,
        side: side as _,
        ts_recv: 1_750_000_000_000_100_000 + sequence as u64,
        ts_in_delta: 100_000,
        sequence,
    }
}

fn write_mbo(path: &Path, compression: Compression, metadata: &Metadata, records: &[MboMsg]) {
    let file = File::create(path).unwrap();
    let writer = DynWriter::new(file, compression).unwrap();
    let mut encoder = DbnEncoder::new(writer, metadata).unwrap();
    encoder.encode_records(records).unwrap();
    encoder.flush().unwrap();
}

fn write_trade_record_with_mbo_metadata(path: &Path) {
    let file = File::create(path).unwrap();
    let writer = DynWriter::new(file, Compression::None).unwrap();
    let metadata = metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false);
    let mut encoder = DbnEncoder::new(writer, &metadata).unwrap();
    encoder.encode_record(&TradeMsg::default()).unwrap();
    encoder.flush().unwrap();
}

fn digest_and_len(path: &Path) -> (Sha256DigestV1, u64) {
    let mut file = File::open(path).unwrap();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    (Sha256DigestV1::from_bytes(digest), bytes.len() as u64)
}

fn xnas_open_error(name: &str, actual_metadata: &Metadata) -> StrictBoundaryErrorV1 {
    let dir = tempdir().unwrap();
    let path = dir.path().join(format!("{name}.dbn"));
    write_mbo(&path, Compression::None, actual_metadata, &[]);
    match StrictDbnLoaderV1::open(
        expectation(&path, "XNAS.ITCH", "mbo", 1, false, 0),
        PublisherPolicyIdV1::XnasItchHistorical,
    ) {
        Ok(_) => panic!("{name} metadata unexpectedly qualified"),
        Err(error) => error,
    }
}

fn expectation(
    path: &Path,
    dataset: &str,
    schema: &str,
    version: u8,
    ts_out: bool,
    expected_records: u64,
) -> CanonicalSourceExpectationV1 {
    support::expectation(path, dataset, schema, version, ts_out, expected_records)
}

fn valid_records() -> Vec<MboMsg> {
    vec![
        message(b'A', b'B', 10, 100, 1),
        message(b'F', b'B', 10, 40, 2),
        message(b'T', b'A', 0, 40, 2),
        message(b'C', b'B', 10, 40, 2),
    ]
}

#[test]
fn valid_compressed_and_uncompressed_streams_reconcile_exactly() {
    for compression in [Compression::None, Compression::Zstd] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("valid.dbn");
        write_mbo(
            &path,
            compression,
            &metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false),
            &valid_records(),
        );
        let mut stream = StrictDbnLoaderV1::open(
            expectation(&path, "XNAS.ITCH", "mbo", 1, false, 4),
            PublisherPolicyIdV1::XnasItchHistorical,
        )
        .unwrap();
        let source_binding = stream.xnas_historical_source().unwrap();
        assert_eq!(source_binding.session_date(), "2025-07-01");
        assert_eq!(source_binding.instruments().len(), 1);
        assert!(source_binding.contains_identity(2, 101));
        assert_eq!(source_binding.instruments()[0].symbol, "TEST");

        let mut ordinals = Vec::new();
        let mut book = 0;
        let mut execution = 0;
        for item in stream.by_ref() {
            let item = item.unwrap();
            let disposition = item.accepted().expect("fixture is accepted").disposition();
            ordinals.push(disposition.event().raw().raw_ordinal);
            match disposition {
                EventDispositionV1::Book(_) => book += 1,
                EventDispositionV1::Execution(_) => execution += 1,
                EventDispositionV1::Control(_) => {}
            }
        }
        assert_eq!(ordinals, [1, 2, 3, 4]);
        assert_eq!(book, 2);
        assert_eq!(execution, 2);
        let receipt = stream.finish().unwrap();
        assert_eq!(receipt.expected_records(), 4);
        assert_eq!(receipt.decoded_records(), 4);
        assert_eq!(receipt.bytes_consumed(), fs::metadata(&path).unwrap().len());
        assert_eq!(
            receipt.contract_sha256().to_hex(),
            CANONICAL_MBO_EVENT_CONTRACT_SHA256
        );
        assert_eq!(receipt.publisher_policy_id(), "xnas_itch_historical_v1");
        assert_eq!(
            receipt
                .xnas_historical_source()
                .unwrap()
                .source_object_sha256(),
            receipt.source().logical.compressed_sha256
        );
    }
}

#[test]
fn predeclared_xnas_metadata_is_bound_before_record_replay() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("metadata-bound.dbn");
    write_mbo(
        &path,
        Compression::None,
        &metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false),
        &valid_records(),
    );
    let source = expectation(&path, "XNAS.ITCH", "mbo", 1, false, 4);
    let expected = support::xnas_metadata_expectation(&source, &[(2, 101, "TEST")]);
    let stream = StrictDbnLoaderV1::open_xnas_expected(source, expected).unwrap();
    assert_eq!(stream.decoded_records(), 0);
    let binding = stream.xnas_historical_source().unwrap();
    assert_eq!(binding.session_date(), "2025-07-01");
    assert_eq!(binding.instruments().len(), 1);
    assert_eq!(binding.instruments()[0].publisher_id, 2);
    assert_eq!(binding.instruments()[0].instrument_id, 101);
    assert_eq!(binding.instruments()[0].symbol, "TEST");
}

#[test]
fn expectation_policy_digest_and_bounds_fail_before_source_io() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("pre-io.dbn");
    write_mbo(
        &path,
        Compression::None,
        &metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false),
        &[],
    );
    let source = expectation(&path, "XNAS.ITCH", "mbo", 1, false, 0);
    fs::remove_file(&path).unwrap();

    let wrong_publisher = XnasDailyMetadataExpectationV1::new(
        source.logical().compressed_sha256,
        support::START_NS,
        support::END_NS,
        "2025-07-01",
        vec![XnasExpectedInstrumentIdentityV1::new(3, 101, "TEST").unwrap()],
    )
    .unwrap();
    assert!(matches!(
        StrictDbnLoaderV1::open_xnas_expected(source.clone(), wrong_publisher),
        Err(StrictBoundaryErrorV1::XnasExpectedPublisherMismatch {
            policy_publisher_id: 2
        })
    ));

    let wrong_digest = XnasDailyMetadataExpectationV1::new(
        Sha256DigestV1::from_bytes([0xA5; 32]),
        support::START_NS,
        support::END_NS,
        "2025-07-01",
        vec![XnasExpectedInstrumentIdentityV1::new(2, 101, "TEST").unwrap()],
    )
    .unwrap();
    assert!(matches!(
        StrictDbnLoaderV1::open_xnas_expected(source.clone(), wrong_digest),
        Err(StrictBoundaryErrorV1::XnasExpectedSourceDigestMismatch { .. })
    ));

    let wrong_population = XnasDailyMetadataExpectationV1::new(
        source.logical().compressed_sha256,
        support::START_NS,
        support::END_NS,
        "2025-07-01",
        vec![
            XnasExpectedInstrumentIdentityV1::new(2, 101, "TEST").unwrap(),
            XnasExpectedInstrumentIdentityV1::new(2, 202, "OTHER").unwrap(),
        ],
    )
    .unwrap();
    assert!(matches!(
        StrictDbnLoaderV1::open_xnas_expected(source.clone(), wrong_population),
        Err(StrictBoundaryErrorV1::XnasExpectedCatalogPopulationMismatch { .. })
    ));

    let wrong_singleton_symbol = XnasDailyMetadataExpectationV1::new(
        source.logical().compressed_sha256,
        support::START_NS,
        support::END_NS,
        "2025-07-01",
        vec![XnasExpectedInstrumentIdentityV1::new(2, 101, "OTHER").unwrap()],
    )
    .unwrap();
    assert!(matches!(
        StrictDbnLoaderV1::open_xnas_expected(source.clone(), wrong_singleton_symbol),
        Err(StrictBoundaryErrorV1::XnasExpectedCatalogSingletonSymbolMismatch { .. })
    ));

    let wrong_day = XnasDailyMetadataExpectationV1::new(
        source.logical().compressed_sha256,
        support::START_NS + 86_400_000_000_000,
        support::END_NS + 86_400_000_000_000,
        "2025-07-02",
        vec![XnasExpectedInstrumentIdentityV1::new(2, 101, "TEST").unwrap()],
    )
    .unwrap();
    assert!(matches!(
        StrictDbnLoaderV1::open_xnas_expected(source, wrong_day),
        Err(StrictBoundaryErrorV1::XnasExpectedCatalogBoundsMismatch { .. })
    ));
}

#[test]
fn invalid_session_date_and_duplicate_expected_identities_are_rejected_locally() {
    let instrument = || XnasExpectedInstrumentIdentityV1::new(2, 101, "TEST").unwrap();
    assert!(matches!(
        XnasDailyMetadataExpectationV1::new(
            Sha256DigestV1::from_bytes([0xA5; 32]),
            support::START_NS,
            support::END_NS,
            "2025-07-02",
            vec![instrument()],
        ),
        Err(StrictBoundaryErrorV1::XnasExpectedSessionNotCompleteUtcDay { .. })
    ));
    assert!(matches!(
        XnasDailyMetadataExpectationV1::new(
            Sha256DigestV1::from_bytes([0xA5; 32]),
            support::START_NS,
            support::END_NS,
            "2025-07-01",
            vec![instrument(), instrument()],
        ),
        Err(StrictBoundaryErrorV1::XnasExpectedDuplicateInstrumentIdentity { .. })
    ));
    assert!(matches!(
        XnasDailyMetadataExpectationV1::new(
            Sha256DigestV1::from_bytes([0xA5; 32]),
            support::START_NS,
            support::END_NS,
            "2025-07-01",
            vec![
                XnasExpectedInstrumentIdentityV1::new(2, 101, "TEST").unwrap(),
                XnasExpectedInstrumentIdentityV1::new(2, 202, "TEST").unwrap(),
            ],
        ),
        Err(StrictBoundaryErrorV1::XnasExpectedDuplicateSymbol { .. })
    ));
    assert!(matches!(
        XnasDailyMetadataExpectationV1::new(
            Sha256DigestV1::from_bytes([0; 32]),
            support::START_NS,
            support::END_NS,
            "2025-07-01",
            vec![instrument()],
        ),
        Err(StrictBoundaryErrorV1::XnasExpectedZeroSourceDigest)
    ));
    assert!(matches!(
        XnasDailyMetadataExpectationV1::new(
            Sha256DigestV1::from_bytes([0xA5; 32]),
            support::START_NS,
            support::END_NS,
            "2025-07-01",
            vec![],
        ),
        Err(StrictBoundaryErrorV1::XnasExpectedEmptyInstrumentUniverse)
    ));
    for invalid in [
        XnasExpectedInstrumentIdentityV1::new(0, 101, "TEST"),
        XnasExpectedInstrumentIdentityV1::new(2, 0, "TEST"),
        XnasExpectedInstrumentIdentityV1::new(2, 101, " TEST"),
        XnasExpectedInstrumentIdentityV1::new(2, 101, "TEST\n"),
    ] {
        assert!(matches!(
            invalid,
            Err(StrictBoundaryErrorV1::XnasExpectedInvalidInstrumentIdentity { .. })
        ));
    }
    assert!(matches!(
        XnasDailyMetadataExpectationV1::new(
            Sha256DigestV1::from_bytes([0xA5; 32]),
            u64::MAX - 1,
            u64::MAX,
            "2262-04-11",
            vec![instrument()],
        ),
        Err(StrictBoundaryErrorV1::XnasExpectedSessionNotCompleteUtcDay { .. })
    ));
}

#[test]
fn expected_instrument_universe_is_order_independent_and_exact() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("instrument-universe.dbn");
    write_mbo(
        &path,
        Compression::None,
        &metadata_for_instruments(&[("AAA", 101), ("BBB", 202)]),
        &[],
    );
    let source = support::expectation_with_population(
        &path,
        "XNAS.ITCH",
        "mbo",
        1,
        false,
        0,
        "AAA,BBB",
        2,
        2,
    );

    let reversed = support::xnas_metadata_expectation(&source, &[(2, 202, "BBB"), (2, 101, "AAA")]);
    let stream = StrictDbnLoaderV1::open_xnas_expected(source.clone(), reversed).unwrap();
    assert_eq!(stream.decoded_records(), 0);

    for wrong in [
        vec![(2, 101, "AAA")],
        vec![(2, 101, "AAA"), (2, 202, "BBB"), (2, 303, "CCC")],
        vec![(2, 101, "BBB"), (2, 202, "AAA")],
    ] {
        let expected = support::xnas_metadata_expectation(&source, &wrong);
        assert!(matches!(
            StrictDbnLoaderV1::open_xnas_expected(source.clone(), expected),
            Err(StrictBoundaryErrorV1::XnasExpectedCatalogPopulationMismatch { .. })
                | Err(StrictBoundaryErrorV1::XnasInstrumentUniverseMismatch { .. })
        ));
    }
}

#[test]
fn actual_publisher_is_record_validated_not_claimed_as_metadata_observed() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("record-publisher.dbn");
    let mut wrong_publisher = message(b'A', b'B', 10, 100, 1);
    wrong_publisher.hd.publisher_id = 3;
    write_mbo(
        &path,
        Compression::None,
        &metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false),
        &[wrong_publisher],
    );
    let source = expectation(&path, "XNAS.ITCH", "mbo", 1, false, 1);

    let wrong_symbol = support::xnas_metadata_expectation(&source, &[(2, 101, "WRONG")]);
    assert!(matches!(
        StrictDbnLoaderV1::open_xnas_expected(source.clone(), wrong_symbol),
        Err(StrictBoundaryErrorV1::XnasExpectedCatalogSingletonSymbolMismatch { .. })
    ));

    let expected = support::xnas_metadata_expectation(&source, &[(2, 101, "TEST")]);
    let mut stream = StrictDbnLoaderV1::open_xnas_expected(source, expected).unwrap();
    assert_eq!(stream.decoded_records(), 0);
    assert!(matches!(
        stream.next().unwrap(),
        Err(
            StrictBoundaryErrorV1::RecordIdentityOutsideMetadataAndPolicyBinding {
                raw_ordinal: 1,
                publisher_id: 3,
                instrument_id: 101,
            }
        )
    ));
}

#[test]
fn wrong_source_digest_and_length_fail_before_iteration() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("identity.dbn");
    write_mbo(
        &path,
        Compression::None,
        &metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false),
        &valid_records(),
    );

    let mut wrong_digest = expectation(&path, "XNAS.ITCH", "mbo", 1, false, 4);
    let projection_path = wrong_digest.custody_projection_path().to_path_buf();
    let storage_root = wrong_digest.storage_root_path().to_path_buf();
    wrong_digest = CanonicalSourceExpectationV1::new(
        LogicalSourceV1 {
            compressed_sha256: Sha256DigestV1::from_bytes([0x55; 32]),
            ..wrong_digest.logical().clone()
        },
        projection_path,
        storage_root,
    )
    .unwrap();
    assert!(matches!(
        StrictDbnLoaderV1::open(wrong_digest, PublisherPolicyIdV1::XnasItchHistorical),
        Err(StrictBoundaryErrorV1::CatalogSelection(_))
    ));

    let base = expectation(&path, "XNAS.ITCH", "mbo", 1, false, 4);
    let projection_path = base.custody_projection_path().to_path_buf();
    let storage_root = base.storage_root_path().to_path_buf();
    let wrong_length = CanonicalSourceExpectationV1::new(
        LogicalSourceV1 {
            compressed_bytes: base.logical().compressed_bytes + 1,
            ..base.logical().clone()
        },
        projection_path,
        storage_root,
    )
    .unwrap();
    assert!(matches!(
        StrictDbnLoaderV1::open(wrong_length, PublisherPolicyIdV1::XnasItchHistorical),
        Err(StrictBoundaryErrorV1::CatalogSelection(_))
    ));
}

#[test]
fn metadata_identity_is_checked_before_first_record() {
    let cases = [
        (
            "dataset",
            metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false),
            "OTHER.DATASET",
            "mbo",
            1,
            false,
        ),
        (
            "schema",
            metadata("XNAS.ITCH", Some(Schema::Trades), 1, false),
            "XNAS.ITCH",
            "mbo",
            1,
            false,
        ),
        (
            "none-schema",
            metadata("XNAS.ITCH", None, 1, false),
            "XNAS.ITCH",
            "mbo",
            1,
            false,
        ),
        (
            "version",
            metadata("XNAS.ITCH", Some(Schema::Mbo), 2, false),
            "XNAS.ITCH",
            "mbo",
            1,
            false,
        ),
        (
            "ts-out",
            metadata("XNAS.ITCH", Some(Schema::Mbo), 1, true),
            "XNAS.ITCH",
            "mbo",
            1,
            false,
        ),
    ];
    for (name, actual_metadata, dataset, schema, version, ts_out) in cases {
        let dir = tempdir().unwrap();
        let path = dir.path().join(format!("{name}.dbn"));
        write_mbo(&path, Compression::None, &actual_metadata, &[]);
        let result = StrictDbnLoaderV1::open(
            expectation(&path, dataset, schema, version, ts_out, 0),
            PublisherPolicyIdV1::RejectAll,
        );
        match name {
            "dataset" => assert!(matches!(
                result,
                Err(StrictBoundaryErrorV1::MetadataDataset { .. })
            )),
            "schema" | "none-schema" => assert!(matches!(
                result,
                Err(StrictBoundaryErrorV1::MetadataSchema { .. })
            )),
            "version" => assert!(matches!(
                result,
                Err(StrictBoundaryErrorV1::MetadataVersion { .. })
            )),
            "ts-out" => assert!(matches!(result, Err(StrictBoundaryErrorV1::MetadataTsOut))),
            _ => unreachable!(),
        }
    }
}

#[test]
fn xnas_daily_universe_binding_rejects_incomplete_or_ambiguous_metadata() {
    let mut wrong_day = metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false);
    wrong_day.start += 1;
    assert!(matches!(
        xnas_open_error("wrong-day", &wrong_day),
        StrictBoundaryErrorV1::MetadataCatalogBounds { .. }
    ));

    let mut limited = metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false);
    limited.limit = std::num::NonZeroU64::new(1);
    assert!(matches!(
        xnas_open_error("limited", &limited),
        StrictBoundaryErrorV1::XnasMetadataLimit
    ));

    let mut wrong_symbology = metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false);
    wrong_symbology.stype_in = Some(SType::InstrumentId);
    assert!(matches!(
        xnas_open_error("wrong-symbology", &wrong_symbology),
        StrictBoundaryErrorV1::XnasMetadataSymbology
    ));

    let mut partial = metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false);
    partial.partial.push("TEST".into());
    assert!(matches!(
        xnas_open_error("partial", &partial),
        StrictBoundaryErrorV1::XnasMetadataIncompleteSymbols
    ));

    let mut duplicate_symbol = metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false);
    duplicate_symbol.symbols.push("TEST".into());
    assert!(matches!(
        xnas_open_error("duplicate-symbol", &duplicate_symbol),
        StrictBoundaryErrorV1::XnasMetadataSymbols
    ));

    let mut malformed_symbol = metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false);
    malformed_symbol.symbols[0] = " TEST".into();
    malformed_symbol.mappings[0].raw_symbol = " TEST".into();
    assert!(matches!(
        xnas_open_error("malformed-symbol", &malformed_symbol),
        StrictBoundaryErrorV1::XnasMetadataSymbols
    ));

    let mut zero_instrument = metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false);
    zero_instrument.mappings[0].intervals[0].symbol = "0".into();
    assert!(matches!(
        xnas_open_error("zero-instrument", &zero_instrument),
        StrictBoundaryErrorV1::XnasMetadataMapping(symbol) if symbol == "TEST"
    ));

    let mut missing_mapping = metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false);
    missing_mapping.mappings.clear();
    assert!(matches!(
        xnas_open_error("missing-mapping", &missing_mapping),
        StrictBoundaryErrorV1::XnasMetadataSymbols
    ));

    let mut duplicate_instrument = metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false);
    let mut second = duplicate_instrument.mappings[0].clone();
    second.raw_symbol = "OTHER".into();
    duplicate_instrument.symbols.push("OTHER".into());
    duplicate_instrument.mappings.push(second);
    assert!(matches!(
        xnas_open_error("duplicate-instrument", &duplicate_instrument),
        StrictBoundaryErrorV1::XnasMetadataDuplicateInstrument(101)
    ));
}

#[test]
fn wrong_rtype_is_fatal_but_semantic_rejections_retain_exact_custody_to_eof() {
    let dir = tempdir().unwrap();
    let wrong_rtype = dir.path().join("wrong-rtype.dbn");
    write_trade_record_with_mbo_metadata(&wrong_rtype);
    let mut stream = StrictDbnLoaderV1::open(
        expectation(&wrong_rtype, "XNAS.ITCH", "mbo", 1, false, 1),
        PublisherPolicyIdV1::XnasItchHistorical,
    )
    .unwrap();
    assert!(matches!(
        stream.next().unwrap(),
        Err(StrictBoundaryErrorV1::RecordShape { raw_ordinal: 1, .. })
    ));
    assert!(stream.next().is_none());
    assert!(matches!(
        stream.finish(),
        Err(StrictBoundaryErrorV1::CannotFinishFailedStream)
    ));

    let bad_action = dir.path().join("bad-action.dbn");
    let unknown_action = message(b'X', b'B', 10, 100, 1);
    let mut maybe_bad_book = message(b'A', b'B', 11, 100, 2);
    maybe_bad_book.flags = (flags::LAST | flags::MAYBE_BAD_BOOK).into();
    write_mbo(
        &bad_action,
        Compression::None,
        &metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false),
        &[unknown_action, maybe_bad_book],
    );
    let mut stream = StrictDbnLoaderV1::open(
        expectation(&bad_action, "XNAS.ITCH", "mbo", 1, false, 2),
        PublisherPolicyIdV1::XnasItchHistorical,
    )
    .unwrap();
    let rejected = stream
        .next()
        .unwrap()
        .unwrap()
        .rejected()
        .expect("unknown action is a lossless semantic rejection")
        .clone();
    assert_eq!(rejected.failure().raw_ordinal, 1);
    assert_eq!(rejected.raw().raw_ordinal, 1);
    assert_eq!(rejected.raw().action_raw, b'X');
    assert_eq!(rejected.raw().order_id, 10);
    assert_eq!(
        rejected.failure().reason,
        ValidationReasonV1::UnknownAction(b'X')
    );
    assert_eq!(
        rejected.stage(),
        VerifiedRejectionStageV1::UniversalValidation
    );

    let rejected = stream
        .next()
        .unwrap()
        .unwrap()
        .rejected()
        .expect("MAYBE_BAD_BOOK is a lossless policy rejection")
        .clone();
    assert_eq!(rejected.raw().raw_ordinal, 2);
    assert_eq!(rejected.raw().order_id, 11);
    assert_eq!(rejected.failure().reason, ValidationReasonV1::MaybeBadBook);
    assert_eq!(
        rejected.stage(),
        VerifiedRejectionStageV1::FullOrderBookPolicy
    );
    assert!(stream.next().is_none());
    let receipt = stream.finish().unwrap();
    assert_eq!(receipt.decoded_records(), 2);
    assert_eq!(receipt.accepted_records(), 0);
    assert_eq!(receipt.rejected_records(), 2);
}

#[test]
fn record_identity_absent_from_opened_metadata_is_source_fatal_and_fuses() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("unmapped-identity.dbn");
    let mut unmapped = message(b'A', b'B', 10, 100, 1);
    unmapped.hd.instrument_id = 202;
    let recovery = message(b'R', b'N', 0, 0, 2);
    write_mbo(
        &path,
        Compression::None,
        &metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false),
        &[unmapped, recovery],
    );
    let mut stream = StrictDbnLoaderV1::open(
        expectation(&path, "XNAS.ITCH", "mbo", 1, false, 2),
        PublisherPolicyIdV1::XnasItchHistorical,
    )
    .unwrap();
    assert!(matches!(
        stream.next().unwrap(),
        Err(
            StrictBoundaryErrorV1::RecordIdentityOutsideMetadataAndPolicyBinding {
                raw_ordinal: 1,
                publisher_id: 2,
                instrument_id: 202,
            }
        )
    ));
    assert_eq!(stream.decoded_records(), 1);
    assert!(stream.next().is_none());
    assert!(matches!(
        stream.finish(),
        Err(StrictBoundaryErrorV1::CannotFinishFailedStream)
    ));
}

#[test]
fn receipt_requires_eof_and_exact_expected_population() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("population.dbn");
    write_mbo(
        &path,
        Compression::None,
        &metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false),
        &valid_records(),
    );

    let stream = StrictDbnLoaderV1::open(
        expectation(&path, "XNAS.ITCH", "mbo", 1, false, 4),
        PublisherPolicyIdV1::XnasItchHistorical,
    )
    .unwrap();
    assert!(matches!(
        stream.finish(),
        Err(StrictBoundaryErrorV1::CannotFinishBeforeEof)
    ));

    let mut stream = StrictDbnLoaderV1::open(
        expectation(&path, "XNAS.ITCH", "mbo", 1, false, 5),
        PublisherPolicyIdV1::XnasItchHistorical,
    )
    .unwrap();
    for item in stream.by_ref() {
        item.unwrap();
    }
    assert!(matches!(
        stream.finish(),
        Err(StrictBoundaryErrorV1::RecordCountMismatch {
            expected: 5,
            actual: 4
        })
    ));
}

#[test]
fn truncated_source_cannot_launder_decoder_none_into_success() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("truncated.dbn");
    write_mbo(
        &path,
        Compression::None,
        &metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false),
        &valid_records(),
    );
    let bytes = fs::read(&path).unwrap();
    fs::write(&path, &bytes[..bytes.len() - 20]).unwrap();

    let mut stream = StrictDbnLoaderV1::open(
        expectation(&path, "XNAS.ITCH", "mbo", 1, false, 4),
        PublisherPolicyIdV1::XnasItchHistorical,
    )
    .unwrap();
    for item in stream.by_ref() {
        item.unwrap();
    }
    assert!(matches!(
        stream.finish(),
        Err(StrictBoundaryErrorV1::RecordCountMismatch { .. })
    ));
}

#[test]
fn pathname_replacement_after_open_does_not_substitute_decoded_bytes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("source.dbn");
    let moved = dir.path().join("opened-object.dbn");
    write_mbo(
        &path,
        Compression::None,
        &metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false),
        &valid_records(),
    );
    let expectation = expectation(&path, "XNAS.ITCH", "mbo", 1, false, 4);
    let expected_digest = expectation.logical().compressed_sha256;
    let mut stream =
        StrictDbnLoaderV1::open(expectation, PublisherPolicyIdV1::XnasItchHistorical).unwrap();

    fs::rename(&path, &moved).unwrap();
    write_mbo(
        &path,
        Compression::None,
        &metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false),
        &[message(b'A', b'A', 999, 1, 99)],
    );

    let mut seen = 0;
    for item in stream.by_ref() {
        let item = item.unwrap();
        assert_eq!(
            item.accepted()
                .expect("fixture is accepted")
                .disposition()
                .event()
                .raw()
                .source_object_sha256,
            expected_digest
        );
        seen += 1;
    }
    assert_eq!(seen, 4);
    assert!(matches!(
        stream.finish(),
        Err(StrictBoundaryErrorV1::SourceRuntimeIdentityChanged)
    ));
    assert_ne!(digest_and_len(&path).0, expected_digest);
}

#[test]
fn decoder_error_reports_the_next_position_without_claiming_a_row_ordinal() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("decoder-error.dbn");
    write_mbo(
        &path,
        Compression::None,
        &metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false),
        &[message(b'A', b'B', 10, 100, 1)],
    );
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&[0_u8; 16])
        .unwrap();

    let mut stream = StrictDbnLoaderV1::open(
        expectation(&path, "XNAS.ITCH", "mbo", 1, false, 2),
        PublisherPolicyIdV1::XnasItchHistorical,
    )
    .unwrap();
    assert!(stream.next().unwrap().is_ok());
    assert_eq!(stream.decoded_records(), 1);
    assert!(matches!(
        stream.next().unwrap(),
        Err(StrictBoundaryErrorV1::Decode {
            next_raw_ordinal: 2,
            ..
        })
    ));
    assert_eq!(stream.decoded_records(), 1);
    assert!(stream.next().is_none());
}

#[test]
fn in_place_source_mutation_is_detected_before_receipt() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("mutated.dbn");
    write_mbo(
        &path,
        Compression::None,
        &metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false),
        &valid_records(),
    );
    let mut stream = StrictDbnLoaderV1::open(
        expectation(&path, "XNAS.ITCH", "mbo", 1, false, 4),
        PublisherPolicyIdV1::XnasItchHistorical,
    )
    .unwrap();

    let mut writer = OpenOptions::new().write(true).open(&path).unwrap();
    writer.seek(SeekFrom::End(-1)).unwrap();
    let last = fs::read(&path).unwrap().last().copied().unwrap();
    writer.write_all(&[last ^ 0xFF]).unwrap();
    writer.flush().unwrap();

    for item in stream.by_ref() {
        item.unwrap();
    }
    assert!(matches!(
        stream.finish(),
        Err(StrictBoundaryErrorV1::SourceRuntimeIdentityChanged)
    ));
}
