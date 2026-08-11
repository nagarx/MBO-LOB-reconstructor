#![cfg(feature = "databento")]

mod support;

use dbn::encode::{DbnEncoder, DynWriter, EncodeRecord};
use dbn::{
    flags, Compression, MappingInterval, MboMsg, Metadata, RecordHeader, SType, Schema,
    SymbolMapping,
};
use hft_mbo_event_contract::{
    PublisherPolicyIdV1, Sha256DigestV1, ValidationReasonV1, UNDEF_PRICE,
};
use mbo_lob_reconstructor::{
    BookTransactionErrorV1, CanonicalSourceExpectationV1, StrictDbnLoaderV1, StrictXnasReplayV1,
    VerifiedRejectionStageV1, XnasCommittedObservationAccumulatorV1, XnasEofTailReasonV1,
    XnasObservationAccountingErrorV1, XnasQuarantineReasonV1, XnasRejectedRecordPhaseV1,
    XnasReplayConfigV1, XnasReplayErrorV1, XnasReplayProbeRequestErrorV1, XnasReplayProbeRequestV1,
    XnasReplayRunV1, XnasSelectedOrdinalRoleV1, XnasTerminalDisqualificationReasonV1,
    XnasTerminalIdentityStatusV1, XnasValidityInvalidationReasonV1,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

const START_NS: u64 = 1_751_328_000_000_000_000;
const END_NS: u64 = START_NS + 86_400_000_000_000;
const BID: i64 = 100_000_000_000;
const ASK: i64 = 105_000_000_000;

fn metadata() -> Metadata {
    let mut metadata = Metadata::builder()
        .version(1)
        .dataset("XNAS.ITCH")
        .schema(Some(Schema::Mbo))
        .start(START_NS)
        .end(NonZeroU64::new(END_NS))
        .stype_in(Some(SType::RawSymbol))
        .stype_out(SType::InstrumentId)
        .ts_out(false)
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

fn two_identity_metadata() -> Metadata {
    let mut value = metadata();
    let start_date = value.start().date();
    value.symbols = vec!["ONE".into(), "TWO".into()];
    value.mappings = vec![
        SymbolMapping {
            raw_symbol: "ONE".into(),
            intervals: vec![MappingInterval {
                start_date,
                end_date: start_date.next_day().unwrap(),
                symbol: "101".into(),
            }],
        },
        SymbolMapping {
            raw_symbol: "TWO".into(),
            intervals: vec![MappingInterval {
                start_date,
                end_date: start_date.next_day().unwrap(),
                symbol: "202".into(),
            }],
        },
    ];
    value
}

#[allow(clippy::too_many_arguments)]
fn message(
    action: u8,
    side: u8,
    order_id: u64,
    price: i64,
    size: u32,
    sequence: u32,
    flags_raw: u8,
    ts_event: u64,
    ts_recv: u64,
) -> MboMsg {
    message_for_instrument(
        101, action, side, order_id, price, size, sequence, flags_raw, ts_event, ts_recv,
    )
}

#[allow(clippy::too_many_arguments)]
fn message_for_instrument(
    instrument_id: u32,
    action: u8,
    side: u8,
    order_id: u64,
    price: i64,
    size: u32,
    sequence: u32,
    flags_raw: u8,
    ts_event: u64,
    ts_recv: u64,
) -> MboMsg {
    MboMsg {
        hd: RecordHeader::new::<MboMsg>(0xA0, 2, instrument_id, ts_event),
        order_id,
        price,
        size,
        flags: flags_raw.into(),
        channel_id: 0,
        action: action as _,
        side: side as _,
        ts_recv,
        ts_in_delta: 0,
        sequence,
    }
}

fn initial_clear(ts_event: u64, ts_recv: u64) -> MboMsg {
    message(
        b'R',
        b'N',
        0,
        UNDEF_PRICE,
        0,
        0,
        flags::BAD_TS_RECV,
        ts_event,
        ts_recv,
    )
}

fn write_dbn(path: &Path, compression: Compression, records: &[MboMsg]) {
    write_dbn_with_metadata(path, compression, records, &metadata());
}

fn write_dbn_with_metadata(
    path: &Path,
    compression: Compression,
    records: &[MboMsg],
    metadata: &Metadata,
) {
    let writer = DynWriter::new(File::create(path).unwrap(), compression).unwrap();
    let mut encoder = DbnEncoder::new(writer, metadata).unwrap();
    encoder.encode_records(records).unwrap();
    encoder.flush().unwrap();
}

fn expectation(path: &Path, expected_records: u64) -> CanonicalSourceExpectationV1 {
    support::expectation(path, "XNAS.ITCH", "mbo", 1, false, expected_records)
}

fn two_identity_expectation(path: &Path, expected_records: u64) -> CanonicalSourceExpectationV1 {
    support::expectation_with_population(
        path,
        "XNAS.ITCH",
        "mbo",
        1,
        false,
        expected_records,
        "ONE,TWO",
        2,
        2,
    )
}

fn open_replay(path: &Path, records: u64) -> StrictXnasReplayV1 {
    open_replay_with_config(path, records, replay_config())
}

fn open_two_identity_replay(path: &Path, records: u64) -> StrictXnasReplayV1 {
    let stream = StrictDbnLoaderV1::open(
        two_identity_expectation(path, records),
        PublisherPolicyIdV1::XnasItchHistorical,
    )
    .unwrap();
    StrictXnasReplayV1::from_strict_stream(stream, replay_config()).unwrap()
}

fn open_replay_with_config(
    path: &Path,
    records: u64,
    config: XnasReplayConfigV1,
) -> StrictXnasReplayV1 {
    let stream = StrictDbnLoaderV1::open(
        expectation(path, records),
        PublisherPolicyIdV1::XnasItchHistorical,
    )
    .unwrap();
    StrictXnasReplayV1::from_strict_stream(stream, config).unwrap()
}

fn replay_config() -> XnasReplayConfigV1 {
    XnasReplayConfigV1::new(
        NonZeroUsize::new(10).unwrap(),
        NonZeroUsize::new(64).unwrap(),
        NonZeroUsize::new(64).unwrap(),
    )
}

fn independent_observation_chain_seed(
    source: Sha256DigestV1,
    config: XnasReplayConfigV1,
) -> Sha256DigestV1 {
    let mut hasher = Sha256::new();
    hasher.update(b"hft.xnas_committed_observation_chain.seed.v2\0");
    hasher.update(source.as_bytes());
    hasher.update(
        u64::try_from(config.snapshot_depth().get())
            .unwrap()
            .to_le_bytes(),
    );
    hasher.update(
        u64::try_from(config.max_envelope_members().get())
            .unwrap()
            .to_le_bytes(),
    );
    hasher.update(
        u64::try_from(config.max_sequence_blocks().get())
            .unwrap()
            .to_le_bytes(),
    );
    Sha256DigestV1::from_bytes(hasher.finalize().into())
}

fn independent_observation_chain_next(
    prior: Sha256DigestV1,
    observation: Sha256DigestV1,
) -> Sha256DigestV1 {
    let mut hasher = Sha256::new();
    hasher.update(b"hft.xnas_committed_observation_chain.v2\0");
    hasher.update(prior.as_bytes());
    hasher.update(observation.as_bytes());
    Sha256DigestV1::from_bytes(hasher.finalize().into())
}

fn run_with_all_traces(replay: StrictXnasReplayV1, records: u64) -> XnasReplayRunV1 {
    let selected = (1..=records).collect::<BTreeSet<_>>();
    replay.run_to_eof_with_selected_ordinals(&selected).unwrap()
}

fn selected_roles(run: &XnasReplayRunV1, raw_ordinal: u64) -> &[XnasSelectedOrdinalRoleV1] {
    run.selected_ordinal_dispositions()
        .iter()
        .find(|value| value.raw_ordinal() == raw_ordinal)
        .unwrap_or_else(|| panic!("missing selected disposition for raw ordinal {raw_ordinal}"))
        .roles()
}

#[derive(Clone, Copy)]
enum RecoverableFailureFixtureV1 {
    LastToNonLast,
    MissingModify,
}

fn recoverable_failure_records(kind: RecoverableFailureFixtureV1) -> Vec<MboMsg> {
    let fourth = match kind {
        RecoverableFailureFixtureV1::LastToNonLast => message(
            b'A',
            b'B',
            3,
            BID - 1_000_000_000,
            10,
            12,
            flags::LAST,
            START_NS + 30,
            START_NS + 120,
        ),
        RecoverableFailureFixtureV1::MissingModify => message(
            b'M',
            b'B',
            999,
            BID,
            1,
            12,
            flags::LAST,
            START_NS + 30,
            START_NS + 120,
        ),
    };
    let fifth = match kind {
        RecoverableFailureFixtureV1::LastToNonLast => message(
            b'A',
            b'B',
            4,
            BID - 2_000_000_000,
            10,
            12,
            0,
            START_NS + 30,
            START_NS + 120,
        ),
        RecoverableFailureFixtureV1::MissingModify => message(
            b'T',
            b'A',
            0,
            BID,
            1,
            13,
            flags::LAST,
            START_NS + 40,
            START_NS + 130,
        ),
    };
    vec![
        initial_clear(START_NS + 1, START_NS + 2),
        message(
            b'A',
            b'B',
            1,
            BID,
            100,
            10,
            flags::LAST,
            START_NS + 10,
            START_NS + 100,
        ),
        message(
            b'A',
            b'A',
            2,
            ASK,
            50,
            11,
            flags::LAST,
            START_NS + 20,
            START_NS + 110,
        ),
        fourth,
        fifth,
        message(
            b'R',
            b'N',
            0,
            UNDEF_PRICE,
            0,
            20,
            0,
            START_NS + 50,
            START_NS + 140,
        ),
        message(
            b'A',
            b'A',
            5,
            ASK,
            25,
            20,
            flags::LAST,
            START_NS + 50,
            START_NS + 140,
        ),
        message(
            b'T',
            b'B',
            0,
            ASK,
            1,
            30,
            flags::LAST,
            START_NS + 60,
            START_NS + 150,
        ),
    ]
}

fn conformance_records(initial_event_ns: u64, initial_recv_ns: u64) -> Vec<MboMsg> {
    vec![
        initial_clear(initial_event_ns, initial_recv_ns),
        message(
            b'A',
            b'B',
            1,
            BID,
            100,
            10,
            flags::LAST,
            START_NS + 10,
            START_NS + 100,
        ),
        message(
            b'A',
            b'A',
            2,
            ASK,
            50,
            11,
            flags::LAST,
            START_NS + 20,
            START_NS + 110,
        ),
        message(
            b'T',
            b'A',
            0,
            BID,
            40,
            12,
            flags::LAST,
            START_NS + 30,
            START_NS + 120,
        ),
        message(
            b'F',
            b'B',
            1,
            BID,
            40,
            12,
            flags::LAST,
            START_NS + 30,
            START_NS + 120,
        ),
        message(
            b'C',
            b'B',
            1,
            BID,
            40,
            12,
            flags::LAST,
            START_NS + 30,
            START_NS + 120,
        ),
        message(
            b'A',
            b'B',
            3,
            BID - 1_000_000_000,
            20,
            13,
            0,
            START_NS + 40,
            START_NS + 130,
        ),
        message(
            b'A',
            b'A',
            4,
            ASK + 1_000_000_000,
            10,
            15,
            0,
            START_NS + 41,
            START_NS + 130,
        ),
        message(
            b'C',
            b'A',
            4,
            ASK + 1_000_000_000,
            10,
            20,
            flags::LAST,
            START_NS + 42,
            START_NS + 130,
        ),
        message(
            b'T',
            b'B',
            0,
            ASK,
            1,
            30,
            flags::LAST,
            START_NS + 50,
            START_NS + 140,
        ),
    ]
}

fn long_trade_stream_records(
    trade_count: usize,
    changed_trade: Option<(usize, u32)>,
) -> Vec<MboMsg> {
    let mut records = Vec::with_capacity(trade_count + 1);
    records.push(initial_clear(START_NS + 1, START_NS + 2));
    for index in 0..trade_count {
        let size = changed_trade
            .filter(|(changed_index, _)| *changed_index == index)
            .map_or(1, |(_, changed_size)| changed_size);
        records.push(message(
            b'T',
            b'N',
            0,
            ASK,
            size,
            u32::try_from(index + 1).unwrap(),
            flags::LAST,
            START_NS + 10 + index as u64,
            START_NS + 100 + index as u64,
        ));
    }
    records
}

fn exact_byte_differences(source: &[u8], alternate: &[u8]) -> Vec<(u64, u8, u8)> {
    assert_eq!(source.len(), alternate.len());
    source
        .iter()
        .zip(alternate)
        .enumerate()
        .filter_map(|(offset, (source, alternate))| {
            (source != alternate).then_some((offset as u64, *source, *alternate))
        })
        .collect()
}

fn apply_byte_differences(file: &mut File, differences: &[(u64, u8, u8)], use_alternate: bool) {
    for (offset, original, alternate) in differences {
        file.seek(SeekFrom::Start(*offset)).unwrap();
        file.write_all(&[if use_alternate { *alternate } else { *original }])
            .unwrap();
    }
    file.sync_data().unwrap();
}

#[test]
fn strict_loader_replay_proves_repeated_last_multiblock_and_population_semantics() {
    for compression in [Compression::None, Compression::Zstd] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("conformance.dbn");
        let records = conformance_records(START_NS + 1, START_NS + 2);
        write_dbn(&path, compression, &records);
        let run = run_with_all_traces(
            open_replay(&path, records.len() as u64),
            records.len() as u64,
        );
        let updates = run.traces();
        assert_eq!(updates.len(), 4);

        assert_eq!(updates[0].events().len(), 1);
        assert_eq!(
            updates[0].source_object_sha256(),
            expectation(&path, 10).logical().compressed_sha256
        );
        assert_eq!(updates[0].validity_epoch_index(), 1);
        assert_eq!(updates[0].first_source_ordinal(), 2);
        assert_eq!(updates[0].last_source_ordinal(), 2);
        assert_eq!(updates[0].terminal_source_ordinal(), 2);
        assert_eq!(updates[0].witness_source_ordinal(), 3);
        assert_eq!(updates[0].book().snapshot().live_orders(), 1);

        assert_eq!(updates[2].events().len(), 3);
        assert_eq!(updates[2].ordered_distinct_sequences(), [12]);
        assert_eq!(updates[2].execution_sequence_blocks(), 1);
        assert_eq!(updates[2].execution_carriers(), 2);
        assert_eq!(updates[2].terminal_source_ordinal(), 6);
        assert_eq!(updates[2].witness_source_ordinal(), 7);
        assert_eq!(updates[2].book().book_commands_committed(), 1);
        assert_eq!(
            updates[2]
                .book()
                .snapshot()
                .best_bid()
                .unwrap()
                .aggregate_size(),
            60
        );
        assert_eq!(updates[2].effective_available_ns(), START_NS + 130);
        assert_eq!(updates[2].closure_confirmation_delay_ns(), 10);

        assert_eq!(updates[3].ordered_distinct_sequences(), [13, 15, 20]);
        assert_eq!(updates[3].events().len(), 3);
        assert_eq!(updates[3].witness_source_ordinal(), 10);
        assert_eq!(updates[3].book().snapshot().live_orders(), 3);

        let receipt = run.receipt();
        let counts = receipt.counts();
        assert_eq!(counts.raw_records_ingested, 10);
        assert_eq!(counts.initial_clear_controls, 1);
        assert_eq!(counts.private_book_resets, 1);
        assert_eq!(counts.completed_envelope_members, 8);
        assert_eq!(counts.pending_members, 0);
        assert_eq!(counts.quarantined_records, 1);
        assert_eq!(counts.completed_update_envelopes, 4);
        assert_eq!(counts.venue_sequence_blocks, 6);
        assert_eq!(counts.execution_sequence_blocks, 1);
        assert_eq!(counts.execution_envelopes, 1);
        assert_eq!(counts.execution_carriers, 2);
        assert_eq!(counts.book_commands_committed, 6);
        assert_eq!(counts.staged_book_updates, 4);
        assert_eq!(counts.tail_quarantine_incidents, 1);
        assert_eq!(counts.eof_tail_quarantined_records, 1);
        assert_eq!(counts.reset_boundary_quarantined_records, 0);
        assert!(counts.population_reconciles());
        assert!(counts.quarantine_reasons_reconcile());
        assert_eq!(receipt.authority(), "development_only_authorizes_nothing");
        assert_eq!(receipt.identities()[0].committed_envelopes(), 4);
        assert_eq!(
            receipt.identities()[0].terminal_status(),
            XnasTerminalIdentityStatusV1::InvalidAfterEofTailQuarantine
        );
        let tail = receipt.identities()[0]
            .eof_tail_quarantine()
            .expect("valid identity must retain its EOF tail");
        assert_eq!(tail.first_source_ordinal(), 10);
        assert_eq!(tail.last_source_ordinal(), 10);
        assert_eq!(tail.member_count(), 1);
        assert_eq!(tail.source_ordinals(), [10]);
        assert_eq!(
            tail.reason(),
            XnasEofTailReasonV1::TerminalCandidateWithoutWitness
        );
        assert!(!tail.recovery_candidate());
        assert_eq!(
            receipt.identities()[0].first_qualified_terminal_source_ordinal(),
            Some(2)
        );
        assert_eq!(
            receipt.identities()[0].first_qualified_effective_available_ns(),
            Some(START_NS + 110)
        );
        assert_eq!(
            receipt.identities()[0]
                .initial_clear_control()
                .expect("identity retained initial control")
                .raw_ordinal,
            1
        );
        assert_eq!(
            receipt.committed_observation_chain_sha256(),
            updates.last().unwrap().committed_observation_chain_sha256()
        );
        assert_eq!(run.selected_ordinal_dispositions().len(), 10);
        assert!(run
            .selected_ordinal_dispositions()
            .iter()
            .all(|value| value.decoded_from_source() && !value.roles().is_empty()));
        let ordinal_7 = run
            .selected_ordinal_dispositions()
            .iter()
            .find(|value| value.raw_ordinal() == 7)
            .unwrap();
        assert!(ordinal_7.roles().iter().any(|role| matches!(
            role,
            XnasSelectedOrdinalRoleV1::ClosureWitness { trace_index: 2, .. }
        )));
        assert!(ordinal_7.roles().iter().any(|role| matches!(
            role,
            XnasSelectedOrdinalRoleV1::CompletedEnvelopeMember { trace_index: 3, .. }
        )));
        let ordinal_10 = run
            .selected_ordinal_dispositions()
            .iter()
            .find(|value| value.raw_ordinal() == 10)
            .unwrap();
        assert!(ordinal_10.roles().iter().any(|role| matches!(
            role,
            XnasSelectedOrdinalRoleV1::EofTailQuarantinedMember { .. }
        )));
    }
}

#[test]
fn qualified_two_pass_replay_exposes_pending_envelopes_then_exact_equivalence() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("two-pass.dbn");
    let records = conformance_records(START_NS + 1, START_NS + 2);
    write_dbn(&path, Compression::None, &records);
    let expected_source = expectation(&path, records.len() as u64)
        .logical()
        .compressed_sha256;

    let plan = open_replay(&path, records.len() as u64)
        .qualify_unbound_development()
        .unwrap();
    assert_eq!(
        plan.qualification_receipt().source().decoded_records(),
        records.len() as u64
    );
    let build = plan.qualification_receipt().build();
    let package_lock = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock"))
        .expect("package lock is present in the repository");
    assert_eq!(build.package_version(), "1.0.0");
    assert_eq!(
        plan.qualification_receipt().schema(),
        "xnas_replay_receipt_v1"
    );
    assert_eq!(build.replay_algorithm_id(), "hft.xnas.strict_replay.v2");
    assert!(build
        .enabled_features()
        .split(',')
        .any(|feature| feature == "CARGO_FEATURE_DATABENTO"));
    assert_eq!(
        build.package_repository_cargo_lock_sha256(),
        Sha256DigestV1::from_bytes(Sha256::digest(package_lock).into())
    );
    if build.git_commit() == "unverified-no-package-repository-commit" {
        assert!(build.git_dirty());
    } else {
        assert_eq!(build.git_commit().len(), 40);
        assert!(build
            .git_commit()
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    }
    if build.rustc_command() != "unverified-non-utf8-rustc-command" {
        let rustc_output = Command::new(build.rustc_command())
            .arg("-vV")
            .output()
            .unwrap();
        assert!(rustc_output.status.success());
        let rustc_verbose = String::from_utf8(rustc_output.stdout)
            .unwrap()
            .trim()
            .to_owned();
        assert_eq!(
            build.rustc_verbose_sha256(),
            Sha256DigestV1::from_bytes(Sha256::digest(rustc_verbose.as_bytes()).into())
        );
        assert_eq!(build.rustc_version(), rustc_verbose.lines().next().unwrap());
    }
    let mut pass = plan.open_revalidation_pass().unwrap();
    let mut accounting = XnasCommittedObservationAccumulatorV1::new(plan.qualification_receipt());
    let mut observations = Vec::new();
    let mut observation_digests = Vec::new();
    let mut observation_chains = Vec::new();
    while let Some(observation) = pass.next_observation().unwrap() {
        accounting.observe(&observation).unwrap();
        observation_digests.push(observation.committed_observation_sha256());
        observation_chains.push(observation.committed_observation_chain_sha256());
        observations.push((
            observation.source_object_sha256(),
            observation.validity_epoch_index(),
            observation.first_source_ordinal(),
            observation.last_source_ordinal(),
            observation.witness_source_ordinal(),
            observation.effective_available_ns(),
            observation.book().exact_endpoint_state_changed(),
        ));
    }
    assert_eq!(observations.len(), 4);
    assert!(observations
        .iter()
        .all(|observation| observation.0 == expected_source && observation.1 == 1));
    // The witness that closes one envelope belongs to the following envelope,
    // never to both member populations.
    assert_eq!(
        (observations[0].2, observations[0].3, observations[0].4),
        (2, 2, 3)
    );
    assert_eq!(observations[1].2, 3);
    for pair in observations.windows(2) {
        assert_eq!(pair[0].4, pair[1].2);
        assert_ne!(pair[0].4, pair[0].3);
    }

    let mut independent_chain =
        independent_observation_chain_seed(expected_source, plan.qualification_receipt().config());
    for (observation_digest, exposed_chain) in observation_digests.iter().zip(&observation_chains) {
        independent_chain =
            independent_observation_chain_next(independent_chain, *observation_digest);
        assert_eq!(*exposed_chain, independent_chain);
    }
    assert_eq!(
        observation_digests[0].to_hex(),
        "772b8a79f0bccb14d81ddbdcf746a04a92ea3ab4971cf0b894a0d3f39ff0aa83"
    );
    assert_eq!(
        independent_chain.to_hex(),
        "73a8db01092d6a20e6b3310005b6086fc6b369a2768c6ecc62a99608e4550236"
    );

    let equivalence = pass.finish().unwrap();
    let closure = accounting.finish(&equivalence).unwrap();
    assert_eq!(closure.observations_consumed(), 4);
    assert_eq!(closure.terminal_chain_sha256(), independent_chain);
    assert_eq!(equivalence.schema(), "xnas_replay_equivalence_receipt_v1");
    assert_eq!(equivalence.exact_receipt(), plan.qualification_receipt());
    assert_eq!(equivalence.verified_complete_replays(), 2);
    assert_eq!(
        equivalence
            .exact_receipt()
            .committed_observation_chain_sha256(),
        independent_chain
    );
    assert_eq!(
        equivalence.authority(),
        "development_only_exact_two_pass_replay_equivalence_not_publication_authority"
    );
}

#[test]
fn observation_accounting_detects_skipped_duplicated_and_reordered_pass_two_items() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("observation-accounting.dbn");
    let records = conformance_records(START_NS + 1, START_NS + 2);
    write_dbn(&path, Compression::None, &records);
    let plan = open_replay(&path, records.len() as u64)
        .qualify_unbound_development()
        .unwrap();

    let mut skipped_pass = plan.open_revalidation_pass().unwrap();
    let _skipped = skipped_pass.next_observation().unwrap().unwrap();
    let second = skipped_pass.next_observation().unwrap().unwrap();
    let mut skipped = XnasCommittedObservationAccumulatorV1::new(plan.qualification_receipt());
    assert!(matches!(
        skipped.observe(&second),
        Err(XnasObservationAccountingErrorV1::ChainMismatch)
    ));

    let mut duplicate_pass = plan.open_revalidation_pass().unwrap();
    let first = duplicate_pass.next_observation().unwrap().unwrap();
    let mut duplicate = XnasCommittedObservationAccumulatorV1::new(plan.qualification_receipt());
    duplicate.observe(&first).unwrap();
    assert!(matches!(
        duplicate.observe(&first),
        Err(XnasObservationAccountingErrorV1::OrdinalRegression { .. })
    ));

    let mut reordered_pass = plan.open_revalidation_pass().unwrap();
    let _first = reordered_pass.next_observation().unwrap().unwrap();
    let second = reordered_pass.next_observation().unwrap().unwrap();
    let mut reordered = XnasCommittedObservationAccumulatorV1::new(plan.qualification_receipt());
    assert!(matches!(
        reordered.observe(&second),
        Err(XnasObservationAccountingErrorV1::ChainMismatch)
    ));
}

