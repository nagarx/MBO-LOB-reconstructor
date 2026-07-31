//! Causal XNAS.ITCH semantics for the bounded FINDING-064 recertification lane.
//!
//! This module is deliberately additive.  It does not use the legacy
//! [`crate::DbnBridge`] representation because that representation drops the
//! source identity, receive clock, flags, channel, and sequence, and maps `F`
//! to `T`.  The types below retain the pinned DBN v0.20 fields losslessly and
//! implement the narrower `XnasCompletedUpdateEnvelopeV1` contract accepted
//! in DECISION-031.
//!
//! The contract is XNAS.ITCH publisher 2 and historical daily DBN only.  It is
//! not a generic MBO packet or economic-execution boundary.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

use crate::{
    Action, BookConsistency, LobConfig, LobReconstructor, LobState, MboMessage, Side, TlobError,
    MAX_LOB_LEVELS,
};

/// XNAS.ITCH publisher identifier under the pinned Databento contract.
pub const XNAS_ITCH_PUBLISHER_ID: u16 = 2;

/// Pinned DBN v0.20 flag bits used by the bounded lane.
pub const DBN_FLAG_LAST: u8 = 0x80;
pub const DBN_FLAG_SNAPSHOT: u8 = 0x20;
pub const DBN_FLAG_BAD_TS_RECV: u8 = 0x08;
pub const DBN_FLAG_MAYBE_BAD_BOOK: u8 = 0x04;

/// Pinned DBN v0.20 record types for the two admitted schemas.
pub const DBN_RTYPE_MBO: u8 = 0xA0;
pub const DBN_RTYPE_MBP_10: u8 = 0x0A;

/// Pinned DBN sentinel values.  They are repeated here so the pure semantic
/// types remain available with `default-features = false`; conversion tests
/// assert equality to the dependency constants when `databento` is enabled.
pub const DBN_UNDEF_PRICE: i64 = i64::MAX;
pub const DBN_UNDEF_TIMESTAMP: u64 = u64::MAX;

/// Versioned wire identity for the provider-normalized envelope.
pub const XNAS_COMPLETED_UPDATE_ENVELOPE_V1: &str = "xnas_completed_update_envelope_v1";

/// Versioned wire identity for the one source-initial clear control.
pub const INITIAL_XNAS_CLEAR_CONTROL_V1: &str = "initial_xnas_clear_control_v1";

/// Versioned signal emitted when an ordinary authoritative reset begins.
pub const AUTHORITATIVE_XNAS_RESET_V1: &str = "authoritative_xnas_reset_v1";

/// A checked, one-based decoded body-record ordinal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceOrdinal(NonZeroU64);

impl SourceOrdinal {
    /// Construct a one-based source ordinal.
    pub fn new(value: u64) -> Result<Self, XnasSemanticsError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(XnasSemanticsError::ZeroSourceOrdinal)
    }

    /// Return the primitive ordinal value.
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Publisher/instrument identity.  Records from distinct identities never
/// join or close one another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct XnasIdentityV1 {
    pub publisher_id: u16,
    pub instrument_id: u32,
}

impl XnasIdentityV1 {
    pub const fn new(publisher_id: u16, instrument_id: u32) -> Self {
        Self {
            publisher_id,
            instrument_id,
        }
    }
}

/// Lossless owned projection of the pinned DBN v0.20 `MboMsg`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawMboRecordV1 {
    pub source_ordinal: SourceOrdinal,
    pub rtype: u8,
    pub publisher_id: u16,
    pub instrument_id: u32,
    pub ts_event: u64,
    pub order_id: u64,
    pub price: i64,
    pub size: u32,
    pub flags: u8,
    pub channel_id: u8,
    pub action: u8,
    pub side: u8,
    pub ts_recv: u64,
    pub ts_in_delta: i32,
    pub sequence: u32,
}

impl RawMboRecordV1 {
    pub const fn identity(&self) -> XnasIdentityV1 {
        XnasIdentityV1::new(self.publisher_id, self.instrument_id)
    }

    pub const fn is_last(&self) -> bool {
        self.flags & DBN_FLAG_LAST != 0
    }

    pub const fn is_execution_carrier(&self) -> bool {
        self.action == b'T' || self.action == b'F'
    }

    fn same_payload(&self, other: &Self) -> bool {
        self.rtype == other.rtype
            && self.publisher_id == other.publisher_id
            && self.instrument_id == other.instrument_id
            && self.ts_event == other.ts_event
            && self.order_id == other.order_id
            && self.price == other.price
            && self.size == other.size
            && self.flags == other.flags
            && self.channel_id == other.channel_id
            && self.action == other.action
            && self.side == other.side
            && self.ts_recv == other.ts_recv
            && self.ts_in_delta == other.ts_in_delta
            && self.sequence == other.sequence
    }

    #[cfg(feature = "databento")]
    pub fn from_dbn(source_ordinal: SourceOrdinal, msg: &dbn::MboMsg) -> Self {
        Self {
            source_ordinal,
            rtype: msg.hd.rtype,
            publisher_id: msg.hd.publisher_id,
            instrument_id: msg.hd.instrument_id,
            ts_event: msg.hd.ts_event,
            order_id: msg.order_id,
            price: msg.price,
            size: msg.size,
            flags: msg.flags.raw(),
            channel_id: msg.channel_id,
            action: msg.action as u8,
            side: msg.side as u8,
            ts_recv: msg.ts_recv,
            ts_in_delta: msg.ts_in_delta,
            sequence: msg.sequence,
        }
    }
}

/// One MBP-10 level copied without price conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Mbp10LevelV1 {
    pub bid_px: i64,
    pub ask_px: i64,
    pub bid_sz: u32,
    pub ask_sz: u32,
    pub bid_ct: u32,
    pub ask_ct: u32,
}

impl Default for Mbp10LevelV1 {
    fn default() -> Self {
        Self {
            bid_px: DBN_UNDEF_PRICE,
            ask_px: DBN_UNDEF_PRICE,
            bid_sz: 0,
            ask_sz: 0,
            bid_ct: 0,
            ask_ct: 0,
        }
    }
}

/// Lossless owned projection of the pinned DBN v0.20 `Mbp10Msg`.
///
/// There is intentionally no channel field.  MBP-10 is structurally
/// channel-less under the pinned schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawMbp10RecordV1 {
    pub source_ordinal: SourceOrdinal,
    pub rtype: u8,
    pub publisher_id: u16,
    pub instrument_id: u32,
    pub ts_event: u64,
    pub price: i64,
    pub size: u32,
    pub action: u8,
    pub side: u8,
    pub flags: u8,
    pub depth: u8,
    pub ts_recv: u64,
    pub ts_in_delta: i32,
    pub sequence: u32,
    pub levels: [Mbp10LevelV1; 10],
}

impl RawMbp10RecordV1 {
    pub const fn identity(&self) -> XnasIdentityV1 {
        XnasIdentityV1::new(self.publisher_id, self.instrument_id)
    }

    pub const fn is_last(&self) -> bool {
        self.flags & DBN_FLAG_LAST != 0
    }

    fn same_payload(&self, other: &Self) -> bool {
        self.rtype == other.rtype
            && self.publisher_id == other.publisher_id
            && self.instrument_id == other.instrument_id
            && self.ts_event == other.ts_event
            && self.price == other.price
            && self.size == other.size
            && self.action == other.action
            && self.side == other.side
            && self.flags == other.flags
            && self.depth == other.depth
            && self.ts_recv == other.ts_recv
            && self.ts_in_delta == other.ts_in_delta
            && self.sequence == other.sequence
            && self.levels == other.levels
    }

    #[cfg(feature = "databento")]
    pub fn from_dbn(source_ordinal: SourceOrdinal, msg: &dbn::Mbp10Msg) -> Self {
        let levels = std::array::from_fn(|idx| {
            let level = &msg.levels[idx];
            Mbp10LevelV1 {
                bid_px: level.bid_px,
                ask_px: level.ask_px,
                bid_sz: level.bid_sz,
                ask_sz: level.ask_sz,
                bid_ct: level.bid_ct,
                ask_ct: level.ask_ct,
            }
        });
        Self {
            source_ordinal,
            rtype: msg.hd.rtype,
            publisher_id: msg.hd.publisher_id,
            instrument_id: msg.hd.instrument_id,
            ts_event: msg.hd.ts_event,
            price: msg.price,
            size: msg.size,
            action: msg.action as u8,
            side: msg.side as u8,
            flags: msg.flags.raw(),
            depth: msg.depth,
            ts_recv: msg.ts_recv,
            ts_in_delta: msg.ts_in_delta,
            sequence: msg.sequence,
            levels,
        }
    }
}

/// Return the receive-clock contribution admitted to the MBP-10 causal
/// watermark. Undefined and BAD_TS_RECV clocks contribute nothing;
/// MAYBE_BAD_BOOK remains a timestamped source record and contributes before
/// its semantic quarantine.
pub(crate) fn xnas_mbp10_watermark_contribution(record: &RawMbp10RecordV1) -> Option<u64> {
    if record.ts_recv == DBN_UNDEF_TIMESTAMP || record.flags & DBN_FLAG_BAD_TS_RECV != 0 {
        None
    } else {
        Some(record.ts_recv)
    }
}

/// The two schemas admitted by the bounded conformance lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum XnasSchemaV1 {
    Mbo,
    Mbp10,
}

/// Opaque evidence token required by each strict stream.
#[derive(Debug)]
pub struct XnasDailySourceQualificationV1 {
    schema: XnasSchemaV1,
    expected_identities: BTreeSet<XnasIdentityV1>,
    source_path: String,
    source_sha256: String,
    manifest_path: String,
    manifest_sha256: String,
}

impl XnasDailySourceQualificationV1 {
    /// Construct the strict token from source and manifest images that were
    /// already read once and verified against the accepted authority.
    ///
    /// This internal constructor performs no path I/O.  Its caller must own
    /// the immutable byte images used by every decoder lane.
    pub(crate) fn from_verified_images(
        schema: XnasSchemaV1,
        expected_identities: BTreeSet<XnasIdentityV1>,
        source_path: String,
        source_sha256: String,
        manifest_path: String,
        manifest_sha256: String,
    ) -> Result<Self, XnasSemanticsError> {
        if expected_identities.is_empty()
            || expected_identities
                .iter()
                .any(|identity| identity.publisher_id != XNAS_ITCH_PUBLISHER_ID)
            || source_path.is_empty()
            || manifest_path.is_empty()
            || !is_lower_hex_sha256(&source_sha256)
            || !is_lower_hex_sha256(&manifest_sha256)
        {
            return Err(XnasSemanticsError::SourceNotQualified);
        }
        Ok(Self {
            schema,
            expected_identities,
            source_path,
            source_sha256,
            manifest_path,
            manifest_sha256,
        })
    }

    pub const fn schema(&self) -> XnasSchemaV1 {
        self.schema
    }

    pub fn expected_identities(&self) -> &BTreeSet<XnasIdentityV1> {
        &self.expected_identities
    }

    pub fn source_path(&self) -> &str {
        &self.source_path
    }

    pub fn source_sha256(&self) -> &str {
        &self.source_sha256
    }

    pub fn manifest_path(&self) -> &str {
        &self.manifest_path
    }

    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Verbatim retained initial clear control.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitialXnasClearControlV1 {
    pub schema: String,
    pub record: RawMboRecordV1,
}

/// Verbatim retained ordinary reset signal for causal consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthoritativeXnasResetV1 {
    pub schema: String,
    pub record: RawMboRecordV1,
}

/// A qualified, witnessed provider-normalized MBO update envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XnasCompletedUpdateEnvelopeV1 {
    pub schema: String,
    pub identity: XnasIdentityV1,
    pub channel_id: u8,
    pub ordered_distinct_sequence_vector: Vec<u32>,
    pub terminal_sequence: u32,
    pub records: Vec<RawMboRecordV1>,
    pub terminal_source_ordinal: SourceOrdinal,
    pub witness_source_ordinal: SourceOrdinal,
    pub endpoint_ns: u64,
    pub witness_ts_recv: u64,
    pub effective_available_ns: u64,
    pub closure_confirmation_delay_ns: u64,
    pub venue_sequence_block_count: u64,
    pub execution_sequence_block_count: u64,
    pub execution_carrier_count: u64,
    pub execution_envelope: bool,
    pub last_execution_price: Option<i64>,
    pub execution_price_change_proxy_v1: Option<u8>,
}

impl XnasCompletedUpdateEnvelopeV1 {
    pub fn is_observable_at(&self, decision_ns: u64) -> bool {
        self.effective_available_ns <= decision_ns
    }
}

/// A qualified, witnessed MBP-10 endpoint.  It is a same-provider comparator,
/// not independent truth and not MBO completion authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mbp10CompletedEndpointV1 {
    pub identity: XnasIdentityV1,
    pub ordered_distinct_sequence_vector: Vec<u32>,
    pub terminal_sequence: u32,
    pub terminal_source_ordinal: SourceOrdinal,
    pub witness_source_ordinal: SourceOrdinal,
    pub endpoint_ns: u64,
    pub witness_ts_recv: u64,
    pub effective_available_ns: u64,
    pub closure_confirmation_delay_ns: u64,
    pub levels: [Mbp10LevelV1; 10],
}

