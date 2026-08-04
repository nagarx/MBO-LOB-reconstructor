//! Strict, source-bound DBN MBO ingestion.
//!
//! This path is deliberately separate from the legacy loader. It has no
//! hot-store substitution, skip-invalid mode, lossy `MboMessage` conversion,
//! or warning-only terminal state. A reusable read receipt is available only
//! after verified EOF, record-count reconciliation, and a second hash of the
//! same opened file object.

use super::{CountingReader, IO_BUFFER_SIZE};
use crate::canonical_dbn::{CanonicalDbnBridgeV1, CanonicalProjectionErrorV1};
use dbn::decode::{DbnMetadata, DecodeRecordRef, DynDecoder};
use dbn::{MboMsg, Record, SType, Schema, VersionUpgradePolicy};
use hft_mbo_event_contract::{
    classify_full_order_book, validate_raw_event, BoundPublisherPolicyV1, EventDispositionV1,
    LogicalSourceV1, OpenedReplicaV1, OpenedRepresentationV1, PublisherPolicyBindingErrorV1,
    PublisherPolicyIdV1, Sha256DigestV1, SourceDescriptorV1, SourceIdentityErrorV1,
    ValidationBoundaryClassV1, ValidationFailureV1, CANONICAL_MBO_EVENT_CONTRACT_ID,
    CANONICAL_MBO_EVENT_CONTRACT_SHA256, CANONICAL_MBO_EVENT_SCHEMA_VERSION,
    EXPECTED_MBO_RECORD_SIZE_BYTES, EXPECTED_MBO_RTYPE, XNAS_ITCH_HISTORICAL_PUBLISHER_IDS_V1,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::num::NonZeroU64;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;

type StrictDecoderV1 = DynDecoder<'static, CountingReader<BufReader<File>>>;

const NS_PER_UTC_DAY: u64 = 86_400_000_000_000;

/// One point-in-time instrument mapping verified from the opened DBN metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifiedInstrumentIdentityV1 {
    pub publisher_id: u16,
    pub instrument_id: u32,
    pub symbol: String,
}

/// Source/metadata binding for one historical XNAS MBO daily object.
///
/// This value is minted only while the strict loader owns the already
/// hash-verified file handle. Callers cannot supply a replacement instrument
/// universe independently of the bytes being decoded. Completeness additionally
/// requires an unforgeable [`CanonicalReadReceiptV1`] after EOF reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasDailyMetadataBindingV1 {
    source_object_sha256: Sha256DigestV1,
    session_start_ns: u64,
    session_end_ns: u64,
    session_date: String,
    instruments: Vec<VerifiedInstrumentIdentityV1>,
}

impl XnasDailyMetadataBindingV1 {
    pub const fn source_object_sha256(&self) -> Sha256DigestV1 {
        self.source_object_sha256
    }

    pub const fn session_start_ns(&self) -> u64 {
        self.session_start_ns
    }

    pub const fn session_end_ns(&self) -> u64 {
        self.session_end_ns
    }

    pub fn session_date(&self) -> &str {
        &self.session_date
    }

    pub fn instruments(&self) -> &[VerifiedInstrumentIdentityV1] {
        &self.instruments
    }

    pub fn contains_identity(&self, publisher_id: u16, instrument_id: u32) -> bool {
        self.instruments.iter().any(|identity| {
            identity.publisher_id == publisher_id && identity.instrument_id == instrument_id
        })
    }

    pub fn symbol_for_identity(&self, publisher_id: u16, instrument_id: u32) -> Option<&str> {
        self.instruments
            .iter()
            .find(|identity| {
                identity.publisher_id == publisher_id && identity.instrument_id == instrument_id
            })
            .map(|identity| identity.symbol.as_str())
    }
}

/// Externally established catalog identity and expected decoded population.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalSourceExpectationV1 {
    logical: LogicalSourceV1,
    expected_records: u64,
}