#[test]
fn observation_accounting_finish_detects_omitted_tail_item() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("observation-accounting-tail.dbn");
    let records = conformance_records(START_NS + 1, START_NS + 2);
    write_dbn(&path, Compression::None, &records);
    let plan = open_replay(&path, records.len() as u64)
        .qualify_unbound_development()
        .unwrap();

    let mut pass = plan.open_revalidation_pass().unwrap();
    let mut accounting = XnasCommittedObservationAccumulatorV1::new(plan.qualification_receipt());
    let mut held_back = None;
    while let Some(observation) = pass.next_observation().unwrap() {
        if let Some(previous) = held_back.replace(observation) {
            accounting.observe(&previous).unwrap();
        }
    }
    assert!(
        held_back.is_some(),
        "fixture must produce an omitted tail item"
    );
    let equivalence = pass.finish().unwrap();
    assert!(matches!(
        accounting.finish(&equivalence),
        Err(XnasObservationAccountingErrorV1::ObservationCountMismatch { .. })
    ));
}

#[test]
fn observation_accounting_is_bound_to_the_exact_qualification_receipt() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("observation-accounting-receipt.dbn");
    let records = conformance_records(START_NS + 1, START_NS + 2);
    write_dbn(&path, Compression::None, &records);
    let plan = open_replay(&path, records.len() as u64)
        .qualify_unbound_development()
        .unwrap();
    let other_config = XnasReplayConfigV1::new(
        NonZeroUsize::new(5).unwrap(),
        NonZeroUsize::new(64).unwrap(),
        NonZeroUsize::new(64).unwrap(),
    );
    let other = open_replay_with_config(&path, records.len() as u64, other_config)
        .qualify_unbound_development()
        .unwrap();
    let mut other_pass = other.open_revalidation_pass().unwrap();
    while other_pass.next_observation().unwrap().is_some() {}
    let other_equivalence = other_pass.finish().unwrap();

    let accounting = XnasCommittedObservationAccumulatorV1::new(plan.qualification_receipt());
    assert!(matches!(
        accounting.finish(&other_equivalence),
        Err(XnasObservationAccountingErrorV1::ReceiptBindingMismatch)
    ));
}