/// Stable quarantine/error reasons used by tests and the conformance artifact.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum XnasSemanticsError {
    #[error("source ordinal must be one-based")]
    ZeroSourceOrdinal,
    #[error("source ordinal mismatch: expected {expected}, observed {observed}")]
    SourceOrdinalMismatch { expected: u64, observed: u64 },
    #[error("source qualification failed")]
    SourceNotQualified,
    #[error("record type {observed:#04x} does not match expected {expected:#04x}")]
    WrongRecordType { expected: u8, observed: u8 },
    #[error("unexpected identity: publisher={publisher_id}, instrument={instrument_id}")]
    UnexpectedIdentity {
        publisher_id: u16,
        instrument_id: u32,
    },
    #[error("initial XNAS clear control signature mismatch")]
    InitialClearSignatureMismatch,
    #[error("a second or later initial-clear control was observed")]
    LaterInitialClear,
    #[error("undefined event timestamp")]
    UndefinedTsEvent,
    #[error("undefined receive timestamp")]
    UndefinedTsRecv,
    #[error("BAD_TS_RECV set")]
    BadTsRecv,
    #[error("MAYBE_BAD_BOOK set")]
    MaybeBadBook,
    #[error("identity-local receive time regressed")]
    NonMonotoneTsRecv,
    #[error("exact duplicate record")]
    ExactDuplicate,
    #[error("records in one sequence block disagree on ts_event or ts_recv")]
    BlockTimestampMismatch,
    #[error("LAST-to-non-LAST transition in one sequence block")]
    LastToNonLast,
    #[error("receive time changed before terminality")]
    ReceiveTimeChangedBeforeTerminal,
    #[error("channel changed inside an open envelope or at its witness")]
    ChannelChange,
    #[error("a reset record cannot witness the preceding envelope")]
    ResetBoundary,
    #[error("snapshot bytes are quarantined and cannot publish in this lane")]
    SnapshotBoundary,
    #[error("source gap invalidated the private book")]
    SourceGap,
    #[error("decode gap invalidated the private book")]
    DecodeGap,
    #[error("session boundary invalidated the pending candidate")]
    SessionBoundary,
    #[error("identity is invalid and requires an authoritative reset")]
    InvalidState,
    #[error("initialization did not reach a witnessed clean envelope before EOF")]
    InitializationIncompleteAtEof,
    #[error("an expected publisher/instrument identity was never observed")]
    MissingExpectedIdentity,
    #[error("terminal MBP block has no book-bearing A/C/M/R record")]
    NoBookBearingTerminalRecord,
    #[error("sequence regressed or was reused")]
    SequenceRegressionOrReuse,
    #[error("unsupported XNAS action byte {0}")]
    UnsupportedAction(u8),
    #[error("unsupported XNAS side byte {0}")]
    UnsupportedSide(u8),
    #[error("execution carrier has undefined price")]
    UndefinedExecutionPrice,
    #[error("terminal candidate reached EOF without a closure witness")]
    TerminalAtEof,
    #[error("open nonterminal candidate reached EOF")]
    OpenAtEof,
    #[error("book mutation failed: {0}")]
    BookMutation(String),
    #[error("book mutation produced a missing-order or collision anomaly")]
    BookMutationAnomaly,
    #[error("published endpoint is locked or crossed")]
    InvalidEndpointBook,
    #[error("timestamp does not fit the legacy private-book timestamp field")]
    TimestampOutOfRange,
    #[error("causal publication order regressed")]
    PublicationOrderRegression,
    #[error(
        "decision times must be strictly increasing: previous {previous}, observed {observed}"
    )]
    DecisionTimeNotStrictlyIncreasing { previous: u64, observed: u64 },
    #[error(
        "decision {decision_ns} is behind the already-consumed receive watermark {observed_watermark_ns}"
    )]
    DecisionBehindObservedPrefix {
        decision_ns: u64,
        observed_watermark_ns: u64,
    },
    #[error(
        "a record from the already-emitted decision prefix was consumed after decision {decision_ns}"
    )]
    RecordInsideClosedDecisionPrefix { decision_ns: u64 },
}

impl XnasSemanticsError {
    /// Stable reason label for artifact counters.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ZeroSourceOrdinal => "ZERO_SOURCE_ORDINAL",
            Self::SourceOrdinalMismatch { .. } => "SOURCE_ORDINAL_MISMATCH",
            Self::SourceNotQualified => "SOURCE_NOT_QUALIFIED",
            Self::WrongRecordType { .. } => "WRONG_RECORD_TYPE",
            Self::UnexpectedIdentity { .. } => "UNEXPECTED_IDENTITY",
            Self::InitialClearSignatureMismatch => "INITIAL_CLEAR_SIGNATURE_MISMATCH",
            Self::LaterInitialClear => "LATER_INITIAL_CLEAR",
            Self::UndefinedTsEvent => "UNDEFINED_TS_EVENT",
            Self::UndefinedTsRecv => "UNDEFINED_TS_RECV",
            Self::BadTsRecv => "BAD_TS_RECV",
            Self::MaybeBadBook => "MAYBE_BAD_BOOK",
            Self::NonMonotoneTsRecv => "NONMONOTONE_TS_RECV",
            Self::ExactDuplicate => "EXACT_DUPLICATE",
            Self::BlockTimestampMismatch => "BLOCK_TIMESTAMP_MISMATCH",
            Self::LastToNonLast => "LAST_TO_NON_LAST",
            Self::ReceiveTimeChangedBeforeTerminal => "RECEIVE_TIME_CHANGED_BEFORE_TERMINAL",
            Self::ChannelChange => "CHANNEL_CHANGE",
            Self::ResetBoundary => "RESET_BOUNDARY",
            Self::SnapshotBoundary => "SNAPSHOT_BOUNDARY",
            Self::SourceGap => "SOURCE_GAP",
            Self::DecodeGap => "DECODE_GAP",
            Self::SessionBoundary => "SESSION_BOUNDARY",
            Self::InvalidState => "INVALID_STATE",
            Self::InitializationIncompleteAtEof => "INITIALIZATION_INCOMPLETE_AT_EOF",
            Self::MissingExpectedIdentity => "MISSING_EXPECTED_IDENTITY",
            Self::NoBookBearingTerminalRecord => "NO_BOOK_BEARING_TERMINAL_RECORD",
            Self::SequenceRegressionOrReuse => "SEQUENCE_REGRESSION_OR_REUSE",
            Self::UnsupportedAction(_) => "UNSUPPORTED_ACTION",
            Self::UnsupportedSide(_) => "UNSUPPORTED_SIDE",
            Self::UndefinedExecutionPrice => "UNDEFINED_EXECUTION_PRICE",
            Self::TerminalAtEof => "TERMINAL_AT_EOF",
            Self::OpenAtEof => "OPEN_AT_EOF",
            Self::BookMutation(_) => "BOOK_MUTATION",
            Self::BookMutationAnomaly => "BOOK_MUTATION_ANOMALY",
            Self::InvalidEndpointBook => "INVALID_ENDPOINT_BOOK",
            Self::TimestampOutOfRange => "TIMESTAMP_OUT_OF_RANGE",
            Self::PublicationOrderRegression => "PUBLICATION_ORDER_REGRESSION",
            Self::DecisionTimeNotStrictlyIncreasing { .. } => {
                "DECISION_TIME_NOT_STRICTLY_INCREASING"
            }
            Self::DecisionBehindObservedPrefix { .. } => "DECISION_BEHIND_OBSERVED_PREFIX",
            Self::RecordInsideClosedDecisionPrefix { .. } => "RECORD_INSIDE_CLOSED_DECISION_PREFIX",
        }
    }
}

/// Explicit non-record boundaries that invalidate every carried private state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum XnasBoundaryV1 {
    SourceGap,
    DecodeGap,
    SessionBoundary,
}

impl XnasBoundaryV1 {
    const fn error(self) -> XnasSemanticsError {
        match self {
            Self::SourceGap => XnasSemanticsError::SourceGap,
            Self::DecodeGap => XnasSemanticsError::DecodeGap,
            Self::SessionBoundary => XnasSemanticsError::SessionBoundary,
        }
    }
}

/// One input record can close the prior envelope while simultaneously
/// becoming the first pending member of the next candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MboIngestDispositionV1 {
    InitialClearControl(InitialXnasClearControlV1),
    AuthoritativeReset(AuthoritativeXnasResetV1),
    Pending,
    Published(Box<PublishedMboBookV1>),
}

/// Scope invalidated by one rejected MBO record.
///
/// Identity-local semantic failures invalidate only the named private state.
/// Source qualification, framing, ordinal, and other terminal failures
/// invalidate every causal consumer attached to the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MboCausalInvalidationScopeV1 {
    Identity(XnasIdentityV1),
    All,
}

/// A rejected record remains a first-class causal outcome.
///
/// Returning the rejection together with its source identity, ordinal,
/// watermark, and invalidation scope prevents callers from silently dropping
/// the state-boundary effect while handling only the error value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MboIngestRejectionV1 {
    pub identity: XnasIdentityV1,
    pub source_ordinal: SourceOrdinal,
    pub global_watermark_after_ns: Option<u64>,
    pub invalidation_scope: MboCausalInvalidationScopeV1,
    pub error: XnasSemanticsError,
}

/// Complete result of consuming one decoded MBO body record.
///
/// This is the mandatory input to the causal midpoint adapter.  The accepted
/// variant also retains the global prefix watermark because pending records
/// and records for other identities still advance the stopping filtration.
#[must_use = "every MBO ingest outcome carries causal watermark and invalidation effects"]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MboIngestOutcomeV1 {
    Accepted {
        identity: XnasIdentityV1,
        source_ordinal: SourceOrdinal,
        global_watermark_after_ns: Option<u64>,
        disposition: MboIngestDispositionV1,
    },
    Rejected(MboIngestRejectionV1),
}

impl MboIngestOutcomeV1 {
    pub const fn global_watermark_after_ns(&self) -> Option<u64> {
        match self {
            Self::Accepted {
                global_watermark_after_ns,
                ..
            } => *global_watermark_after_ns,
            Self::Rejected(rejection) => rejection.global_watermark_after_ns,
        }
    }

    /// Result-compatible test and diagnostic convenience.
    ///
    /// Causal consumers must observe the complete outcome rather than
    /// extracting only this disposition.
    pub fn unwrap(self) -> MboIngestDispositionV1 {
        match self {
            Self::Accepted { disposition, .. } => disposition,
            Self::Rejected(rejection) => {
                panic!("called MboIngestOutcomeV1::unwrap on {}", rejection.error)
            }
        }
    }

    /// Result-compatible test and diagnostic convenience.
    pub fn unwrap_err(self) -> XnasSemanticsError {
        match self {
            Self::Accepted { .. } => {
                panic!("called MboIngestOutcomeV1::unwrap_err on an accepted outcome")
            }
            Self::Rejected(rejection) => rejection.error,
        }
    }
}

/// Three coherent populations are retained for each quarantine reason.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantinePopulationV1 {
    /// Number of distinct failure or boundary observations.
    pub incident_count: u64,
    /// Number of already-materialized open candidates invalidated.
    pub open_candidate_count: u64,
    /// Number of decoded raw records assigned to quarantine.
    pub record_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MboSemanticsCountsV1 {
    pub raw_record_count: u64,
    pub initial_xnas_clear_control_count: u64,
    pub completed_member_record_count: u64,
    pub pending_record_count: u64,
    pub quarantined_record_count: u64,
    /// Actual private-state clears: exact initial controls plus every admitted
    /// authoritative R boundary, including a reset whose envelope later
    /// remains unwitnessed or fails qualification.
    pub private_book_reset_count: u64,
    pub completed_update_envelope_count: u64,
    pub venue_sequence_block_count: u64,
    pub execution_sequence_block_count: u64,
    pub execution_envelope_count: u64,
    pub execution_carrier_count: u64,
    pub published_book_state_count: u64,
    pub first_valid_publication_ordinal: Option<SourceOrdinal>,
    pub first_valid_publication_time_ns: Option<u64>,
    pub quarantined_by_reason: BTreeMap<String, QuarantinePopulationV1>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mbp10SemanticsCountsV1 {
    pub raw_record_count: u64,
    pub completed_member_record_count: u64,
    pub pending_record_count: u64,
    pub quarantined_record_count: u64,
    pub completed_endpoint_count: u64,
    pub sequence_block_count: u64,
    pub quarantined_by_reason: BTreeMap<String, QuarantinePopulationV1>,
}

/// Final MBO populations are retained even when EOF correctly fails closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MboSemanticsFinishReportV1 {
    pub counts: MboSemanticsCountsV1,
    pub terminal_error: Option<XnasSemanticsError>,
}

/// Final MBP-10 populations are retained even when EOF correctly fails closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mbp10SemanticsFinishReportV1 {
    pub counts: Mbp10SemanticsCountsV1,
    pub terminal_error: Option<XnasSemanticsError>,
}