impl CanonicalSourceExpectationV1 {
    pub fn new(logical: LogicalSourceV1, expected_records: u64) -> Self {
        Self {
            logical,
            expected_records,
        }
    }

    pub const fn logical(&self) -> &LogicalSourceV1 {
        &self.logical
    }

    pub const fn expected_records(&self) -> u64 {
        self.expected_records
    }
}

/// Constructor namespace for the strict canonical loader.
#[derive(Debug, Default, Clone, Copy)]
pub struct StrictDbnLoaderV1;

impl StrictDbnLoaderV1 {
    /// Open and pre-verify the exact configured file before decoding metadata.
    pub fn open(
        expectation: CanonicalSourceExpectationV1,
        configured_path: impl AsRef<Path>,
        publisher_policy_id: PublisherPolicyIdV1,
    ) -> Result<StrictMboEventIteratorV1, StrictBoundaryErrorV1> {
        let path = configured_path.as_ref();
        let path_text = path
            .to_str()
            .ok_or_else(|| StrictBoundaryErrorV1::NonUtf8Path(path.to_path_buf()))?
            .to_owned();

        let mut file = File::open(path).map_err(|source| StrictBoundaryErrorV1::Open {
            path: path_text.clone(),
            source,
        })?;
        let initial_metadata =
            file.metadata()
                .map_err(|source| StrictBoundaryErrorV1::PreReadIo {
                    path: path_text.clone(),
                    source,
                })?;
        if !initial_metadata.file_type().is_file() {
            return Err(StrictBoundaryErrorV1::NotRegularFile(path_text));
        }

        let (opened_sha256, opened_bytes) =
            hash_file_from_start(&mut file).map_err(|source| StrictBoundaryErrorV1::PreReadIo {
                path: path_text.clone(),
                source,
            })?;
        if opened_bytes != initial_metadata.len() {
            return Err(StrictBoundaryErrorV1::FileChangedDuringPreVerification {
                metadata_bytes: initial_metadata.len(),
                hashed_bytes: opened_bytes,
            });
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|source| StrictBoundaryErrorV1::PreReadIo {
                path: path_text.clone(),
                source,
            })?;
        let mut post_read_file =
            file.try_clone()
                .map_err(|source| StrictBoundaryErrorV1::PreReadIo {
                    path: path_text.clone(),
                    source,
                })?;
        post_read_file.seek(SeekFrom::Start(0)).map_err(|source| {
            StrictBoundaryErrorV1::PreReadIo {
                path: path_text.clone(),
                source,
            }
        })?;

        let source = SourceDescriptorV1 {
            logical: expectation.logical,
            opened: OpenedReplicaV1 {
                configured_path: path_text.clone(),
                opened_path: path_text.clone(),
                representation: OpenedRepresentationV1::CanonicalObject,
                opened_sha256,
                opened_bytes,
            },
        };
        source.validate_strict()?;

        let bytes_read = Arc::new(AtomicU64::new(0));
        let reader = CountingReader::new(
            BufReader::with_capacity(IO_BUFFER_SIZE, file),
            Arc::clone(&bytes_read),
        );
        let decoder = DynDecoder::inferred_with_buffer(reader, VersionUpgradePolicy::AsIs)
            .map_err(StrictBoundaryErrorV1::MetadataDecode)?;
        validate_metadata(decoder.metadata(), &source.logical)?;
        let publisher_policy = BoundPublisherPolicyV1::bind(publisher_policy_id, &source)?;
        let xnas_historical_source = match publisher_policy_id {
            PublisherPolicyIdV1::RejectAll => None,
            PublisherPolicyIdV1::XnasItchHistorical => {
                Some(verify_xnas_historical_source(decoder.metadata(), &source)?)
            }
        };

        Ok(StrictMboEventIteratorV1 {
            decoder,
            post_read_file,
            source,
            publisher_policy,
            xnas_historical_source,
            expected_records: expectation.expected_records,
            decoded_records: 0,
            accepted_records: 0,
            rejected_records: 0,
            next_raw_ordinal: Some(NonZeroU64::MIN),
            bytes_read,
            terminal: false,
            saw_eof: false,
            failed: false,
        })
    }
}