#[test]
fn qualified_two_pass_replay_preserves_quarantine_recovery_and_epoch_semantics() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("two-pass-recovery.dbn");
    let records = recoverable_failure_records(RecoverableFailureFixtureV1::MissingModify);
    write_dbn(&path, Compression::None, &records);

    let plan = open_replay(&path, records.len() as u64)
        .qualify_unbound_development()
        .unwrap();
    assert_eq!(
        plan.qualification_receipt()
            .counts()
            .semantic_quarantine_incidents,
        1
    );
    let mut pass = plan.open_revalidation_pass().unwrap();
    let mut observations = Vec::new();
    while let Some(observation) = pass.next_observation().unwrap() {
        observations.push((
            observation.is_recovery(),
            observation.validity_epoch_index(),
            observation.book().reset_epoch(),
            observation.terminal_source_ordinal(),
        ));
    }
    assert_eq!(observations.len(), 3);
    assert_eq!(observations[0], (false, 1, 1, 2));
    assert_eq!(observations[1], (false, 1, 1, 3));
    assert_eq!(observations[2], (true, 2, 2, 7));

    let equivalence = pass.finish().unwrap();
    assert_eq!(equivalence.exact_receipt(), plan.qualification_receipt());
    assert_eq!(equivalence.verified_complete_replays(), 2);
}

#[test]
fn revalidation_cannot_finish_before_eof_or_after_source_mutation() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("two-pass-terminal-gate.dbn");
    let records = conformance_records(START_NS + 1, START_NS + 2);
    write_dbn(&path, Compression::None, &records);
    let plan = open_replay(&path, records.len() as u64)
        .qualify_unbound_development()
        .unwrap();

    let pass = plan.open_revalidation_pass().unwrap();
    assert!(matches!(
        pass.finish(),
        Err(XnasReplayErrorV1::CannotFinishRevalidationBeforeEof)
    ));

    let mut pass = plan.open_revalidation_pass().unwrap();
    while let Some(observation) = pass.next_observation().unwrap() {
        let _ = observation;
    }
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"terminal-mutation")
        .unwrap();
    assert!(matches!(
        pass.finish(),
        Err(XnasReplayErrorV1::Boundary(
            mbo_lob_reconstructor::StrictBoundaryErrorV1::SourceRuntimeIdentityChanged
        ))
    ));
}

#[test]
fn revalidation_stream_error_is_returned_once_then_fused_and_cannot_finish() {
    const TRADE_COUNT: usize = 30_000;
    const CHANGED_TRADE: usize = 29_000;

    let dir = tempdir().unwrap();
    let path = dir.path().join("two-pass-fused-error-a.dbn");
    let alternate_path = dir.path().join("two-pass-fused-error-b.dbn");
    let records = long_trade_stream_records(TRADE_COUNT, None);
    let mut alternate_records = records.clone();
    alternate_records[CHANGED_TRADE + 1] = message_for_instrument(
        202,
        b'T',
        b'N',
        0,
        ASK,
        1,
        u32::try_from(CHANGED_TRADE + 1).unwrap(),
        flags::LAST,
        START_NS + 10 + CHANGED_TRADE as u64,
        START_NS + 100 + CHANGED_TRADE as u64,
    );
    write_dbn(&path, Compression::None, &records);
    write_dbn(&alternate_path, Compression::None, &alternate_records);
    let source_bytes = std::fs::read(&path).unwrap();
    let alternate_bytes = std::fs::read(&alternate_path).unwrap();
    let differences = exact_byte_differences(&source_bytes, &alternate_bytes);
    assert!(!differences.is_empty());
    assert!(differences.iter().all(|(offset, _, _)| *offset > 1_000_000));

    let plan = open_replay(&path, records.len() as u64)
        .qualify_unbound_development()
        .unwrap();
    let mut pass = plan.open_revalidation_pass().unwrap();
    let mut source = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    apply_byte_differences(&mut source, &differences, true);
    let first_error = loop {
        match pass.next_observation() {
            Ok(Some(_)) => {}
            Ok(None) => panic!("late unmapped identity was mistaken for verified EOF"),
            Err(error) => break error,
        }
    };
    assert!(matches!(first_error, XnasReplayErrorV1::PrefixFailed(_)));
    assert!(matches!(
        pass.next_observation(),
        Err(XnasReplayErrorV1::CannotContinueFailedRevalidation)
    ));
    assert!(matches!(
        pass.finish(),
        Err(XnasReplayErrorV1::CannotFinishFailedReplay)
    ));
}

#[test]
fn runtime_identity_detects_late_same_length_mutation_restored_before_posthash() {
    const TRADE_COUNT: usize = 30_000;
    const CHANGED_TRADE: usize = 29_000;

    let dir = tempdir().unwrap();
    let source_path = dir.path().join("two-pass-transient-a.dbn");
    let alternate_path = dir.path().join("two-pass-transient-b.dbn");
    let source_records = long_trade_stream_records(TRADE_COUNT, None);
    let alternate_records = long_trade_stream_records(TRADE_COUNT, Some((CHANGED_TRADE, 7)));
    write_dbn(&source_path, Compression::None, &source_records);
    write_dbn(&alternate_path, Compression::None, &alternate_records);
    let source_bytes = std::fs::read(&source_path).unwrap();
    let alternate_bytes = std::fs::read(&alternate_path).unwrap();
    let differences = exact_byte_differences(&source_bytes, &alternate_bytes);
    assert!(!differences.is_empty());
    assert!(differences.iter().all(|(offset, _, _)| *offset > 1_000_000));

    let plan = open_replay(&source_path, source_records.len() as u64)
        .qualify_unbound_development()
        .unwrap();
    let mut pass = plan.open_revalidation_pass().unwrap();
    let mut source = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&source_path)
        .unwrap();
    apply_byte_differences(&mut source, &differences, true);

    while pass.next_observation().unwrap().is_some() {}

    apply_byte_differences(&mut source, &differences, false);
    drop(source);
    assert_eq!(std::fs::read(&source_path).unwrap(), source_bytes);

    assert!(matches!(
        pass.finish(),
        Err(XnasReplayErrorV1::Boundary(
            mbo_lob_reconstructor::StrictBoundaryErrorV1::SourceRuntimeIdentityChanged
        ))
    ));
}