impl MboSemanticsCountsV1 {
    /// Every admitted raw record has exactly one retained disposition.
    pub fn population_reconciles(&self) -> bool {
        self.raw_record_count
            == self.initial_xnas_clear_control_count
                + self.completed_member_record_count
                + self.pending_record_count
                + self.quarantined_record_count
            && self.quarantined_record_count
                == self
                    .quarantined_by_reason
                    .values()
                    .map(|population| population.record_count)
                    .sum::<u64>()
    }

    fn admit_raw(&mut self) {
        self.raw_record_count += 1;
        self.pending_record_count += 1;
        debug_assert!(self.population_reconciles());
    }

    fn consume_initial_control(&mut self) {
        self.pending_record_count = self
            .pending_record_count
            .checked_sub(1)
            .expect("new initial control is pending");
        self.initial_xnas_clear_control_count += 1;
        debug_assert!(self.population_reconciles());
    }

    fn complete_pending(&mut self, records: u64) {
        self.pending_record_count = self
            .pending_record_count
            .checked_sub(records)
            .expect("completed envelope members are pending");
        self.completed_member_record_count += records;
        debug_assert!(self.population_reconciles());
    }

    fn quarantine(
        &mut self,
        error: &XnasSemanticsError,
        open_candidate_count: u64,
        record_count: u64,
    ) {
        self.pending_record_count = self
            .pending_record_count
            .checked_sub(record_count)
            .expect("quarantined MBO records are pending");
        self.quarantined_record_count += record_count;
        let population = self
            .quarantined_by_reason
            .entry(error.code().to_owned())
            .or_default();
        population.incident_count += 1;
        population.open_candidate_count += open_candidate_count;
        population.record_count += record_count;
        debug_assert!(self.population_reconciles());
    }
}

impl Mbp10SemanticsCountsV1 {
    /// Every admitted raw record has exactly one retained disposition.
    pub fn population_reconciles(&self) -> bool {
        self.raw_record_count
            == self.completed_member_record_count
                + self.pending_record_count
                + self.quarantined_record_count
            && self.quarantined_record_count
                == self
                    .quarantined_by_reason
                    .values()
                    .map(|population| population.record_count)
                    .sum::<u64>()
    }

    fn admit_raw(&mut self) {
        self.raw_record_count += 1;
        self.pending_record_count += 1;
        debug_assert!(self.population_reconciles());
    }

    fn complete_pending(&mut self, records: u64) {
        self.pending_record_count = self
            .pending_record_count
            .checked_sub(records)
            .expect("completed MBP-10 members are pending");
        self.completed_member_record_count += records;
        debug_assert!(self.population_reconciles());
    }