/// One accepted event from a pre-verified source stream.
///
/// This value is not a publication receipt. A caller must still complete the
/// iterator and obtain [`CanonicalReadReceiptV1`] before making derived output
/// reusable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedStreamEventV1 {
    disposition: EventDispositionV1,
}

/// One losslessly decoded record that cannot enter an accepted semantic lane.
///
/// This is not an iterator failure: the raw record remains source-bound and is
/// available only so an owning policy coordinator can quarantine it, invalidate
/// the affected identity, and continue toward an EOF receipt. It can never be
/// reinterpreted as an accepted event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRejectedStreamEventV1 {
    raw: hft_mbo_event_contract::RawMboEventV1,
    failure: ValidationFailureV1,
    stage: VerifiedRejectionStageV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifiedRejectionStageV1 {
    UniversalValidation,
    FullOrderBookPolicy,
}

impl VerifiedRejectedStreamEventV1 {
    pub const fn raw(&self) -> &hft_mbo_event_contract::RawMboEventV1 {
        &self.raw
    }

    pub const fn failure(&self) -> &ValidationFailureV1 {
        &self.failure
    }

    pub const fn stage(&self) -> VerifiedRejectionStageV1 {
        self.stage
    }
}

/// Exact outcome for one decoded source record. Structural decode and source
/// failures remain iterator errors; record-local semantic rejection is data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifiedStreamRecordV1 {
    Accepted(VerifiedStreamEventV1),
    Rejected(VerifiedRejectedStreamEventV1),
}

impl VerifiedStreamRecordV1 {
    pub const fn raw(&self) -> &hft_mbo_event_contract::RawMboEventV1 {
        match self {
            Self::Accepted(event) => event.disposition().event().raw(),
            Self::Rejected(event) => event.raw(),
        }
    }

    pub const fn accepted(&self) -> Option<&VerifiedStreamEventV1> {
        match self {
            Self::Accepted(event) => Some(event),
            Self::Rejected(_) => None,
        }
    }

    pub const fn rejected(&self) -> Option<&VerifiedRejectedStreamEventV1> {
        match self {
            Self::Accepted(_) => None,
            Self::Rejected(event) => Some(event),
        }
    }
}

impl VerifiedStreamEventV1 {
    pub const fn disposition(&self) -> &EventDispositionV1 {
        &self.disposition
    }
}

/// Fused strict iterator. The first error permanently terminates the stream.
pub struct StrictMboEventIteratorV1 {
    decoder: StrictDecoderV1,
    post_read_file: File,
    source: SourceDescriptorV1,
    publisher_policy: BoundPublisherPolicyV1,
    xnas_historical_source: Option<XnasDailyMetadataBindingV1>,
    expected_records: u64,
    decoded_records: u64,
    accepted_records: u64,
    rejected_records: u64,
    next_raw_ordinal: Option<NonZeroU64>,
    bytes_read: Arc<AtomicU64>,
    terminal: bool,
    saw_eof: bool,
    failed: bool,
}

impl StrictMboEventIteratorV1 {
    pub const fn source(&self) -> &SourceDescriptorV1 {
        &self.source
    }

    pub const fn decoded_records(&self) -> u64 {
        self.decoded_records
    }

    pub const fn accepted_records(&self) -> u64 {
        self.accepted_records
    }

    pub const fn rejected_records(&self) -> u64 {
        self.rejected_records
    }

    pub const fn xnas_historical_source(&self) -> Option<&XnasDailyMetadataBindingV1> {
        self.xnas_historical_source.as_ref()
    }

    pub fn bytes_consumed(&self) -> u64 {
        self.bytes_read.load(Ordering::Relaxed)
    }