#[test]
fn selected_ordinal_beyond_decoded_population_is_explicitly_unobserved() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("selected-beyond-eof.dbn");
    let records = conformance_records(START_NS + 1, START_NS + 2);
    write_dbn(&path, Compression::None, &records);
    let beyond_eof = records.len() as u64 + 1;
    let selected = BTreeSet::from([beyond_eof]);
    let run = open_replay(&path, records.len() as u64)
        .run_to_eof_with_selected_ordinals(&selected)
        .unwrap();
    assert!(run.traces().is_empty());
    assert_eq!(run.selected_ordinal_dispositions().len(), 1);
    let disposition = &run.selected_ordinal_dispositions()[0];
    assert_eq!(disposition.raw_ordinal(), beyond_eof);
    assert!(!disposition.decoded_from_source());
    assert!(disposition.roles().is_empty());
}

#[test]
fn cross_identity_records_never_witness_but_do_raise_global_availability() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cross-identity.dbn");
    let records = vec![
        initial_clear(START_NS + 1, START_NS + 2),
        message_for_instrument(
            202,
            b'R',
            b'N',
            0,
            UNDEF_PRICE,
            0,
            0,
            flags::BAD_TS_RECV,
            START_NS + 3,
            START_NS + 4,
        ),
        message_for_instrument(
            101,
            b'A',
            b'B',
            1,
            BID,
            10,
            10,
            flags::LAST,
            START_NS + 10,
            START_NS + 100,
        ),
        message_for_instrument(
            202,
            b'A',
            b'B',
            2,
            BID - 1_000_000_000,
            20,
            20,
            flags::LAST,
            START_NS + 20,
            START_NS + 1_000,
        ),
        message_for_instrument(
            101,
            b'T',
            b'A',
            0,
            BID,
            1,
            11,
            flags::LAST,
            START_NS + 30,
            START_NS + 110,
        ),
        message_for_instrument(
            202,
            b'T',
            b'A',
            0,
            BID,
            1,
            21,
            flags::LAST,
            START_NS + 40,
            START_NS + 1_010,
        ),
    ];
    write_dbn_with_metadata(&path, Compression::None, &records, &two_identity_metadata());
    let run = run_with_all_traces(
        open_two_identity_replay(&path, records.len() as u64),
        records.len() as u64,
    );
    assert_eq!(run.traces().len(), 2);
    assert_eq!(run.traces()[0].identity().instrument_id(), 101);
    assert_eq!(run.traces()[0].terminal_source_ordinal(), 3);
    assert_eq!(run.traces()[0].witness_source_ordinal(), 5);
    assert_eq!(run.traces()[0].effective_available_ns(), START_NS + 1_000);
    assert_eq!(run.traces()[1].identity().instrument_id(), 202);
    assert_eq!(run.traces()[1].terminal_source_ordinal(), 4);
    assert_eq!(run.traces()[1].witness_source_ordinal(), 6);
    assert_eq!(run.traces()[1].effective_available_ns(), START_NS + 1_010);
    assert_eq!(run.receipt().counts().initial_clear_controls, 2);
    assert_eq!(run.receipt().counts().quarantined_records, 2);
    assert_eq!(run.receipt().counts().eof_tail_quarantined_records, 2);
    assert_eq!(run.receipt().counts().tail_quarantine_incidents, 2);
    assert_eq!(
        run.receipt().counts().tail_quarantine_incidents,
        run.receipt()
            .identities()
            .iter()
            .filter(|identity| identity.eof_tail_quarantine().is_some())
            .count() as u64
    );
    assert!(run.receipt().counts().quarantine_reasons_reconcile());
    assert_eq!(run.receipt().identities().len(), 2);
}

#[test]
fn decoded_rejection_advances_global_causal_watermark_without_cross_identity_quarantine() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("rejected-clock-causality.dbn");
    let records = vec![
        initial_clear(START_NS + 1, START_NS + 2),
        message_for_instrument(
            202,
            b'R',
            b'N',
            0,
            UNDEF_PRICE,
            0,
            0,
            flags::BAD_TS_RECV,
            START_NS + 3,
            START_NS + 4,
        ),
        message_for_instrument(
            101,
            b'A',
            b'B',
            1,
            BID,
            10,
            10,
            flags::LAST,
            START_NS + 10,
            START_NS + 100,
        ),
        message_for_instrument(
            202,
            b'X',
            b'B',
            77,
            BID,
            10,
            20,
            flags::LAST,
            START_NS + 20,
            START_NS + 1_000,
        ),
        message_for_instrument(
            101,
            b'T',
            b'A',
            0,
            BID,
            1,
            11,
            flags::LAST,
            START_NS + 30,
            START_NS + 110,
        ),
        message_for_instrument(
            202,
            b'R',
            b'N',
            0,
            UNDEF_PRICE,
            0,
            30,
            0,
            START_NS + 40,
            START_NS + 1_010,
        ),
        message_for_instrument(
            202,
            b'A',
            b'A',
            2,
            ASK,
            20,
            30,
            flags::LAST,
            START_NS + 40,
            START_NS + 1_010,
        ),
        message_for_instrument(
            202,
            b'T',
            b'B',
            0,
            ASK,
            1,
            31,
            flags::LAST,
            START_NS + 50,
            START_NS + 1_020,
        ),
    ];
    write_dbn_with_metadata(&path, Compression::None, &records, &two_identity_metadata());
    let run = run_with_all_traces(
        open_two_identity_replay(&path, records.len() as u64),
        records.len() as u64,
    );

    let identity_a_trace = run
        .traces()
        .iter()
        .find(|trace| trace.identity().instrument_id() == 101)
        .expect("identity A must qualify");
    assert_eq!(identity_a_trace.terminal_source_ordinal(), 3);
    assert_eq!(identity_a_trace.witness_source_ordinal(), 5);
    assert_eq!(identity_a_trace.effective_available_ns(), START_NS + 1_000);

    let identity_a = run
        .receipt()
        .identities()
        .iter()
        .find(|identity| identity.identity().instrument_id() == 101)
        .unwrap();
    let identity_b = run
        .receipt()
        .identities()
        .iter()
        .find(|identity| identity.identity().instrument_id() == 202)
        .unwrap();
    assert!(identity_a.semantic_quarantines().is_empty());
    assert_eq!(identity_b.semantic_quarantines().len(), 1);
    assert_eq!(identity_b.rejected_record_quarantines().len(), 1);
    assert_eq!(identity_b.recovery_qualifications().len(), 1);
    assert_eq!(
        identity_b.rejected_record_quarantines()[0]
            .raw()
            .raw_ordinal,
        4
    );
}

#[test]
fn one_identity_quarantine_does_not_contaminate_an_interleaved_identity() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cross-identity-quarantine.dbn");
    let records = vec![
        initial_clear(START_NS + 1, START_NS + 2),
        message_for_instrument(
            202,
            b'R',
            b'N',
            0,
            UNDEF_PRICE,
            0,
            0,
            flags::BAD_TS_RECV,
            START_NS + 3,
            START_NS + 4,
        ),
        message_for_instrument(
            101,
            b'A',
            b'B',
            1,
            BID,
            10,
            10,
            flags::LAST,
            START_NS + 10,
            START_NS + 100,
        ),
        message_for_instrument(
            202,
            b'A',
            b'A',
            2,
            ASK,
            20,
            20,
            flags::LAST,
            START_NS + 20,
            START_NS + 105,
        ),
        message_for_instrument(
            101,
            b'A',
            b'B',
            3,
            BID - 1_000_000_000,
            10,
            10,
            0,
            START_NS + 10,
            START_NS + 100,
        ),
        message_for_instrument(
            202,
            b'T',
            b'B',
            0,
            ASK,
            1,
            21,
            flags::LAST,
            START_NS + 30,
            START_NS + 110,
        ),
        message_for_instrument(
            101,
            b'R',
            b'N',
            0,
            UNDEF_PRICE,
            0,
            30,
            0,
            START_NS + 40,
            START_NS + 115,
        ),
        message_for_instrument(
            101,
            b'A',
            b'B',
            4,
            BID,
            25,
            30,
            flags::LAST,
            START_NS + 40,
            START_NS + 115,
        ),
        message_for_instrument(
            101,
            b'T',
            b'A',
            0,
            BID,
            1,
            31,
            flags::LAST,
            START_NS + 50,
            START_NS + 120,
        ),
        message_for_instrument(
            202,
            b'T',
            b'B',
            0,
            ASK,
            1,
            22,
            flags::LAST,
            START_NS + 60,
            START_NS + 125,
        ),
    ];
    write_dbn_with_metadata(&path, Compression::None, &records, &two_identity_metadata());
    let run = run_with_all_traces(
        open_two_identity_replay(&path, records.len() as u64),
        records.len() as u64,
    );
    assert_eq!(run.traces().len(), 3);
    let one = run
        .receipt()
        .identities()
        .iter()
        .find(|identity| identity.identity().instrument_id() == 101)
        .unwrap();
    let two = run
        .receipt()
        .identities()
        .iter()
        .find(|identity| identity.identity().instrument_id() == 202)
        .unwrap();
    assert_eq!(one.semantic_quarantines().len(), 1);
    assert_eq!(
        one.semantic_quarantines()[0].candidate_source_ordinals(),
        [3, 5]
    );
    assert_eq!(
        one.semantic_quarantines()[0]
            .recovery()
            .unwrap()
            .reset_source_ordinal(),
        7
    );
    assert_eq!(one.committed_envelopes(), 1);
    assert_eq!(two.semantic_quarantines().len(), 0);
    assert_eq!(two.recovery_qualifications().len(), 0);
    assert_eq!(two.committed_envelopes(), 2);
    assert_eq!(two.validity_epochs().len(), 1);
}

#[test]
fn initial_control_timestamps_cannot_change_availability_or_book_values() {
    let mut outcomes = Vec::new();
    for (event_ns, recv_ns) in [
        (START_NS + 1, START_NS + 2),
        (START_NS + 999, START_NS + 777),
    ] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("initial-time.dbn");
        let records = conformance_records(event_ns, recv_ns);
        write_dbn(&path, Compression::None, &records);
        let selected = BTreeSet::from([2]);
        let run = open_replay(&path, records.len() as u64)
            .run_to_eof_with_selected_ordinals(&selected)
            .unwrap();
        let first = &run.traces()[0];
        outcomes.push((
            first.endpoint_ns(),
            first.witness_ts_recv(),
            first.effective_available_ns(),
            first.book().snapshot().best_bid().unwrap().price_raw(),
            first.book().snapshot().best_bid().unwrap().aggregate_size(),
        ));
    }
    assert_eq!(outcomes[0], outcomes[1]);
}