    fn quarantine(
        &mut self,
        error: &XnasSemanticsError,
        open_candidate_count: u64,
        record_count: u64,
    ) {
        self.pending_record_count = self
            .pending_record_count
            .checked_sub(record_count)
            .expect("quarantined MBP-10 records are pending");
        self.quarantined_record_count += record_count;
        let population = self
            .quarantined_by_reason
            .entry(error.code().to_owned())
            .or_default();
        population.incident_count += 1;
        population.open_candidate_count += open_candidate_count;
        population.record_count += record_count;
        debug_assert!(self.population_reconciles());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MboInitializationState {
    Uninitialized,
    ClearedAwaitingFirstCleanEnvelope,
    RecoveringAuthoritativeReset,
    Valid,
    Invalid,
}

#[derive(Debug)]
struct OpenMboEnvelope {
    identity: XnasIdentityV1,
    channel_id: u8,
    common_ts_recv: u64,
    records: Vec<RawMboRecordV1>,
    sequences: Vec<u32>,
    current_sequence: u32,
    current_ts_event: u64,
    current_saw_last: bool,
    completed_execution_blocks: u64,
    current_has_execution: bool,
}

impl OpenMboEnvelope {
    fn new(record: RawMboRecordV1) -> Self {
        Self {
            identity: record.identity(),
            channel_id: record.channel_id,
            common_ts_recv: record.ts_recv,
            sequences: vec![record.sequence],
            current_sequence: record.sequence,
            current_ts_event: record.ts_event,
            current_saw_last: record.is_last(),
            completed_execution_blocks: 0,
            current_has_execution: record.is_execution_carrier(),
            records: vec![record],
        }
    }

    fn append_same_block(&mut self, record: RawMboRecordV1) -> Result<(), XnasSemanticsError> {
        if record.channel_id != self.channel_id {
            return Err(XnasSemanticsError::ChannelChange);
        }
        if record.ts_event != self.current_ts_event || record.ts_recv != self.common_ts_recv {
            return Err(XnasSemanticsError::BlockTimestampMismatch);
        }
        if self.records.iter().any(|prior| prior.same_payload(&record)) {
            return Err(XnasSemanticsError::ExactDuplicate);
        }
        if self.current_saw_last && !record.is_last() {
            return Err(XnasSemanticsError::LastToNonLast);
        }
        self.current_saw_last |= record.is_last();
        self.current_has_execution |= record.is_execution_carrier();
        self.records.push(record);
        Ok(())
    }

    fn append_next_nonterminal_block(
        &mut self,
        record: RawMboRecordV1,
    ) -> Result<(), XnasSemanticsError> {
        if record.channel_id != self.channel_id {
            return Err(XnasSemanticsError::ChannelChange);
        }
        if record.sequence <= self.current_sequence {
            return Err(XnasSemanticsError::SequenceRegressionOrReuse);
        }
        if record.ts_recv != self.common_ts_recv {
            return Err(XnasSemanticsError::ReceiveTimeChangedBeforeTerminal);
        }
        if self.current_has_execution {
            self.completed_execution_blocks += 1;
        }
        self.sequences.push(record.sequence);
        self.current_sequence = record.sequence;
        self.current_ts_event = record.ts_event;
        self.current_saw_last = record.is_last();
        self.current_has_execution = record.is_execution_carrier();
        self.records.push(record);
        Ok(())
    }

    fn close(
        self,
        witness: &RawMboRecordV1,
        effective_available_ns: u64,
    ) -> Result<XnasCompletedUpdateEnvelopeV1, XnasSemanticsError> {
        if witness.channel_id != self.channel_id {
            return Err(XnasSemanticsError::ChannelChange);
        }
        if witness.sequence <= self.current_sequence {
            return Err(XnasSemanticsError::SequenceRegressionOrReuse);
        }
        if !self.current_saw_last {
            return Err(XnasSemanticsError::OpenAtEof);
        }
        let terminal_source_ordinal = self
            .records
            .last()
            .expect("open envelope is nonempty")
            .source_ordinal;
        let execution_sequence_block_count =
            self.completed_execution_blocks + u64::from(self.current_has_execution);
        let execution_carrier_count = self
            .records
            .iter()
            .filter(|record| record.is_execution_carrier())
            .count() as u64;
        let last_execution_price = self
            .records
            .iter()
            .rev()
            .find(|record| record.is_execution_carrier())
            .map(|record| record.price);
        Ok(XnasCompletedUpdateEnvelopeV1 {
            schema: XNAS_COMPLETED_UPDATE_ENVELOPE_V1.to_owned(),
            identity: self.identity,
            channel_id: self.channel_id,
            terminal_sequence: self.current_sequence,
            venue_sequence_block_count: self.sequences.len() as u64,
            ordered_distinct_sequence_vector: self.sequences,
            records: self.records,
            terminal_source_ordinal,
            witness_source_ordinal: witness.source_ordinal,
            endpoint_ns: self.common_ts_recv,
            witness_ts_recv: witness.ts_recv,
            effective_available_ns,
            closure_confirmation_delay_ns: effective_available_ns
                .checked_sub(self.common_ts_recv)
                .expect("monotone watermark is not below endpoint"),
            execution_sequence_block_count,
            execution_carrier_count,
            execution_envelope: execution_carrier_count > 0,
            last_execution_price,
            execution_price_change_proxy_v1: None,
        })
    }
}

#[derive(Debug)]
struct MboIdentityState {
    initialization: MboInitializationState,
    last_valid_ts_recv: Option<u64>,
    open: Option<OpenMboEnvelope>,
    book: Option<LobReconstructor>,
    previous_execution_price: Option<i64>,
}

impl MboIdentityState {
    fn uninitialized() -> Self {
        Self {
            initialization: MboInitializationState::Uninitialized,
            last_valid_ts_recv: None,
            open: None,
            book: None,
            previous_execution_price: None,
        }
    }

    fn clear_private_book(&mut self, levels: usize) {
        self.book = Some(LobReconstructor::with_config(
            LobConfig::new(levels)
                .with_logging(false)
                .with_skip_system_messages(false),
        ));
        self.open = None;
        self.previous_execution_price = None;
    }
}

/// Streaming primary assembler.  Global source order is checked and the
/// receive-time watermark is updated before a witness closes the prior
/// identity-local envelope.
#[derive(Debug)]
pub struct XnasMboStreamV1 {
    qualification: XnasDailySourceQualificationV1,
    book_levels: usize,
    next_ordinal: u64,
    global_watermark: Option<u64>,
    identities: BTreeMap<XnasIdentityV1, MboIdentityState>,
    causal_midpoints: BTreeMap<XnasIdentityV1, XnasCausalMidpointSeriesV1>,
    observed_identities: BTreeSet<XnasIdentityV1>,
    counts: MboSemanticsCountsV1,
    terminal_error: Option<XnasSemanticsError>,
}

impl XnasMboStreamV1 {
    pub fn new(qualification: XnasDailySourceQualificationV1) -> Self {
        let causal_midpoints = qualification
            .expected_identities()
            .iter()
            .copied()
            .map(|identity| (identity, XnasCausalMidpointSeriesV1::new(identity)))
            .collect();
        Self {
            qualification,
            book_levels: 10,
            next_ordinal: 1,
            global_watermark: None,
            identities: BTreeMap::new(),
            causal_midpoints,
            observed_identities: BTreeSet::new(),
            counts: MboSemanticsCountsV1::default(),
            terminal_error: None,
        }
    }

    pub fn counts(&self) -> &MboSemanticsCountsV1 {
        &self.counts
    }

    pub const fn global_watermark(&self) -> Option<u64> {
        self.global_watermark
    }

    /// Check both raw-record conservation and the live pending population.
    pub fn population_reconciles(&self) -> bool {
        self.counts.population_reconciles()
            && self.counts.pending_record_count
                == self
                    .identities
                    .values()
                    .filter_map(|state| state.open.as_ref())
                    .map(|open| open.records.len() as u64)
                    .sum::<u64>()
    }

    /// Consume one record into the private reducer and retain both accepted
    /// and rejected causal effects for the owning public transition.
    fn ingest_outcome(&mut self, record: RawMboRecordV1) -> MboIngestOutcomeV1 {
        let identity = record.identity();
        let source_ordinal = record.source_ordinal;
        let result = self.push_inner(record);
        let global_watermark_after_ns = self.global_watermark;
        match result {
            Ok(disposition) => MboIngestOutcomeV1::Accepted {
                identity,
                source_ordinal,
                global_watermark_after_ns,
                disposition,
            },
            Err(error) => {
                let invalidation_scope = if self.terminal_error.is_some() {
                    MboCausalInvalidationScopeV1::All
                } else {
                    MboCausalInvalidationScopeV1::Identity(identity)
                };
                MboIngestOutcomeV1::Rejected(MboIngestRejectionV1 {
                    identity,
                    source_ordinal,
                    global_watermark_after_ns,
                    invalidation_scope,
                    error,
                })
            }
        }
    }

    /// Atomically expose a record transition to every source-qualified causal
    /// midpoint consumer owned by this stream.
    ///
    /// A semantic rejection is returned as `Ok(Rejected(..))` after its
    /// invalidation has been applied.  `Err` is reserved for a causal-adapter
    /// ordering violation, which invalidates every owned series and is
    /// artifact-terminal.  No public API exposes either the raw reducer
    /// transition or a separately mutable causal series.
    pub(crate) fn push_causally(
        &mut self,
        record: RawMboRecordV1,
    ) -> Result<MboIngestOutcomeV1, XnasSemanticsError> {
        let outcome = self.ingest_outcome(record);
        if let Err(error) = self
            .causal_midpoints
            .values_mut()
            .try_for_each(|series| series.observe_outcome(&outcome))
        {
            for series in self.causal_midpoints.values_mut() {
                series.invalidate();
            }
            let _: Result<(), XnasSemanticsError> = self.fail_terminal(error.clone());
            return Err(error);
        }
        Ok(outcome)
    }

    /// Emit the source-qualified causal midpoint for one expected identity
    /// after an owning source cursor has proved the cutoff prefix complete.
    ///
    /// This primitive is deliberately non-public: a caller holding only the
    /// reducer cannot know that it consumed the complete original-order
    /// prefix through `N(t)`.  The pinned source adapter must own decoder
    /// lookahead and call this only while holding the first record that would
    /// lift the receive-time watermark above `decision_ns`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn emit_causal_midpoint_after_complete_prefix(
        &mut self,
        identity: XnasIdentityV1,
        decision_ns: u64,
    ) -> Result<Option<CausalBinnedMidpointV1>, XnasSemanticsError> {
        let result = self
            .causal_midpoints
            .get_mut(&identity)
            .ok_or(XnasSemanticsError::UnexpectedIdentity {
                publisher_id: identity.publisher_id,
                instrument_id: identity.instrument_id,
            })?
            .emit_at(decision_ns);
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                for series in self.causal_midpoints.values_mut() {
                    series.invalidate();
                }
                let _: Result<(), XnasSemanticsError> = self.fail_terminal(error.clone());
                Err(error)
            }
        }
    }

    /// Internal diagnostic retained for white-box causal tests.
    #[cfg_attr(not(test), allow(dead_code))]
    fn causal_bin_count(&self, identity: XnasIdentityV1) -> Result<usize, XnasSemanticsError> {
        self.causal_midpoints
            .get(&identity)
            .map(XnasCausalMidpointSeriesV1::bin_count)
            .ok_or(XnasSemanticsError::UnexpectedIdentity {
                publisher_id: identity.publisher_id,
                instrument_id: identity.instrument_id,
            })
    }

    fn push_inner(
        &mut self,
        record: RawMboRecordV1,
    ) -> Result<MboIngestDispositionV1, XnasSemanticsError> {
        if let Some(error) = &self.terminal_error {
            return Err(error.clone());
        }
        if self.qualification.schema() != XnasSchemaV1::Mbo {
            return self.fail_terminal(XnasSemanticsError::SourceNotQualified);
        }
        if record.source_ordinal.get() != self.next_ordinal {
            self.counts.admit_raw();
            return self.fail_terminal_admitted(
                XnasSemanticsError::SourceOrdinalMismatch {
                    expected: self.next_ordinal,
                    observed: record.source_ordinal.get(),
                },
                1,
            );
        }
        self.next_ordinal += 1;
        self.counts.admit_raw();

        let identity = record.identity();
        if !self.qualification.expected_identities().contains(&identity) {
            return self.fail_terminal_admitted(
                XnasSemanticsError::UnexpectedIdentity {
                    publisher_id: record.publisher_id,
                    instrument_id: record.instrument_id,
                },
                1,
            );
        }
        self.observed_identities.insert(identity);

        let initialization = self
            .identities
            .entry(identity)
            .or_insert_with(MboIdentityState::uninitialized)
            .initialization;
        if is_initial_xnas_clear_control(&record) {
            if matches!(initialization, MboInitializationState::Invalid) {
                return self.fail_identity_with_current(identity, XnasSemanticsError::InvalidState);
            }
            if !matches!(initialization, MboInitializationState::Uninitialized) {
                return self
                    .fail_identity_with_current(identity, XnasSemanticsError::LaterInitialClear);
            }
            let retained = InitialXnasClearControlV1 {
                schema: INITIAL_XNAS_CLEAR_CONTROL_V1.to_owned(),
                record,
            };
            let state = self
                .identities
                .get_mut(&identity)
                .expect("identity was inserted above");
            state.clear_private_book(self.book_levels);
            state.initialization = MboInitializationState::ClearedAwaitingFirstCleanEnvelope;
            self.counts.consume_initial_control();
            self.counts.private_book_reset_count += 1;
            return Ok(MboIngestDispositionV1::InitialClearControl(retained));
        }

        if matches!(initialization, MboInitializationState::Uninitialized) {
            return self
                .fail_terminal_admitted(XnasSemanticsError::InitialClearSignatureMismatch, 1);
        }
        if record.rtype != DBN_RTYPE_MBO {
            return self.fail_terminal_admitted(
                XnasSemanticsError::WrongRecordType {
                    expected: DBN_RTYPE_MBO,
                    observed: record.rtype,
                },
                1,
            );
        }
        if let Err(error) = validate_receive_clock(record.ts_recv, record.flags) {
            return self.fail_identity_with_current(identity, error);
        }

        // Every finite, non-BAD receive clock in the decoded global prefix
        // contributes to H_n even when later semantic validation quarantines
        // the record. The source cursor uses this same helper before deciding
        // whether the record lies inside N(t).
        let receive_watermark_ns = xnas_mbo_watermark_contribution(&record)
            .expect("the exact initial clear returned before receive-clock validation");
        self.global_watermark = Some(self.global_watermark.map_or(receive_watermark_ns, |prior| {
            prior.max(receive_watermark_ns)
        }));
        {
            let state = self
                .identities
                .get(&identity)
                .expect("identity initialized");
            if let Some(last) = state.last_valid_ts_recv {
                if record.ts_recv < last {
                    return self.fail_identity_with_current(
                        identity,
                        XnasSemanticsError::NonMonotoneTsRecv,
                    );
                }
            }
        }
        self.identities
            .get_mut(&identity)
            .expect("identity initialized")
            .last_valid_ts_recv = Some(record.ts_recv);

        if let Err(error) = validate_record_semantics(
            record.ts_event,
            record.flags,
            record.action,
            record.side,
            record.price,
        ) {
            return self.fail_identity_with_current(identity, error);
        }

        // An authoritative reset is a boundary, never a witness or a member of
        // a preceding candidate.  It begins a fresh recovery envelope whose
        // transaction starts from an empty private book.
        if record.action == b'R' {
            let prior_open_records = self
                .identities
                .get_mut(&identity)
                .expect("identity initialized")
                .open
                .take()
                .map_or(0, |open| open.records.len() as u64);
            if prior_open_records > 0 {
                self.counts
                    .quarantine(&XnasSemanticsError::ResetBoundary, 1, prior_open_records);
            }
            let reset = AuthoritativeXnasResetV1 {
                schema: AUTHORITATIVE_XNAS_RESET_V1.to_owned(),
                record: record.clone(),
            };
            let state = self
                .identities
                .get_mut(&identity)
                .expect("identity initialized");
            state.clear_private_book(self.book_levels);
            state.initialization = MboInitializationState::RecoveringAuthoritativeReset;
            state.last_valid_ts_recv = Some(record.ts_recv);
            self.counts.private_book_reset_count += 1;
            state.open = Some(OpenMboEnvelope::new(record));
            return Ok(MboIngestDispositionV1::AuthoritativeReset(reset));
        }

        if matches!(
            self.identities
                .get(&identity)
                .expect("identity initialized")
                .initialization,
            MboInitializationState::Invalid
        ) {
            return self.fail_identity_with_current(identity, XnasSemanticsError::InvalidState);
        }

        let state = self
            .identities
            .get_mut(&identity)
            .expect("identity initialized");
        let Some(mut open) = state.open.take() else {
            state.open = Some(OpenMboEnvelope::new(record));
            return Ok(MboIngestDispositionV1::Pending);
        };
        let prior_open_records = open.records.len() as u64;

        if record.sequence == open.current_sequence {
            if let Err(error) = open.append_same_block(record) {
                return self.fail_identity_detached(identity, error, prior_open_records + 1);
            }
            state.open = Some(open);
            return Ok(MboIngestDispositionV1::Pending);
        }

        if record.sequence < open.current_sequence {
            return self.fail_identity_detached(
                identity,
                XnasSemanticsError::SequenceRegressionOrReuse,
                prior_open_records + 1,
            );
        }

        if !open.current_saw_last {
            if let Err(error) = open.append_next_nonterminal_block(record) {
                return self.fail_identity_detached(identity, error, prior_open_records + 1);
            }
            state.open = Some(open);
            return Ok(MboIngestDispositionV1::Pending);
        }

        let effective_available_ns = self
            .global_watermark
            .expect("validated record establishes a watermark");
        let completed = match open.close(&record, effective_available_ns) {
            Ok(completed) => completed,
            Err(error) => {
                return self.fail_identity_detached(identity, error, prior_open_records + 1)
            }
        };
        let mut completed = completed;
        completed.execution_price_change_proxy_v1 = completed.last_execution_price.map(|price| {
            u8::from(
                state
                    .previous_execution_price
                    .is_some_and(|previous| previous != price),
            )
        });

        // Apply to a clone and commit the cloned book only after every
        // mutation and endpoint invariant passes.  On failure the entire
        // identity state is invalidated and neither counts nor book state
        // escape.
        let book = state.book.as_ref().ok_or(XnasSemanticsError::InvalidState);
        let (next_book, publication) =
            match book.and_then(|book| apply_xnas_envelope_transactionally(book, &completed)) {
                Ok(result) => result,
                Err(error) => {
                    return self.fail_identity_detached(
                        identity,
                        error,
                        completed.records.len() as u64 + 1,
                    )
                }
            };
        state.book = Some(next_book);
        state.initialization = MboInitializationState::Valid;
        state.open = Some(OpenMboEnvelope::new(record));
        if let Some(price) = completed.last_execution_price {
            state.previous_execution_price = Some(price);
        }

        self.counts.complete_pending(completed.records.len() as u64);
        self.counts.completed_update_envelope_count += 1;
        self.counts.venue_sequence_block_count += completed.venue_sequence_block_count;
        self.counts.execution_sequence_block_count += completed.execution_sequence_block_count;
        self.counts.execution_carrier_count += completed.execution_carrier_count;
        self.counts.execution_envelope_count += u64::from(completed.execution_envelope);
        self.counts.published_book_state_count += 1;
        if self.counts.first_valid_publication_ordinal.is_none() {
            self.counts.first_valid_publication_ordinal = Some(completed.witness_source_ordinal);
            self.counts.first_valid_publication_time_ns = Some(completed.effective_available_ns);
        }
        Ok(MboIngestDispositionV1::Published(Box::new(publication)))
    }

    /// Quarantine every open tail.  EOF never serves as a closure witness.
    pub fn finish(self) -> Result<MboSemanticsCountsV1, XnasSemanticsError> {
        let report = self.finish_report();
        match report.terminal_error {
            Some(error) => Err(error),
            None => Ok(report.counts),
        }
    }

    /// Finalize while preserving auditable populations on a fail-closed EOF.
    pub fn finish_report(mut self) -> MboSemanticsFinishReportV1 {
        let mut terminal_failure = self.terminal_error.take();
        if terminal_failure.is_none() {
            let missing_identity = self
                .qualification
                .expected_identities()
                .iter()
                .any(|expected| !self.observed_identities.contains(expected));
            if missing_identity {
                self.counts
                    .quarantine(&XnasSemanticsError::MissingExpectedIdentity, 0, 0);
                terminal_failure.get_or_insert(XnasSemanticsError::MissingExpectedIdentity);
            }
            for state in self.identities.values_mut() {
                let initialization = state.initialization;
                if let Some(open) = state.open.take() {
                    let error = if matches!(
                        initialization,
                        MboInitializationState::ClearedAwaitingFirstCleanEnvelope
                            | MboInitializationState::RecoveringAuthoritativeReset
                    ) {
                        XnasSemanticsError::InitializationIncompleteAtEof
                    } else if open.current_saw_last {
                        XnasSemanticsError::TerminalAtEof
                    } else {
                        XnasSemanticsError::OpenAtEof
                    };
                    self.counts.quarantine(&error, 1, open.records.len() as u64);
                    if matches!(
                        initialization,
                        MboInitializationState::ClearedAwaitingFirstCleanEnvelope
                            | MboInitializationState::RecoveringAuthoritativeReset
                    ) {
                        terminal_failure.get_or_insert(error);
                    }
                } else if matches!(
                    initialization,
                    MboInitializationState::ClearedAwaitingFirstCleanEnvelope
                        | MboInitializationState::RecoveringAuthoritativeReset
                ) {
                    let error = XnasSemanticsError::InitializationIncompleteAtEof;
                    self.counts.quarantine(&error, 0, 0);
                    terminal_failure.get_or_insert(error);
                } else if matches!(initialization, MboInitializationState::Invalid) {
                    terminal_failure.get_or_insert(XnasSemanticsError::InvalidState);
                }
            }
        }
        debug_assert!(self.counts.population_reconciles());
        debug_assert_eq!(self.counts.pending_record_count, 0);
        MboSemanticsFinishReportV1 {
            counts: self.counts,
            terminal_error: terminal_failure,
        }
    }

    /// Atomically invalidate book and every owned midpoint series at an
    /// external boundary.
    ///
    /// This is the only public MBO boundary transition.  Keeping the private
    /// stream mutation and all causal-consumer mutations behind one operation
    /// prevents a caller from redirecting invalidation to a throwaway series.
    /// A subsequent clean authoritative R envelope is the only
    /// ordinary-record recovery path.
    pub fn invalidate_boundary_causally(&mut self, boundary: XnasBoundaryV1) -> XnasSemanticsError {
        let error = self.invalidate_boundary_inner(boundary);
        for series in self.causal_midpoints.values_mut() {
            series.observe_boundary(boundary);
        }
        error
    }

    fn invalidate_boundary_inner(&mut self, boundary: XnasBoundaryV1) -> XnasSemanticsError {
        let error = boundary.error();
        let expected = self
            .qualification
            .expected_identities()
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let mut candidate_count = 0_u64;
        let mut record_count = 0_u64;
        for identity in expected {
            let state = self
                .identities
                .entry(identity)
                .or_insert_with(MboIdentityState::uninitialized);
            if let Some(open) = state.open.take() {
                candidate_count += 1;
                record_count += open.records.len() as u64;
            }
            state.initialization = MboInitializationState::Invalid;
            state.book = None;
            state.previous_execution_price = None;
        }
        self.counts
            .quarantine(&error, candidate_count, record_count);
        error
    }

    fn fail_terminal<T>(&mut self, error: XnasSemanticsError) -> Result<T, XnasSemanticsError> {
        let (open_candidate_count, record_count) = self.drain_all_open();
        self.counts
            .quarantine(&error, open_candidate_count, record_count);
        self.terminal_error = Some(error.clone());
        Err(error)
    }

    fn fail_terminal_admitted<T>(
        &mut self,
        error: XnasSemanticsError,
        admitted_current_records: u64,
    ) -> Result<T, XnasSemanticsError> {
        let (open_candidate_count, open_record_count) = self.drain_all_open();
        self.counts.quarantine(
            &error,
            open_candidate_count,
            open_record_count + admitted_current_records,
        );
        self.terminal_error = Some(error.clone());
        Err(error)
    }

    fn fail_identity_with_current<T>(
        &mut self,
        identity: XnasIdentityV1,
        error: XnasSemanticsError,
    ) -> Result<T, XnasSemanticsError> {
        let mut record_count = 1_u64;
        let mut open_candidate_count = 0_u64;
        if let Some(state) = self.identities.get_mut(&identity) {
            if let Some(open) = state.open.take() {
                open_candidate_count = 1;
                record_count += open.records.len() as u64;
            }
            state.initialization = MboInitializationState::Invalid;
            state.book = None;
            state.previous_execution_price = None;
        }
        self.counts
            .quarantine(&error, open_candidate_count, record_count);
        Err(error)
    }

    fn fail_identity_detached<T>(
        &mut self,
        identity: XnasIdentityV1,
        error: XnasSemanticsError,
        record_count: u64,
    ) -> Result<T, XnasSemanticsError> {
        if let Some(state) = self.identities.get_mut(&identity) {
            state.open = None;
            state.book = None;
            state.previous_execution_price = None;
            state.initialization = MboInitializationState::Invalid;
        }
        self.counts.quarantine(&error, 1, record_count);
        Err(error)
    }

    fn drain_all_open(&mut self) -> (u64, u64) {
        let mut open_candidate_count = 0_u64;
        let mut record_count = 0_u64;
        for state in self.identities.values_mut() {
            if let Some(open) = state.open.take() {
                open_candidate_count += 1;
                record_count += open.records.len() as u64;
            }
            state.initialization = MboInitializationState::Invalid;
            state.book = None;
            state.previous_execution_price = None;
        }
        (open_candidate_count, record_count)
    }
}

