#![cfg(feature = "databento")]

use dbn::encode::{DbnEncoder, DynWriter, EncodeRecord};
use dbn::{
    flags, Compression, MappingInterval, MboMsg, Metadata, RecordHeader, SType, Schema,
    SymbolMapping, TradeMsg,
};
use hft_mbo_event_contract::{
    EventDispositionV1, LogicalSourceV1, PublisherPolicyIdV1, Sha256DigestV1,
    CANONICAL_MBO_EVENT_CONTRACT_SHA256,
};
use mbo_lob_reconstructor::{
    CanonicalSourceExpectationV1, StrictBoundaryErrorV1, StrictDbnLoaderV1,
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
        &path,
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
    let (digest, bytes) = digest_and_len(path);
    CanonicalSourceExpectationV1::new(
        LogicalSourceV1 {
            catalog_release_id: "dbc-test-v1".into(),
            catalog_object_id: "synthetic-object".into(),
            canonical_path: path.to_str().unwrap().into(),
            canonical_sha256: digest,
            canonical_bytes: bytes,
            dbn_version: version,
            dbn_ts_out: ts_out,
            dataset: dataset.into(),
            schema: schema.into(),
        },
        expected_records,
    )
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
            &path,
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
        while let Some(item) = stream.next() {
            let item = item.unwrap();
            let disposition = item.disposition();
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
            receipt.source().logical.canonical_sha256
        );
    }
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
    wrong_digest = CanonicalSourceExpectationV1::new(
        LogicalSourceV1 {
            canonical_sha256: Sha256DigestV1::from_bytes([0x55; 32]),
            ..wrong_digest.logical().clone()
        },
        4,
    );
    assert!(matches!(
        StrictDbnLoaderV1::open(wrong_digest, &path, PublisherPolicyIdV1::XnasItchHistorical),
        Err(StrictBoundaryErrorV1::SourceIdentity(_))
    ));

    let base = expectation(&path, "XNAS.ITCH", "mbo", 1, false, 4);
    let wrong_length = CanonicalSourceExpectationV1::new(
        LogicalSourceV1 {
            canonical_bytes: base.logical().canonical_bytes + 1,
            ..base.logical().clone()
        },
        4,
    );
    assert!(matches!(
        StrictDbnLoaderV1::open(wrong_length, &path, PublisherPolicyIdV1::XnasItchHistorical),
        Err(StrictBoundaryErrorV1::SourceIdentity(_))
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
            &path,
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
        StrictBoundaryErrorV1::XnasMetadataDayBoundary { .. }
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
fn wrong_rtype_and_semantic_failure_own_their_one_based_ordinals_and_fuse() {
    let dir = tempdir().unwrap();
    let wrong_rtype = dir.path().join("wrong-rtype.dbn");
    write_trade_record_with_mbo_metadata(&wrong_rtype);
    let mut stream = StrictDbnLoaderV1::open(
        expectation(&wrong_rtype, "XNAS.ITCH", "mbo", 1, false, 1),
        &wrong_rtype,
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
    write_mbo(
        &bad_action,
        Compression::None,
        &metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false),
        &[message(b'X', b'B', 10, 100, 1)],
    );
    let mut stream = StrictDbnLoaderV1::open(
        expectation(&bad_action, "XNAS.ITCH", "mbo", 1, false, 1),
        &bad_action,
        PublisherPolicyIdV1::XnasItchHistorical,
    )
    .unwrap();
    match stream.next().unwrap() {
        Err(StrictBoundaryErrorV1::Validation(failure)) => {
            assert_eq!(failure.raw_ordinal, 1)
        }
        other => panic!("unexpected result: {other:?}"),
    }
    assert!(stream.next().is_none());
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
        &path,
        PublisherPolicyIdV1::XnasItchHistorical,
    )
    .unwrap();
    assert!(matches!(
        stream.finish(),
        Err(StrictBoundaryErrorV1::CannotFinishBeforeEof)
    ));

    let mut stream = StrictDbnLoaderV1::open(
        expectation(&path, "XNAS.ITCH", "mbo", 1, false, 5),
        &path,
        PublisherPolicyIdV1::XnasItchHistorical,
    )
    .unwrap();
    while let Some(item) = stream.next() {
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
        &path,
        PublisherPolicyIdV1::XnasItchHistorical,
    )
    .unwrap();
    while let Some(item) = stream.next() {
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
    let expected_digest = expectation.logical().canonical_sha256;
    let mut stream =
        StrictDbnLoaderV1::open(expectation, &path, PublisherPolicyIdV1::XnasItchHistorical)
            .unwrap();

    fs::rename(&path, &moved).unwrap();
    write_mbo(
        &path,
        Compression::None,
        &metadata("XNAS.ITCH", Some(Schema::Mbo), 1, false),
        &[message(b'A', b'A', 999, 1, 99)],
    );

    let mut seen = 0;
    while let Some(item) = stream.next() {
        let item = item.unwrap();
        assert_eq!(
            item.disposition().event().raw().source_object_sha256,
            expected_digest
        );
        seen += 1;
    }
    assert_eq!(seen, 4);
    let receipt = stream.finish().unwrap();
    assert_eq!(receipt.source().opened.opened_sha256, expected_digest);
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
        &path,
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
        &path,
        PublisherPolicyIdV1::XnasItchHistorical,
    )
    .unwrap();

    let mut writer = OpenOptions::new().write(true).open(&path).unwrap();
    writer.seek(SeekFrom::End(-1)).unwrap();
    let last = fs::read(&path).unwrap().last().copied().unwrap();
    writer.write_all(&[last ^ 0xFF]).unwrap();
    writer.flush().unwrap();

    while let Some(item) = stream.next() {
        item.unwrap();
    }
    assert!(matches!(
        stream.finish(),
        Err(StrictBoundaryErrorV1::SourceChangedDuringDecode { .. })
    ));
}
