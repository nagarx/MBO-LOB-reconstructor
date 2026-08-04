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
use dbn::{MboMsg, Record, Schema, VersionUpgradePolicy};
use hft_mbo_event_contract::{
    classify_full_order_book, validate_raw_event, BoundPublisherPolicyV1, EventDispositionV1,
    LogicalSourceV1, OpenedReplicaV1, OpenedRepresentationV1, PublisherPolicyBindingErrorV1,
    PublisherPolicyIdV1, Sha256DigestV1, SourceDescriptorV1, SourceIdentityErrorV1,
    ValidationFailureV1, CANONICAL_MBO_EVENT_CONTRACT_ID, CANONICAL_MBO_EVENT_CONTRACT_SHA256,
    CANONICAL_MBO_EVENT_SCHEMA_VERSION, EXPECTED_MBO_RECORD_SIZE_BYTES, EXPECTED_MBO_RTYPE,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::num::NonZeroU64;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;

type StrictDecoderV1 = DynDecoder<'static, CountingReader<BufReader<File>>>;

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

        Ok(StrictMboEventIteratorV1 {
            decoder,
            post_read_file,
            source,
            publisher_policy,
            expected_records: expectation.expected_records,
            decoded_records: 0,
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
    expected_records: u64,
    decoded_records: u64,
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

        Ok(CanonicalReadReceiptV1 {
            contract_id: CANONICAL_MBO_EVENT_CONTRACT_ID,
            contract_schema_version: CANONICAL_MBO_EVENT_SCHEMA_VERSION,
            contract_sha256: Sha256DigestV1::from_hex(CANONICAL_MBO_EVENT_CONTRACT_SHA256)
                .expect("build-time validated canonical contract digest"),
            source: self.source,
            publisher_policy_id: self.publisher_policy.id().as_str(),
            version_upgrade_policy: "as_is",
            expected_records: self.expected_records,
            decoded_records: self.decoded_records,
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
    type Item = Result<VerifiedStreamEventV1, StrictBoundaryErrorV1>;

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
        let validated = match validate_raw_event(raw) {
            Ok(value) => value,
            Err(source) => {
                let error = self.fail(StrictBoundaryErrorV1::Validation(source));
                return Some(Err(error));
            }
        };
        let disposition = match classify_full_order_book(validated, &self.publisher_policy) {
            Ok(value) => value,
            Err(source) => {
                let error = self.fail(StrictBoundaryErrorV1::Validation(source));
                return Some(Err(error));
            }
        };
        Some(Ok(VerifiedStreamEventV1 { disposition }))
    }
}

impl std::iter::FusedIterator for StrictMboEventIteratorV1 {}

/// Receipt available only after strict stream completion and reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CanonicalReadReceiptV1 {
    pub contract_id: &'static str,
    pub contract_schema_version: &'static str,
    pub contract_sha256: Sha256DigestV1,
    pub source: SourceDescriptorV1,
    pub publisher_policy_id: &'static str,
    pub version_upgrade_policy: &'static str,
    pub expected_records: u64,
    pub decoded_records: u64,
    pub bytes_consumed: u64,
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
