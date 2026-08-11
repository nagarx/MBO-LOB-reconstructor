//! Strict, source-bound DBN MBO ingestion.
//!
//! This path is deliberately separate from the legacy loader. It has no
//! hot-store substitution, skip-invalid mode, lossy `MboMessage` conversion,
//! or warning-only terminal state. A reusable read receipt is available only
//! after verified EOF, record-count reconciliation, and a second hash of the
//! same opened file object.

use super::catalog::{open_catalog_object_no_symlinks, verify_catalog_membership};
use super::{CatalogSelectionErrorV1, CountingReader, IO_BUFFER_SIZE};
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
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;

type StrictDecoderV1 = DynDecoder<'static, CountingReader<BufReader<File>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeFileIdentityV1 {
    device_id: u64,
    inode: u64,
    metadata_bytes: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl RuntimeFileIdentityV1 {
    fn from_opened(opened: &OpenedReplicaV1) -> Self {
        Self {
            device_id: opened.device_id,
            inode: opened.inode,
            metadata_bytes: opened.metadata_bytes,
            modified_seconds: opened.modified_seconds,
            modified_nanoseconds: opened.modified_nanoseconds,
            changed_seconds: opened.changed_seconds,
            changed_nanoseconds: opened.changed_nanoseconds,
        }
    }

    fn matches(self, metadata: &std::fs::Metadata) -> bool {
        metadata.file_type().is_file()
            && metadata.dev() == self.device_id
            && metadata.ino() == self.inode
            && metadata.len() == self.metadata_bytes
            && metadata.mtime() == self.modified_seconds
            && metadata.mtime_nsec() == self.modified_nanoseconds
            && metadata.ctime() == self.changed_seconds
            && metadata.ctime_nsec() == self.changed_nanoseconds
    }
}

const NS_PER_UTC_DAY: u64 = 86_400_000_000_000;

/// One point-in-time instrument mapping bound from DBN metadata and the
/// singleton XNAS publisher policy.
///
/// DBN metadata does not contain a publisher field. `publisher_id` is policy
/// bound here and remains independently enforced on every decoded record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasPolicyBoundInstrumentIdentityV1 {
    pub publisher_id: u16,
    pub instrument_id: u32,
    pub symbol: String,
}

/// Instrument identity declared by the selection owner before source replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasExpectedInstrumentIdentityV1 {
    publisher_id: u16,
    instrument_id: u32,
    symbol: String,
}

impl XnasExpectedInstrumentIdentityV1 {
    pub fn new(
        publisher_id: u16,
        instrument_id: u32,
        symbol: impl Into<String>,
    ) -> Result<Self, StrictBoundaryErrorV1> {
        let symbol = symbol.into();
        if publisher_id == 0 || instrument_id == 0 || !is_valid_xnas_symbol(&symbol) {
            return Err(
                StrictBoundaryErrorV1::XnasExpectedInvalidInstrumentIdentity {
                    publisher_id,
                    instrument_id,
                    symbol,
                },
            );
        }
        Ok(Self {
            publisher_id,
            instrument_id,
            symbol,
        })
    }

    pub const fn publisher_id(&self) -> u16 {
        self.publisher_id
    }

    pub const fn instrument_id(&self) -> u32 {
        self.instrument_id
    }

    pub fn symbol(&self) -> &str {
        &self.symbol
    }
}

/// Exact source/session/instrument denominator declared before replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasDailyMetadataExpectationV1 {
    /// SHA-256 of the exact compressed source bytes. This is the canonical
    /// event `source_object_sha256`, not the feature-carrier portable
    /// `source_object_id` projection.
    source_object_sha256: Sha256DigestV1,
    session_start_ns: u64,
    session_end_ns: u64,
    session_date: String,
    instruments: Vec<XnasExpectedInstrumentIdentityV1>,
}