#[test]
fn every_initial_control_signature_field_is_fail_loud() {
    let base = initial_clear(START_NS + 1, START_NS + 2);
    let mut mutations = Vec::new();
    let mut value = base.clone();
    value.hd.publisher_id = 3;
    mutations.push(value);
    let mut value = base.clone();
    value.hd.instrument_id = 999;
    mutations.push(value);
    let mut value = base.clone();
    value.channel_id = 1;
    mutations.push(value);
    let mut value = base.clone();
    value.sequence = 1;
    mutations.push(value);
    let mut value = base.clone();
    value.order_id = 1;
    mutations.push(value);
    let mut value = base.clone();
    value.price = 0;
    mutations.push(value);
    let mut value = base.clone();
    value.size = 1;
    mutations.push(value);
    let mut value = base.clone();
    value.ts_in_delta = 1;
    mutations.push(value);
    let mut value = base.clone();
    value.flags = flags::LAST.into();
    mutations.push(value);
    let mut value = base.clone();
    value.action = b'N' as _;
    mutations.push(value);
    let mut value = base;
    value.side = b'B' as _;
    mutations.push(value);

    for (index, mutation) in mutations.into_iter().enumerate() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(format!("initial-mutation-{index}.dbn"));
        write_dbn(&path, Compression::None, &[mutation]);
        let stream = StrictDbnLoaderV1::open(
            expectation(&path, 1),
            PublisherPolicyIdV1::XnasItchHistorical,
        )
        .unwrap();
        let replay = StrictXnasReplayV1::from_strict_stream(stream, replay_config()).unwrap();
        assert!(replay.run_to_eof().is_err(), "mutation {index}");
    }
}

#[test]
fn eof_cannot_witness_the_first_envelope() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("eof.dbn");
    let records = vec![
        initial_clear(START_NS + 1, START_NS + 2),
        message(
            b'A',
            b'B',
            1,
            BID,
            100,
            10,
            flags::LAST,
            START_NS + 10,
            START_NS + 100,
        ),
    ];
    write_dbn(&path, Compression::None, &records);
    let replay = open_replay(&path, 2);
    let error = replay.run_to_eof().unwrap_err();
    match error {
        XnasReplayErrorV1::TerminalDisqualified(diagnostic) => {
            assert!(matches!(
                diagnostic.reason(),
                XnasTerminalDisqualificationReasonV1::IncompleteInitialization(_)
            ));
            assert_eq!(diagnostic.source().decoded_records(), 2);
            assert_eq!(diagnostic.authority(), "nonconsumable_terminal_diagnostic");
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn identity_local_envelope_and_book_failures_quarantine_then_recover_exactly() {
    for (name, kind) in [
        (
            "last-to-non-last",
            RecoverableFailureFixtureV1::LastToNonLast,
        ),
        ("missing-modify", RecoverableFailureFixtureV1::MissingModify),
    ] {
        let dir = tempdir().unwrap();
        let path = dir.path().join(format!("{name}.dbn"));
        let records = recoverable_failure_records(kind);
        write_dbn(&path, Compression::None, &records);
        let run = run_with_all_traces(
            open_replay(&path, records.len() as u64),
            records.len() as u64,
        );
        assert_eq!(run.traces().len(), 3);
        assert_eq!(run.traces()[0].terminal_source_ordinal(), 2);
        assert_eq!(run.traces()[1].terminal_source_ordinal(), 3);
        assert!(run.traces()[2].is_recovery());
        assert_eq!(run.traces()[2].events().len(), 2);
        assert_eq!(run.traces()[2].terminal_source_ordinal(), 7);
        assert_eq!(run.traces()[2].witness_source_ordinal(), 8);
        assert!(run.traces()[2].book().snapshot().best_bid().is_none());
        assert_eq!(
            run.traces()[2]
                .book()
                .snapshot()
                .best_ask()
                .unwrap()
                .aggregate_size(),
            25
        );

        let receipt = run.receipt();
        assert_eq!(receipt.source().decoded_records(), 8);
        assert_eq!(receipt.source().accepted_records(), 8);
        assert_eq!(receipt.source().rejected_records(), 0);
        let counts = receipt.counts();
        assert_eq!(counts.raw_records_ingested, 8);
        assert_eq!(counts.initial_clear_controls, 1);
        assert_eq!(counts.completed_envelope_members, 4);
        assert_eq!(counts.pending_members, 0);
        assert_eq!(
            counts.semantic_candidate_quarantined_records,
            if name == "missing-modify" { 1 } else { 2 }
        );
        assert_eq!(
            counts.semantic_while_invalid_quarantined_records,
            if name == "missing-modify" { 1 } else { 0 }
        );
        assert_eq!(counts.semantic_quarantined_records, 2);
        assert_eq!(counts.semantic_quarantine_incidents, 1);
        assert_eq!(counts.eof_tail_quarantined_records, 1);
        assert_eq!(counts.reset_boundary_quarantined_records, 0);
        assert_eq!(counts.quarantined_records, 3);
        assert_eq!(counts.completed_update_envelopes, 3);
        assert_eq!(counts.staged_book_updates, 3);
        assert_eq!(counts.venue_sequence_blocks, 3);
        assert_eq!(counts.book_commands_committed, 4);
        assert_eq!(counts.private_book_resets, 2);
        assert_eq!(counts.reset_recovery_candidates, 1);
        assert_eq!(counts.decoded_semantic_rejections, 0);
        assert!(counts.population_reconciles());
        assert!(counts.quarantine_reasons_reconcile());
        assert!(counts.semantic_population_reconciles());

        let identity = &receipt.identities()[0];
        assert_eq!(identity.semantic_quarantines().len(), 1);
        let incident = &identity.semantic_quarantines()[0];
        assert_eq!(incident.detected_at().raw_ordinal, 5);
        let expected_watermark = match kind {
            RecoverableFailureFixtureV1::LastToNonLast => START_NS + 120,
            RecoverableFailureFixtureV1::MissingModify => START_NS + 130,
        };
        assert_eq!(
            incident.global_receive_watermark_ns(),
            Some(expected_watermark)
        );
        assert_eq!(incident.recovery().unwrap().reset_source_ordinal(), 6);
        match kind {
            RecoverableFailureFixtureV1::LastToNonLast => {
                assert_eq!(incident.candidate_source_ordinals(), [4, 5]);
                assert_eq!(incident.offending_candidate_source_ordinal(), Some(5));
                assert!(incident.while_invalid_records().is_empty());
                assert!(matches!(
                    incident.reason(),
                    XnasQuarantineReasonV1::Envelope {
                        source: mbo_lob_reconstructor::XnasEnvelopeErrorV1::LastToNonLast
                    }
                ));
            }
            RecoverableFailureFixtureV1::MissingModify => {
                assert_eq!(incident.candidate_source_ordinals(), [4]);
                assert_eq!(incident.offending_candidate_source_ordinal(), Some(4));
                assert_eq!(incident.while_invalid_records().len(), 1);
                assert_eq!(incident.while_invalid_records()[0].raw().raw_ordinal, 5);
                assert!(matches!(
                    incident.while_invalid_records()[0].reason(),
                    XnasQuarantineReasonV1::ClosureWitnessAfterFailedTransaction
                ));
                assert!(matches!(
                    incident.reason(),
                    XnasQuarantineReasonV1::Book {
                        source: BookTransactionErrorV1::MissingModify {
                            order_id: 999,
                            raw_ordinal: 4
                        }
                    }
                ));
            }
        }

        let epochs = identity.validity_epochs();
        assert_eq!(epochs.len(), 2);
        assert_eq!(epochs[0].qualification().terminal_source_ordinal(), 2);
        assert_eq!(epochs[0].qualification().witness_source_ordinal(), 3);
        assert_eq!(epochs[0].last_committed_terminal_source_ordinal(), 3);
        let first_invalidation = epochs[0].invalidation();
        assert_eq!(first_invalidation.first_ineligible_source_ordinal(), 4);
        assert_eq!(first_invalidation.detected_at_source_ordinal(), Some(5));
        assert!(matches!(
            first_invalidation.reason(),
            XnasValidityInvalidationReasonV1::SemanticQuarantine { incident_index: 0 }
        ));
        assert_eq!(
            epochs[1].qualification().recovery_reset_source_ordinal(),
            Some(6)
        );
        assert_eq!(epochs[1].qualification().terminal_source_ordinal(), 7);
        assert_eq!(epochs[1].qualification().witness_source_ordinal(), 8);
        let second_invalidation = epochs[1].invalidation();
        assert_eq!(second_invalidation.first_ineligible_source_ordinal(), 8);
        assert_eq!(second_invalidation.detected_at_source_ordinal(), None);
        assert!(matches!(
            second_invalidation.reason(),
            XnasValidityInvalidationReasonV1::EofTail {
                reason: XnasEofTailReasonV1::TerminalCandidateWithoutWitness
            }
        ));
        assert_eq!(
            epochs[1].last_committed_book_state_sha256(),
            identity.last_committed_book_state_sha256()
        );
        assert_eq!(
            epochs[1].last_transition_chain_sha256(),
            identity.transition_chain_sha256()
        );
    }
}

#[test]
fn decoded_rejections_preserve_raw_reason_stage_incident_phase_and_recovery() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("decoded-rejections.dbn");
    let mut maybe_bad = message(
        b'A',
        b'B',
        3,
        BID - 1_000_000_000,
        10,
        13,
        flags::LAST,
        START_NS + 40,
        START_NS + 130,
    );
    maybe_bad.flags = (flags::LAST | flags::MAYBE_BAD_BOOK).into();
    let records = vec![
        initial_clear(START_NS + 1, START_NS + 2),
        message(
            b'A',
            b'B',
            1,
            BID,
            100,
            10,
            flags::LAST,
            START_NS + 10,
            START_NS + 100,
        ),
        message(
            b'A',
            b'A',
            2,
            ASK,
            50,
            11,
            flags::LAST,
            START_NS + 20,
            START_NS + 110,
        ),
        message(
            b'X',
            b'B',
            77,
            BID,
            10,
            12,
            flags::LAST,
            START_NS + 30,
            START_NS + 120,
        ),
        maybe_bad,
        message(
            b'R',
            b'N',
            0,
            UNDEF_PRICE,
            0,
            20,
            0,
            START_NS + 50,
            START_NS + 140,
        ),
        message(
            b'A',
            b'A',
            5,
            ASK,
            25,
            20,
            flags::LAST,
            START_NS + 50,
            START_NS + 140,
        ),
        message(
            b'T',
            b'B',
            0,
            ASK,
            1,
            30,
            flags::LAST,
            START_NS + 60,
            START_NS + 150,
        ),
    ];
    write_dbn(&path, Compression::None, &records);
    let run = run_with_all_traces(
        open_replay(&path, records.len() as u64),
        records.len() as u64,
    );
    let receipt = run.receipt();
    assert_eq!(receipt.source().decoded_records(), 8);
    assert_eq!(receipt.source().accepted_records(), 6);
    assert_eq!(receipt.source().rejected_records(), 2);
    assert_eq!(receipt.counts().decoded_semantic_rejections, 2);
    assert_eq!(receipt.counts().semantic_candidate_quarantined_records, 2);
    assert_eq!(
        receipt.counts().semantic_while_invalid_quarantined_records,
        1
    );
    assert_eq!(receipt.counts().eof_tail_quarantined_records, 1);
    assert_eq!(receipt.counts().quarantined_records, 4);
    assert_eq!(receipt.counts().completed_envelope_members, 3);
    assert_eq!(receipt.counts().book_commands_committed, 3);
    assert_eq!(receipt.counts().private_book_resets, 2);

    let identity = &receipt.identities()[0];
    let incident = &identity.semantic_quarantines()[0];
    assert_eq!(incident.candidate_source_ordinals(), [3, 4]);
    assert_eq!(incident.while_invalid_records().len(), 1);
    assert_eq!(incident.while_invalid_records()[0].raw().raw_ordinal, 5);
    assert_eq!(incident.recovery().unwrap().reset_source_ordinal(), 6);
    let rejected = identity.rejected_record_quarantines();
    assert_eq!(rejected.len(), 2);
    assert_eq!(rejected[0].raw().raw_ordinal, 4);
    assert_eq!(rejected[0].raw().action_raw, b'X');
    assert_eq!(
        rejected[0].failure().reason,
        ValidationReasonV1::UnknownAction(b'X')
    );
    assert_eq!(
        rejected[0].stage(),
        VerifiedRejectionStageV1::UniversalValidation
    );
    assert_eq!(rejected[0].identity_incident_index(), 0);
    assert_eq!(
        rejected[0].phase(),
        XnasRejectedRecordPhaseV1::CandidateTrigger
    );
    assert_eq!(rejected[1].raw().raw_ordinal, 5);
    assert_eq!(
        rejected[1].failure().reason,
        ValidationReasonV1::MaybeBadBook
    );
    assert_eq!(
        rejected[1].stage(),
        VerifiedRejectionStageV1::FullOrderBookPolicy
    );
    assert_eq!(rejected[1].identity_incident_index(), 0);
    assert_eq!(rejected[1].phase(), XnasRejectedRecordPhaseV1::WhileInvalid);

    assert!(matches!(
        selected_roles(&run, 1),
        [XnasSelectedOrdinalRoleV1::InitialClearControl { .. }]
    ));
    assert!(matches!(
        selected_roles(&run, 3),
        [
            XnasSelectedOrdinalRoleV1::ClosureWitness {
                trace_index: 0,
                terminal_source_ordinal: 2,
                ..
            },
            XnasSelectedOrdinalRoleV1::SemanticQuarantinedMember {
                incident_index: 0,
                detected_at_source_ordinal: 4,
                ..
            }
        ]
    ));
    assert!(matches!(
        selected_roles(&run, 4),
        [
            XnasSelectedOrdinalRoleV1::SemanticQuarantinedMember {
                incident_index: 0,
                detected_at_source_ordinal: 4,
                ..
            },
            XnasSelectedOrdinalRoleV1::DecodedSemanticRejection {
                reason: ValidationReasonV1::UnknownAction(b'X'),
                ..
            }
        ]
    ));
    assert!(matches!(
        selected_roles(&run, 5),
        [
            XnasSelectedOrdinalRoleV1::SemanticQuarantinedMember {
                incident_index: 0,
                detected_at_source_ordinal: 4,
                ..
            },
            XnasSelectedOrdinalRoleV1::DecodedSemanticRejection {
                reason: ValidationReasonV1::MaybeBadBook,
                ..
            }
        ]
    ));
    assert!(matches!(
        selected_roles(&run, 6),
        [XnasSelectedOrdinalRoleV1::CompletedEnvelopeMember {
            trace_index: 1,
            terminal_source_ordinal: 7,
            ..
        }]
    ));
    assert!(matches!(
        selected_roles(&run, 8),
        [
            XnasSelectedOrdinalRoleV1::ClosureWitness {
                trace_index: 1,
                terminal_source_ordinal: 7,
                ..
            },
            XnasSelectedOrdinalRoleV1::EofTailQuarantinedMember {
                reason: XnasEofTailReasonV1::TerminalCandidateWithoutWitness,
                ..
            }
        ]
    ));
}

#[test]
fn invalid_first_condition_can_only_qualify_through_witnessed_reset_recovery() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("invalid-first-then-recovery.dbn");
    let records = vec![
        message(
            b'A',
            b'B',
            1,
            BID,
            100,
            10,
            flags::LAST,
            START_NS + 10,
            START_NS + 100,
        ),
        message(
            b'R',
            b'N',
            0,
            UNDEF_PRICE,
            0,
            20,
            0,
            START_NS + 20,
            START_NS + 110,
        ),
        message(
            b'A',
            b'A',
            2,
            ASK,
            25,
            20,
            flags::LAST,
            START_NS + 20,
            START_NS + 110,
        ),
        message(
            b'T',
            b'B',
            0,
            ASK,
            1,
            30,
            flags::LAST,
            START_NS + 30,
            START_NS + 120,
        ),
    ];
    write_dbn(&path, Compression::None, &records);
    let run = run_with_all_traces(
        open_replay(&path, records.len() as u64),
        records.len() as u64,
    );
    assert_eq!(run.traces().len(), 1);
    assert!(run.traces()[0].is_recovery());
    assert_eq!(run.traces()[0].validity_epoch_index(), 1);
    assert_eq!(run.traces()[0].book().reset_epoch(), 2);
    assert_eq!(run.traces()[0].events().len(), 2);
    assert_eq!(run.traces()[0].terminal_source_ordinal(), 3);
    assert_eq!(run.traces()[0].witness_source_ordinal(), 4);
    let receipt = run.receipt();
    assert_eq!(receipt.counts().initial_clear_controls, 0);
    assert_eq!(receipt.counts().semantic_candidate_quarantined_records, 1);
    assert_eq!(receipt.counts().completed_envelope_members, 2);
    assert_eq!(receipt.counts().eof_tail_quarantined_records, 1);
    assert_eq!(receipt.counts().quarantined_records, 2);
    assert_eq!(receipt.counts().private_book_resets, 1);
    assert_eq!(receipt.counts().reset_recovery_candidates, 1);
    let identity = &receipt.identities()[0];
    assert!(identity.initial_clear_control().is_none());
    assert_eq!(identity.semantic_quarantines().len(), 1);
    assert_eq!(
        identity.semantic_quarantines()[0].candidate_source_ordinals(),
        [1]
    );
    assert!(matches!(
        identity.semantic_quarantines()[0].reason(),
        XnasQuarantineReasonV1::InvalidInitialCondition
    ));
    assert_eq!(
        identity.semantic_quarantines()[0]
            .recovery()
            .unwrap()
            .reset_source_ordinal(),
        2
    );
    assert_eq!(identity.validity_epochs().len(), 1);
    assert_eq!(
        identity.validity_epochs()[0]
            .qualification()
            .recovery_reset_source_ordinal(),
        Some(2)
    );
}