fn is_initial_xnas_clear_control(record: &RawMboRecordV1) -> bool {
    record.rtype == DBN_RTYPE_MBO
        && record.publisher_id == XNAS_ITCH_PUBLISHER_ID
        && record.channel_id == 0
        && record.sequence == 0
        && record.action == b'R'
        && record.side == b'N'
        && record.order_id == 0
        && record.price == DBN_UNDEF_PRICE
        && record.size == 0
        && record.ts_in_delta == 0
        && record.flags == DBN_FLAG_BAD_TS_RECV
}

/// Return the only receive-clock contribution admitted to the global causal
/// watermark. The exact source-initial control and every undefined or
/// BAD_TS_RECV clock contribute nothing. MAYBE_BAD_BOOK remains a timestamped
/// source record and therefore contributes before its semantic quarantine.
pub(crate) fn xnas_mbo_watermark_contribution(record: &RawMboRecordV1) -> Option<u64> {
    if is_initial_xnas_clear_control(record)
        || record.ts_recv == DBN_UNDEF_TIMESTAMP
        || record.flags & DBN_FLAG_BAD_TS_RECV != 0
    {
        None
    } else {
        Some(record.ts_recv)
    }
}

fn validate_receive_clock(ts_recv: u64, flags: u8) -> Result<(), XnasSemanticsError> {
    if ts_recv == DBN_UNDEF_TIMESTAMP {
        return Err(XnasSemanticsError::UndefinedTsRecv);
    }
    if flags & DBN_FLAG_BAD_TS_RECV != 0 {
        return Err(XnasSemanticsError::BadTsRecv);
    }
    Ok(())
}

fn validate_record_semantics(
    ts_event: u64,
    flags: u8,
    action: u8,
    side: u8,
    price: i64,
) -> Result<(), XnasSemanticsError> {
    if flags & DBN_FLAG_SNAPSHOT != 0 {
        return Err(XnasSemanticsError::SnapshotBoundary);
    }
    if ts_event == DBN_UNDEF_TIMESTAMP {
        return Err(XnasSemanticsError::UndefinedTsEvent);
    }
    if flags & DBN_FLAG_MAYBE_BAD_BOOK != 0 {
        return Err(XnasSemanticsError::MaybeBadBook);
    }
    validate_action_side(action, side)?;
    if matches!(action, b'T' | b'F') && price == DBN_UNDEF_PRICE {
        return Err(XnasSemanticsError::UndefinedExecutionPrice);
    }
    Ok(())
}

fn validate_action_side(action: u8, side: u8) -> Result<(), XnasSemanticsError> {
    match action {
        b'A' | b'C' | b'M' if matches!(side, b'A' | b'B') => Ok(()),
        b'R' if side == b'N' => Ok(()),
        b'T' | b'F' if matches!(side, b'A' | b'B' | b'N') => Ok(()),
        b'A' | b'C' | b'M' | b'R' | b'T' | b'F' => Err(XnasSemanticsError::UnsupportedSide(side)),
        other => Err(XnasSemanticsError::UnsupportedAction(other)),
    }
}

#[derive(Debug)]
struct OpenMbpEnvelope {
    identity: XnasIdentityV1,
    common_ts_recv: u64,
    records: Vec<RawMbp10RecordV1>,
    sequences: Vec<u32>,
    current_sequence: u32,
    current_ts_event: u64,
    current_saw_last: bool,
    current_block_start: usize,
}

impl OpenMbpEnvelope {
    fn new(record: RawMbp10RecordV1) -> Self {
        Self {
            identity: record.identity(),
            common_ts_recv: record.ts_recv,
            sequences: vec![record.sequence],
            current_sequence: record.sequence,
            current_ts_event: record.ts_event,
            current_saw_last: record.is_last(),
            current_block_start: 0,
            records: vec![record],
        }
    }

    fn append_same_block(&mut self, record: RawMbp10RecordV1) -> Result<(), XnasSemanticsError> {
        if record.ts_event != self.current_ts_event || record.ts_recv != self.common_ts_recv {
            return Err(XnasSemanticsError::BlockTimestampMismatch);
        }
        if self.records.iter().any(|prior| prior.same_payload(&record)) {
            return Err(XnasSemanticsError::ExactDuplicate);
        }
        if self.current_saw_last && !record.is_last() {
            return Err(XnasSemanticsError::LastToNonLast);
        }
        self.current_saw_last |= record.is_last();
        self.records.push(record);
        Ok(())
    }

    fn append_next_nonterminal_block(
        &mut self,
        record: RawMbp10RecordV1,
    ) -> Result<(), XnasSemanticsError> {
        if record.sequence <= self.current_sequence {
            return Err(XnasSemanticsError::SequenceRegressionOrReuse);
        }
        if record.ts_recv != self.common_ts_recv {
            return Err(XnasSemanticsError::ReceiveTimeChangedBeforeTerminal);
        }
        self.sequences.push(record.sequence);
        self.current_sequence = record.sequence;
        self.current_ts_event = record.ts_event;
        self.current_saw_last = record.is_last();
        self.current_block_start = self.records.len();
        self.records.push(record);
        Ok(())
    }