    /// Reconcile the terminal stream and mint the only success receipt.
    pub fn finish(mut self) -> Result<CanonicalReadReceiptV1, StrictBoundaryErrorV1> {
        if self.failed {
            return Err(StrictBoundaryErrorV1::CannotFinishFailedStream);
        }
        if !self.saw_eof {
            return Err(StrictBoundaryErrorV1::CannotFinishBeforeEof);
        }

        let post_metadata = self
            .post_read_file
            .metadata()
            .map_err(StrictBoundaryErrorV1::PostReadIo)?;
        if !post_metadata.file_type().is_file() {
            return Err(StrictBoundaryErrorV1::PostReadNotRegularFile);
        }
        let (post_sha256, post_bytes) = hash_file_from_start(&mut self.post_read_file)
            .map_err(StrictBoundaryErrorV1::PostReadIo)?;
        if post_sha256 != self.source.opened.opened_sha256
            || post_bytes != self.source.opened.opened_bytes
            || post_metadata.len() != self.source.opened.opened_bytes
        {
            return Err(StrictBoundaryErrorV1::SourceChangedDuringDecode {
                expected_sha256: self.source.opened.opened_sha256,
                actual_sha256: post_sha256,
                expected_bytes: self.source.opened.opened_bytes,
                actual_bytes: post_bytes,
                metadata_bytes: post_metadata.len(),
            });
        }
        if self.decoded_records != self.expected_records {
            return Err(StrictBoundaryErrorV1::RecordCountMismatch {
                expected: self.expected_records,
                actual: self.decoded_records,
            });
        }
        let bytes_consumed = self.bytes_consumed();
        if bytes_consumed != self.source.opened.opened_bytes {
            return Err(StrictBoundaryErrorV1::ByteConsumptionMismatch {
                expected: self.source.opened.opened_bytes,
                actual: bytes_consumed,
            });
        }

        if self.accepted_records.checked_add(self.rejected_records) != Some(self.decoded_records) {
            return Err(StrictBoundaryErrorV1::SemanticPopulationMismatch {
                decoded: self.decoded_records,
                accepted: self.accepted_records,
                rejected: self.rejected_records,
            });
        }

        Ok(CanonicalReadReceiptV1 {
            contract_id: CANONICAL_MBO_EVENT_CONTRACT_ID,
            contract_schema_version: CANONICAL_MBO_EVENT_SCHEMA_VERSION,
            contract_sha256: Sha256DigestV1::from_hex(CANONICAL_MBO_EVENT_CONTRACT_SHA256)
                .expect("build-time validated canonical contract digest"),
            source: self.source,
            publisher_policy_id: self.publisher_policy.id().as_str(),
            xnas_historical_source: self.xnas_historical_source,
            version_upgrade_policy: "as_is",
            expected_records: self.expected_records,
            decoded_records: self.decoded_records,
            accepted_records: self.accepted_records,
            rejected_records: self.rejected_records,
            bytes_consumed,
        })
    }

    fn fail(&mut self, error: StrictBoundaryErrorV1) -> StrictBoundaryErrorV1 {
        self.failed = true;
        self.terminal = true;
        error
    }
}

impl Iterator for StrictMboEventIteratorV1 {
    type Item = Result<VerifiedStreamRecordV1, StrictBoundaryErrorV1>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.terminal {
            return None;
        }
        let next_ordinal = match self.next_raw_ordinal {
            Some(value) => value,
            None => {
                let error = self.fail(StrictBoundaryErrorV1::RawOrdinalOverflow);
                return Some(Err(error));
            }
        };

        let record = match self.decoder.decode_record_ref() {
            Ok(Some(record)) => record,
            Ok(None) => {
                self.saw_eof = true;
                self.terminal = true;
                return None;
            }
            Err(source) => {
                let error = self.fail(StrictBoundaryErrorV1::Decode {
                    next_raw_ordinal: next_ordinal.get(),
                    source,
                });
                return Some(Err(error));
            }
        };