#[test]
fn later_initial_signature_invalidates_and_only_distinct_clean_reset_recovers() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("later-initial.dbn");
    let records = vec![
        initial_clear(START_NS + 1, START_NS + 2),
        message(
            b'A',
            b'B',
            1,
            BID,
            100,
            10,
            flags::LAST,
            START_NS + 10,
            START_NS + 100,
        ),
        message(
            b'T',
            b'A',
            0,
            BID,
            1,
            11,
            flags::LAST,
            START_NS + 20,
            START_NS + 110,
        ),
        initial_clear(START_NS + 30, START_NS + 120),
        message(
            b'A',
            b'B',
            2,
            BID - 1_000_000_000,
            10,
            12,
            flags::LAST,
            START_NS + 40,
            START_NS + 130,
        ),
        message(
            b'R',
            b'N',
            0,
            UNDEF_PRICE,
            0,
            20,
            0,
            START_NS + 50,
            START_NS + 140,
        ),
        message(
            b'A',
            b'A',
            3,
            ASK,
            25,
            20,
            flags::LAST,
            START_NS + 50,
            START_NS + 140,
        ),
        message(
            b'T',
            b'B',
            0,
            ASK,
            1,
            30,
            flags::LAST,
            START_NS + 60,
            START_NS + 150,
        ),
    ];
    write_dbn(&path, Compression::None, &records);
    let run = run_with_all_traces(
        open_replay(&path, records.len() as u64),
        records.len() as u64,
    );
    assert_eq!(run.traces().len(), 2);
    assert!(run.traces()[1].is_recovery());
    let receipt = run.receipt();
    assert_eq!(receipt.counts().initial_clear_controls, 1);
    assert_eq!(receipt.counts().completed_envelope_members, 3);
    assert_eq!(receipt.counts().semantic_candidate_quarantined_records, 2);
    assert_eq!(
        receipt.counts().semantic_while_invalid_quarantined_records,
        1
    );
    assert_eq!(receipt.counts().eof_tail_quarantined_records, 1);
    assert_eq!(receipt.counts().quarantined_records, 4);
    assert_eq!(receipt.counts().private_book_resets, 2);
    let identity = &receipt.identities()[0];
    assert_eq!(identity.semantic_quarantines().len(), 1);
    let incident = &identity.semantic_quarantines()[0];
    assert_eq!(incident.candidate_source_ordinals(), [3, 4]);
    assert_eq!(incident.detected_at().raw_ordinal, 4);
    assert_eq!(incident.offending_candidate_source_ordinal(), Some(4));
    assert!(matches!(
        incident.reason(),
        XnasQuarantineReasonV1::LaterInitialClearControl
    ));
    assert_eq!(incident.while_invalid_records()[0].raw().raw_ordinal, 5);
    assert_eq!(incident.recovery().unwrap().reset_source_ordinal(), 6);
}

#[test]
fn ordinary_n_and_identity_receive_time_regression_are_terminally_quarantined() {
    let cases = [
        message(
            b'N',
            b'N',
            0,
            UNDEF_PRICE,
            0,
            11,
            flags::LAST,
            START_NS + 20,
            START_NS + 110,
        ),
        message(
            b'A',
            b'A',
            2,
            ASK,
            50,
            11,
            flags::LAST,
            START_NS + 20,
            START_NS + 90,
        ),
    ];
    for (index, offending) in cases.into_iter().enumerate() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(format!("ordinary-failure-{index}.dbn"));
        let records = vec![
            initial_clear(START_NS + 1, START_NS + 2),
            message(
                b'A',
                b'B',
                1,
                BID,
                100,
                10,
                flags::LAST,
                START_NS + 10,
                START_NS + 100,
            ),
            offending,
        ];
        write_dbn(&path, Compression::None, &records);
        let replay = open_replay(&path, records.len() as u64);
        let error = replay.run_to_eof().unwrap_err();
        let diagnostic = match error {
            XnasReplayErrorV1::TerminalDisqualified(value) => value,
            other => panic!("expected EOF-sealed disqualification, got {other}"),
        };
        assert_eq!(diagnostic.source().decoded_records(), 3);
        assert_eq!(diagnostic.counts().raw_records_ingested, 3);
        assert_eq!(diagnostic.counts().initial_clear_controls, 1);
        assert_eq!(
            diagnostic.counts().semantic_candidate_quarantined_records,
            2
        );
        assert_eq!(diagnostic.counts().quarantined_records, 2);
        assert_eq!(diagnostic.counts().pending_members, 0);
        assert!(matches!(
            diagnostic.reason(),
            XnasTerminalDisqualificationReasonV1::IncompleteInitialization(_)
        ));
        let identity = &diagnostic.identities()[0];
        assert_eq!(
            identity.terminal_status(),
            XnasTerminalIdentityStatusV1::NeverQualified
        );
        assert!(identity.validity_epochs().is_empty());
        let incident = &identity.semantic_quarantines()[0];
        assert_eq!(incident.candidate_source_ordinals(), [2, 3]);
        assert_eq!(incident.detected_at().raw_ordinal, 3);
        assert_eq!(incident.offending_candidate_source_ordinal(), Some(3));
        match index {
            0 => assert!(matches!(
                incident.reason(),
                XnasQuarantineReasonV1::UnsupportedOrdinaryControl
            )),
            1 => assert!(matches!(
                incident.reason(),
                XnasQuarantineReasonV1::IdentityReceiveTimeRegression {
                    previous,
                    actual
                } if *previous == START_NS + 100 && *actual == START_NS + 90
            )),
            _ => unreachable!(),
        }
    }
}

