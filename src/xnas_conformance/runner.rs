//! Executable composition for the bounded DECISION-031 conformance artifact.
//!
//! This module is intentionally narrow. It binds the accepted blocker, the
//! exact source images, the provider CLI reference decoder, the primary
//! reducers, and independently authored reference reducers into the next
//! named artifact. It is not a generic experiment framework.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dbn::decode::{DbnMetadata, DecodeRecordRef, DynDecoder};
use dbn::{MboMsg, Mbp10Msg, Record, VersionUpgradePolicy};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::xnas_reference::{
    ReferenceMboFinishReportV1, ReferenceMboPublicationV1, ReferenceMboReducerV1,
    ReferenceMbp10EndpointV1, ReferenceMbp10FinishReportV1, ReferenceMbp10ReducerV1,
};
use crate::xnas_semantics::{
    MboIngestDispositionV1, MboIngestOutcomeV1, Mbp10LevelV1, RawMboRecordV1,
    RawMbp10RecordV1, SourceOrdinal, XnasDailySourceQualificationV1, XnasIdentityV1,
    XnasMboStreamV1, XnasMbp10StreamV1, XnasSchemaV1, DBN_FLAG_LAST,
};

use super::{
    qualify_mbo_metadata, qualify_mbp10_metadata, resolve_accepted_blocker, sha256_bytes,
    utf8_path, BlockerSourceRoleV1, QualifiedDbnMetadataV1, QualifiedReferenceExecutableV1,
    QualifiedSourceImageV1, ResolvedBlockerV1, XnasConformanceError, BLOCKER_COMMIT_V1,
    BLOCKER_PATH_V1, BLOCKER_SHA256_V1,
};

const CONFORMANCE_SCHEMA_VERSION_V1: &str = "1.0";
const CONFORMANCE_ARTIFACT_ID_V1: &str = "MBO_SEMANTICS_CONFORMANCE_V1";
const CONFORMANCE_STATUS_V1: &str = "PASS";
const BLOCKER_V2_ARTIFACT_ID: &str = "MBO_SEMANTICS_BLOCKER_V2";
const BLOCKER_V2_STATUS: &str = "BLOCKED_CONFORMANCE";
const JULY_SESSION_V1: &str = "20250703";
const FEBRUARY_SESSION_V1: &str = "20250203";
const SECOND_NS: u64 = 1_000_000_000;

/// The durable outcome of one no-overwrite conformance invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XnasConformanceArtifactDispositionV1 {
    /// Every accepted gate passed and the conformance artifact was created.
    Passed { path: PathBuf, sha256: String },
    /// A fail-closed error was persisted as the terminal V2 blocker.
    Blocked { path: PathBuf, sha256: String },
}

#[derive(Debug, Clone, Serialize)]
struct GitIdentityV1 {
    repository: String,
    commit: String,
    tree: String,
    clean_before_compute: bool,
}