        self.decoded_records = match self.decoded_records.checked_add(1) {
            Some(value) => value,
            None => {
                let error = self.fail(StrictBoundaryErrorV1::RawOrdinalOverflow);
                return Some(Err(error));
            }
        };
        self.next_raw_ordinal = next_ordinal.get().checked_add(1).and_then(NonZeroU64::new);

        let rtype = record.header().rtype;
        let record_size = record.record_size();
        if rtype != EXPECTED_MBO_RTYPE || record_size != usize::from(EXPECTED_MBO_RECORD_SIZE_BYTES)
        {
            let error = self.fail(StrictBoundaryErrorV1::RecordShape {
                raw_ordinal: next_ordinal.get(),
                rtype,
                record_size,
            });
            return Some(Err(error));
        }
        let message = match record.try_get::<MboMsg>() {
            Ok(value) => value.clone(),
            Err(source) => {
                let error = self.fail(StrictBoundaryErrorV1::RecordConversion {
                    raw_ordinal: next_ordinal.get(),
                    source,
                });
                return Some(Err(error));
            }
        };
        let raw = match CanonicalDbnBridgeV1::project(
            &message,
            self.source.logical.canonical_sha256,
            next_ordinal,
        ) {
            Ok(value) => value,
            Err(source) => {
                let error = self.fail(StrictBoundaryErrorV1::Projection {
                    raw_ordinal: next_ordinal.get(),
                    source,
                });
                return Some(Err(error));
            }
        };
        if self
            .xnas_historical_source
            .as_ref()
            .is_some_and(|binding| !binding.contains_identity(raw.publisher_id, raw.instrument_id))
        {
            let error = self.fail(StrictBoundaryErrorV1::RecordIdentityNotInMetadata {
                raw_ordinal: raw.raw_ordinal,
                publisher_id: raw.publisher_id,
                instrument_id: raw.instrument_id,
            });
            return Some(Err(error));
        }
        let validated = match validate_raw_event(raw) {
            Ok(value) => value,
            Err(source) => {
                if source.reason.boundary_class() == ValidationBoundaryClassV1::SourceStreamFatal {
                    let error = self.fail(StrictBoundaryErrorV1::Validation(source));
                    return Some(Err(error));
                }
                self.rejected_records = match self.rejected_records.checked_add(1) {
                    Some(value) => value,
                    None => {
                        let error = self.fail(StrictBoundaryErrorV1::RawOrdinalOverflow);
                        return Some(Err(error));
                    }
                };
                return Some(Ok(VerifiedStreamRecordV1::Rejected(
                    VerifiedRejectedStreamEventV1 {
                        raw,
                        failure: source,
                        stage: VerifiedRejectionStageV1::UniversalValidation,
                    },
                )));
            }
        };
        let disposition = match classify_full_order_book(validated, &self.publisher_policy) {
            Ok(value) => value,
            Err(source) => {
                if source.reason.boundary_class() == ValidationBoundaryClassV1::SourceStreamFatal {
                    let error = self.fail(StrictBoundaryErrorV1::Validation(source));
                    return Some(Err(error));
                }
                self.rejected_records = match self.rejected_records.checked_add(1) {
                    Some(value) => value,
                    None => {
                        let error = self.fail(StrictBoundaryErrorV1::RawOrdinalOverflow);
                        return Some(Err(error));
                    }
                };
                return Some(Ok(VerifiedStreamRecordV1::Rejected(
                    VerifiedRejectedStreamEventV1 {
                        raw,
                        failure: source,
                        stage: VerifiedRejectionStageV1::FullOrderBookPolicy,
                    },
                )));
            }
        };
        self.accepted_records = match self.accepted_records.checked_add(1) {
            Some(value) => value,
            None => {
                let error = self.fail(StrictBoundaryErrorV1::RawOrdinalOverflow);
                return Some(Err(error));
            }
        };
        Some(Ok(VerifiedStreamRecordV1::Accepted(
            VerifiedStreamEventV1 { disposition },
        )))
    }
}