#[test]
fn ordinary_bad_receive_time_and_out_of_day_event_are_exactly_quarantined() {
    let cases = [
        (
            "bad-receive-time",
            message(
                b'A',
                b'A',
                2,
                ASK,
                50,
                11,
                flags::LAST | flags::BAD_TS_RECV,
                START_NS + 20,
                START_NS + 110,
            ),
            START_NS + 100,
        ),
        (
            "outside-source-day",
            message(
                b'A',
                b'A',
                2,
                ASK,
                50,
                11,
                flags::LAST,
                END_NS,
                START_NS + 110,
            ),
            START_NS + 110,
        ),
    ];

    for (index, (name, offending, expected_watermark)) in cases.into_iter().enumerate() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(format!("{name}.dbn"));
        let records = vec![
            initial_clear(START_NS + 1, START_NS + 2),
            message(
                b'A',
                b'B',
                1,
                BID,
                100,
                10,
                flags::LAST,
                START_NS + 10,
                START_NS + 100,
            ),
            offending,
        ];
        write_dbn(&path, Compression::None, &records);
        let error = open_replay(&path, records.len() as u64)
            .run_to_eof()
            .unwrap_err();
        let diagnostic = match error {
            XnasReplayErrorV1::TerminalDisqualified(value) => value,
            other => panic!("expected EOF-sealed disqualification, got {other}"),
        };
        let incident = &diagnostic.identities()[0].semantic_quarantines()[0];
        assert_eq!(incident.candidate_source_ordinals(), [2, 3]);
        assert_eq!(incident.detected_at().raw_ordinal, 3);
        assert_eq!(
            incident.global_receive_watermark_ns(),
            Some(expected_watermark)
        );
        match index {
            0 => assert!(matches!(
                incident.reason(),
                XnasQuarantineReasonV1::BadReceiveTimestamp
            )),
            1 => assert!(matches!(
                incident.reason(),
                XnasQuarantineReasonV1::EventOutsideSourceDay { ts_event }
                    if *ts_event == END_NS
            )),
            _ => unreachable!(),
        }
    }
}

#[test]
fn ordinary_reset_is_a_boundary_and_commits_only_after_its_own_witness() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("recovery.dbn");
    let records = vec![
        initial_clear(START_NS + 1, START_NS + 2),
        message(
            b'A',
            b'B',
            1,
            BID,
            100,
            10,
            flags::LAST,
            START_NS + 10,
            START_NS + 100,
        ),
        message(
            b'T',
            b'A',
            0,
            BID,
            1,
            11,
            flags::LAST,
            START_NS + 20,
            START_NS + 110,
        ),
        message(
            b'R',
            b'N',
            0,
            UNDEF_PRICE,
            0,
            20,
            0,
            START_NS + 30,
            START_NS + 120,
        ),
        message(
            b'A',
            b'A',
            2,
            ASK,
            25,
            20,
            flags::LAST,
            START_NS + 30,
            START_NS + 120,
        ),
        message(
            b'T',
            b'B',
            0,
            ASK,
            1,
            30,
            flags::LAST,
            START_NS + 40,
            START_NS + 130,
        ),
    ];
    write_dbn(&path, Compression::None, &records);
    let run = run_with_all_traces(
        open_replay(&path, records.len() as u64),
        records.len() as u64,
    );
    let first = &run.traces()[0];
    assert_eq!(first.terminal_source_ordinal(), 2);
    let recovery = &run.traces()[1];
    assert!(recovery.is_recovery());
    assert_eq!(recovery.events().len(), 2);
    assert_eq!(recovery.witness_source_ordinal(), 6);
    assert!(recovery.book().snapshot().best_bid().is_none());
    assert_eq!(
        recovery
            .book()
            .snapshot()
            .best_ask()
            .unwrap()
            .aggregate_size(),
        25
    );
    let receipt = run.receipt();
    assert_eq!(receipt.counts().reset_recovery_candidates, 1);
    assert_eq!(receipt.counts().reset_boundary_quarantine_incidents, 1);
    assert_eq!(receipt.counts().reset_boundary_quarantined_records, 1);
    assert_eq!(receipt.counts().tail_quarantine_incidents, 1);
    assert_eq!(receipt.counts().eof_tail_quarantined_records, 1);
    assert_eq!(receipt.counts().quarantined_records, 2);
    assert_eq!(receipt.counts().private_book_resets, 2);
    let reset_quarantine = &receipt.identities()[0].reset_boundary_quarantines()[0];
    assert_eq!(reset_quarantine.reset_source_ordinal(), 4);
    assert_eq!(reset_quarantine.quarantined_source_ordinals(), [3]);
    assert!(matches!(
        selected_roles(&run, 3),
        [
            XnasSelectedOrdinalRoleV1::ClosureWitness {
                trace_index: 0,
                terminal_source_ordinal: 2,
                ..
            },
            XnasSelectedOrdinalRoleV1::ResetBoundaryQuarantinedMember {
                reset_source_ordinal: 4,
                ..
            }
        ]
    ));
    assert!(matches!(
        selected_roles(&run, 4),
        [
            XnasSelectedOrdinalRoleV1::CompletedEnvelopeMember {
                trace_index: 1,
                terminal_source_ordinal: 5,
                ..
            },
            XnasSelectedOrdinalRoleV1::ResetBoundaryTrigger { .. }
        ]
    ));
    assert!(matches!(
        selected_roles(&run, 6),
        [
            XnasSelectedOrdinalRoleV1::ClosureWitness {
                trace_index: 1,
                terminal_source_ordinal: 5,
                ..
            },
            XnasSelectedOrdinalRoleV1::EofTailQuarantinedMember {
                reason: XnasEofTailReasonV1::TerminalCandidateWithoutWitness,
                ..
            }
        ]
    ));
}

#[test]
fn eof_quarantine_preserves_every_member_of_a_nonterminal_envelope() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("multi-member-nonterminal-tail.dbn");
    let records = vec![
        initial_clear(START_NS + 1, START_NS + 2),
        message(
            b'A',
            b'B',
            1,
            BID,
            100,
            10,
            flags::LAST,
            START_NS + 10,
            START_NS + 100,
        ),
        message(b'A', b'A', 2, ASK, 25, 11, 0, START_NS + 20, START_NS + 110),
        message(
            b'A',
            b'A',
            3,
            ASK + 1_000_000_000,
            10,
            11,
            0,
            START_NS + 20,
            START_NS + 110,
        ),
    ];
    write_dbn(&path, Compression::None, &records);
    let run = run_with_all_traces(
        open_replay(&path, records.len() as u64),
        records.len() as u64,
    );
    let receipt = run.receipt();
    let identity = &receipt.identities()[0];
    let tail = identity
        .eof_tail_quarantine()
        .expect("the open nonterminal envelope must be quarantined");
    assert_eq!(tail.first_source_ordinal(), 3);
    assert_eq!(tail.last_source_ordinal(), 4);
    assert_eq!(tail.member_count(), 2);
    assert_eq!(tail.source_ordinals(), [3, 4]);
    assert_eq!(tail.reason(), XnasEofTailReasonV1::NonterminalEnvelope);
    assert_eq!(receipt.counts().tail_quarantine_incidents, 1);
    assert_eq!(receipt.counts().eof_tail_quarantined_records, 2);
    assert_eq!(identity.validity_epochs().len(), 1);
    let invalidation = identity.validity_epochs()[0].invalidation();
    assert_eq!(invalidation.first_ineligible_source_ordinal(), 3);
    assert_eq!(invalidation.detected_at_source_ordinal(), None);
    assert!(matches!(
        invalidation.reason(),
        XnasValidityInvalidationReasonV1::EofTail {
            reason: XnasEofTailReasonV1::NonterminalEnvelope
        }
    ));
}

#[test]
fn final_recovery_qualification_closes_every_incident_in_one_invalid_interval() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("chained-quarantine-recovery.dbn");
    let records = vec![
        initial_clear(START_NS + 1, START_NS + 2),
        message(
            b'A',
            b'B',
            1,
            BID,
            100,
            10,
            flags::LAST,
            START_NS + 10,
            START_NS + 100,
        ),
        message(
            b'A',
            b'A',
            2,
            ASK,
            50,
            11,
            flags::LAST,
            START_NS + 20,
            START_NS + 110,
        ),
        message(
            b'A',
            b'A',
            3,
            ASK + 1_000_000_000,
            10,
            11,
            0,
            START_NS + 20,
            START_NS + 110,
        ),
        message(
            b'R',
            b'N',
            0,
            UNDEF_PRICE,
            0,
            20,
            0,
            START_NS + 30,
            START_NS + 120,
        ),
        message(
            b'M',
            b'B',
            999,
            BID,
            1,
            20,
            flags::LAST,
            START_NS + 30,
            START_NS + 120,
        ),
        message(
            b'T',
            b'A',
            0,
            BID,
            1,
            21,
            flags::LAST,
            START_NS + 40,
            START_NS + 130,
        ),
        message(
            b'R',
            b'N',
            0,
            UNDEF_PRICE,
            0,
            30,
            0,
            START_NS + 50,
            START_NS + 140,
        ),
        message(
            b'A',
            b'A',
            4,
            ASK,
            25,
            30,
            flags::LAST,
            START_NS + 50,
            START_NS + 140,
        ),
        message(
            b'T',
            b'B',
            0,
            ASK,
            1,
            31,
            flags::LAST,
            START_NS + 60,
            START_NS + 150,
        ),
    ];
    write_dbn(&path, Compression::None, &records);
    let run = run_with_all_traces(
        open_replay(&path, records.len() as u64),
        records.len() as u64,
    );
    let receipt = run.receipt();
    let identity = &receipt.identities()[0];
    assert_eq!(identity.semantic_quarantines().len(), 2);
    let first = &identity.semantic_quarantines()[0];
    let second = &identity.semantic_quarantines()[1];
    assert_eq!(first.candidate_source_ordinals(), [3, 4]);
    assert_eq!(first.detected_at().raw_ordinal, 4);
    assert!(matches!(
        first.reason(),
        XnasQuarantineReasonV1::Envelope { .. }
    ));
    assert_eq!(second.candidate_source_ordinals(), [5, 6]);
    assert_eq!(second.detected_at().raw_ordinal, 7);
    assert_eq!(second.offending_candidate_source_ordinal(), Some(6));
    assert_eq!(second.while_invalid_records().len(), 1);
    assert_eq!(second.while_invalid_records()[0].raw().raw_ordinal, 7);
    assert!(matches!(
        second.reason(),
        XnasQuarantineReasonV1::Book {
            source: BookTransactionErrorV1::MissingModify {
                order_id: 999,
                raw_ordinal: 6
            }
        }
    ));
    let final_recovery = first.recovery().expect("first incident must close");
    assert_eq!(second.recovery(), Some(final_recovery));
    assert_eq!(final_recovery.reset_source_ordinal(), 8);
    assert_eq!(final_recovery.terminal_source_ordinal(), 9);
    assert_eq!(final_recovery.witness_source_ordinal(), 10);
    assert_eq!(
        identity.recovery_qualifications(),
        std::slice::from_ref(final_recovery)
    );
    assert_eq!(identity.validity_epochs().len(), 2);
    assert_eq!(
        identity.validity_epochs()[0]
            .invalidation()
            .first_ineligible_source_ordinal(),
        3
    );
    assert_eq!(
        identity.validity_epochs()[0]
            .invalidation()
            .detected_at_source_ordinal(),
        Some(4)
    );
    assert_eq!(
        identity.validity_epochs()[1]
            .qualification()
            .recovery_reset_source_ordinal(),
        Some(8)
    );
    assert_eq!(receipt.counts().semantic_quarantine_incidents, 2);
    assert_eq!(receipt.counts().semantic_candidate_quarantined_records, 4);
    assert_eq!(
        receipt.counts().semantic_while_invalid_quarantined_records,
        1
    );
}