    fn close(
        self,
        witness: &RawMbp10RecordV1,
        effective_available_ns: u64,
    ) -> Result<Mbp10CompletedEndpointV1, XnasSemanticsError> {
        if witness.sequence <= self.current_sequence {
            return Err(XnasSemanticsError::SequenceRegressionOrReuse);
        }
        let terminal = self.records[self.current_block_start..]
            .iter()
            .rev()
            .find(|record| matches!(record.action, b'A' | b'C' | b'M' | b'R'))
            .ok_or(XnasSemanticsError::NoBookBearingTerminalRecord)?;
        Ok(Mbp10CompletedEndpointV1 {
            identity: self.identity,
            terminal_sequence: self.current_sequence,
            ordered_distinct_sequence_vector: self.sequences,
            terminal_source_ordinal: terminal.source_ordinal,
            witness_source_ordinal: witness.source_ordinal,
            endpoint_ns: self.common_ts_recv,
            witness_ts_recv: witness.ts_recv,
            effective_available_ns,
            closure_confirmation_delay_ns: effective_available_ns
                .checked_sub(self.common_ts_recv)
                .expect("monotone MBP watermark is not below endpoint"),
            levels: terminal.levels,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MbpValidity {
    Valid,
    Recovering,
    Invalid,
}

#[derive(Debug)]
struct MbpIdentityState {
    last_valid_ts_recv: Option<u64>,
    open: Option<OpenMbpEnvelope>,
    validity: MbpValidity,
}

impl Default for MbpIdentityState {
    fn default() -> Self {
        Self {
            last_valid_ts_recv: None,
            open: None,
            validity: MbpValidity::Valid,
        }
    }
}

/// Structurally channel-less MBP-10 corroboration assembler.
#[derive(Debug)]
pub struct XnasMbp10StreamV1 {
    qualification: XnasDailySourceQualificationV1,
    next_ordinal: u64,
    global_watermark: Option<u64>,
    identities: BTreeMap<XnasIdentityV1, MbpIdentityState>,
    observed_identities: BTreeSet<XnasIdentityV1>,
    counts: Mbp10SemanticsCountsV1,
    terminal_error: Option<XnasSemanticsError>,
}

impl XnasMbp10StreamV1 {
    pub fn new(qualification: XnasDailySourceQualificationV1) -> Self {
        Self {
            qualification,
            next_ordinal: 1,
            global_watermark: None,
            identities: BTreeMap::new(),
            observed_identities: BTreeSet::new(),
            counts: Mbp10SemanticsCountsV1::default(),
            terminal_error: None,
        }
    }

    pub fn counts(&self) -> &Mbp10SemanticsCountsV1 {
        &self.counts
    }

    pub const fn global_watermark(&self) -> Option<u64> {
        self.global_watermark
    }

    pub(crate) fn terminal_error(&self) -> Option<&XnasSemanticsError> {
        self.terminal_error.as_ref()
    }

    /// Check both raw-record conservation and the live pending population.
    pub fn population_reconciles(&self) -> bool {
        self.counts.population_reconciles()
            && self.counts.pending_record_count
                == self
                    .identities
                    .values()
                    .filter_map(|state| state.open.as_ref())
                    .map(|open| open.records.len() as u64)
                    .sum::<u64>()
    }

    pub fn push(
        &mut self,
        record: RawMbp10RecordV1,
    ) -> Result<Option<Mbp10CompletedEndpointV1>, XnasSemanticsError> {
        if let Some(error) = &self.terminal_error {
            return Err(error.clone());
        }
        if self.qualification.schema() != XnasSchemaV1::Mbp10 {
            return self.fail_terminal(XnasSemanticsError::SourceNotQualified);
        }
        if record.source_ordinal.get() != self.next_ordinal {
            self.counts.admit_raw();
            return self.fail_terminal_admitted(
                XnasSemanticsError::SourceOrdinalMismatch {
                    expected: self.next_ordinal,
                    observed: record.source_ordinal.get(),
                },
                1,
            );
        }
        self.next_ordinal += 1;
        self.counts.admit_raw();
        let identity = record.identity();
        if !self.qualification.expected_identities().contains(&identity) {
            return self.fail_terminal_admitted(
                XnasSemanticsError::UnexpectedIdentity {
                    publisher_id: record.publisher_id,
                    instrument_id: record.instrument_id,
                },
                1,
            );
        }
        self.observed_identities.insert(identity);
        self.identities.entry(identity).or_default();
        if record.rtype != DBN_RTYPE_MBP_10 {
            return self.fail_terminal_admitted(
                XnasSemanticsError::WrongRecordType {
                    expected: DBN_RTYPE_MBP_10,
                    observed: record.rtype,
                },
                1,
            );
        }
        if let Err(error) = validate_receive_clock(record.ts_recv, record.flags) {
            return self.fail_identity_with_current(identity, error);
        }

        let receive_watermark_ns = xnas_mbp10_watermark_contribution(&record)
            .expect("validated MBP record has a finite non-BAD receive clock");
        self.global_watermark = Some(self.global_watermark.map_or(receive_watermark_ns, |prior| {
            prior.max(receive_watermark_ns)
        }));
        {
            let state = self.identities.get(&identity).expect("MBP identity exists");
            if let Some(last) = state.last_valid_ts_recv {
                if record.ts_recv < last {
                    return self.fail_identity_with_current(
                        identity,
                        XnasSemanticsError::NonMonotoneTsRecv,
                    );
                }
            }
        }
        self.identities
            .get_mut(&identity)
            .expect("MBP identity exists")
            .last_valid_ts_recv = Some(record.ts_recv);

        if let Err(error) = validate_record_semantics(
            record.ts_event,
            record.flags,
            record.action,
            record.side,
            record.price,
        ) {
            return self.fail_identity_with_current(identity, error);
        }

        if record.action == b'R' {
            let prior_open_records = self
                .identities
                .get_mut(&identity)
                .expect("MBP identity exists")
                .open
                .take()
                .map_or(0, |open| open.records.len() as u64);
            if prior_open_records > 0 {
                self.counts
                    .quarantine(&XnasSemanticsError::ResetBoundary, 1, prior_open_records);
            }
            let state = self
                .identities
                .get_mut(&identity)
                .expect("MBP identity exists");
            state.validity = MbpValidity::Recovering;
            state.open = Some(OpenMbpEnvelope::new(record));
            return Ok(None);
        }

        if matches!(
            self.identities
                .get(&identity)
                .expect("MBP identity exists")
                .validity,
            MbpValidity::Invalid
        ) {
            return self.fail_identity_with_current(identity, XnasSemanticsError::InvalidState);
        }

        let state = self
            .identities
            .get_mut(&identity)
            .expect("MBP identity exists");
        let Some(mut open) = state.open.take() else {
            state.open = Some(OpenMbpEnvelope::new(record));
            return Ok(None);
        };
        let prior_open_records = open.records.len() as u64;
        if record.sequence == open.current_sequence {
            if let Err(error) = open.append_same_block(record) {
                return self.fail_identity_detached(identity, error, prior_open_records + 1);
            }
            state.open = Some(open);
            return Ok(None);
        }
        if record.sequence < open.current_sequence {
            return self.fail_identity_detached(
                identity,
                XnasSemanticsError::SequenceRegressionOrReuse,
                prior_open_records + 1,
            );
        }
        if !open.current_saw_last {
            if let Err(error) = open.append_next_nonterminal_block(record) {
                return self.fail_identity_detached(identity, error, prior_open_records + 1);
            }
            state.open = Some(open);
            return Ok(None);
        }
        let completed = match open.close(
            &record,
            self.global_watermark
                .expect("validated MBP record establishes watermark"),
        ) {
            Ok(completed) => completed,
            Err(error) => {
                return self.fail_identity_detached(identity, error, prior_open_records + 1)
            }
        };
        // The member population is record-based, not block-based.
        self.counts.complete_pending(prior_open_records);
        self.counts.completed_endpoint_count += 1;
        self.counts.sequence_block_count += completed.ordered_distinct_sequence_vector.len() as u64;
        state.validity = MbpValidity::Valid;
        state.open = Some(OpenMbpEnvelope::new(record));
        Ok(Some(completed))
    }

    pub fn finish(self) -> Result<Mbp10SemanticsCountsV1, XnasSemanticsError> {
        let report = self.finish_report();
        match report.terminal_error {
            Some(error) => Err(error),
            None => Ok(report.counts),
        }
    }

    /// Finalize while preserving auditable populations on a fail-closed EOF.
    pub fn finish_report(mut self) -> Mbp10SemanticsFinishReportV1 {
        let mut terminal_failure = self.terminal_error.take();
        if terminal_failure.is_none() {
            let missing = self
                .qualification
                .expected_identities()
                .iter()
                .any(|expected| !self.observed_identities.contains(expected));
            if missing {
                self.counts
                    .quarantine(&XnasSemanticsError::MissingExpectedIdentity, 0, 0);
                terminal_failure.get_or_insert(XnasSemanticsError::MissingExpectedIdentity);
            }
            for state in self.identities.values_mut() {
                if let Some(open) = state.open.take() {
                    let error = if matches!(state.validity, MbpValidity::Recovering) {
                        XnasSemanticsError::InitializationIncompleteAtEof
                    } else if open.current_saw_last {
                        XnasSemanticsError::TerminalAtEof
                    } else {
                        XnasSemanticsError::OpenAtEof
                    };
                    self.counts.quarantine(&error, 1, open.records.len() as u64);
                    if matches!(state.validity, MbpValidity::Recovering) {
                        terminal_failure.get_or_insert(error);
                    }
                } else if matches!(state.validity, MbpValidity::Recovering) {
                    let error = XnasSemanticsError::InitializationIncompleteAtEof;
                    self.counts.quarantine(&error, 0, 0);
                    terminal_failure.get_or_insert(error);
                } else if matches!(state.validity, MbpValidity::Invalid) {
                    terminal_failure.get_or_insert(XnasSemanticsError::InvalidState);
                }
            }
        }
        debug_assert!(self.counts.population_reconciles());
        debug_assert_eq!(self.counts.pending_record_count, 0);
        Mbp10SemanticsFinishReportV1 {
            counts: self.counts,
            terminal_error: terminal_failure,
        }
    }

    pub fn invalidate_boundary(&mut self, boundary: XnasBoundaryV1) -> XnasSemanticsError {
        let error = boundary.error();
        let expected = self
            .qualification
            .expected_identities()
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let mut candidate_count = 0_u64;
        let mut record_count = 0_u64;
        for identity in expected {
            let state = self.identities.entry(identity).or_default();
            if let Some(open) = state.open.take() {
                candidate_count += 1;
                record_count += open.records.len() as u64;
            }
            state.validity = MbpValidity::Invalid;
        }
        self.counts
            .quarantine(&error, candidate_count, record_count);
        error
    }

    fn fail_terminal<T>(&mut self, error: XnasSemanticsError) -> Result<T, XnasSemanticsError> {
        let (open_candidate_count, record_count) = self.drain_all_open();
        self.counts
            .quarantine(&error, open_candidate_count, record_count);
        self.terminal_error = Some(error.clone());
        Err(error)
    }

    fn fail_terminal_admitted<T>(
        &mut self,
        error: XnasSemanticsError,
        admitted_current_records: u64,
    ) -> Result<T, XnasSemanticsError> {
        let (open_candidate_count, open_record_count) = self.drain_all_open();
        self.counts.quarantine(
            &error,
            open_candidate_count,
            open_record_count + admitted_current_records,
        );
        self.terminal_error = Some(error.clone());
        Err(error)
    }

    fn fail_identity_with_current<T>(
        &mut self,
        identity: XnasIdentityV1,
        error: XnasSemanticsError,
    ) -> Result<T, XnasSemanticsError> {
        let mut record_count = 1_u64;
        let mut open_candidate_count = 0_u64;
        if let Some(state) = self.identities.get_mut(&identity) {
            if let Some(open) = state.open.take() {
                open_candidate_count = 1;
                record_count += open.records.len() as u64;
            }
            state.validity = MbpValidity::Invalid;
        }
        self.counts
            .quarantine(&error, open_candidate_count, record_count);
        Err(error)
    }

    fn fail_identity_detached<T>(
        &mut self,
        identity: XnasIdentityV1,
        error: XnasSemanticsError,
        record_count: u64,
    ) -> Result<T, XnasSemanticsError> {
        if let Some(state) = self.identities.get_mut(&identity) {
            state.validity = MbpValidity::Invalid;
            state.open = None;
        }
        self.counts.quarantine(&error, 1, record_count);
        Err(error)
    }

    fn drain_all_open(&mut self) -> (u64, u64) {
        let mut open_candidate_count = 0_u64;
        let mut record_count = 0_u64;
        for state in self.identities.values_mut() {
            if let Some(open) = state.open.take() {
                open_candidate_count += 1;
                record_count += open.records.len() as u64;
            }
            state.validity = MbpValidity::Invalid;
        }
        (open_candidate_count, record_count)
    }
}

/// MBO endpoint snapshot with per-level order counts for exact MBP-10 parity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedMboBookV1 {
    pub envelope: XnasCompletedUpdateEnvelopeV1,
    pub state: LobState,
    pub levels: [Mbp10LevelV1; 10],
}

impl PublishedMboBookV1 {
    pub const fn identity(&self) -> XnasIdentityV1 {
        self.envelope.identity
    }

    pub const fn endpoint_ns(&self) -> u64 {
        self.envelope.endpoint_ns
    }

    pub const fn effective_available_ns(&self) -> u64 {
        self.envelope.effective_available_ns
    }
}

/// Apply one already-qualified envelope to a cloned private book and return
/// the clone only after every mutation and endpoint invariant passes.  T and F
/// remain distinct retained carriers but never enter the book mutator.
fn apply_xnas_envelope_transactionally(
    book: &LobReconstructor,
    envelope: &XnasCompletedUpdateEnvelopeV1,
) -> Result<(LobReconstructor, PublishedMboBookV1), XnasSemanticsError> {
    let mut next_book = book.clone();
    let warnings_before = next_book.stats().total_warnings();
    let mut scratch = LobState::new(next_book.config().levels);
    for record in &envelope.records {
        let action = match record.action {
            b'A' => Action::Add,
            b'C' => Action::Cancel,
            b'M' => Action::Modify,
            b'R' => Action::Clear,
            b'T' | b'F' => continue,
            other => return Err(XnasSemanticsError::UnsupportedAction(other)),
        };
        let side = match record.side {
            b'B' => Side::Bid,
            b'A' => Side::Ask,
            b'N' if action == Action::Clear => Side::None,
            other => return Err(XnasSemanticsError::UnsupportedSide(other)),
        };
        let timestamp =
            i64::try_from(record.ts_event).map_err(|_| XnasSemanticsError::TimestampOutOfRange)?;
        let message = MboMessage {
            order_id: record.order_id,
            action,
            side,
            price: record.price,
            size: record.size,
            timestamp: Some(timestamp),
        };
        next_book
            .process_message_into(&message, &mut scratch)
            .map_err(|error| XnasSemanticsError::BookMutation(error.to_string()))?;
    }
    if next_book.stats().total_warnings() != warnings_before {
        return Err(XnasSemanticsError::BookMutationAnomaly);
    }
    let endpoint_timestamp =
        i64::try_from(envelope.endpoint_ns).map_err(|_| XnasSemanticsError::TimestampOutOfRange)?;
    let state = next_book.get_lob_state_with_metadata(Some(endpoint_timestamp));
    if matches!(
        state.check_consistency(),
        BookConsistency::Locked | BookConsistency::Crossed
    ) {
        return Err(XnasSemanticsError::InvalidEndpointBook);
    }
    let (bid_counts, ask_counts) = next_book.level_order_counts();
    let mut levels = [Mbp10LevelV1::default(); 10];
    for idx in 0..10 {
        levels[idx] = Mbp10LevelV1 {
            bid_px: if state.bid_prices[idx] == 0 {
                DBN_UNDEF_PRICE
            } else {
                state.bid_prices[idx]
            },
            ask_px: if state.ask_prices[idx] == 0 {
                DBN_UNDEF_PRICE
            } else {
                state.ask_prices[idx]
            },
            bid_sz: state.bid_sizes[idx],
            ask_sz: state.ask_sizes[idx],
            bid_ct: u32::try_from(bid_counts[idx]).map_err(|_| {
                XnasSemanticsError::BookMutation(
                    "bid level order count exceeds MBP-10 u32 range".to_owned(),
                )
            })?,
            ask_ct: u32::try_from(ask_counts[idx]).map_err(|_| {
                XnasSemanticsError::BookMutation(
                    "ask level order count exceeds MBP-10 u32 range".to_owned(),
                )
            })?,
        };
    }
    Ok((
        next_book,
        PublishedMboBookV1 {
            envelope: envelope.clone(),
            state,
            levels,
        },
    ))
}

/// Exact cross-schema key, excluding the structurally absent MBP channel.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct XnasEndpointMatchKeyV1 {
    pub session: String,
    pub publisher_id: u16,
    pub instrument_id: u32,
    pub endpoint_ns: u64,
    pub ordered_distinct_sequence_vector: Vec<u32>,
    pub terminal_sequence: u32,
}

impl XnasEndpointMatchKeyV1 {
    pub fn from_mbo(session: impl Into<String>, value: &XnasCompletedUpdateEnvelopeV1) -> Self {
        Self {
            session: session.into(),
            publisher_id: value.identity.publisher_id,
            instrument_id: value.identity.instrument_id,
            endpoint_ns: value.endpoint_ns,
            ordered_distinct_sequence_vector: value.ordered_distinct_sequence_vector.clone(),
            terminal_sequence: value.terminal_sequence,
        }
    }

    pub fn from_mbp(session: impl Into<String>, value: &Mbp10CompletedEndpointV1) -> Self {
        Self {
            session: session.into(),
            publisher_id: value.identity.publisher_id,
            instrument_id: value.identity.instrument_id,
            endpoint_ns: value.endpoint_ns,
            ordered_distinct_sequence_vector: value.ordered_distinct_sequence_vector.clone(),
            terminal_sequence: value.terminal_sequence,
        }
    }
}

/// Integer-second causal bin for a publication.
fn publication_bin_start_ns(effective_available_ns: u64) -> u64 {
    effective_available_ns / 1_000_000_000 * 1_000_000_000
}

/// A publication in `[s,s+1s)` is usable only at `s+1s`.
fn publication_bin_available_ns(effective_available_ns: u64) -> Result<u64, XnasSemanticsError> {
    publication_bin_start_ns(effective_available_ns)
        .checked_add(1_000_000_000)
        .ok_or(XnasSemanticsError::TimestampOutOfRange)
}

/// Frozen DECISION-030 healthy no-event carry bound.
const XNAS_CAUSAL_STALENESS_NS: u64 = 5_000_000_000;

/// Exact midpoint publication retained by a completed integer-second bin.
///
/// `midpoint_twice` is `best_bid + best_ask` in DBN fixed-price units.  The
/// explicit denominator of two avoids introducing binary floating-point into
/// the conformance lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CausalBinnedMidpointV1 {
    pub(crate) identity: XnasIdentityV1,
    pub(crate) bin_start_ns: u64,
    pub(crate) bin_available_ns: u64,
    pub(crate) effective_available_ns: u64,
    pub(crate) endpoint_ns: u64,
    pub(crate) witness_source_ordinal: SourceOrdinal,
    pub(crate) midpoint_twice: i128,
}

/// One-identity causal midpoint adapter for the corrected F064 row.
///
/// Publications must arrive in the order emitted by the global source stream.
/// A boundary clears carry state.  A later publication can requalify the
/// series only because `XnasMboStreamV1` emits it after a witnessed,
/// transactionally valid recovery envelope.
#[derive(Debug)]
struct XnasCausalMidpointSeriesV1 {
    identity: XnasIdentityV1,
    last_publication_key: Option<(u64, SourceOrdinal)>,
    observed_prefix_watermark_ns: Option<u64>,
    last_decision_ns: Option<u64>,
    bins: BTreeMap<u64, CausalBinnedMidpointV1>,
    qualified: bool,
}

impl XnasCausalMidpointSeriesV1 {
    fn new(identity: XnasIdentityV1) -> Self {
        Self {
            identity,
            last_publication_key: None,
            observed_prefix_watermark_ns: None,
            last_decision_ns: None,
            bins: BTreeMap::new(),
            qualified: false,
        }
    }