impl std::iter::FusedIterator for StrictMboEventIteratorV1 {}

/// Receipt available only after strict stream completion and reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CanonicalReadReceiptV1 {
    contract_id: &'static str,
    contract_schema_version: &'static str,
    contract_sha256: Sha256DigestV1,
    source: SourceDescriptorV1,
    publisher_policy_id: &'static str,
    xnas_historical_source: Option<XnasDailyMetadataBindingV1>,
    version_upgrade_policy: &'static str,
    expected_records: u64,
    decoded_records: u64,
    accepted_records: u64,
    rejected_records: u64,
    bytes_consumed: u64,
}

impl CanonicalReadReceiptV1 {
    pub const fn contract_id(&self) -> &'static str {
        self.contract_id
    }

    pub const fn contract_schema_version(&self) -> &'static str {
        self.contract_schema_version
    }

    pub const fn contract_sha256(&self) -> Sha256DigestV1 {
        self.contract_sha256
    }

    pub const fn source(&self) -> &SourceDescriptorV1 {
        &self.source
    }

    pub const fn publisher_policy_id(&self) -> &'static str {
        self.publisher_policy_id
    }

    pub const fn xnas_historical_source(&self) -> Option<&XnasDailyMetadataBindingV1> {
        self.xnas_historical_source.as_ref()
    }

    pub const fn version_upgrade_policy(&self) -> &'static str {
        self.version_upgrade_policy
    }

    pub const fn expected_records(&self) -> u64 {
        self.expected_records
    }

    pub const fn decoded_records(&self) -> u64 {
        self.decoded_records
    }

    pub const fn accepted_records(&self) -> u64 {
        self.accepted_records
    }

    pub const fn rejected_records(&self) -> u64 {
        self.rejected_records
    }

    pub const fn bytes_consumed(&self) -> u64 {
        self.bytes_consumed
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StrictBoundaryErrorV1 {
    #[error("configured source path is not valid UTF-8: {0:?}")]
    NonUtf8Path(std::path::PathBuf),
    #[error("failed to open configured source {path}: {source}")]
    Open {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("pre-read source verification failed for {path}: {source}")]
    PreReadIo {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("configured source is not a regular file: {0}")]
    NotRegularFile(String),
    #[error(
        "source changed during pre-verification: metadata={metadata_bytes}, hashed={hashed_bytes}"
    )]
    FileChangedDuringPreVerification {
        metadata_bytes: u64,
        hashed_bytes: u64,
    },
    #[error(transparent)]
    SourceIdentity(#[from] SourceIdentityErrorV1),
    #[error("failed to decode strict DBN metadata: {0}")]
    MetadataDecode(#[source] dbn::Error),
    #[error("DBN metadata version mismatch: expected={expected}, actual={actual}")]
    MetadataVersion { expected: u8, actual: u8 },
    #[error("DBN metadata dataset mismatch: expected={expected}, actual={actual}")]
    MetadataDataset { expected: String, actual: String },
    #[error("DBN metadata schema must be exactly Some(Mbo), found {actual}")]
    MetadataSchema { actual: String },
    #[error("DBN metadata ts_out must be false for canonical event v1")]
    MetadataTsOut,
    #[error("XNAS historical source metadata is not a complete UTC day: start={start_ns}, end={end_ns:?}")]
    XnasMetadataDayBoundary { start_ns: u64, end_ns: Option<u64> },
    #[error("XNAS historical source metadata must have no record limit")]
    XnasMetadataLimit,
    #[error("XNAS historical source metadata symbology must be raw_symbol -> instrument_id")]
    XnasMetadataSymbology,
    #[error("XNAS historical source metadata must have a nonempty unique requested-symbol set")]
    XnasMetadataSymbols,
    #[error("XNAS historical source metadata reports partial or not-found symbols")]
    XnasMetadataIncompleteSymbols,
    #[error(
        "XNAS historical source metadata has no unique active mapping for requested symbol {0}"
    )]
    XnasMetadataMapping(String),
    #[error("XNAS historical source metadata maps multiple symbols to instrument {0}")]
    XnasMetadataDuplicateInstrument(u32),
    #[error(transparent)]
    PublisherPolicy(#[from] PublisherPolicyBindingErrorV1),
    #[error("DBN decoder failed before source ordinal {next_raw_ordinal}: {source}")]
    Decode {
        next_raw_ordinal: u64,
        #[source]
        source: dbn::Error,
    },
    #[error(
        "decoded source record {raw_ordinal} has rtype={rtype:#04x}, size={record_size}; expected MBO/56"
    )]
    RecordShape {
        raw_ordinal: u64,
        rtype: u8,
        record_size: usize,
    },
    #[error("decoded source record {raw_ordinal} cannot convert to exact MboMsg: {source}")]
    RecordConversion {
        raw_ordinal: u64,
        #[source]
        source: dbn::Error,
    },
    #[error("canonical projection failed at source ordinal {raw_ordinal}: {source}")]
    Projection {
        raw_ordinal: u64,
        #[source]
        source: CanonicalProjectionErrorV1,
    },
    #[error(
        "decoded source record {raw_ordinal} identity ({publisher_id},{instrument_id}) is absent from same-file metadata"
    )]
    RecordIdentityNotInMetadata {
        raw_ordinal: u64,
        publisher_id: u16,
        instrument_id: u32,
    },
    #[error(transparent)]
    Validation(#[from] ValidationFailureV1),
    #[error("raw source ordinal overflow")]
    RawOrdinalOverflow,
    #[error("a failed strict stream cannot produce a success receipt")]
    CannotFinishFailedStream,
    #[error("strict stream must reach EOF before it can produce a success receipt")]
    CannotFinishBeforeEof,
    #[error("post-read verification failed: {0}")]
    PostReadIo(#[source] io::Error),
    #[error("opened source ceased to be a regular file during decode")]
    PostReadNotRegularFile,
    #[error(
        "source changed during decode: expected={expected_sha256}/{expected_bytes}, actual={actual_sha256}/{actual_bytes}, metadata={metadata_bytes}"
    )]
    SourceChangedDuringDecode {
        expected_sha256: Sha256DigestV1,
        actual_sha256: Sha256DigestV1,
        expected_bytes: u64,
        actual_bytes: u64,
        metadata_bytes: u64,
    },
    #[error("decoded record count mismatch: expected={expected}, actual={actual}")]
    RecordCountMismatch { expected: u64, actual: u64 },
    #[error(
        "decoded semantic population mismatch: decoded={decoded}, accepted={accepted}, rejected={rejected}"
    )]
    SemanticPopulationMismatch {
        decoded: u64,
        accepted: u64,
        rejected: u64,
    },
    #[error("compressed byte consumption mismatch: expected={expected}, actual={actual}")]
    ByteConsumptionMismatch { expected: u64, actual: u64 },
}