#[test]
fn nonconsecutive_exact_duplicate_reset_cannot_restart_recovery_and_clear_state() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("duplicate-reset.dbn");
    let reset = message(
        b'R',
        b'N',
        0,
        UNDEF_PRICE,
        0,
        20,
        0,
        START_NS + 30,
        START_NS + 120,
    );
    let records = vec![
        initial_clear(START_NS + 1, START_NS + 2),
        message(
            b'A',
            b'B',
            1,
            BID,
            100,
            10,
            flags::LAST,
            START_NS + 10,
            START_NS + 100,
        ),
        message(
            b'T',
            b'A',
            0,
            BID,
            1,
            11,
            flags::LAST,
            START_NS + 20,
            START_NS + 110,
        ),
        reset.clone(),
        message(b'A', b'A', 2, ASK, 25, 21, 0, START_NS + 31, START_NS + 120),
        reset,
    ];
    write_dbn(&path, Compression::None, &records);
    let run = run_with_all_traces(
        open_replay(&path, records.len() as u64),
        records.len() as u64,
    );
    assert_eq!(run.traces().len(), 1);
    assert_eq!(run.traces()[0].book().snapshot().live_orders(), 1);
    let receipt = run.receipt();
    let counts = receipt.counts();
    assert_eq!(counts.raw_records_ingested, 6);
    assert_eq!(counts.initial_clear_controls, 1);
    assert_eq!(counts.completed_envelope_members, 1);
    assert_eq!(counts.reset_boundary_quarantined_records, 1);
    assert_eq!(counts.semantic_candidate_quarantined_records, 3);
    assert_eq!(counts.quarantined_records, 4);
    assert_eq!(counts.pending_members, 0);
    assert_eq!(counts.private_book_resets, 1);
    assert_eq!(counts.reset_recovery_candidates, 1);
    assert_eq!(receipt.identities()[0].committed_envelopes(), 1);
    assert_eq!(
        receipt.identities()[0].terminal_status(),
        XnasTerminalIdentityStatusV1::InvalidAwaitingRecoveryAtEof
    );
    let incident = &receipt.identities()[0].semantic_quarantines()[0];
    assert_eq!(incident.candidate_source_ordinals(), [4, 5, 6]);
    assert_eq!(incident.offending_candidate_source_ordinal(), Some(6));
    assert!(incident.recovery().is_none());
    assert!(matches!(
        incident.reason(),
        XnasQuarantineReasonV1::Envelope {
            source: mbo_lob_reconstructor::XnasEnvelopeErrorV1::ExactDuplicate
        }
    ));
    let epoch = &receipt.identities()[0].validity_epochs()[0];
    assert_eq!(epoch.last_committed_terminal_source_ordinal(), 2);
    let invalidation = epoch.invalidation();
    assert_eq!(invalidation.first_ineligible_source_ordinal(), 3);
    assert_eq!(invalidation.detected_at_source_ordinal(), Some(4));
    assert!(matches!(
        invalidation.reason(),
        XnasValidityInvalidationReasonV1::ResetBoundary
    ));
}

#[test]
fn output_configuration_is_receipt_bound() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config-bound.dbn");
    let records = conformance_records(START_NS + 1, START_NS + 2);
    write_dbn(&path, Compression::None, &records);
    let mut outcomes = Vec::new();
    for depth in [1, 10] {
        let config = XnasReplayConfigV1::new(
            NonZeroUsize::new(depth).unwrap(),
            NonZeroUsize::new(64).unwrap(),
            NonZeroUsize::new(64).unwrap(),
        );
        let receipt = open_replay_with_config(&path, records.len() as u64, config)
            .run_to_eof()
            .unwrap();
        outcomes.push((
            receipt.config(),
            receipt.committed_observation_chain_sha256(),
            receipt.identities()[0].last_committed_book_state_sha256(),
        ));
    }
    assert_ne!(outcomes[0].0, outcomes[1].0);
    assert_ne!(outcomes[0].1, outcomes[1].1);
    assert_eq!(outcomes[0].2, outcomes[1].2);
}

#[test]
fn configured_envelope_resource_limits_fail_before_unbounded_growth() {
    let cases = [
        (
            2,
            64,
            vec![
                initial_clear(START_NS + 1, START_NS + 2),
                message(b'A', b'B', 1, BID, 1, 10, 0, START_NS + 10, START_NS + 100),
                message(b'A', b'B', 2, BID, 1, 10, 0, START_NS + 10, START_NS + 100),
                message(b'A', b'B', 3, BID, 1, 10, 0, START_NS + 10, START_NS + 100),
            ],
        ),
        (
            64,
            1,
            vec![
                initial_clear(START_NS + 1, START_NS + 2),
                message(b'A', b'B', 1, BID, 1, 10, 0, START_NS + 10, START_NS + 100),
                message(b'A', b'B', 2, BID, 1, 12, 0, START_NS + 11, START_NS + 100),
            ],
        ),
    ];
    for (index, (member_limit, block_limit, records)) in cases.into_iter().enumerate() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(format!("limit-{index}.dbn"));
        write_dbn(&path, Compression::None, &records);
        let config = XnasReplayConfigV1::new(
            NonZeroUsize::new(10).unwrap(),
            NonZeroUsize::new(member_limit).unwrap(),
            NonZeroUsize::new(block_limit).unwrap(),
        );
        let replay = open_replay_with_config(&path, records.len() as u64, config);
        let error = replay.run_to_eof().unwrap_err();
        match index {
            0 => assert!(matches!(
                error.root_cause(),
                XnasReplayErrorV1::Envelope {
                    source: mbo_lob_reconstructor::XnasEnvelopeErrorV1::MemberLimit { limit: 2 },
                    ..
                }
            )),
            1 => assert!(matches!(
                error.root_cause(),
                XnasReplayErrorV1::Envelope {
                    source: mbo_lob_reconstructor::XnasEnvelopeErrorV1::SequenceBlockLimit {
                        limit: 1
                    },
                    ..
                }
            )),
            _ => unreachable!(),
        }
    }
}

#[test]
fn production_probe_rejects_every_unadmitted_source_before_terminal_output() {
    let dir = tempdir().unwrap();
    let source_path = dir.path().join("probe-source.dbn");
    let records = vec![
        initial_clear(START_NS + 1, START_NS + 2),
        message(
            b'A',
            b'B',
            1,
            BID,
            100,
            10,
            flags::LAST,
            START_NS + 10,
            START_NS + 100,
        ),
    ];
    write_dbn(&source_path, Compression::None, &records);
    let expectation_path = dir.path().join("probe-expectation.json");
    let expectation = support::probe_request_value(&source_path, records.len() as u64);
    let parsed: XnasReplayProbeRequestV1 = serde_json::from_value(expectation.clone()).unwrap();
    parsed.validate().unwrap();
    let predeclared = parsed.logical_source().unwrap();
    assert_eq!(
        predeclared.relative_path,
        source_path.file_name().unwrap().to_str().unwrap()
    );
    assert_eq!(predeclared.compressed_sha256, parsed.compressed_sha256());
    assert_eq!(predeclared.expected_records, parsed.expected_records());
    assert!(matches!(
        parsed.validate_admitted(),
        Err(XnasReplayProbeRequestErrorV1::SourceIdentity(
            hft_mbo_event_contract::SourceIdentityErrorV1::CatalogReleaseNotAccepted(_)
        ))
    ));
    let mut unknown = expectation.clone();
    unknown["unknown_field"] = true.into();
    assert!(serde_json::from_value::<XnasReplayProbeRequestV1>(unknown).is_err());
    std::fs::write(
        &expectation_path,
        serde_json::to_vec_pretty(&expectation).unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_xnas_replay_probe"))
        .arg(&expectation_path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());

    let qualified_source_path = dir.path().join("probe-qualified.dbn");
    let qualified_records = conformance_records(START_NS + 1, START_NS + 2);
    write_dbn(
        &qualified_source_path,
        Compression::None,
        &qualified_records,
    );
    let qualified =
        support::probe_request_value(&qualified_source_path, qualified_records.len() as u64);
    std::fs::write(
        &expectation_path,
        serde_json::to_vec_pretty(&qualified).unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_xnas_replay_probe"))
        .arg(&expectation_path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());

    let midstream_source_path = dir.path().join("probe-midstream-fatal.dbn");
    let midstream_records = vec![
        initial_clear(START_NS + 1, START_NS + 2),
        message(
            b'A',
            b'B',
            1,
            BID,
            100,
            10,
            flags::LAST,
            START_NS + 10,
            START_NS + 100,
        ),
        message_for_instrument(
            202,
            b'A',
            b'A',
            2,
            ASK,
            25,
            20,
            flags::LAST,
            START_NS + 20,
            START_NS + 110,
        ),
    ];
    write_dbn(
        &midstream_source_path,
        Compression::None,
        &midstream_records,
    );
    let midstream =
        support::probe_request_value(&midstream_source_path, midstream_records.len() as u64);
    std::fs::write(
        &expectation_path,
        serde_json::to_vec_pretty(&midstream).unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_xnas_replay_probe"))
        .arg(&expectation_path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());

    let wrong_count =
        support::probe_request_value(&qualified_source_path, qualified_records.len() as u64 + 1);
    std::fs::write(
        &expectation_path,
        serde_json::to_vec_pretty(&wrong_count).unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_xnas_replay_probe"))
        .arg(&expectation_path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());

    let mut wrong_identity = expectation;
    wrong_identity["compressed_sha256"] =
        serde_json::Value::String(Sha256DigestV1::from_bytes([7; 32]).to_hex());
    std::fs::write(
        &expectation_path,
        serde_json::to_vec_pretty(&wrong_identity).unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_xnas_replay_probe"))
        .arg(&expectation_path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}