    /// Consume the complete causal outcome exposed by the MBO stream.
    ///
    /// Accepted pending records and other identities still advance the global
    /// stopping-prefix watermark.  A rejection invalidates its named identity,
    /// or every identity when the source is terminal.
    fn observe_outcome(&mut self, outcome: &MboIngestOutcomeV1) -> Result<(), XnasSemanticsError> {
        let observed_watermark_ns = outcome.global_watermark_after_ns();
        if let Some(last_decision_ns) = self.last_decision_ns {
            if observed_watermark_ns.is_none_or(|watermark| watermark <= last_decision_ns) {
                self.invalidate();
                return Err(XnasSemanticsError::RecordInsideClosedDecisionPrefix {
                    decision_ns: last_decision_ns,
                });
            }
        }
        if let Some(watermark) = observed_watermark_ns {
            self.observed_prefix_watermark_ns = Some(
                self.observed_prefix_watermark_ns
                    .map_or(watermark, |prior| prior.max(watermark)),
            );
        }

        match outcome {
            MboIngestOutcomeV1::Accepted { disposition, .. } => {
                self.observe_disposition(disposition)
            }
            MboIngestOutcomeV1::Rejected(rejection) => {
                if matches!(
                    rejection.invalidation_scope,
                    MboCausalInvalidationScopeV1::All
                ) || matches!(
                    rejection.invalidation_scope,
                    MboCausalInvalidationScopeV1::Identity(identity)
                        if identity == self.identity
                ) {
                    self.invalidate();
                }
                Ok(())
            }
        }
    }

    /// Consume one accepted stream disposition.
    fn observe_disposition(
        &mut self,
        disposition: &MboIngestDispositionV1,
    ) -> Result<(), XnasSemanticsError> {
        match disposition {
            MboIngestDispositionV1::InitialClearControl(value)
                if value.record.identity() == self.identity =>
            {
                self.invalidate();
                Ok(())
            }
            MboIngestDispositionV1::AuthoritativeReset(value)
                if value.record.identity() == self.identity =>
            {
                self.invalidate();
                Ok(())
            }
            MboIngestDispositionV1::Published(publication)
                if publication.identity() == self.identity =>
            {
                self.push_publication(publication)
            }
            MboIngestDispositionV1::InitialClearControl(_)
            | MboIngestDispositionV1::AuthoritativeReset(_)
            | MboIngestDispositionV1::Pending
            | MboIngestDispositionV1::Published(_) => Ok(()),
        }
    }

    /// Apply the midpoint half of the atomic MBO boundary transition.
    fn observe_boundary(&mut self, _boundary: XnasBoundaryV1) {
        self.invalidate();
    }

    /// Consume one already-qualified publication.  One-sided endpoints do not
    /// erase an earlier finite midpoint in the same bin.
    fn push_publication(
        &mut self,
        publication: &PublishedMboBookV1,
    ) -> Result<(), XnasSemanticsError> {
        let key = (
            publication.effective_available_ns(),
            publication.envelope.witness_source_ordinal,
        );
        if self
            .last_publication_key
            .is_some_and(|previous| key <= previous)
        {
            self.qualified = false;
            self.bins.clear();
            return Err(XnasSemanticsError::PublicationOrderRegression);
        }
        self.last_publication_key = Some(key);
        self.qualified = true;

        let top = publication.levels[0];
        if top.bid_px == DBN_UNDEF_PRICE
            || top.ask_px == DBN_UNDEF_PRICE
            || top.bid_px >= top.ask_px
        {
            return Ok(());
        }

        let bin_start_ns = publication_bin_start_ns(publication.effective_available_ns());
        let value = CausalBinnedMidpointV1 {
            identity: self.identity,
            bin_start_ns,
            bin_available_ns: publication_bin_available_ns(publication.effective_available_ns())?,
            effective_available_ns: publication.effective_available_ns(),
            endpoint_ns: publication.endpoint_ns(),
            witness_source_ordinal: publication.envelope.witness_source_ordinal,
            midpoint_twice: i128::from(top.bid_px) + i128::from(top.ask_px),
        };
        self.bins.insert(bin_start_ns, value);
        Ok(())
    }

    /// Invalidate all prior carry at a source, decode, session, or equivalent
    /// state boundary.  The global publication key remains monotone.
    fn invalidate(&mut self) {
        self.bins.clear();
        self.qualified = false;
    }

    /// Emit P(u) once, in strictly increasing decision-time order.
    ///
    /// The caller must peek the next decoded record and invoke this method
    /// only after consuming the complete original-order prefix through N(u),
    /// but before consuming a record that would lift H above `u`.  The adapter
    /// rejects both retroactive/repeated decisions and decisions below an
    /// already-consumed watermark.  It also rejects a later record that proves
    /// an already-emitted prefix was incomplete.
    #[cfg_attr(not(test), allow(dead_code))]
    fn emit_at(
        &mut self,
        decision_ns: u64,
    ) -> Result<Option<CausalBinnedMidpointV1>, XnasSemanticsError> {
        if let Some(previous) = self.last_decision_ns {
            if decision_ns <= previous {
                self.invalidate();
                return Err(XnasSemanticsError::DecisionTimeNotStrictlyIncreasing {
                    previous,
                    observed: decision_ns,
                });
            }
        }
        if let Some(observed_watermark_ns) = self.observed_prefix_watermark_ns {
            if observed_watermark_ns > decision_ns {
                self.invalidate();
                return Err(XnasSemanticsError::DecisionBehindObservedPrefix {
                    decision_ns,
                    observed_watermark_ns,
                });
            }
        }
        self.last_decision_ns = Some(decision_ns);
        if !self.qualified {
            return Ok(None);
        }
        Ok(self
            .bins
            .values()
            .rev()
            .find(|value| {
                value.bin_available_ns <= decision_ns
                    && decision_ns
                        .checked_sub(value.endpoint_ns)
                        .is_some_and(|age| age <= XNAS_CAUSAL_STALENESS_NS)
            })
            .cloned())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn bin_count(&self) -> usize {
        self.bins.len()
    }
}

impl From<TlobError> for XnasSemanticsError {
    fn from(value: TlobError) -> Self {
        Self::BookMutation(value.to_string())
    }
}

#[cfg(test)]
mod causal_midpoint_series_tests {
    use super::*;

    const TEST_INSTRUMENT: u32 = 11_667;

    fn ordinal(value: u64) -> SourceOrdinal {
        SourceOrdinal::new(value).expect("test ordinals are nonzero")
    }