impl XnasDailyMetadataExpectationV1 {
    pub fn new(
        source_object_sha256: Sha256DigestV1,
        session_start_ns: u64,
        session_end_ns: u64,
        session_date: impl Into<String>,
        mut instruments: Vec<XnasExpectedInstrumentIdentityV1>,
    ) -> Result<Self, StrictBoundaryErrorV1> {
        let session_date = session_date.into();
        if source_object_sha256.is_zero() {
            return Err(StrictBoundaryErrorV1::XnasExpectedZeroSourceDigest);
        }
        let derived_date =
            time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(session_start_ns))
                .map_err(
                    |_| StrictBoundaryErrorV1::XnasExpectedSessionNotCompleteUtcDay {
                        start_ns: session_start_ns,
                        end_ns: session_end_ns,
                        session_date: session_date.clone(),
                    },
                )?
                .date()
                .to_string();
        if session_start_ns % NS_PER_UTC_DAY != 0
            || session_start_ns.checked_add(NS_PER_UTC_DAY) != Some(session_end_ns)
            || session_date != derived_date
        {
            return Err(
                StrictBoundaryErrorV1::XnasExpectedSessionNotCompleteUtcDay {
                    start_ns: session_start_ns,
                    end_ns: session_end_ns,
                    session_date,
                },
            );
        }
        if instruments.is_empty() {
            return Err(StrictBoundaryErrorV1::XnasExpectedEmptyInstrumentUniverse);
        }
        instruments.sort_by(|left, right| {
            (left.publisher_id, left.instrument_id, left.symbol.as_str()).cmp(&(
                right.publisher_id,
                right.instrument_id,
                right.symbol.as_str(),
            ))
        });
        let mut identities = BTreeSet::new();
        let mut symbols = BTreeSet::new();
        for instrument in &instruments {
            if !identities.insert((instrument.publisher_id, instrument.instrument_id)) {
                return Err(
                    StrictBoundaryErrorV1::XnasExpectedDuplicateInstrumentIdentity {
                        publisher_id: instrument.publisher_id,
                        instrument_id: instrument.instrument_id,
                    },
                );
            }
            if !symbols.insert(instrument.symbol.clone()) {
                return Err(StrictBoundaryErrorV1::XnasExpectedDuplicateSymbol {
                    symbol: instrument.symbol.clone(),
                });
            }
        }
        Ok(Self {
            source_object_sha256,
            session_start_ns,
            session_end_ns,
            session_date,
            instruments,
        })
    }

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

    pub fn instruments(&self) -> &[XnasExpectedInstrumentIdentityV1] {
        &self.instruments
    }

    /// Convert a hash-verified, EOF-qualified vendor metadata binding into the
    /// exact expectation required by a later replay pass.
    pub fn from_verified_binding(
        binding: &XnasDailyMetadataBindingV1,
    ) -> Result<Self, StrictBoundaryErrorV1> {
        let instruments = binding
            .instruments
            .iter()
            .map(|identity| {
                XnasExpectedInstrumentIdentityV1::new(
                    identity.publisher_id,
                    identity.instrument_id,
                    identity.symbol.clone(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(
            binding.source_object_sha256,
            binding.session_start_ns,
            binding.session_end_ns,
            binding.session_date.clone(),
            instruments,
        )
    }
}

/// Source/metadata binding for one historical XNAS MBO daily object.
///
/// This value is minted only while the strict loader owns the already
/// hash-verified file handle. Callers cannot supply a replacement instrument
/// universe independently of the bytes being decoded. Completeness additionally
/// requires an unforgeable [`CanonicalReadReceiptV1`] after EOF reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasDailyMetadataBindingV1 {
    /// SHA-256 of the exact compressed source bytes. This is deliberately not
    /// a feature-carrier `source_object_id`.
    source_object_sha256: Sha256DigestV1,
    session_start_ns: u64,
    session_end_ns: u64,
    session_date: String,
    instruments: Vec<XnasPolicyBoundInstrumentIdentityV1>,
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

    pub fn instruments(&self) -> &[XnasPolicyBoundInstrumentIdentityV1] {
        &self.instruments
    }

    pub fn contains_identity(&self, publisher_id: u16, instrument_id: u32) -> bool {
        self.instruments
            .binary_search_by_key(&(publisher_id, instrument_id), |identity| {
                (identity.publisher_id, identity.instrument_id)
            })
            .is_ok()
    }

    pub fn symbol_for_identity(&self, publisher_id: u16, instrument_id: u32) -> Option<&str> {
        self.instruments
            .binary_search_by_key(&(publisher_id, instrument_id), |identity| {
                (identity.publisher_id, identity.instrument_id)
            })
            .ok()
            .map(|index| &self.instruments[index])
            .map(|identity| identity.symbol.as_str())
    }
}

/// Externally established catalog identity and expected decoded population.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalSourceExpectationV1 {
    logical: LogicalSourceV1,
    custody_projection_path: PathBuf,
    storage_root_path: PathBuf,
}

impl CanonicalSourceExpectationV1 {
    pub fn new(
        logical: LogicalSourceV1,
        custody_projection_path: PathBuf,
        storage_root_path: PathBuf,
    ) -> Result<Self, SourceIdentityErrorV1> {
        logical.validate_strict()?;
        Ok(Self {
            logical,
            custody_projection_path,
            storage_root_path,
        })
    }

    pub const fn logical(&self) -> &LogicalSourceV1 {
        &self.logical
    }

    pub const fn expected_records(&self) -> u64 {
        self.logical.expected_records
    }

    pub fn custody_projection_path(&self) -> &std::path::Path {
        &self.custody_projection_path
    }

    pub fn storage_root_path(&self) -> &std::path::Path {
        &self.storage_root_path
    }
}

/// Constructor namespace for the strict canonical loader.
#[derive(Debug, Default, Clone, Copy)]
pub struct StrictDbnLoaderV1;

impl StrictDbnLoaderV1 {
    /// Open a strict XNAS stream only when the independently declared daily
    /// denominator matches the admitted source and its decoded metadata.
    pub fn open_xnas_expected(
        expectation: CanonicalSourceExpectationV1,
        expected_metadata: XnasDailyMetadataExpectationV1,
    ) -> Result<StrictMboEventIteratorV1, StrictBoundaryErrorV1> {
        let policy_publisher = singleton_xnas_policy_publisher()?;
        if expected_metadata
            .instruments
            .iter()
            .any(|instrument| instrument.publisher_id != policy_publisher)
        {
            return Err(StrictBoundaryErrorV1::XnasExpectedPublisherMismatch {
                policy_publisher_id: policy_publisher,
            });
        }
        if expected_metadata.source_object_sha256 != expectation.logical.compressed_sha256 {
            return Err(StrictBoundaryErrorV1::XnasExpectedSourceDigestMismatch {
                expected: expected_metadata.source_object_sha256,
                logical_source_digest: expectation.logical.compressed_sha256,
            });
        }
        let expected_instrument_count = u64::try_from(expected_metadata.instruments.len())
            .map_err(|_| StrictBoundaryErrorV1::XnasExpectedInstrumentPopulationTooLarge)?;
        if expected_instrument_count != expectation.logical.symbols_n
            || expected_instrument_count != expectation.logical.active_instruments_n
        {
            return Err(
                StrictBoundaryErrorV1::XnasExpectedCatalogPopulationMismatch {
                    expected_instruments: expected_instrument_count,
                    catalog_symbols: expectation.logical.symbols_n,
                    catalog_active_instruments: expectation.logical.active_instruments_n,
                },
            );
        }
        if expected_metadata.instruments.len() == 1
            && expected_metadata.instruments[0].symbol
                != expectation.logical.requested_symbols_preview
        {
            return Err(
                StrictBoundaryErrorV1::XnasExpectedCatalogSingletonSymbolMismatch {
                    expected_symbol: expected_metadata.instruments[0].symbol.clone(),
                    catalog_symbol: expectation.logical.requested_symbols_preview.clone(),
                },
            );
        }
        if expected_metadata.session_start_ns != expectation.logical.metadata_start_ns
            || expected_metadata.session_end_ns != expectation.logical.metadata_end_ns
        {
            return Err(StrictBoundaryErrorV1::XnasExpectedCatalogBoundsMismatch {
                expected_start_ns: expected_metadata.session_start_ns,
                expected_end_ns: expected_metadata.session_end_ns,
                catalog_start_ns: expectation.logical.metadata_start_ns,
                catalog_end_ns: expectation.logical.metadata_end_ns,
            });
        }

        let stream = Self::open(expectation, PublisherPolicyIdV1::XnasItchHistorical)?;
        if stream.decoded_records() != 0 {
            return Err(StrictBoundaryErrorV1::XnasMetadataAdmissionAfterReplay);
        }
        let actual = stream
            .xnas_historical_source()
            .ok_or(StrictBoundaryErrorV1::MissingXnasMetadataBinding)?;
        if actual.source_object_sha256 != expected_metadata.source_object_sha256
            || actual.session_start_ns != expected_metadata.session_start_ns
            || actual.session_end_ns != expected_metadata.session_end_ns
            || actual.session_date != expected_metadata.session_date
        {
            return Err(StrictBoundaryErrorV1::XnasMetadataExpectationMismatch(
                "source/session binding",
            ));
        }
        let expected_instruments = expected_metadata
            .instruments
            .iter()
            .map(|value| {
                (
                    value.publisher_id,
                    value.instrument_id,
                    value.symbol.clone(),
                )
            })
            .collect::<Vec<_>>();
        let mut actual_instruments = actual
            .instruments
            .iter()
            .map(|value| {
                (
                    value.publisher_id,
                    value.instrument_id,
                    value.symbol.clone(),
                )
            })
            .collect::<Vec<_>>();
        actual_instruments.sort();
        if actual_instruments != expected_instruments {
            return Err(StrictBoundaryErrorV1::XnasInstrumentUniverseMismatch {
                expected: expected_instruments,
                actual: actual_instruments,
            });
        }
        Ok(stream)
    }

    /// Resolve exact catalog membership and pre-verify the derived file before decoding metadata.
    pub fn open(
        expectation: CanonicalSourceExpectationV1,
        publisher_policy_id: PublisherPolicyIdV1,
    ) -> Result<StrictMboEventIteratorV1, StrictBoundaryErrorV1> {
        verify_catalog_membership(
            &expectation.logical,
            &expectation.custody_projection_path,
            &expectation.storage_root_path,
        )?;
        let opened = open_catalog_object_no_symlinks(
            &expectation.storage_root_path,
            &expectation.logical.relative_path,
        )?;
        let path_text = opened.opened_path.clone();
        let runtime_identity = RuntimeFileIdentityV1 {
            device_id: opened.device_id,
            inode: opened.inode,
            metadata_bytes: opened.metadata_bytes,
            modified_seconds: opened.modified_seconds,
            modified_nanoseconds: opened.modified_nanoseconds,
            changed_seconds: opened.changed_seconds,
            changed_nanoseconds: opened.changed_nanoseconds,
        };
        let mut file = opened.file;
        let initial_metadata =
            file.metadata()
                .map_err(|source| StrictBoundaryErrorV1::PreReadIo {
                    path: path_text.clone(),
                    source,
                })?;
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
        let verified_metadata =
            file.metadata()
                .map_err(|source| StrictBoundaryErrorV1::PreReadIo {
                    path: path_text.clone(),
                    source,
                })?;
        if !runtime_identity.matches(&initial_metadata)
            || !runtime_identity.matches(&verified_metadata)
        {
            return Err(StrictBoundaryErrorV1::RuntimeIdentityChangedDuringPreVerification);
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

        let expected_records = expectation.logical.expected_records;
        let relative_path = expectation.logical.relative_path.clone();
        let logical = expectation.logical;
        let source = SourceDescriptorV1 {
            logical,
            opened: OpenedReplicaV1 {
                custody_projection_path: expectation
                    .custody_projection_path
                    .to_string_lossy()
                    .into_owned(),
                storage_root_path: expectation.storage_root_path.to_string_lossy().into_owned(),
                relative_path,
                opened_path: path_text.clone(),
                representation: OpenedRepresentationV1::CanonicalObject,
                opened_sha256,
                opened_bytes,
                device_id: runtime_identity.device_id,
                inode: runtime_identity.inode,
                metadata_bytes: runtime_identity.metadata_bytes,
                modified_seconds: runtime_identity.modified_seconds,
                modified_nanoseconds: runtime_identity.modified_nanoseconds,
                changed_seconds: runtime_identity.changed_seconds,
                changed_nanoseconds: runtime_identity.changed_nanoseconds,
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
            expected_records,
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
        if !RuntimeFileIdentityV1::from_opened(&self.source.opened).matches(&post_metadata) {
            return Err(StrictBoundaryErrorV1::SourceRuntimeIdentityChanged);
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
            self.source.logical.compressed_sha256,
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
            let error = self.fail(
                StrictBoundaryErrorV1::RecordIdentityOutsideMetadataAndPolicyBinding {
                    raw_ordinal: raw.raw_ordinal,
                    publisher_id: raw.publisher_id,
                    instrument_id: raw.instrument_id,
                },
            );
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
    #[error(transparent)]
    CatalogSelection(#[from] CatalogSelectionErrorV1),
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
    #[error("source runtime identity changed during pre-verification")]
    RuntimeIdentityChangedDuringPreVerification,
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
    #[error(
        "DBN metadata time bounds differ from catalog: expected=[{expected_start_ns},{expected_end_ns}), actual=[{actual_start_ns},{actual_end_ns:?})"
    )]
    MetadataCatalogBounds {
        expected_start_ns: u64,
        expected_end_ns: u64,
        actual_start_ns: u64,
        actual_end_ns: Option<u64>,
    },
    #[error(
        "DBN metadata symbol population differs from catalog: expected symbols={expected_symbols}/active={expected_active}, actual symbols={actual_symbols}/active={actual_active}"
    )]
    MetadataCatalogPopulation {
        expected_symbols: u64,
        expected_active: u64,
        actual_symbols: usize,
        actual_active: usize,
    },
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
    #[error("predeclared XNAS source digest must be nonzero")]
    XnasExpectedZeroSourceDigest,
    #[error(
        "predeclared XNAS session is not one complete UTC day: start={start_ns}, end={end_ns}, date={session_date}"
    )]
    XnasExpectedSessionNotCompleteUtcDay {
        start_ns: u64,
        end_ns: u64,
        session_date: String,
    },
    #[error("predeclared XNAS instrument universe must be nonempty")]
    XnasExpectedEmptyInstrumentUniverse,
    #[error(
        "invalid predeclared XNAS instrument identity: publisher={publisher_id}, instrument={instrument_id}, symbol={symbol:?}"
    )]
    XnasExpectedInvalidInstrumentIdentity {
        publisher_id: u16,
        instrument_id: u32,
        symbol: String,
    },
    #[error(
        "duplicate predeclared XNAS instrument identity: publisher={publisher_id}, instrument={instrument_id}"
    )]
    XnasExpectedDuplicateInstrumentIdentity {
        publisher_id: u16,
        instrument_id: u32,
    },
    #[error("duplicate predeclared XNAS symbol: {symbol}")]
    XnasExpectedDuplicateSymbol { symbol: String },
    #[error("XNAS publisher policy must contain exactly one publisher, found {actual}")]
    XnasPublisherPolicyNotSingleton { actual: usize },
    #[error(
        "predeclared publisher differs from singleton XNAS policy publisher {policy_publisher_id}"
    )]
    XnasExpectedPublisherMismatch { policy_publisher_id: u16 },
    #[error("predeclared source digest {expected} differs from logical source digest {logical_source_digest}")]
    XnasExpectedSourceDigestMismatch {
        expected: Sha256DigestV1,
        logical_source_digest: Sha256DigestV1,
    },
    #[error("predeclared XNAS instrument population is not representable as u64")]
    XnasExpectedInstrumentPopulationTooLarge,
    #[error(
        "predeclared XNAS instrument population differs from catalog: expected={expected_instruments}, catalog_symbols={catalog_symbols}, catalog_active={catalog_active_instruments}"
    )]
    XnasExpectedCatalogPopulationMismatch {
        expected_instruments: u64,
        catalog_symbols: u64,
        catalog_active_instruments: u64,
    },
    #[error(
        "predeclared singleton XNAS symbol {expected_symbol:?} differs from catalog symbol {catalog_symbol:?}"
    )]
    XnasExpectedCatalogSingletonSymbolMismatch {
        expected_symbol: String,
        catalog_symbol: String,
    },
    #[error("predeclared session bounds [{expected_start_ns},{expected_end_ns}) differ from catalog bounds [{catalog_start_ns},{catalog_end_ns})")]
    XnasExpectedCatalogBoundsMismatch {
        expected_start_ns: u64,
        expected_end_ns: u64,
        catalog_start_ns: u64,
        catalog_end_ns: u64,
    },
    #[error("strict XNAS metadata admission was attempted after record replay began")]
    XnasMetadataAdmissionAfterReplay,
    #[error("strict XNAS stream has no metadata binding")]
    MissingXnasMetadataBinding,
    #[error("predeclared XNAS metadata differs from decoded metadata: {0}")]
    XnasMetadataExpectationMismatch(&'static str),
    #[error("predeclared instrument universe differs from decoded metadata: expected={expected:?}, actual={actual:?}")]
    XnasInstrumentUniverseMismatch {
        expected: Vec<(u16, u32, String)>,
        actual: Vec<(u16, u32, String)>,
    },
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
        "decoded source record {raw_ordinal} identity ({publisher_id},{instrument_id}) is outside the metadata instrument universe and policy-bound publisher"
    )]
    RecordIdentityOutsideMetadataAndPolicyBinding {
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
    #[error("source device/inode/size/mtime/ctime identity changed during decode")]
    SourceRuntimeIdentityChanged,
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
    if metadata.start != source.logical.metadata_start_ns
        || end_ns != Some(source.logical.metadata_end_ns)
    {
        return Err(StrictBoundaryErrorV1::MetadataCatalogBounds {
            expected_start_ns: source.logical.metadata_start_ns,
            expected_end_ns: source.logical.metadata_end_ns,
            actual_start_ns: metadata.start,
            actual_end_ns: end_ns,
        });
    }
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
    if requested.is_empty()
        || requested.len() != metadata.symbols.len()
        || requested.iter().any(|symbol| !is_valid_xnas_symbol(symbol))
    {
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
        if instrument_id == 0 {
            return Err(StrictBoundaryErrorV1::XnasMetadataMapping(
                mapping.raw_symbol.clone(),
            ));
        }
        active.insert(mapping.raw_symbol.clone(), instrument_id);
    }
    if active.len() != requested.len() || active.keys().ne(requested.iter()) {
        return Err(StrictBoundaryErrorV1::XnasMetadataSymbols);
    }
    let active_len = active.len();
    let mut instrument_ids = BTreeSet::new();
    let publisher_id = singleton_xnas_policy_publisher()?;
    let mut instruments = active
        .into_iter()
        .map(|(symbol, instrument_id)| {
            if !instrument_ids.insert(instrument_id) {
                return Err(StrictBoundaryErrorV1::XnasMetadataDuplicateInstrument(
                    instrument_id,
                ));
            }
            Ok(XnasPolicyBoundInstrumentIdentityV1 {
                publisher_id,
                instrument_id,
                symbol,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    instruments.sort_by_key(|identity| (identity.publisher_id, identity.instrument_id));
    if requested.len() != source.logical.symbols_n as usize
        || active_len != source.logical.active_instruments_n as usize
        || (requested.len() == 1
            && requested.first().map(String::as_str)
                != Some(source.logical.requested_symbols_preview.as_str()))
    {
        return Err(StrictBoundaryErrorV1::MetadataCatalogPopulation {
            expected_symbols: source.logical.symbols_n,
            expected_active: source.logical.active_instruments_n,
            actual_symbols: requested.len(),
            actual_active: active_len,
        });
    }

    Ok(XnasDailyMetadataBindingV1 {
        source_object_sha256: source.logical.compressed_sha256,
        session_start_ns: metadata.start,
        session_end_ns: end_ns.expect("validated complete-day end"),
        session_date: session_date.to_string(),
        instruments,
    })
}

fn is_valid_xnas_symbol(symbol: &str) -> bool {
    !symbol.is_empty() && symbol.trim() == symbol && !symbol.chars().any(char::is_control)
}

fn singleton_xnas_policy_publisher() -> Result<u16, StrictBoundaryErrorV1> {
    match XNAS_ITCH_HISTORICAL_PUBLISHER_IDS_V1 {
        [publisher_id] => Ok(*publisher_id),
        publishers => Err(StrictBoundaryErrorV1::XnasPublisherPolicyNotSingleton {
            actual: publishers.len(),
        }),
    }
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