#[derive(Debug, Clone, Serialize)]
struct AuthorityBindingV1 {
    blocker_commit: &'static str,
    blocker_path: &'static str,
    blocker_sha256: &'static str,
    resolved_blocker_sha256: String,
    reference_executable_version: String,
    reference_executable_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
struct SourceEvidenceV1 {
    role: &'static str,
    session: String,
    path: String,
    size_bytes: u64,
    sha256: String,
    manifest_path: String,
    manifest_sha256: String,
    metadata: QualifiedMetadataArtifactV1,
}

#[derive(Debug, Clone, Serialize)]
struct QualifiedMetadataArtifactV1 {
    version: u8,
    dataset: String,
    schema: XnasSchemaV1,
    query_start_ns: u64,
    query_end_ns: u64,
    session_date_yyyymmdd: String,
    raw_symbol: String,
    identity: XnasIdentityV1,
}

impl From<&QualifiedDbnMetadataV1> for QualifiedMetadataArtifactV1 {
    fn from(value: &QualifiedDbnMetadataV1) -> Self {
        Self {
            version: value.version,
            dataset: value.dataset.clone(),
            schema: value.schema,
            query_start_ns: value.query_start_ns,
            query_end_ns: value.query_end_ns,
            session_date_yyyymmdd: value.session_date_yyyymmdd.clone(),
            raw_symbol: value.raw_symbol.clone(),
            identity: value.identity,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
struct MboGrammarMeasurementsV1 {
    decoded_records: u64,
    sequence_blocks: u64,
    multi_record_sequence_blocks: u64,
    repeated_last_sequence_blocks: u64,
    tfc_sequence_blocks: u64,
    tfc_all_last_sequence_blocks: u64,
    tfc_no_last_sequence_blocks: u64,
    execution_bearing_terminal_sequence_blocks: u64,
    block_timestamp_mismatch_count: u64,
    receive_time_changed_inside_block_count: u64,
    last_to_non_last_transition_count: u64,
    action_counts: BTreeMap<String, u64>,
    flag_counts: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
struct Mbp10GrammarMeasurementsV1 {
    decoded_records: u64,
    sequence_blocks: u64,
    multi_record_sequence_blocks: u64,
    repeated_last_sequence_blocks: u64,
    tc_sequence_blocks: u64,
    tc_all_last_sequence_blocks: u64,
    block_timestamp_mismatch_count: u64,
    receive_time_changed_inside_block_count: u64,
    last_to_non_last_transition_count: u64,
    action_counts: BTreeMap<String, u64>,
    flag_counts: BTreeMap<String, u64>,
}

#[derive(Debug, Clone)]
struct OpenMboGrammarBlockV1 {
    key: (u16, u32, u8, u32),
    record_count: u64,
    last_count: u64,
    saw_last: bool,
    has_t: bool,
    has_f: bool,
    has_c: bool,
    ts_event: u64,
    ts_recv: u64,
}

#[derive(Debug, Default)]
struct MboGrammarScannerV1 {
    measurements: MboGrammarMeasurementsV1,
    open: Option<OpenMboGrammarBlockV1>,
}

impl MboGrammarScannerV1 {
    fn push(&mut self, record: &RawMboRecordV1) {
        self.measurements.decoded_records += 1;
        *self
            .measurements
            .action_counts
            .entry(char::from(record.action).to_string())
            .or_default() += 1;
        *self
            .measurements
            .flag_counts
            .entry(format!("0x{:02x}", record.flags))
            .or_default() += 1;
        let key = (
            record.publisher_id,
            record.instrument_id,
            record.channel_id,
            record.sequence,
        );
        if self.open.as_ref().is_some_and(|open| open.key != key) {
            self.flush();
        }
        let is_last = record.flags & DBN_FLAG_LAST != 0;
        let open = self.open.get_or_insert_with(|| OpenMboGrammarBlockV1 {
            key,
            record_count: 0,
            last_count: 0,
            saw_last: false,
            has_t: false,
            has_f: false,
            has_c: false,
            ts_event: record.ts_event,
            ts_recv: record.ts_recv,
        });
        if open.record_count > 0 {
            self.measurements.block_timestamp_mismatch_count +=
                u64::from(record.ts_event != open.ts_event);
            self.measurements
                .receive_time_changed_inside_block_count += u64::from(record.ts_recv != open.ts_recv);
            self.measurements.last_to_non_last_transition_count +=
                u64::from(open.saw_last && !is_last);
        }
        open.record_count += 1;
        open.last_count += u64::from(is_last);
        open.saw_last |= is_last;
        open.has_t |= record.action == b'T';
        open.has_f |= record.action == b'F';
        open.has_c |= record.action == b'C';
    }

    fn finish(mut self) -> MboGrammarMeasurementsV1 {
        self.flush();
        self.measurements
    }

    fn flush(&mut self) {
        let Some(open) = self.open.take() else {
            return;
        };
        self.measurements.sequence_blocks += 1;
        self.measurements.multi_record_sequence_blocks += u64::from(open.record_count > 1);
        self.measurements.repeated_last_sequence_blocks += u64::from(open.last_count > 1);
        let tfc = open.has_t && open.has_f && open.has_c;
        self.measurements.tfc_sequence_blocks += u64::from(tfc);
        self.measurements.tfc_all_last_sequence_blocks +=
            u64::from(tfc && open.last_count == open.record_count);
        self.measurements.tfc_no_last_sequence_blocks += u64::from(tfc && open.last_count == 0);
        self.measurements
            .execution_bearing_terminal_sequence_blocks +=
            u64::from((open.has_t || open.has_f) && open.last_count > 0);
    }
}

#[derive(Debug, Clone)]
struct OpenMbp10GrammarBlockV1 {
    key: (u16, u32, u32),
    record_count: u64,
    last_count: u64,
    saw_last: bool,
    has_t: bool,
    has_c: bool,
    ts_event: u64,
    ts_recv: u64,
}

#[derive(Debug, Default)]
struct Mbp10GrammarScannerV1 {
    measurements: Mbp10GrammarMeasurementsV1,
    open: Option<OpenMbp10GrammarBlockV1>,
}

impl Mbp10GrammarScannerV1 {
    fn push(&mut self, record: &RawMbp10RecordV1) {
        self.measurements.decoded_records += 1;
        *self
            .measurements
            .action_counts
            .entry(char::from(record.action).to_string())
            .or_default() += 1;
        *self
            .measurements
            .flag_counts
            .entry(format!("0x{:02x}", record.flags))
            .or_default() += 1;
        let key = (
            record.publisher_id,
            record.instrument_id,
            record.sequence,
        );
        if self.open.as_ref().is_some_and(|open| open.key != key) {
            self.flush();
        }
        let is_last = record.flags & DBN_FLAG_LAST != 0;
        let open = self
            .open
            .get_or_insert_with(|| OpenMbp10GrammarBlockV1 {
                key,
                record_count: 0,
                last_count: 0,
                saw_last: false,
                has_t: false,
                has_c: false,
                ts_event: record.ts_event,
                ts_recv: record.ts_recv,
            });
        if open.record_count > 0 {
            self.measurements.block_timestamp_mismatch_count +=
                u64::from(record.ts_event != open.ts_event);
            self.measurements
                .receive_time_changed_inside_block_count += u64::from(record.ts_recv != open.ts_recv);
            self.measurements.last_to_non_last_transition_count +=
                u64::from(open.saw_last && !is_last);
        }
        open.record_count += 1;
        open.last_count += u64::from(is_last);
        open.saw_last |= is_last;
        open.has_t |= record.action == b'T';
        open.has_c |= record.action == b'C';
    }

    fn finish(mut self) -> Mbp10GrammarMeasurementsV1 {
        self.flush();
        self.measurements
    }

    fn flush(&mut self) {
        let Some(open) = self.open.take() else {
            return;
        };
        self.measurements.sequence_blocks += 1;
        self.measurements.multi_record_sequence_blocks += u64::from(open.record_count > 1);
        self.measurements.repeated_last_sequence_blocks += u64::from(open.last_count > 1);
        let tc = open.has_t && open.has_c;
        self.measurements.tc_sequence_blocks += u64::from(tc);
        self.measurements.tc_all_last_sequence_blocks +=
            u64::from(tc && open.last_count == open.record_count);
    }
}

#[derive(Debug, Clone, Default, Serialize)]
struct ClarificationImpactV1 {
    envelope_count: u64,
    positive_closure_delay_count: u64,
    one_second_publication_bin_changed_count: u64,
    decision_cutoff_inclusion_changed_count: u64,
    integer_second_cutoffs_crossed_total: u64,
    maximum_integer_second_cutoffs_crossed: u64,
}

impl ClarificationImpactV1 {
    fn observe(
        &mut self,
        endpoint_ns: u64,
        effective_available_ns: u64,
    ) -> Result<(), XnasConformanceError> {
        if effective_available_ns < endpoint_ns {
            return Err(XnasConformanceError::ConformanceInvariant(
                "effective availability precedes endpoint".to_owned(),
            ));
        }
        self.envelope_count += 1;
        self.positive_closure_delay_count += u64::from(effective_available_ns > endpoint_ns);
        let hypothetical = publication_available_ns(endpoint_ns)?;
        let witnessed = publication_available_ns(effective_available_ns)?;
        let changed = hypothetical != witnessed;
        self.one_second_publication_bin_changed_count += u64::from(changed);
        self.decision_cutoff_inclusion_changed_count += u64::from(changed);
        let crossed = (witnessed - hypothetical) / SECOND_NS;
        self.integer_second_cutoffs_crossed_total += crossed;
        self.maximum_integer_second_cutoffs_crossed =
            self.maximum_integer_second_cutoffs_crossed.max(crossed);
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
struct DelayDistributionV1 {
    count: u64,
    minimum_ns: Option<u64>,
    maximum_ns: Option<u64>,
    sum_ns: String,
    mean_ns: Option<f64>,
    exact_quantiles_ns: BTreeMap<String, u64>,
    exact_bucket_counts: BTreeMap<String, u64>,
    sorted_u64_le_sha256: String,
}

fn summarize_delays(mut values: Vec<u64>) -> DelayDistributionV1 {
    values.sort_unstable();
    let count = values.len() as u64;
    let sum = values
        .iter()
        .fold(0_u128, |accumulator, value| accumulator + u128::from(*value));
    let mut exact_quantiles_ns = BTreeMap::new();
    for (label, numerator, denominator) in [
        ("p000", 0_u64, 1000_u64),
        ("p010", 10, 1000),
        ("p050", 50, 1000),
        ("p100", 100, 1000),
        ("p250", 250, 1000),
        ("p500", 500, 1000),
        ("p750", 750, 1000),
        ("p900", 900, 1000),
        ("p950", 950, 1000),
        ("p990", 990, 1000),
        ("p999", 999, 1000),
        ("p1000", 1000, 1000),
    ] {
        if !values.is_empty() {
            let last = values.len() - 1;
            let index = usize::try_from(
                u128::try_from(last).expect("usize fits u128") * u128::from(numerator)
                    / u128::from(denominator),
            )
            .expect("quantile index fits usize");
            exact_quantiles_ns.insert(label.to_owned(), values[index]);
        }
    }
    let mut exact_bucket_counts = BTreeMap::from([
        ("[0,1us)".to_owned(), 0_u64),
        ("[1us,10us)".to_owned(), 0),
        ("[10us,100us)".to_owned(), 0),
        ("[100us,1ms)".to_owned(), 0),
        ("[1ms,10ms)".to_owned(), 0),
        ("[10ms,100ms)".to_owned(), 0),
        ("[100ms,1s)".to_owned(), 0),
        ("[1s,5s)".to_owned(), 0),
        ("[5s,inf)".to_owned(), 0),
    ]);
    for value in &values {
        let label = match *value {
            0..=999 => "[0,1us)",
            1_000..=9_999 => "[1us,10us)",
            10_000..=99_999 => "[10us,100us)",
            100_000..=999_999 => "[100us,1ms)",
            1_000_000..=9_999_999 => "[1ms,10ms)",
            10_000_000..=99_999_999 => "[10ms,100ms)",
            100_000_000..=999_999_999 => "[100ms,1s)",
            1_000_000_000..=4_999_999_999 => "[1s,5s)",
            _ => "[5s,inf)",
        };
        *exact_bucket_counts
            .get_mut(label)
            .expect("all delay buckets are initialized") += 1;
    }
    let mut hasher = Sha256::new();
    for value in &values {
        hasher.update(value.to_le_bytes());
    }
    DelayDistributionV1 {
        count,
        minimum_ns: values.first().copied(),
        maximum_ns: values.last().copied(),
        sum_ns: sum.to_string(),
        mean_ns: (count > 0).then(|| sum as f64 / count as f64),
        exact_quantiles_ns,
        exact_bucket_counts,
        sorted_u64_le_sha256: format!("{:x}", hasher.finalize()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CompactEndpointKeyV1 {
    publisher_id: u16,
    instrument_id: u32,
    endpoint_ns: u64,
    ordered_distinct_sequence_vector: Vec<u32>,
    terminal_sequence: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompactEndpointV1 {
    key: CompactEndpointKeyV1,
    effective_available_ns: u64,
    levels: [Mbp10LevelV1; 10],
}

struct EndpointSpoolWriterV1 {
    writer: BufWriter<File>,
    last_key: Option<CompactEndpointKeyV1>,
    count: u64,
    digest: Sha256,
}

struct EndpointSpoolV1 {
    file: File,
    count: u64,
    sha256: String,
}

impl EndpointSpoolWriterV1 {
    fn new() -> Result<Self, XnasConformanceError> {
        let file =
            tempfile::tempfile().map_err(|error| artifact_io("create endpoint spool", error))?;
        Ok(Self {
            writer: BufWriter::new(file),
            last_key: None,
            count: 0,
            digest: Sha256::new(),
        })
    }

    fn push(&mut self, value: &CompactEndpointV1) -> Result<(), XnasConformanceError> {
        if self
            .last_key
            .as_ref()
            .is_some_and(|previous| previous > &value.key)
        {
            return Err(XnasConformanceError::ConformanceInvariant(
                "endpoint keys regressed in source order".to_owned(),
            ));
        }
        let mut bytes = Vec::with_capacity(384);
        encode_endpoint(value, &mut bytes)?;
        self.writer
            .write_all(&bytes)
            .map_err(|error| artifact_io("write endpoint spool", error))?;
        self.digest.update(&bytes);
        self.last_key = Some(value.key.clone());
        self.count += 1;
        Ok(())
    }

    fn finish(mut self) -> Result<EndpointSpoolV1, XnasConformanceError> {
        self.writer
            .flush()
            .map_err(|error| artifact_io("flush endpoint spool", error))?;
        let mut file = self
            .writer
            .into_inner()
            .map_err(|error| artifact_io("finish endpoint spool", error.into_error()))?;
        file.seek(SeekFrom::Start(0))
            .map_err(|error| artifact_io("rewind endpoint spool", error))?;
        Ok(EndpointSpoolV1 {
            file,
            count: self.count,
            sha256: format!("{:x}", self.digest.finalize()),
        })
    }
}

struct EndpointSpoolReaderV1 {
    reader: BufReader<File>,
    held: Option<CompactEndpointV1>,
}

impl EndpointSpoolReaderV1 {
    fn new(mut spool: EndpointSpoolV1) -> Result<(Self, u64, String), XnasConformanceError> {
        spool
            .file
            .seek(SeekFrom::Start(0))
            .map_err(|error| artifact_io("rewind endpoint spool", error))?;
        let count = spool.count;
        let sha256 = spool.sha256;
        Ok((
            Self {
                reader: BufReader::new(spool.file),
                held: None,
            },
            count,
            sha256,
        ))
    }

    fn next(&mut self) -> Result<Option<CompactEndpointV1>, XnasConformanceError> {
        if self.held.is_some() {
            return Ok(self.held.take());
        }
        decode_endpoint(&mut self.reader)
    }
}

fn encode_endpoint(
    value: &CompactEndpointV1,
    output: &mut Vec<u8>,
) -> Result<(), XnasConformanceError> {
    output.extend_from_slice(&value.key.publisher_id.to_le_bytes());
    output.extend_from_slice(&value.key.instrument_id.to_le_bytes());
    output.extend_from_slice(&value.key.endpoint_ns.to_le_bytes());
    let sequence_count = u32::try_from(value.key.ordered_distinct_sequence_vector.len()).map_err(
        |_| XnasConformanceError::ConformanceInvariant("sequence vector exceeds u32".to_owned()),
    )?;
    output.extend_from_slice(&sequence_count.to_le_bytes());
    for sequence in &value.key.ordered_distinct_sequence_vector {
        output.extend_from_slice(&sequence.to_le_bytes());
    }
    output.extend_from_slice(&value.key.terminal_sequence.to_le_bytes());
    output.extend_from_slice(&value.effective_available_ns.to_le_bytes());
    for level in &value.levels {
        output.extend_from_slice(&level.bid_px.to_le_bytes());
        output.extend_from_slice(&level.ask_px.to_le_bytes());
        output.extend_from_slice(&level.bid_sz.to_le_bytes());
        output.extend_from_slice(&level.ask_sz.to_le_bytes());
        output.extend_from_slice(&level.bid_ct.to_le_bytes());
        output.extend_from_slice(&level.ask_ct.to_le_bytes());
    }
    Ok(())
}

fn decode_endpoint(
    reader: &mut BufReader<File>,
) -> Result<Option<CompactEndpointV1>, XnasConformanceError> {
    let Some(publisher_id) = read_u16_or_eof(reader)? else {
        return Ok(None);
    };
    let instrument_id = read_u32(reader)?;
    let endpoint_ns = read_u64(reader)?;
    let sequence_count = read_u32(reader)?;
    if sequence_count > 1_000_000 {
        return Err(XnasConformanceError::ConformanceInvariant(
            "endpoint spool sequence vector is implausibly large".to_owned(),
        ));
    }
    let mut sequences = Vec::with_capacity(sequence_count as usize);
    for _ in 0..sequence_count {
        sequences.push(read_u32(reader)?);
    }
    let terminal_sequence = read_u32(reader)?;
    let effective_available_ns = read_u64(reader)?;
    let mut levels = [Mbp10LevelV1::default(); 10];
    for level in &mut levels {
        *level = Mbp10LevelV1 {
            bid_px: read_i64(reader)?,
            ask_px: read_i64(reader)?,
            bid_sz: read_u32(reader)?,
            ask_sz: read_u32(reader)?,
            bid_ct: read_u32(reader)?,
            ask_ct: read_u32(reader)?,
        };
    }
    Ok(Some(CompactEndpointV1 {
        key: CompactEndpointKeyV1 {
            publisher_id,
            instrument_id,
            endpoint_ns,
            ordered_distinct_sequence_vector: sequences,
            terminal_sequence,
        },
        effective_available_ns,
        levels,
    }))
}

fn read_u16_or_eof(
    reader: &mut BufReader<File>,
) -> Result<Option<u16>, XnasConformanceError> {
    let mut bytes = [0_u8; 2];
    let read = reader
        .read(&mut bytes)
        .map_err(|error| artifact_io("read endpoint spool", error))?;
    if read == 0 {
        return Ok(None);
    }
    if read != bytes.len() {
        reader
            .read_exact(&mut bytes[read..])
            .map_err(|error| artifact_io("read endpoint spool", error))?;
    }
    Ok(Some(u16::from_le_bytes(bytes)))
}

fn read_u32(reader: &mut BufReader<File>) -> Result<u32, XnasConformanceError> {
    let mut bytes = [0_u8; 4];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| artifact_io("read endpoint spool", error))?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(reader: &mut BufReader<File>) -> Result<u64, XnasConformanceError> {
    let mut bytes = [0_u8; 8];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| artifact_io("read endpoint spool", error))?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_i64(reader: &mut BufReader<File>) -> Result<i64, XnasConformanceError> {
    let mut bytes = [0_u8; 8];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| artifact_io("read endpoint spool", error))?;
    Ok(i64::from_le_bytes(bytes))
}

fn publication_available_ns(value: u64) -> Result<u64, XnasConformanceError> {
    (value / SECOND_NS)
        .checked_add(1)
        .and_then(|seconds| seconds.checked_mul(SECOND_NS))
        .ok_or_else(|| {
            XnasConformanceError::ConformanceInvariant(
                "publication availability overflow".to_owned(),
            )
        })
}

fn artifact_io(context: &str, error: std::io::Error) -> XnasConformanceError {
    XnasConformanceError::ArtifactIo(format!("{context}: {}", error.kind()))
}

#[derive(Debug, Clone, Serialize)]
struct RawDecoderParityV1 {
    exact_record_count: u64,
    primary_raw_stream_sha256: String,
    reference_raw_stream_sha256: String,
    exact: bool,
}

#[derive(Debug, Clone, Serialize)]
struct ReducerParityV1 {
    primary_counts: serde_json::Value,
    reference_counts: serde_json::Value,
    primary_terminal_error_code: Option<String>,
    reference_terminal_error_code: Option<String>,
    publication_count: u64,
    publication_stream_sha256: String,
    exact: bool,
}

#[derive(Debug, Clone, Serialize)]
struct MboSourceConformanceV1 {
    evidence: SourceEvidenceV1,
    reference_decoded_body_record_count: u64,
    raw_decoder_parity: RawDecoderParityV1,
    grammar: MboGrammarMeasurementsV1,
    sealed_measurement_counts_match: Option<bool>,
    reducer_parity: ReducerParityV1,
    closure_confirmation_delay_ns: DelayDistributionV1,
    clarification_impact: ClarificationImpactV1,
    initial_clear_zero_contribution: bool,
}

#[derive(Debug, Clone, Serialize)]
struct Mbp10SourceConformanceV1 {
    evidence: SourceEvidenceV1,
    reference_decoded_body_record_count: u64,
    raw_decoder_parity: RawDecoderParityV1,
    grammar: Mbp10GrammarMeasurementsV1,
    sealed_measurement_counts_match: bool,
    reducer_parity: ReducerParityV1,
}

struct MboScanOutcomeV1 {
    report: MboSourceConformanceV1,
    endpoints: Option<EndpointSpoolV1>,
}

struct Mbp10ScanOutcomeV1 {
    report: Mbp10SourceConformanceV1,
    endpoints: EndpointSpoolV1,
}

enum PrimaryMboStepV1 {
    None,
    Published(Box<crate::xnas_semantics::PublishedMboBookV1>),
    Rejected(String),
}

fn scan_mbo_source(
    image: &QualifiedSourceImageV1,
    reference_executable: &QualifiedReferenceExecutableV1,
    retain_endpoints: bool,
    sealed_measurements: Option<&serde_json::Value>,
) -> Result<MboScanOutcomeV1, XnasConformanceError> {
    let source_bytes = image.source_bytes();
    let mut primary_decoder = DynDecoder::inferred_with_buffer(
        std::io::Cursor::new(Arc::clone(&source_bytes)),
        VersionUpgradePolicy::AsIs,
    )
    .map_err(|error| XnasConformanceError::DbnDecode(error.to_string()))?;
    let metadata = qualify_mbo_metadata(image, primary_decoder.metadata())?;
    let qualification = source_qualification(image, &metadata, XnasSchemaV1::Mbo)?;
    let mut primary_reducer = XnasMboStreamV1::new(qualification);
    let mut reference_reducer =
        ReferenceMboReducerV1::new(std::collections::BTreeSet::from([metadata.identity]));
    let mut grammar = MboGrammarScannerV1::default();
    let mut primary_raw_digest = Sha256::new();
    let mut reference_raw_digest = Sha256::new();
    let mut delays = Vec::new();
    let mut clarification_impact = ClarificationImpactV1::default();
    let mut endpoint_writer = retain_endpoints
        .then(EndpointSpoolWriterV1::new)
        .transpose()?;
    let mut endpoint_digest = Sha256::new();
    let mut publication_count = 0_u64;
    let mut primary_decoded_count = 0_u64;

    let reference_run = reference_executable.decode_mbo_source(image, |reference_record| {
        primary_decoded_count = primary_decoded_count.checked_add(1).ok_or_else(|| {
            XnasConformanceError::ConformanceInvariant(
                "primary MBO source ordinal overflow".to_owned(),
            )
        })?;
        let primary_record =
            decode_next_primary_mbo(&mut primary_decoder, primary_decoded_count)?;
        update_mbo_raw_digest(&mut primary_raw_digest, &primary_record);
        update_mbo_raw_digest(&mut reference_raw_digest, &reference_record);
        if primary_record != reference_record {
            return Err(XnasConformanceError::ConformanceInvariant(format!(
                "primary/reference MBO raw record mismatch at ordinal {primary_decoded_count}"
            )));
        }
        grammar.push(&reference_record);

        let primary_step = match primary_reducer.push_causally(primary_record)? {
            MboIngestOutcomeV1::Accepted { disposition, .. } => match disposition {
                MboIngestDispositionV1::Published(publication) => {
                    PrimaryMboStepV1::Published(publication)
                }
                MboIngestDispositionV1::InitialClearControl(_)
                | MboIngestDispositionV1::AuthoritativeReset(_)
                | MboIngestDispositionV1::Pending => PrimaryMboStepV1::None,
            },
            MboIngestOutcomeV1::Rejected(rejection) => {
                PrimaryMboStepV1::Rejected(rejection.error.code().to_owned())
            }
        };
        let reference_step = reference_reducer.push(reference_record);
        match (primary_step, reference_step) {
            (PrimaryMboStepV1::None, Ok(None)) => Ok(()),
            (PrimaryMboStepV1::Rejected(primary), Err(reference))
                if primary == reference.code() =>
            {
                Ok(())
            }
            (PrimaryMboStepV1::Published(primary), Ok(Some(reference))) => {
                compare_mbo_publications(&primary, &reference, primary_decoded_count)?;
                let compact = CompactEndpointV1 {
                    key: CompactEndpointKeyV1 {
                        publisher_id: primary.envelope.identity.publisher_id,
                        instrument_id: primary.envelope.identity.instrument_id,
                        endpoint_ns: primary.envelope.endpoint_ns,
                        ordered_distinct_sequence_vector: primary
                            .envelope
                            .ordered_distinct_sequence_vector
                            .clone(),
                        terminal_sequence: primary.envelope.terminal_sequence,
                    },
                    effective_available_ns: primary.envelope.effective_available_ns,
                    levels: primary.levels,
                };
                delays.push(primary.envelope.closure_confirmation_delay_ns);
                clarification_impact.observe(
                    primary.envelope.endpoint_ns,
                    primary.envelope.effective_available_ns,
                )?;
                publication_count += 1;
                if let Some(writer) = endpoint_writer.as_mut() {
                    writer.push(&compact)?;
                } else {
                    let mut bytes = Vec::with_capacity(384);
                    encode_endpoint(&compact, &mut bytes)?;
                    endpoint_digest.update(bytes);
                }
                Ok(())
            }
            (primary, reference) => Err(XnasConformanceError::ConformanceInvariant(format!(
                "primary/reference MBO reducer disposition mismatch at ordinal \
                 {primary_decoded_count}: primary={}, reference={}",
                describe_primary_mbo_step(&primary),
                describe_reference_mbo_step(&reference)
            ))),
        }
    })?;

    if decode_optional_primary_mbo(&mut primary_decoder, primary_decoded_count + 1)?.is_some() {
        return Err(XnasConformanceError::ConformanceInvariant(
            "primary MBO decoder retained records after reference EOF".to_owned(),
        ));
    }
    if reference_run.decoded_body_record_count != primary_decoded_count {
        return Err(XnasConformanceError::ConformanceInvariant(
            "primary/reference MBO decoded counts differ".to_owned(),
        ));
    }

    let grammar = grammar.finish();
    let primary_finish = primary_reducer.finish_report();
    let reference_finish = reference_reducer.finish();
    let reducer_parity = mbo_reducer_parity(
        &primary_finish,
        &reference_finish,
        publication_count,
        endpoint_writer.as_ref().map_or_else(
            || format!("{:x}", endpoint_digest.finalize()),
            |writer| format!("{:x}", writer.digest.clone().finalize()),
        ),
    )?;
    let endpoints = endpoint_writer.map(EndpointSpoolWriterV1::finish).transpose()?;
    let sealed_measurement_counts_match = sealed_measurements
        .map(|value| validate_sealed_mbo_measurements(&grammar, value))
        .transpose()?;
    let initial_clear_zero_contribution = primary_finish.counts.initial_xnas_clear_control_count
        == reference_finish.counts.initial_xnas_clear_control_count
        && primary_finish.counts.raw_record_count
            == primary_finish.counts.initial_xnas_clear_control_count
                + primary_finish.counts.completed_member_record_count
                + primary_finish.counts.quarantined_record_count
        && primary_finish.counts.completed_update_envelope_count == publication_count;
    if !initial_clear_zero_contribution {
        return Err(XnasConformanceError::ConformanceInvariant(
            "initial clear did not remain outside all envelope/publication populations".to_owned(),
        ));
    }

    Ok(MboScanOutcomeV1 {
        report: MboSourceConformanceV1 {
            evidence: source_evidence(image, &metadata, "mbo")?,
            reference_decoded_body_record_count: reference_run.decoded_body_record_count,
            raw_decoder_parity: RawDecoderParityV1 {
                exact_record_count: primary_decoded_count,
                primary_raw_stream_sha256: format!("{:x}", primary_raw_digest.finalize()),
                reference_raw_stream_sha256: format!("{:x}", reference_raw_digest.finalize()),
                exact: true,
            },
            grammar,
            sealed_measurement_counts_match,
            reducer_parity,
            closure_confirmation_delay_ns: summarize_delays(delays),
            clarification_impact,
            initial_clear_zero_contribution,
        },
        endpoints,
    })
}

fn decode_next_primary_mbo(
    decoder: &mut super::InMemoryDbnDecoderV1,
    ordinal: u64,
) -> Result<RawMboRecordV1, XnasConformanceError> {
    decode_optional_primary_mbo(decoder, ordinal)?.ok_or_else(|| {
        XnasConformanceError::ConformanceInvariant(format!(
            "primary MBO decoder reached EOF before reference at ordinal {ordinal}"
        ))
    })
}

fn decode_optional_primary_mbo(
    decoder: &mut super::InMemoryDbnDecoderV1,
    ordinal: u64,
) -> Result<Option<RawMboRecordV1>, XnasConformanceError> {
    let decoded = decoder
        .decode_record_ref()
        .map_err(|error| XnasConformanceError::DbnDecode(error.to_string()))?;
    let Some(record) = decoded else {
        return Ok(None);
    };
    if !record.has::<MboMsg>() || record.record_size() < size_of::<MboMsg>() {
        return Err(XnasConformanceError::DbnDecode(format!(
            "body record {ordinal} is not a complete MBO record"
        )));
    }
    let message = record
        .get::<MboMsg>()
        .expect("rtype and encoded length were checked");
    Ok(Some(RawMboRecordV1::from_dbn(
        SourceOrdinal::new(ordinal)?,
        message,
    )))
}

fn source_qualification(
    image: &QualifiedSourceImageV1,
    metadata: &QualifiedDbnMetadataV1,
    schema: XnasSchemaV1,
) -> Result<XnasDailySourceQualificationV1, XnasConformanceError> {
    Ok(XnasDailySourceQualificationV1::from_verified_images(
        schema,
        std::collections::BTreeSet::from([metadata.identity]),
        utf8_path(image.source_path(), "source")?,
        image.authority.sha256.clone(),
        utf8_path(image.manifest_path(), "manifest")?,
        image.authority.manifest_sha256.clone(),
    )?)
}

fn source_evidence(
    image: &QualifiedSourceImageV1,
    metadata: &QualifiedDbnMetadataV1,
    role: &'static str,
) -> Result<SourceEvidenceV1, XnasConformanceError> {
    Ok(SourceEvidenceV1 {
        role,
        session: metadata.session_date_yyyymmdd.clone(),
        path: utf8_path(image.source_path(), "source")?,
        size_bytes: u64::try_from(image.source_bytes().len()).map_err(|_| {
            XnasConformanceError::ConformanceInvariant(
                "source image length exceeds u64".to_owned(),
            )
        })?,
        sha256: image.authority.sha256.clone(),
        manifest_path: utf8_path(image.manifest_path(), "manifest")?,
        manifest_sha256: image.authority.manifest_sha256.clone(),
        metadata: metadata.into(),
    })
}

fn compare_mbo_publications(
    primary: &crate::xnas_semantics::PublishedMboBookV1,
    reference: &ReferenceMboPublicationV1,
    witness_ordinal: u64,
) -> Result<(), XnasConformanceError> {
    let envelope = &primary.envelope;
    let other = &reference.envelope;
    let exact = envelope.identity == other.identity
        && envelope.channel_id == other.channel_id
        && envelope.ordered_distinct_sequence_vector
            == other.ordered_distinct_sequence_vector
        && envelope.terminal_sequence == other.terminal_sequence
        && envelope.records == other.records
        && envelope.terminal_source_ordinal == other.terminal_source_ordinal
        && envelope.witness_source_ordinal == other.witness_source_ordinal
        && envelope.endpoint_ns == other.endpoint_ns
        && envelope.witness_ts_recv == other.witness_ts_recv
        && envelope.effective_available_ns == other.effective_available_ns
        && envelope.closure_confirmation_delay_ns == other.closure_confirmation_delay_ns
        && envelope.venue_sequence_block_count == other.venue_sequence_block_count
        && envelope.execution_sequence_block_count == other.execution_sequence_block_count
        && envelope.execution_carrier_count == other.execution_carrier_count
        && envelope.execution_envelope == other.execution_envelope
        && envelope.last_execution_price == other.last_execution_price
        && envelope.execution_price_change_proxy_v1 == other.execution_price_change_proxy_v1
        && primary.levels == reference.levels;
    if exact {
        Ok(())
    } else {
        Err(XnasConformanceError::ConformanceInvariant(format!(
            "primary/reference MBO publication mismatch at witness ordinal {witness_ordinal}"
        )))
    }
}

fn describe_primary_mbo_step(value: &PrimaryMboStepV1) -> String {
    match value {
        PrimaryMboStepV1::None => "none".to_owned(),
        PrimaryMboStepV1::Published(_) => "published".to_owned(),
        PrimaryMboStepV1::Rejected(code) => format!("rejected:{code}"),
    }
}

fn describe_reference_mbo_step(
    value: &Result<Option<ReferenceMboPublicationV1>, crate::xnas_reference::ReferenceSemanticErrorV1>,
) -> String {
    match value {
        Ok(None) => "none".to_owned(),
        Ok(Some(_)) => "published".to_owned(),
        Err(error) => format!("rejected:{}", error.code()),
    }
}

fn mbo_reducer_parity(
    primary: &crate::xnas_semantics::MboSemanticsFinishReportV1,
    reference: &ReferenceMboFinishReportV1,
    publication_count: u64,
    publication_stream_sha256: String,
) -> Result<ReducerParityV1, XnasConformanceError> {
    let primary_counts = serde_json::to_value(&primary.counts)
        .map_err(|error| XnasConformanceError::AuthorityJson(error.to_string()))?;
    let reference_counts = serde_json::to_value(&reference.counts)
        .map_err(|error| XnasConformanceError::AuthorityJson(error.to_string()))?;
    let primary_terminal_error_code = primary
        .terminal_error
        .as_ref()
        .map(|error| error.code().to_owned());
    let reference_terminal_error_code = reference
        .terminal_error
        .as_ref()
        .map(|error| error.code().to_owned());
    let exact =
        primary_counts == reference_counts && primary_terminal_error_code == reference_terminal_error_code;
    if !exact {
        return Err(XnasConformanceError::ConformanceInvariant(
            "primary/reference MBO final populations differ".to_owned(),
        ));
    }
    Ok(ReducerParityV1 {
        primary_counts,
        reference_counts,
        primary_terminal_error_code,
        reference_terminal_error_code,
        publication_count,
        publication_stream_sha256,
        exact,
    })
}

fn update_mbo_raw_digest(digest: &mut Sha256, record: &RawMboRecordV1) {
    digest.update(record.source_ordinal.get().to_le_bytes());
    digest.update([record.rtype]);
    digest.update(record.publisher_id.to_le_bytes());
    digest.update(record.instrument_id.to_le_bytes());
    digest.update(record.ts_event.to_le_bytes());
    digest.update(record.order_id.to_le_bytes());
    digest.update(record.price.to_le_bytes());
    digest.update(record.size.to_le_bytes());
    digest.update([record.flags, record.channel_id, record.action, record.side]);
    digest.update(record.ts_recv.to_le_bytes());
    digest.update(record.ts_in_delta.to_le_bytes());
    digest.update(record.sequence.to_le_bytes());
}

fn validate_sealed_mbo_measurements(
    observed: &MboGrammarMeasurementsV1,
    sealed: &serde_json::Value,
) -> Result<bool, XnasConformanceError> {
    let fields = [
        ("decoded_records", observed.decoded_records),
        ("sequence_blocks", observed.sequence_blocks),
        (
            "multi_record_sequence_blocks",
            observed.multi_record_sequence_blocks,
        ),
        (
            "repeated_last_sequence_blocks",
            observed.repeated_last_sequence_blocks,
        ),
        ("tfc_sequence_blocks", observed.tfc_sequence_blocks),
        (
            "tfc_all_last_sequence_blocks",
            observed.tfc_all_last_sequence_blocks,
        ),
        (
            "tfc_no_last_sequence_blocks",
            observed.tfc_no_last_sequence_blocks,
        ),
        (
            "execution_bearing_terminal_sequence_blocks",
            observed.execution_bearing_terminal_sequence_blocks,
        ),
    ];
    for (field, value) in fields {
        if sealed.get(field).and_then(serde_json::Value::as_u64) != Some(value) {
            return Err(XnasConformanceError::ConformanceInvariant(format!(
                "sealed July MBO measurement {field} does not reproduce"
            )));
        }
    }
    Ok(true)
}

fn scan_mbp10_source(
    image: &QualifiedSourceImageV1,
    reference_executable: &QualifiedReferenceExecutableV1,
    sealed_measurements: &serde_json::Value,
) -> Result<Mbp10ScanOutcomeV1, XnasConformanceError> {
    let source_bytes = image.source_bytes();
    let mut primary_decoder = DynDecoder::inferred_with_buffer(
        std::io::Cursor::new(Arc::clone(&source_bytes)),
        VersionUpgradePolicy::AsIs,
    )
    .map_err(|error| XnasConformanceError::DbnDecode(error.to_string()))?;
    let metadata = qualify_mbp10_metadata(image, primary_decoder.metadata())?;
    let qualification = source_qualification(image, &metadata, XnasSchemaV1::Mbp10)?;
    let mut primary_reducer = XnasMbp10StreamV1::new(qualification);
    let mut reference_reducer =
        ReferenceMbp10ReducerV1::new(std::collections::BTreeSet::from([metadata.identity]));
    let mut grammar = Mbp10GrammarScannerV1::default();
    let mut primary_raw_digest = Sha256::new();
    let mut reference_raw_digest = Sha256::new();
    let mut endpoint_writer = EndpointSpoolWriterV1::new()?;
    let mut primary_decoded_count = 0_u64;
    let mut publication_count = 0_u64;

    let reference_run = reference_executable.decode_mbp10_source(image, |reference_record| {
        primary_decoded_count = primary_decoded_count.checked_add(1).ok_or_else(|| {
            XnasConformanceError::ConformanceInvariant(
                "primary MBP-10 source ordinal overflow".to_owned(),
            )
        })?;
        let primary_record =
            decode_next_primary_mbp10(&mut primary_decoder, primary_decoded_count)?;
        update_mbp10_raw_digest(&mut primary_raw_digest, &primary_record);
        update_mbp10_raw_digest(&mut reference_raw_digest, &reference_record);
        if primary_record != reference_record {
            return Err(XnasConformanceError::ConformanceInvariant(format!(
                "primary/reference MBP-10 raw record mismatch at ordinal {primary_decoded_count}"
            )));
        }
        grammar.push(&reference_record);

        let primary_step = primary_reducer.push(primary_record);
        let reference_step = reference_reducer.push(reference_record);
        match (primary_step, reference_step) {
            (Ok(None), Ok(None)) => Ok(()),
            (Err(primary), Err(reference)) if primary.code() == reference.code() => Ok(()),
            (Ok(Some(primary)), Ok(Some(reference))) => {
                compare_mbp10_endpoints(&primary, &reference, primary_decoded_count)?;
                endpoint_writer.push(&CompactEndpointV1 {
                    key: CompactEndpointKeyV1 {
                        publisher_id: primary.identity.publisher_id,
                        instrument_id: primary.identity.instrument_id,
                        endpoint_ns: primary.endpoint_ns,
                        ordered_distinct_sequence_vector: primary
                            .ordered_distinct_sequence_vector
                            .clone(),
                        terminal_sequence: primary.terminal_sequence,
                    },
                    effective_available_ns: primary.effective_available_ns,
                    levels: primary.levels,
                })?;
                publication_count += 1;
                Ok(())
            }
            (primary, reference) => Err(XnasConformanceError::ConformanceInvariant(format!(
                "primary/reference MBP-10 reducer disposition mismatch at ordinal \
                 {primary_decoded_count}: primary={}, reference={}",
                describe_primary_mbp10_step(&primary),
                describe_reference_mbp10_step(&reference)
            ))),
        }
    })?;

    if decode_optional_primary_mbp10(&mut primary_decoder, primary_decoded_count + 1)?.is_some() {
        return Err(XnasConformanceError::ConformanceInvariant(
            "primary MBP-10 decoder retained records after reference EOF".to_owned(),
        ));
    }
    if reference_run.decoded_body_record_count != primary_decoded_count {
        return Err(XnasConformanceError::ConformanceInvariant(
            "primary/reference MBP-10 decoded counts differ".to_owned(),
        ));
    }

    let grammar = grammar.finish();
    let primary_finish = primary_reducer.finish_report();
    let reference_finish = reference_reducer.finish();
    let endpoint_stream_sha256 = format!("{:x}", endpoint_writer.digest.clone().finalize());
    let reducer_parity = mbp10_reducer_parity(
        &primary_finish,
        &reference_finish,
        publication_count,
        endpoint_stream_sha256,
    )?;
    let sealed_measurement_counts_match =
        validate_sealed_mbp10_measurements(&grammar, sealed_measurements)?;
    let endpoints = endpoint_writer.finish()?;
    Ok(Mbp10ScanOutcomeV1 {
        report: Mbp10SourceConformanceV1 {
            evidence: source_evidence(image, &metadata, "mbp-10")?,
            reference_decoded_body_record_count: reference_run.decoded_body_record_count,
            raw_decoder_parity: RawDecoderParityV1 {
                exact_record_count: primary_decoded_count,
                primary_raw_stream_sha256: format!("{:x}", primary_raw_digest.finalize()),
                reference_raw_stream_sha256: format!("{:x}", reference_raw_digest.finalize()),
                exact: true,
            },
            grammar,
            sealed_measurement_counts_match,
            reducer_parity,
        },
        endpoints,
    })
}

fn decode_next_primary_mbp10(
    decoder: &mut super::InMemoryDbnDecoderV1,
    ordinal: u64,
) -> Result<RawMbp10RecordV1, XnasConformanceError> {
    decode_optional_primary_mbp10(decoder, ordinal)?.ok_or_else(|| {
        XnasConformanceError::ConformanceInvariant(format!(
            "primary MBP-10 decoder reached EOF before reference at ordinal {ordinal}"
        ))
    })
}

fn decode_optional_primary_mbp10(
    decoder: &mut super::InMemoryDbnDecoderV1,
    ordinal: u64,
) -> Result<Option<RawMbp10RecordV1>, XnasConformanceError> {
    let decoded = decoder
        .decode_record_ref()
        .map_err(|error| XnasConformanceError::DbnDecode(error.to_string()))?;
    let Some(record) = decoded else {
        return Ok(None);
    };
    if !record.has::<Mbp10Msg>() || record.record_size() < size_of::<Mbp10Msg>() {
        return Err(XnasConformanceError::DbnDecode(format!(
            "body record {ordinal} is not a complete MBP-10 record"
        )));
    }
    let message = record
        .get::<Mbp10Msg>()
        .expect("rtype and encoded length were checked");
    Ok(Some(RawMbp10RecordV1::from_dbn(
        SourceOrdinal::new(ordinal)?,
        message,
    )))
}

fn compare_mbp10_endpoints(
    primary: &crate::xnas_semantics::Mbp10CompletedEndpointV1,
    reference: &ReferenceMbp10EndpointV1,
    witness_ordinal: u64,
) -> Result<(), XnasConformanceError> {
    let exact = primary.identity == reference.identity
        && primary.ordered_distinct_sequence_vector
            == reference.ordered_distinct_sequence_vector
        && primary.terminal_sequence == reference.terminal_sequence
        && primary.terminal_source_ordinal == reference.terminal_source_ordinal
        && primary.witness_source_ordinal == reference.witness_source_ordinal
        && primary.endpoint_ns == reference.endpoint_ns
        && primary.witness_ts_recv == reference.witness_ts_recv
        && primary.effective_available_ns == reference.effective_available_ns
        && primary.closure_confirmation_delay_ns == reference.closure_confirmation_delay_ns
        && primary.levels == reference.levels;
    if exact {
        Ok(())
    } else {
        Err(XnasConformanceError::ConformanceInvariant(format!(
            "primary/reference MBP-10 endpoint mismatch at witness ordinal {witness_ordinal}"
        )))
    }
}

fn describe_primary_mbp10_step(
    value: &Result<
        Option<crate::xnas_semantics::Mbp10CompletedEndpointV1>,
        crate::xnas_semantics::XnasSemanticsError,
    >,
) -> String {
    match value {
        Ok(None) => "none".to_owned(),
        Ok(Some(_)) => "published".to_owned(),
        Err(error) => format!("rejected:{}", error.code()),
    }
}

fn describe_reference_mbp10_step(
    value: &Result<
        Option<ReferenceMbp10EndpointV1>,
        crate::xnas_reference::ReferenceSemanticErrorV1,
    >,
) -> String {
    match value {
        Ok(None) => "none".to_owned(),
        Ok(Some(_)) => "published".to_owned(),
        Err(error) => format!("rejected:{}", error.code()),
    }
}

fn mbp10_reducer_parity(
    primary: &crate::xnas_semantics::Mbp10SemanticsFinishReportV1,
    reference: &ReferenceMbp10FinishReportV1,
    publication_count: u64,
    publication_stream_sha256: String,
) -> Result<ReducerParityV1, XnasConformanceError> {
    let primary_counts = serde_json::to_value(&primary.counts)
        .map_err(|error| XnasConformanceError::AuthorityJson(error.to_string()))?;
    let reference_counts = serde_json::to_value(&reference.counts)
        .map_err(|error| XnasConformanceError::AuthorityJson(error.to_string()))?;
    let primary_terminal_error_code = primary
        .terminal_error
        .as_ref()
        .map(|error| error.code().to_owned());
    let reference_terminal_error_code = reference
        .terminal_error
        .as_ref()
        .map(|error| error.code().to_owned());
    let exact =
        primary_counts == reference_counts && primary_terminal_error_code == reference_terminal_error_code;
    if !exact {
        return Err(XnasConformanceError::ConformanceInvariant(
            "primary/reference MBP-10 final populations differ".to_owned(),
        ));
    }
    Ok(ReducerParityV1 {
        primary_counts,
        reference_counts,
        primary_terminal_error_code,
        reference_terminal_error_code,
        publication_count,
        publication_stream_sha256,
        exact,
    })
}

fn update_mbp10_raw_digest(digest: &mut Sha256, record: &RawMbp10RecordV1) {
    digest.update(record.source_ordinal.get().to_le_bytes());
    digest.update([record.rtype]);
    digest.update(record.publisher_id.to_le_bytes());
    digest.update(record.instrument_id.to_le_bytes());
    digest.update(record.ts_event.to_le_bytes());
    digest.update(record.price.to_le_bytes());
    digest.update(record.size.to_le_bytes());
    digest.update([record.action, record.side, record.flags, record.depth]);
    digest.update(record.ts_recv.to_le_bytes());
    digest.update(record.ts_in_delta.to_le_bytes());
    digest.update(record.sequence.to_le_bytes());
    for level in &record.levels {
        digest.update(level.bid_px.to_le_bytes());
        digest.update(level.ask_px.to_le_bytes());
        digest.update(level.bid_sz.to_le_bytes());
        digest.update(level.ask_sz.to_le_bytes());
        digest.update(level.bid_ct.to_le_bytes());
        digest.update(level.ask_ct.to_le_bytes());
    }
}

fn validate_sealed_mbp10_measurements(
    observed: &Mbp10GrammarMeasurementsV1,
    sealed: &serde_json::Value,
) -> Result<bool, XnasConformanceError> {
    let fields = [
        ("decoded_records", observed.decoded_records),
        ("sequence_blocks", observed.sequence_blocks),
        (
            "multi_record_sequence_blocks",
            observed.multi_record_sequence_blocks,
        ),
        (
            "repeated_last_sequence_blocks",
            observed.repeated_last_sequence_blocks,
        ),
        ("tc_sequence_blocks", observed.tc_sequence_blocks),
        (
            "tc_all_last_sequence_blocks",
            observed.tc_all_last_sequence_blocks,
        ),
    ];
    for (field, value) in fields {
        if sealed.get(field).and_then(serde_json::Value::as_u64) != Some(value) {
            return Err(XnasConformanceError::ConformanceInvariant(format!(
                "sealed July MBP-10 measurement {field} does not reproduce"
            )));
        }
    }
    Ok(true)
}

#[derive(Debug, Clone, Default, Serialize)]
struct CrossSchemaMatchSummaryV1 {
    match_key_definition: &'static str,
    nearest_timestamp_matching_applied: bool,
    mbo_qualified_endpoint_count: u64,
    mbp10_qualified_endpoint_count: u64,
    mbo_endpoint_stream_sha256: String,
    mbp10_endpoint_stream_sha256: String,
    exact_one_to_one_match_count: u64,
    unmatched_mbo_endpoint_count: u64,
    unmatched_mbp10_endpoint_count: u64,
    ambiguous_key_count: u64,
    ambiguous_mbo_endpoint_count: u64,
    ambiguous_mbp10_endpoint_count: u64,
    exact_ten_level_endpoint_count: u64,
    residual_endpoint_count: u64,
    residual_class_counts: BTreeMap<String, u64>,
    residual_field_counts: BTreeMap<String, u64>,
    matched_population_reconciles: bool,
    mbo_population_reconciles: bool,
    mbp10_population_reconciles: bool,
}

#[derive(Debug)]
struct EndpointGroupV1 {
    key: CompactEndpointKeyV1,
    count: u64,
    first: CompactEndpointV1,
}

fn reconcile_endpoints(
    mbo: EndpointSpoolV1,
    mbp10: EndpointSpoolV1,
) -> Result<CrossSchemaMatchSummaryV1, XnasConformanceError> {
    let (mut mbo_reader, mbo_count, mbo_sha256) = EndpointSpoolReaderV1::new(mbo)?;
    let (mut mbp_reader, mbp_count, mbp_sha256) = EndpointSpoolReaderV1::new(mbp10)?;
    let mut mbo_group = read_endpoint_group(&mut mbo_reader)?;
    let mut mbp_group = read_endpoint_group(&mut mbp_reader)?;
    let mut summary = CrossSchemaMatchSummaryV1 {
        match_key_definition:
            "(session,publisher_id,instrument_id,endpoint_ns,ordered_distinct_sequence_vector,terminal_sequence)",
        nearest_timestamp_matching_applied: false,
        mbo_qualified_endpoint_count: mbo_count,
        mbp10_qualified_endpoint_count: mbp_count,
        mbo_endpoint_stream_sha256: mbo_sha256,
        mbp10_endpoint_stream_sha256: mbp_sha256,
        ..CrossSchemaMatchSummaryV1::default()
    };
    while mbo_group.is_some() || mbp_group.is_some() {
        match (&mbo_group, &mbp_group) {
            (Some(left), Some(right)) if left.key < right.key => {
                summary.unmatched_mbo_endpoint_count += left.count;
                mbo_group = read_endpoint_group(&mut mbo_reader)?;
            }
            (Some(left), Some(right)) if left.key > right.key => {
                summary.unmatched_mbp10_endpoint_count += right.count;
                mbp_group = read_endpoint_group(&mut mbp_reader)?;
            }
            (Some(left), Some(right)) => {
                debug_assert_eq!(left.key, right.key);
                if left.count != 1 || right.count != 1 {
                    summary.ambiguous_key_count += 1;
                    summary.ambiguous_mbo_endpoint_count += left.count;
                    summary.ambiguous_mbp10_endpoint_count += right.count;
                } else {
                    summary.exact_one_to_one_match_count += 1;
                    classify_level_residuals(&left.first.levels, &right.first.levels, &mut summary);
                }
                mbo_group = read_endpoint_group(&mut mbo_reader)?;
                mbp_group = read_endpoint_group(&mut mbp_reader)?;
            }
            (Some(left), None) => {
                summary.unmatched_mbo_endpoint_count += left.count;
                mbo_group = read_endpoint_group(&mut mbo_reader)?;
            }
            (None, Some(right)) => {
                summary.unmatched_mbp10_endpoint_count += right.count;
                mbp_group = read_endpoint_group(&mut mbp_reader)?;
            }
            (None, None) => break,
        }
    }
    summary.matched_population_reconciles = summary.exact_one_to_one_match_count
        == summary.exact_ten_level_endpoint_count + summary.residual_endpoint_count;
    summary.mbo_population_reconciles = summary.mbo_qualified_endpoint_count
        == summary.exact_one_to_one_match_count
            + summary.unmatched_mbo_endpoint_count
            + summary.ambiguous_mbo_endpoint_count;
    summary.mbp10_population_reconciles = summary.mbp10_qualified_endpoint_count
        == summary.exact_one_to_one_match_count
            + summary.unmatched_mbp10_endpoint_count
            + summary.ambiguous_mbp10_endpoint_count;
    if !summary.matched_population_reconciles
        || !summary.mbo_population_reconciles
        || !summary.mbp10_population_reconciles
    {
        return Err(XnasConformanceError::ConformanceInvariant(
            "cross-schema endpoint populations do not reconcile".to_owned(),
        ));
    }
    Ok(summary)
}

fn read_endpoint_group(
    reader: &mut EndpointSpoolReaderV1,
) -> Result<Option<EndpointGroupV1>, XnasConformanceError> {
    let Some(first) = reader.next()? else {
        return Ok(None);
    };
    // The spool is already sorted. Peeking without an unread buffer would
    // consume the first member of the next group, so the reader owns one held
    // record.
    reader_group_from_first(reader, first)
}

fn reader_group_from_first(
    reader: &mut EndpointSpoolReaderV1,
    first: CompactEndpointV1,
) -> Result<Option<EndpointGroupV1>, XnasConformanceError> {
    let key = first.key.clone();
    let mut count = 1_u64;
    while let Some(next) = reader.next()? {
        if next.key == key {
            count += 1;
        } else {
            reader.held = Some(next);
            break;
        }
    }
    Ok(Some(EndpointGroupV1 { key, count, first }))
}

fn classify_level_residuals(
    mbo: &[Mbp10LevelV1; 10],
    mbp10: &[Mbp10LevelV1; 10],
    summary: &mut CrossSchemaMatchSummaryV1,
) {
    if mbo == mbp10 {
        summary.exact_ten_level_endpoint_count += 1;
        *summary
            .residual_class_counts
            .entry("EXACT".to_owned())
            .or_default() += 1;
        return;
    }
    summary.residual_endpoint_count += 1;
    let mut price = false;
    let mut size = false;
    let mut count = false;
    for (left, right) in mbo.iter().zip(mbp10) {
        for (field, differs) in [
            ("bid_px", left.bid_px != right.bid_px),
            ("ask_px", left.ask_px != right.ask_px),
            ("bid_sz", left.bid_sz != right.bid_sz),
            ("ask_sz", left.ask_sz != right.ask_sz),
            ("bid_ct", left.bid_ct != right.bid_ct),
            ("ask_ct", left.ask_ct != right.ask_ct),
        ] {
            if differs {
                *summary
                    .residual_field_counts
                    .entry(field.to_owned())
                    .or_default() += 1;
            }
        }
        price |= left.bid_px != right.bid_px || left.ask_px != right.ask_px;
        size |= left.bid_sz != right.bid_sz || left.ask_sz != right.ask_sz;
        count |= left.bid_ct != right.bid_ct || left.ask_ct != right.ask_ct;
    }
    let class = match (price, size, count) {
        (true, false, false) => "PRICE_ONLY",
        (false, true, false) => "SIZE_ONLY",
        (false, false, true) => "ORDER_COUNT_ONLY",
        (true, true, false) => "PRICE_AND_SIZE",
        (true, false, true) => "PRICE_AND_ORDER_COUNT",
        (false, true, true) => "SIZE_AND_ORDER_COUNT",
        (true, true, true) => "PRICE_SIZE_AND_ORDER_COUNT",
        (false, false, false) => unreachable!("non-equal endpoints have one residual"),
    };
    *summary
        .residual_class_counts
        .entry(class.to_owned())
        .or_default() += 1;
}

// Metamorphic controls and no-overwrite artifact persistence follow.

/// Build and persist the exact DECISION-031 conformance outcome.
///
/// The final implementation is completed below together with the MBP-10
/// reconciliation path; this declaration keeps the public composition point
/// stable while the two source lanes are compiled independently.
pub fn run_and_persist_xnas_semantics_conformance_v1(
    _repository: &Path,
    _pass_path: &Path,
) -> Result<XnasConformanceArtifactDispositionV1, XnasConformanceError> {
    Err(XnasConformanceError::ConformanceInvariant(
        "conformance runner composition is incomplete".to_owned(),
    ))
}