    fn synthetic_publication(
        effective_available_ns: u64,
        endpoint_ns: u64,
        witness_ordinal: u64,
        bid_px: i64,
        ask_px: i64,
    ) -> PublishedMboBookV1 {
        let identity = XnasIdentityV1::new(XNAS_ITCH_PUBLISHER_ID, TEST_INSTRUMENT);
        let mut levels = [Mbp10LevelV1::default(); 10];
        levels[0] = Mbp10LevelV1 {
            bid_px,
            ask_px,
            bid_sz: 1,
            ask_sz: 1,
            bid_ct: 1,
            ask_ct: 1,
        };
        PublishedMboBookV1 {
            envelope: XnasCompletedUpdateEnvelopeV1 {
                schema: XNAS_COMPLETED_UPDATE_ENVELOPE_V1.to_owned(),
                identity,
                channel_id: 0,
                ordered_distinct_sequence_vector: vec![
                    u32::try_from(witness_ordinal).expect("small test ordinal")
                ],
                terminal_sequence: u32::try_from(witness_ordinal).expect("small test ordinal"),
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
            levels,
        }
    }

    fn accepted_publication_outcome(publication: PublishedMboBookV1) -> MboIngestOutcomeV1 {
        MboIngestOutcomeV1::Accepted {
            identity: publication.identity(),
            source_ordinal: publication.envelope.witness_source_ordinal,
            global_watermark_after_ns: Some(publication.effective_available_ns()),
            disposition: MboIngestDispositionV1::Published(Box::new(publication)),
        }
    }

    fn test_qualification(instruments: &[u32]) -> XnasDailySourceQualificationV1 {
        XnasDailySourceQualificationV1 {
            schema: XnasSchemaV1::Mbo,
            expected_identities: instruments
                .iter()
                .map(|instrument_id| XnasIdentityV1::new(XNAS_ITCH_PUBLISHER_ID, *instrument_id))
                .collect(),
            source_path: "test-source.dbn.zst".to_owned(),
            source_sha256: "0".repeat(64),
            manifest_path: "test-manifest.json".to_owned(),
            manifest_sha256: "1".repeat(64),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn mbo(
        source_ordinal: u64,
        instrument_id: u32,
        sequence: u32,
        ts_recv: u64,
        action: u8,
        side: u8,
        order_id: u64,
        price: i64,
        flags: u8,
    ) -> RawMboRecordV1 {
        RawMboRecordV1 {
            source_ordinal: ordinal(source_ordinal),
            rtype: DBN_RTYPE_MBO,
            publisher_id: XNAS_ITCH_PUBLISHER_ID,
            instrument_id,
            ts_event: ts_recv - 10,
            order_id,
            price,
            size: 1,
            flags,
            channel_id: 0,
            action,
            side,
            ts_recv,
            ts_in_delta: 10,
            sequence,
        }
    }

    fn initial_control(source_ordinal: u64, instrument_id: u32) -> RawMboRecordV1 {
        RawMboRecordV1 {
            source_ordinal: ordinal(source_ordinal),
            rtype: DBN_RTYPE_MBO,
            publisher_id: XNAS_ITCH_PUBLISHER_ID,
            instrument_id,
            ts_event: 123,
            order_id: 0,
            price: DBN_UNDEF_PRICE,
            size: 0,
            flags: DBN_FLAG_BAD_TS_RECV,
            channel_id: 0,
            action: b'R',
            side: b'N',
            ts_recv: 456,
            ts_in_delta: 0,
            sequence: 0,
        }
    }

    fn seed_two_sided_midpoint(
        stream: &mut XnasMboStreamV1,
        instrument_id: u32,
        first_ordinal: u64,
        first_ts_recv: u64,
        first_order_id: u64,
        bid_px: i64,
        ask_px: i64,
    ) {
        for record in [
            mbo(
                first_ordinal,
                instrument_id,
                10,
                first_ts_recv,
                b'A',
                b'B',
                first_order_id,
                bid_px,
                DBN_FLAG_LAST,
            ),
            mbo(
                first_ordinal + 1,
                instrument_id,
                20,
                first_ts_recv + 100_000_000,
                b'A',
                b'A',
                first_order_id + 1,
                ask_px,
                DBN_FLAG_LAST,
            ),
            mbo(
                first_ordinal + 2,
                instrument_id,
                30,
                first_ts_recv + 200_000_000,
                b'A',
                b'B',
                first_order_id + 2,
                bid_px - 1,
                DBN_FLAG_LAST,
            ),
        ] {
            stream.push_causally(record).unwrap().unwrap();
        }
    }

    #[test]
    fn causal_series_enforces_online_stopping_prefix_order() {
        let identity = XnasIdentityV1::new(XNAS_ITCH_PUBLISHER_ID, TEST_INSTRUMENT);

        let mut beyond = XnasCausalMidpointSeriesV1::new(identity);
        beyond
            .observe_outcome(&accepted_publication_outcome(synthetic_publication(
                2_000_000_000,
                2_000_000_000,
                1,
                100,
                102,
            )))
            .unwrap();
        assert_eq!(
            beyond.emit_at(1_999_999_999).unwrap_err(),
            XnasSemanticsError::DecisionBehindObservedPrefix {
                decision_ns: 1_999_999_999,
                observed_watermark_ns: 2_000_000_000,
            }
        );

        let mut incomplete = XnasCausalMidpointSeriesV1::new(identity);
        incomplete
            .observe_outcome(&accepted_publication_outcome(synthetic_publication(
                1_000_000_000,
                1_000_000_000,
                1,
                100,
                102,
            )))
            .unwrap();
        incomplete.emit_at(2_000_000_000).unwrap();
        assert_eq!(
            incomplete
                .observe_outcome(&accepted_publication_outcome(synthetic_publication(
                    1_500_000_000,
                    1_500_000_000,
                    2,
                    101,
                    103,
                )))
                .unwrap_err(),
            XnasSemanticsError::RecordInsideClosedDecisionPrefix {
                decision_ns: 2_000_000_000,
            }
        );

        let mut repeated = XnasCausalMidpointSeriesV1::new(identity);
        repeated
            .observe_outcome(&accepted_publication_outcome(synthetic_publication(
                1_000_000_000,
                1_000_000_000,
                1,
                100,
                102,
            )))
            .unwrap();
        repeated.emit_at(2_000_000_000).unwrap();
        assert_eq!(
            repeated.emit_at(2_000_000_000).unwrap_err(),
            XnasSemanticsError::DecisionTimeNotStrictlyIncreasing {
                previous: 2_000_000_000,
                observed: 2_000_000_000,
            }
        );
    }

    #[test]
    fn same_timestamp_publications_are_ordered_by_source_ordinal() {
        let identity = XnasIdentityV1::new(XNAS_ITCH_PUBLISHER_ID, TEST_INSTRUMENT);
        let mut series = XnasCausalMidpointSeriesV1::new(identity);
        series
            .observe_outcome(&accepted_publication_outcome(synthetic_publication(
                999_999_999,
                999_999_999,
                1,
                100,
                102,
            )))
            .unwrap();
        series
            .observe_outcome(&accepted_publication_outcome(synthetic_publication(
                999_999_999,
                999_999_999,
                2,
                101,
                103,
            )))
            .unwrap();
        let retained = series
            .emit_at(1_000_000_000)
            .unwrap()
            .expect("same-bin publication is available at bin end");
        assert_eq!(retained.witness_source_ordinal.get(), 2);
        assert_eq!(retained.midpoint_twice, 204);
    }

    #[test]
    fn causal_bins_use_bin_end_exact_order_and_endpoint_staleness() {
        let identity = XnasIdentityV1::new(XNAS_ITCH_PUBLISHER_ID, TEST_INSTRUMENT);
        let mut series = XnasCausalMidpointSeriesV1::new(identity);
        series
            .observe_outcome(&accepted_publication_outcome(synthetic_publication(
                1_999_999_999,
                1_999_999_999,
                1,
                100_000_000_000,
                100_010_000_000,
            )))
            .unwrap();
        assert!(series.emit_at(1_999_999_999).unwrap().is_none());
        assert_eq!(
            series
                .emit_at(2_000_000_000)
                .unwrap()
                .unwrap()
                .midpoint_twice,
            200_010_000_000
        );

        let mut exact_series = XnasCausalMidpointSeriesV1::new(identity);
        exact_series
            .observe_outcome(&accepted_publication_outcome(synthetic_publication(
                2_000_000_000,
                2_000_000_000,
                2,
                101_000_000_000,
                101_010_000_000,
            )))
            .unwrap();
        assert!(exact_series.emit_at(2_000_000_000).unwrap().is_none());
        assert_eq!(
            exact_series
                .emit_at(3_000_000_000)
                .unwrap()
                .unwrap()
                .midpoint_twice,
            202_010_000_000
        );

        let mut delayed_series = XnasCausalMidpointSeriesV1::new(identity);
        delayed_series
            .observe_outcome(&accepted_publication_outcome(synthetic_publication(
                10_000_000_000,
                1_000_000_000,
                3,
                102_000_000_000,
                102_010_000_000,
            )))
            .unwrap();
        assert!(delayed_series.emit_at(11_000_000_000).unwrap().is_none());
    }

    #[test]
    fn boundary_clears_carry_and_order_regression_fails_closed() {
        let identity = XnasIdentityV1::new(XNAS_ITCH_PUBLISHER_ID, TEST_INSTRUMENT);
        let mut series = XnasCausalMidpointSeriesV1::new(identity);
        let first = synthetic_publication(1_000_000_000, 1_000_000_000, 1, 100, 102);
        series
            .observe_outcome(&accepted_publication_outcome(first.clone()))
            .unwrap();
        assert!(series.emit_at(2_000_000_000).unwrap().is_some());
        series.observe_boundary(XnasBoundaryV1::SourceGap);
        assert_eq!(series.bin_count(), 0);

        series
            .observe_outcome(&accepted_publication_outcome(synthetic_publication(
                3_000_000_000,
                3_000_000_000,
                2,
                101,
                103,
            )))
            .unwrap();
        assert_eq!(series.bin_count(), 1);
        let regressed = MboIngestOutcomeV1::Accepted {
            identity,
            source_ordinal: ordinal(3),
            global_watermark_after_ns: Some(3_000_000_000),
            disposition: MboIngestDispositionV1::Published(Box::new(first)),
        };
        assert_eq!(
            series.observe_outcome(&regressed).unwrap_err(),
            XnasSemanticsError::PublicationOrderRegression
        );
        assert!(series.emit_at(4_000_000_000).unwrap().is_none());
    }

    #[test]
    fn owned_stream_boundary_clears_every_identity_without_selection() {
        const OTHER_INSTRUMENT: u32 = 22_001;
        let first_identity = XnasIdentityV1::new(XNAS_ITCH_PUBLISHER_ID, TEST_INSTRUMENT);
        let second_identity = XnasIdentityV1::new(XNAS_ITCH_PUBLISHER_ID, OTHER_INSTRUMENT);
        let mut stream =
            XnasMboStreamV1::new(test_qualification(&[TEST_INSTRUMENT, OTHER_INSTRUMENT]));
        stream
            .push_causally(initial_control(1, TEST_INSTRUMENT))
            .unwrap()
            .unwrap();
        stream
            .push_causally(initial_control(2, OTHER_INSTRUMENT))
            .unwrap()
            .unwrap();
        seed_two_sided_midpoint(&mut stream, TEST_INSTRUMENT, 3, 1_100_000_000, 1, 100, 110);
        seed_two_sided_midpoint(
            &mut stream,
            OTHER_INSTRUMENT,
            6,
            1_400_000_000,
            10,
            200,
            210,
        );
        assert!(stream
            .emit_causal_midpoint_after_complete_prefix(first_identity, 2_000_000_000)
            .unwrap()
            .is_some());
        assert!(stream
            .emit_causal_midpoint_after_complete_prefix(second_identity, 2_000_000_000)
            .unwrap()
            .is_some());

        assert_eq!(
            stream.invalidate_boundary_causally(XnasBoundaryV1::DecodeGap),
            XnasSemanticsError::DecodeGap
        );
        assert_eq!(stream.causal_bin_count(first_identity).unwrap(), 0);
        assert_eq!(stream.causal_bin_count(second_identity).unwrap(), 0);
        assert!(stream
            .emit_causal_midpoint_after_complete_prefix(first_identity, 2_100_000_000)
            .unwrap()
            .is_none());
        assert!(stream
            .emit_causal_midpoint_after_complete_prefix(second_identity, 2_100_000_000)
            .unwrap()
            .is_none());
    }

    #[test]
    fn every_external_boundary_clears_owned_carry() {
        let identity = XnasIdentityV1::new(XNAS_ITCH_PUBLISHER_ID, TEST_INSTRUMENT);
        for boundary in [
            XnasBoundaryV1::SourceGap,
            XnasBoundaryV1::DecodeGap,
            XnasBoundaryV1::SessionBoundary,
        ] {
            let mut stream = XnasMboStreamV1::new(test_qualification(&[TEST_INSTRUMENT]));
            stream
                .push_causally(initial_control(1, TEST_INSTRUMENT))
                .unwrap()
                .unwrap();
            seed_two_sided_midpoint(&mut stream, TEST_INSTRUMENT, 2, 1_100_000_000, 1, 100, 110);
            assert!(stream
                .emit_causal_midpoint_after_complete_prefix(identity, 2_000_000_000)
                .unwrap()
                .is_some());
            stream.invalidate_boundary_causally(boundary);
            assert_eq!(stream.causal_bin_count(identity).unwrap(), 0);
            assert!(stream
                .emit_causal_midpoint_after_complete_prefix(identity, 2_100_000_000)
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn rejected_record_clears_owned_carry_until_witnessed_reset_recovery() {
        let identity = XnasIdentityV1::new(XNAS_ITCH_PUBLISHER_ID, TEST_INSTRUMENT);
        let mut stream = XnasMboStreamV1::new(test_qualification(&[TEST_INSTRUMENT]));
        stream
            .push_causally(initial_control(1, TEST_INSTRUMENT))
            .unwrap()
            .unwrap();
        seed_two_sided_midpoint(&mut stream, TEST_INSTRUMENT, 2, 1_100_000_000, 1, 100, 110);
        assert!(stream
            .emit_causal_midpoint_after_complete_prefix(identity, 2_000_000_000)
            .unwrap()
            .is_some());
        let rejected = stream
            .push_causally(mbo(
                5,
                TEST_INSTRUMENT,
                40,
                2_100_000_000,
                b'A',
                b'B',
                4,
                98,
                DBN_FLAG_MAYBE_BAD_BOOK,
            ))
            .unwrap();
        assert_eq!(rejected.unwrap_err(), XnasSemanticsError::MaybeBadBook);
        assert!(stream
            .emit_causal_midpoint_after_complete_prefix(identity, 3_000_000_000)
            .unwrap()
            .is_none());

        for record in [
            mbo(
                6,
                TEST_INSTRUMENT,
                50,
                3_100_000_000,
                b'R',
                b'N',
                0,
                DBN_UNDEF_PRICE,
                DBN_FLAG_LAST,
            ),
            mbo(
                7,
                TEST_INSTRUMENT,
                60,
                3_200_000_000,
                b'A',
                b'B',
                10,
                100,
                DBN_FLAG_LAST,
            ),
            mbo(
                8,
                TEST_INSTRUMENT,
                70,
                3_300_000_000,
                b'A',
                b'A',
                11,
                110,
                DBN_FLAG_LAST,
            ),
            mbo(
                9,
                TEST_INSTRUMENT,
                80,
                3_400_000_000,
                b'A',
                b'B',
                12,
                99,
                DBN_FLAG_LAST,
            ),
        ] {
            stream.push_causally(record).unwrap().unwrap();
        }
        assert!(stream
            .emit_causal_midpoint_after_complete_prefix(identity, 4_000_000_000)
            .unwrap()
            .is_some());
    }

    #[test]
    fn late_record_after_internal_emission_is_artifact_terminal() {
        let identity = XnasIdentityV1::new(XNAS_ITCH_PUBLISHER_ID, TEST_INSTRUMENT);
        let mut stream = XnasMboStreamV1::new(test_qualification(&[TEST_INSTRUMENT]));
        stream
            .push_causally(initial_control(1, TEST_INSTRUMENT))
            .unwrap()
            .unwrap();
        seed_two_sided_midpoint(&mut stream, TEST_INSTRUMENT, 2, 1_100_000_000, 1, 100, 110);
        assert!(stream
            .emit_causal_midpoint_after_complete_prefix(identity, 2_000_000_000)
            .unwrap()
            .is_some());
        assert_eq!(
            stream
                .push_causally(mbo(
                    5,
                    TEST_INSTRUMENT,
                    40,
                    1_400_000_000,
                    b'A',
                    b'B',
                    4,
                    98,
                    DBN_FLAG_LAST,
                ))
                .unwrap_err(),
            XnasSemanticsError::RecordInsideClosedDecisionPrefix {
                decision_ns: 2_000_000_000,
            }
        );
        let report = stream.finish_report();
        assert_eq!(
            report.terminal_error,
            Some(XnasSemanticsError::RecordInsideClosedDecisionPrefix {
                decision_ns: 2_000_000_000,
            })
        );
        assert!(report.counts.population_reconciles());
        assert_eq!(report.counts.pending_record_count, 0);
    }
}

// Compile-time guard: the fixed reconstructor representation must retain at
// least ten levels for the MBP-10 comparison.
const _: () = assert!(MAX_LOB_LEVELS >= 10);