fn validate_metadata(
    metadata: &dbn::Metadata,
    logical: &LogicalSourceV1,
) -> Result<(), StrictBoundaryErrorV1> {
    if metadata.version != logical.dbn_version {
        return Err(StrictBoundaryErrorV1::MetadataVersion {
            expected: logical.dbn_version,
            actual: metadata.version,
        });
    }
    if metadata.dataset != logical.dataset {
        return Err(StrictBoundaryErrorV1::MetadataDataset {
            expected: logical.dataset.clone(),
            actual: metadata.dataset.clone(),
        });
    }
    if metadata.schema != Some(Schema::Mbo) {
        return Err(StrictBoundaryErrorV1::MetadataSchema {
            actual: format!("{:?}", metadata.schema),
        });
    }
    if metadata.ts_out || logical.dbn_ts_out {
        return Err(StrictBoundaryErrorV1::MetadataTsOut);
    }
    Ok(())
}

fn verify_xnas_historical_source(
    metadata: &dbn::Metadata,
    source: &SourceDescriptorV1,
) -> Result<XnasDailyMetadataBindingV1, StrictBoundaryErrorV1> {
    let end_ns = metadata.end.map(NonZeroU64::get);
    let expected_end = metadata.start.checked_add(NS_PER_UTC_DAY);
    if metadata.start % NS_PER_UTC_DAY != 0 || end_ns != expected_end {
        return Err(StrictBoundaryErrorV1::XnasMetadataDayBoundary {
            start_ns: metadata.start,
            end_ns,
        });
    }
    if metadata.limit.is_some() {
        return Err(StrictBoundaryErrorV1::XnasMetadataLimit);
    }
    if metadata.stype_in != Some(SType::RawSymbol) || metadata.stype_out != SType::InstrumentId {
        return Err(StrictBoundaryErrorV1::XnasMetadataSymbology);
    }
    let requested = metadata.symbols.iter().cloned().collect::<BTreeSet<_>>();
    if requested.is_empty() || requested.len() != metadata.symbols.len() {
        return Err(StrictBoundaryErrorV1::XnasMetadataSymbols);
    }
    if !metadata.partial.is_empty() || !metadata.not_found.is_empty() {
        return Err(StrictBoundaryErrorV1::XnasMetadataIncompleteSymbols);
    }

    let session_date = metadata.start().date();
    let mut active = BTreeMap::<String, u32>::new();
    for mapping in &metadata.mappings {
        if !requested.contains(&mapping.raw_symbol) || active.contains_key(&mapping.raw_symbol) {
            return Err(StrictBoundaryErrorV1::XnasMetadataMapping(
                mapping.raw_symbol.clone(),
            ));
        }
        let intervals = mapping
            .intervals
            .iter()
            .filter(|interval| {
                session_date >= interval.start_date && session_date < interval.end_date
            })
            .collect::<Vec<_>>();
        if intervals.len() != 1 {
            return Err(StrictBoundaryErrorV1::XnasMetadataMapping(
                mapping.raw_symbol.clone(),
            ));
        }
        let instrument_id = intervals[0]
            .symbol
            .parse::<u32>()
            .map_err(|_| StrictBoundaryErrorV1::XnasMetadataMapping(mapping.raw_symbol.clone()))?;
        active.insert(mapping.raw_symbol.clone(), instrument_id);
    }
    if active.len() != requested.len() || active.keys().ne(requested.iter()) {
        return Err(StrictBoundaryErrorV1::XnasMetadataSymbols);
    }
    let mut instrument_ids = BTreeSet::new();
    let publisher_id = *XNAS_ITCH_HISTORICAL_PUBLISHER_IDS_V1
        .first()
        .expect("build-time validated XNAS publisher allowlist");
    let instruments = active
        .into_iter()
        .map(|(symbol, instrument_id)| {
            if !instrument_ids.insert(instrument_id) {
                return Err(StrictBoundaryErrorV1::XnasMetadataDuplicateInstrument(
                    instrument_id,
                ));
            }
            Ok(VerifiedInstrumentIdentityV1 {
                publisher_id,
                instrument_id,
                symbol,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(XnasDailyMetadataBindingV1 {
        source_object_sha256: source.logical.canonical_sha256,
        session_start_ns: metadata.start,
        session_end_ns: end_ns.expect("validated complete-day end"),
        session_date: session_date.to_string(),
        instruments,
    })
}

fn hash_file_from_start(file: &mut File) -> io::Result<(Sha256DigestV1, u64)> {
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::other("source byte count overflow"))?;
        hasher.update(&buffer[..read]);
    }
    let digest: [u8; 32] = hasher.finalize().into();
    Ok((Sha256DigestV1::from_bytes(digest), total))
}
