mod book;
mod diagnostics;
mod envelope;
mod qualified;

use crate::loader::{
    CanonicalReadReceiptV1, StrictBoundaryErrorV1, StrictMboEventIteratorV1,
    VerifiedRejectedStreamEventV1, VerifiedRejectionStageV1, VerifiedStreamRecordV1,
    XnasDailyMetadataBindingV1,
};
use book::ExactBookProjectorV1;
use envelope::{
    encode_raw_event, event_semantic_tag, EnvelopeAssemblyErrorV1, OpenEnvelopeV1,
    ReadyEnvelopeTxnV1,
};
use hft_mbo_event_contract::{
    BookCommandV1, EventDispositionV1, RawMboEventV1, Sha256DigestV1, ValidationBoundaryClassV1,
    ValidationFailureV1, ValidationReasonV1, ACTION_CLEAR, FLAG_BAD_TS_RECV, SIDE_NONE,
    UNDEF_PRICE,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct XnasIdentityV1 {
    publisher_id: u16,
    instrument_id: u32,
}

impl XnasIdentityV1 {
    pub(crate) const fn new(publisher_id: u16, instrument_id: u32) -> Self {
        Self {
            publisher_id,
            instrument_id,
        }
    }

    pub const fn publisher_id(self) -> u16 {
        self.publisher_id
    }

    pub const fn instrument_id(self) -> u32 {
        self.instrument_id
    }
}

pub use book::{BookTransactionErrorV1, XnasBookCommitV1, XnasBookLevelV1, XnasBookSnapshotV1};
pub use diagnostics::XnasReplayCountsV1;
pub use envelope::EnvelopeAssemblyErrorV1 as XnasEnvelopeErrorV1;
pub use qualified::{
    XnasPendingEnvelopeObservationV1, XnasQualifiedReplayPlanV1, XnasReplayEquivalenceReceiptV1,
    XnasReplayRevalidationPassV1,
};

const XNAS_REPLAY_ALGORITHM_ID_V2: &str = "hft.xnas.strict_replay.v2";

/// Reconstructor package/build identity of the code that executed a replay.
///
/// The package lock is scoped to this repository. A final executable owner must
/// separately bind its active workspace lockfile, invocation, and binary digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasReplayBuildIdentityV1 {
    package_version: &'static str,
    git_commit: &'static str,
    git_dirty: bool,
    package_repository_cargo_lock_sha256: Sha256DigestV1,
    rustc_command: &'static str,
    rustc_version: &'static str,
    rustc_verbose_sha256: Sha256DigestV1,
    target: &'static str,
    profile: &'static str,
    enabled_features: &'static str,
    replay_algorithm_id: &'static str,
}

impl XnasReplayBuildIdentityV1 {
    fn current() -> Self {
        Self {
            package_version: env!("CARGO_PKG_VERSION"),
            git_commit: env!("HFT_RECON_GIT_COMMIT"),
            git_dirty: env!("HFT_RECON_GIT_DIRTY") == "true",
            package_repository_cargo_lock_sha256: Sha256DigestV1::from_hex(env!(
                "HFT_RECON_PACKAGE_LOCK_SHA256"
            ))
            .expect("build.rs emits a lowercase package-repository Cargo.lock digest"),
            rustc_command: env!("HFT_RECON_RUSTC_COMMAND"),
            rustc_version: env!("HFT_RECON_RUSTC_VERSION"),
            rustc_verbose_sha256: Sha256DigestV1::from_hex(env!("HFT_RECON_RUSTC_VERBOSE_SHA256"))
                .expect("build.rs emits a lowercase rustc -vV SHA-256 digest"),
            target: env!("HFT_RECON_TARGET"),
            profile: env!("HFT_RECON_PROFILE"),
            enabled_features: env!("HFT_RECON_ENABLED_FEATURES"),
            replay_algorithm_id: XNAS_REPLAY_ALGORITHM_ID_V2,
        }
    }

    pub const fn package_version(&self) -> &'static str {
        self.package_version
    }
    pub const fn git_commit(&self) -> &'static str {
        self.git_commit
    }
    pub const fn git_dirty(&self) -> bool {
        self.git_dirty
    }
    pub const fn package_repository_cargo_lock_sha256(&self) -> Sha256DigestV1 {
        self.package_repository_cargo_lock_sha256
    }
    pub const fn rustc_command(&self) -> &'static str {
        self.rustc_command
    }
    pub const fn rustc_version(&self) -> &'static str {
        self.rustc_version
    }
    pub const fn rustc_verbose_sha256(&self) -> Sha256DigestV1 {
        self.rustc_verbose_sha256
    }
    pub const fn target(&self) -> &'static str {
        self.target
    }
    pub const fn profile(&self) -> &'static str {
        self.profile
    }
    pub const fn enabled_features(&self) -> &'static str {
        self.enabled_features
    }
    pub const fn replay_algorithm_id(&self) -> &'static str {
        self.replay_algorithm_id
    }
    /// Whether every package-local identity component was captured rather than
    /// replaced by an explicit unverified sentinel. This still does not bind a
    /// final executable, consumer workspace lock, or invocation.
    pub fn is_identity_complete(&self) -> bool {
        self.git_commit.len() == 40
            && self
                .git_commit
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && !self.rustc_command.starts_with("unverified-")
            && !self.rustc_version.starts_with("unverified-")
            && !self.target.starts_with("unverified-")
            && !self.profile.starts_with("unverified-")
    }

    /// Narrow local fact, not publication or scientific admission authority.
    pub fn is_clean_git_release_profile(&self) -> bool {
        self.profile == "release" && !self.git_dirty && self.is_identity_complete()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct XnasReplayConfigV1 {
    snapshot_depth: NonZeroUsize,
    max_envelope_members: NonZeroUsize,
    max_sequence_blocks: NonZeroUsize,
}

impl XnasReplayConfigV1 {
    pub const fn new(
        snapshot_depth: NonZeroUsize,
        max_envelope_members: NonZeroUsize,
        max_sequence_blocks: NonZeroUsize,
    ) -> Self {
        Self {
            snapshot_depth,
            max_envelope_members,
            max_sequence_blocks,
        }
    }

    pub const fn snapshot_depth(self) -> NonZeroUsize {
        self.snapshot_depth
    }

    pub const fn max_envelope_members(self) -> NonZeroUsize {
        self.max_envelope_members
    }

    pub const fn max_sequence_blocks(self) -> NonZeroUsize {
        self.max_sequence_blocks
    }
}

#[derive(Debug)]
struct XnasStagedBookUpdateV1 {
    source_object_sha256: Sha256DigestV1,
    validity_epoch_index: u64,
    symbol: Arc<str>,
    envelope_sha256: Sha256DigestV1,
    committed_observation_sha256: Sha256DigestV1,
    committed_observation_chain_sha256: Sha256DigestV1,
    envelope: ReadyEnvelopeTxnV1,
    book: XnasBookCommitV1,
}

impl XnasStagedBookUpdateV1 {
    fn events(&self) -> &[EventDispositionV1] {
        self.envelope.events()
    }

    const fn witness_source_ordinal(&self) -> u64 {
        self.envelope.witness().event().raw().raw_ordinal
    }
}

/// Bounded success trace returned only together with a successful EOF-sealed
/// replay receipt. The two-pass API may expose the same data earlier only inside
/// the explicitly non-publishable `XnasPendingEnvelopeObservationV1` wrapper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasReplayTraceV1 {
    source_object_sha256: Sha256DigestV1,
    validity_epoch_index: u64,
    symbol: Arc<str>,
    identity: XnasIdentityV1,
    envelope_sha256: Sha256DigestV1,
    committed_observation_sha256: Sha256DigestV1,
    committed_observation_chain_sha256: Sha256DigestV1,
    ordered_distinct_sequences: Vec<u32>,
    events: Vec<EventDispositionV1>,
    terminal_sequence: u32,
    terminal_source_ordinal: u64,
    witness: EventDispositionV1,
    endpoint_ns: u64,
    witness_ts_recv: u64,
    effective_available_ns: u64,
    closure_confirmation_delay_ns: u64,
    execution_sequence_blocks: u64,
    execution_carriers: u64,
    recovery: bool,
    book: XnasBookCommitV1,
}

impl XnasReplayTraceV1 {
    fn from_staged(update: XnasStagedBookUpdateV1) -> Self {
        let identity = update.envelope.identity();
        let terminal_sequence = update.envelope.terminal_sequence();
        let terminal_source_ordinal = update.envelope.terminal_source_ordinal();
        let witness = *update.envelope.witness();
        let endpoint_ns = update.envelope.endpoint_ns();
        let witness_ts_recv = update.envelope.witness_ts_recv();
        let effective_available_ns = update.envelope.effective_available_ns();
        let closure_confirmation_delay_ns = update.envelope.closure_confirmation_delay_ns();
        let execution_sequence_blocks = update.envelope.execution_sequence_blocks();
        let execution_carriers = update.envelope.execution_carrier_count();
        let recovery = update.envelope.is_recovery();
        let (ordered_distinct_sequences, events) = update.envelope.into_sequences_and_events();
        Self {
            source_object_sha256: update.source_object_sha256,
            validity_epoch_index: update.validity_epoch_index,
            symbol: update.symbol,
            identity,
            envelope_sha256: update.envelope_sha256,
            committed_observation_sha256: update.committed_observation_sha256,
            committed_observation_chain_sha256: update.committed_observation_chain_sha256,
            ordered_distinct_sequences,
            terminal_sequence,
            terminal_source_ordinal,
            witness,
            endpoint_ns,
            witness_ts_recv,
            effective_available_ns,
            closure_confirmation_delay_ns,
            execution_sequence_blocks,
            execution_carriers,
            recovery,
            events,
            book: update.book,
        }
    }

    pub const fn identity(&self) -> XnasIdentityV1 {
        self.identity
    }
    pub const fn source_object_sha256(&self) -> Sha256DigestV1 {
        self.source_object_sha256
    }
    /// One-based qualified-validity interval for this identity. This is not the
    /// book reset epoch: an initially invalid identity can first qualify in
    /// validity epoch 1 after the exact book has advanced to reset epoch 2.
    pub const fn validity_epoch_index(&self) -> u64 {
        self.validity_epoch_index
    }
    pub fn symbol(&self) -> &str {
        &self.symbol
    }
    pub const fn envelope_sha256(&self) -> Sha256DigestV1 {
        self.envelope_sha256
    }
    /// Digest of the exact consumer-visible committed observation, including
    /// source/identity/epoch, envelope semantics, causal clocks, and exported
    /// book snapshot values.
    pub const fn committed_observation_sha256(&self) -> Sha256DigestV1 {
        self.committed_observation_sha256
    }
    pub const fn committed_observation_chain_sha256(&self) -> Sha256DigestV1 {
        self.committed_observation_chain_sha256
    }
    pub fn ordered_distinct_sequences(&self) -> &[u32] {
        &self.ordered_distinct_sequences
    }
    /// Exact committed member population. Downstream analytical flow consumes
    /// these events once, in source order.
    pub fn events(&self) -> &[EventDispositionV1] {
        &self.events
    }
    pub fn first_source_ordinal(&self) -> u64 {
        self.events
            .first()
            .expect("a committed envelope always has at least one member")
            .event()
            .raw()
            .raw_ordinal
    }
    pub fn last_source_ordinal(&self) -> u64 {
        self.events
            .last()
            .expect("a committed envelope always has at least one member")
            .event()
            .raw()
            .raw_ordinal
    }
    pub const fn terminal_sequence(&self) -> u32 {
        self.terminal_sequence
    }
    pub const fn terminal_source_ordinal(&self) -> u64 {
        self.terminal_source_ordinal
    }
    /// Closure evidence only. This is not a current-envelope member and
    /// normally becomes a member of its own later envelope.
    pub const fn witness(&self) -> &EventDispositionV1 {
        &self.witness
    }
    pub const fn witness_source_ordinal(&self) -> u64 {
        self.witness.event().raw().raw_ordinal
    }
    pub const fn endpoint_ns(&self) -> u64 {
        self.endpoint_ns
    }
    pub const fn witness_ts_recv(&self) -> u64 {
        self.witness_ts_recv
    }
    pub const fn effective_available_ns(&self) -> u64 {
        self.effective_available_ns
    }
    pub const fn closure_confirmation_delay_ns(&self) -> u64 {
        self.closure_confirmation_delay_ns
    }
    pub const fn execution_sequence_blocks(&self) -> u64 {
        self.execution_sequence_blocks
    }
    pub const fn execution_carriers(&self) -> u64 {
        self.execution_carriers
    }
    pub const fn is_recovery(&self) -> bool {
        self.recovery
    }
    pub const fn book(&self) -> &XnasBookCommitV1 {
        &self.book
    }
}

/// A diagnostic trace batch and the terminal receipt that makes it reusable.
#[derive(Debug, Serialize)]
pub struct XnasReplayRunV1 {
    selected_raw_ordinals: Vec<u64>,
    selected_ordinal_dispositions: Vec<XnasSelectedOrdinalDispositionV1>,
    traces: Vec<XnasReplayTraceV1>,
    receipt: XnasReplayReceiptV1,
}

impl XnasReplayRunV1 {
    pub fn selected_raw_ordinals(&self) -> &[u64] {
        &self.selected_raw_ordinals
    }
    pub fn traces(&self) -> &[XnasReplayTraceV1] {
        &self.traces
    }
    pub fn selected_ordinal_dispositions(&self) -> &[XnasSelectedOrdinalDispositionV1] {
        &self.selected_ordinal_dispositions
    }
    pub const fn receipt(&self) -> &XnasReplayReceiptV1 {
        &self.receipt
    }
    pub fn into_receipt(self) -> XnasReplayReceiptV1 {
        self.receipt
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum XnasSelectedOrdinalRoleV1 {
    InitialClearControl {
        identity: XnasIdentityV1,
    },
    CompletedEnvelopeMember {
        identity: XnasIdentityV1,
        trace_index: u64,
        terminal_source_ordinal: u64,
    },
    ClosureWitness {
        identity: XnasIdentityV1,
        trace_index: u64,
        terminal_source_ordinal: u64,
    },
    ResetBoundaryTrigger {
        identity: XnasIdentityV1,
    },
    ResetBoundaryQuarantinedMember {
        identity: XnasIdentityV1,
        reset_source_ordinal: u64,
    },
    SemanticQuarantinedMember {
        identity: XnasIdentityV1,
        incident_index: u64,
        detected_at_source_ordinal: u64,
        reason: XnasQuarantineReasonV1,
    },
    DecodedSemanticRejection {
        identity: XnasIdentityV1,
        reason: ValidationReasonV1,
    },
    EofTailQuarantinedMember {
        identity: XnasIdentityV1,
        reason: XnasEofTailReasonV1,
    },
}

impl XnasSelectedOrdinalRoleV1 {
    const fn is_primary(&self) -> bool {
        matches!(
            self,
            Self::InitialClearControl { .. }
                | Self::CompletedEnvelopeMember { .. }
                | Self::ResetBoundaryQuarantinedMember { .. }
                | Self::SemanticQuarantinedMember { .. }
                | Self::EofTailQuarantinedMember { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasSelectedOrdinalDispositionV1 {
    raw_ordinal: u64,
    decoded_from_source: bool,
    roles: Vec<XnasSelectedOrdinalRoleV1>,
}

impl XnasSelectedOrdinalDispositionV1 {
    pub const fn raw_ordinal(&self) -> u64 {
        self.raw_ordinal
    }
    pub const fn decoded_from_source(&self) -> bool {
        self.decoded_from_source
    }
    pub fn roles(&self) -> &[XnasSelectedOrdinalRoleV1] {
        &self.roles
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XnasTerminalIdentityStatusV1 {
    NeverQualified,
    InvalidAfterEofTailQuarantine,
    InvalidAfterQuarantinedRecovery,
    InvalidAwaitingRecoveryAtEof,
}

/// Stable reason attached to an exact identity-local quarantine incident.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum XnasQuarantineReasonV1 {
    InvalidInitialCondition,
    LaterInitialClearControl,
    AwaitingRecovery,
    ClosureWitnessAfterFailedTransaction,
    Validation { reason: ValidationReasonV1 },
    BadReceiveTimestamp,
    EventOutsideSourceDay { ts_event: u64 },
    IdentityReceiveTimeRegression { previous: u64, actual: u64 },
    UnsupportedOrdinaryControl,
    Envelope { source: EnvelopeAssemblyErrorV1 },
    Book { source: BookTransactionErrorV1 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasRecoveryQualificationV1 {
    reset_source_ordinal: u64,
    terminal_source_ordinal: u64,
    witness_source_ordinal: u64,
    effective_available_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasValidityEpochQualificationV1 {
    terminal_source_ordinal: u64,
    witness_source_ordinal: u64,
    effective_available_ns: u64,
    recovery_reset_source_ordinal: Option<u64>,
}

impl XnasValidityEpochQualificationV1 {
    pub const fn terminal_source_ordinal(&self) -> u64 {
        self.terminal_source_ordinal
    }
    pub const fn witness_source_ordinal(&self) -> u64 {
        self.witness_source_ordinal
    }
    pub const fn effective_available_ns(&self) -> u64 {
        self.effective_available_ns
    }
    pub const fn recovery_reset_source_ordinal(&self) -> Option<u64> {
        self.recovery_reset_source_ordinal
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum XnasValidityInvalidationReasonV1 {
    SemanticQuarantine { incident_index: u64 },
    ResetBoundary,
    EofTail { reason: XnasEofTailReasonV1 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasValidityInvalidationV1 {
    first_ineligible_source_ordinal: u64,
    detected_at_source_ordinal: Option<u64>,
    global_receive_watermark_ns: Option<u64>,
    reason: XnasValidityInvalidationReasonV1,
}

impl XnasValidityInvalidationV1 {
    pub const fn first_ineligible_source_ordinal(&self) -> u64 {
        self.first_ineligible_source_ordinal
    }
    pub const fn detected_at_source_ordinal(&self) -> Option<u64> {
        self.detected_at_source_ordinal
    }
    pub const fn global_receive_watermark_ns(&self) -> Option<u64> {
        self.global_receive_watermark_ns
    }
    pub const fn reason(&self) -> &XnasValidityInvalidationReasonV1 {
        &self.reason
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasValidityEpochV1 {
    qualification: XnasValidityEpochQualificationV1,
    last_committed_terminal_source_ordinal: u64,
    last_effective_available_ns: u64,
    last_committed_book_state_sha256: Sha256DigestV1,
    last_transition_chain_sha256: Sha256DigestV1,
    invalidation: XnasValidityInvalidationV1,
}

impl XnasValidityEpochV1 {
    pub const fn qualification(&self) -> &XnasValidityEpochQualificationV1 {
        &self.qualification
    }
    pub const fn last_committed_terminal_source_ordinal(&self) -> u64 {
        self.last_committed_terminal_source_ordinal
    }
    pub const fn last_effective_available_ns(&self) -> u64 {
        self.last_effective_available_ns
    }
    pub const fn last_committed_book_state_sha256(&self) -> Sha256DigestV1 {
        self.last_committed_book_state_sha256
    }
    pub const fn last_transition_chain_sha256(&self) -> Sha256DigestV1 {
        self.last_transition_chain_sha256
    }
    pub const fn invalidation(&self) -> &XnasValidityInvalidationV1 {
        &self.invalidation
    }
}

struct ActiveValidityEpochV1 {
    qualification: XnasValidityEpochQualificationV1,
    last_committed_terminal_source_ordinal: u64,
    last_effective_available_ns: u64,
    last_transition_chain_sha256: Sha256DigestV1,
}

impl XnasRecoveryQualificationV1 {
    pub const fn reset_source_ordinal(&self) -> u64 {
        self.reset_source_ordinal
    }
    pub const fn terminal_source_ordinal(&self) -> u64 {
        self.terminal_source_ordinal
    }
    pub const fn witness_source_ordinal(&self) -> u64 {
        self.witness_source_ordinal
    }
    pub const fn effective_available_ns(&self) -> u64 {
        self.effective_available_ns
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasSemanticQuarantineIncidentV1 {
    detected_at: RawMboEventV1,
    offending_candidate_source_ordinal: Option<u64>,
    reason: XnasQuarantineReasonV1,
    global_receive_watermark_ns: Option<u64>,
    candidate_source_ordinals: Vec<u64>,
    while_invalid_records: Vec<XnasInvalidStateQuarantinedRecordV1>,
    recovery: Option<XnasRecoveryQualificationV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasInvalidStateQuarantinedRecordV1 {
    raw: RawMboEventV1,
    reason: XnasQuarantineReasonV1,
}

impl XnasInvalidStateQuarantinedRecordV1 {
    pub const fn raw(&self) -> &RawMboEventV1 {
        &self.raw
    }
    pub const fn reason(&self) -> &XnasQuarantineReasonV1 {
        &self.reason
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasRejectedRecordQuarantineV1 {
    raw: RawMboEventV1,
    failure: ValidationFailureV1,
    stage: VerifiedRejectionStageV1,
    identity_incident_index: u64,
    phase: XnasRejectedRecordPhaseV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XnasRejectedRecordPhaseV1 {
    CandidateTrigger,
    WhileInvalid,
}

impl XnasRejectedRecordQuarantineV1 {
    pub const fn raw(&self) -> &RawMboEventV1 {
        &self.raw
    }
    pub const fn failure(&self) -> &ValidationFailureV1 {
        &self.failure
    }
    pub const fn stage(&self) -> VerifiedRejectionStageV1 {
        self.stage
    }
    pub const fn identity_incident_index(&self) -> u64 {
        self.identity_incident_index
    }
    pub const fn phase(&self) -> XnasRejectedRecordPhaseV1 {
        self.phase
    }
}

impl XnasSemanticQuarantineIncidentV1 {
    pub const fn detected_at(&self) -> &RawMboEventV1 {
        &self.detected_at
    }
    pub const fn offending_candidate_source_ordinal(&self) -> Option<u64> {
        self.offending_candidate_source_ordinal
    }
    pub const fn reason(&self) -> &XnasQuarantineReasonV1 {
        &self.reason
    }
    pub const fn global_receive_watermark_ns(&self) -> Option<u64> {
        self.global_receive_watermark_ns
    }
    pub fn candidate_source_ordinals(&self) -> &[u64] {
        &self.candidate_source_ordinals
    }
    pub fn while_invalid_records(&self) -> &[XnasInvalidStateQuarantinedRecordV1] {
        &self.while_invalid_records
    }
    pub const fn recovery(&self) -> Option<&XnasRecoveryQualificationV1> {
        self.recovery.as_ref()
    }
    pub fn total_quarantined_records(&self) -> Option<u64> {
        u64::try_from(self.candidate_source_ordinals.len())
            .ok()?
            .checked_add(u64::try_from(self.while_invalid_records.len()).ok()?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XnasEofTailReasonV1 {
    NonterminalEnvelope,
    TerminalCandidateWithoutWitness,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasEofTailQuarantineV1 {
    first_source_ordinal: u64,
    last_source_ordinal: u64,
    member_count: u64,
    source_ordinals: Vec<u64>,
    reason: XnasEofTailReasonV1,
    recovery_candidate: bool,
}

impl XnasEofTailQuarantineV1 {
    pub const fn first_source_ordinal(&self) -> u64 {
        self.first_source_ordinal
    }
    pub const fn last_source_ordinal(&self) -> u64 {
        self.last_source_ordinal
    }
    pub const fn member_count(&self) -> u64 {
        self.member_count
    }
    pub fn source_ordinals(&self) -> &[u64] {
        &self.source_ordinals
    }
    pub const fn reason(&self) -> XnasEofTailReasonV1 {
        self.reason
    }
    pub const fn recovery_candidate(&self) -> bool {
        self.recovery_candidate
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasResetBoundaryQuarantineV1 {
    reset_source_ordinal: u64,
    quarantined_source_ordinals: Vec<u64>,
}

impl XnasResetBoundaryQuarantineV1 {
    pub const fn reset_source_ordinal(&self) -> u64 {
        self.reset_source_ordinal
    }
    pub fn quarantined_source_ordinals(&self) -> &[u64] {
        &self.quarantined_source_ordinals
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasIdentityReplayReceiptV1 {
    identity: XnasIdentityV1,
    symbol: String,
    initial_clear_control: Option<RawMboEventV1>,
    terminal_status: XnasTerminalIdentityStatusV1,
    last_committed_book_state_sha256: Sha256DigestV1,
    transition_chain_sha256: Sha256DigestV1,
    committed_envelopes: u64,
    first_qualified_effective_available_ns: Option<u64>,
    first_qualified_terminal_source_ordinal: Option<u64>,
    last_effective_available_ns: Option<u64>,
    last_terminal_source_ordinal: Option<u64>,
    eof_tail_quarantine: Option<XnasEofTailQuarantineV1>,
    reset_boundary_quarantines: Vec<XnasResetBoundaryQuarantineV1>,
    semantic_quarantines: Vec<XnasSemanticQuarantineIncidentV1>,
    recovery_qualifications: Vec<XnasRecoveryQualificationV1>,
    rejected_record_quarantines: Vec<XnasRejectedRecordQuarantineV1>,
    validity_epochs: Vec<XnasValidityEpochV1>,
}

impl XnasIdentityReplayReceiptV1 {
    pub const fn identity(&self) -> XnasIdentityV1 {
        self.identity
    }

    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    pub const fn initial_clear_control(&self) -> Option<&RawMboEventV1> {
        self.initial_clear_control.as_ref()
    }

    pub const fn terminal_status(&self) -> XnasTerminalIdentityStatusV1 {
        self.terminal_status
    }

    pub const fn last_committed_book_state_sha256(&self) -> Sha256DigestV1 {
        self.last_committed_book_state_sha256
    }

    pub const fn transition_chain_sha256(&self) -> Sha256DigestV1 {
        self.transition_chain_sha256
    }

    pub const fn committed_envelopes(&self) -> u64 {
        self.committed_envelopes
    }

    pub const fn first_qualified_effective_available_ns(&self) -> Option<u64> {
        self.first_qualified_effective_available_ns
    }

    pub const fn first_qualified_terminal_source_ordinal(&self) -> Option<u64> {
        self.first_qualified_terminal_source_ordinal
    }

    pub const fn last_effective_available_ns(&self) -> Option<u64> {
        self.last_effective_available_ns
    }

    pub const fn last_terminal_source_ordinal(&self) -> Option<u64> {
        self.last_terminal_source_ordinal
    }

    pub const fn eof_tail_quarantine(&self) -> Option<&XnasEofTailQuarantineV1> {
        self.eof_tail_quarantine.as_ref()
    }

    pub fn reset_boundary_quarantines(&self) -> &[XnasResetBoundaryQuarantineV1] {
        &self.reset_boundary_quarantines
    }

    pub fn semantic_quarantines(&self) -> &[XnasSemanticQuarantineIncidentV1] {
        &self.semantic_quarantines
    }

    pub fn recovery_qualifications(&self) -> &[XnasRecoveryQualificationV1] {
        &self.recovery_qualifications
    }

    pub fn rejected_record_quarantines(&self) -> &[XnasRejectedRecordQuarantineV1] {
        &self.rejected_record_quarantines
    }

    pub fn validity_epochs(&self) -> &[XnasValidityEpochV1] {
        &self.validity_epochs
    }
}

/// EOF-sealed replay receipt. This proves source and replay reconciliation; it
/// does not authorize publication, admission, historical rewrite, or research use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasReplayReceiptV1 {
    schema: &'static str,
    build: XnasReplayBuildIdentityV1,
    source: CanonicalReadReceiptV1,
    config: XnasReplayConfigV1,
    counts: XnasReplayCountsV1,
    identities: Vec<XnasIdentityReplayReceiptV1>,
    committed_observation_chain_sha256: Sha256DigestV1,
    authority: &'static str,
}

impl XnasReplayReceiptV1 {
    pub const fn schema(&self) -> &'static str {
        self.schema
    }
    pub const fn build(&self) -> &XnasReplayBuildIdentityV1 {
        &self.build
    }
    pub const fn source(&self) -> &CanonicalReadReceiptV1 {
        &self.source
    }

    pub const fn config(&self) -> XnasReplayConfigV1 {
        self.config
    }

    pub const fn counts(&self) -> &XnasReplayCountsV1 {
        &self.counts
    }

    pub fn identities(&self) -> &[XnasIdentityReplayReceiptV1] {
        &self.identities
    }

    /// Chain over every exact committed observation in global replay order.
    pub const fn committed_observation_chain_sha256(&self) -> Sha256DigestV1 {
        self.committed_observation_chain_sha256
    }

    pub const fn authority(&self) -> &'static str {
        self.authority
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum XnasTerminalDisqualificationReasonV1 {
    #[error("terminal source receipt differs from the replay binding")]
    SourceBindingMismatch,
    #[error("terminal source/replay record count mismatch: source={source_records}, replay={replay_records}")]
    RecordCountMismatch {
        source_records: u64,
        replay_records: u64,
    },
    #[error("source/replay rejected-record mismatch: source={source_rejected}, replay={replay_rejected}")]
    RejectedRecordCountMismatch {
        source_rejected: u64,
        replay_rejected: u64,
    },
    #[error("replay population does not reconcile")]
    PopulationReconciliation,
    #[error("quarantined record total does not reconcile to reason-specific populations")]
    QuarantineReasonReconciliation,
    #[error("semantic quarantine incidents do not exactly reconcile to semantic counters")]
    SemanticQuarantineLedgerMismatch,
    #[error("identity validity epochs do not reconcile to committed and invalidated state")]
    ValidityEpochLedgerMismatch,
    #[error("completed envelope and staged update populations do not reconcile")]
    StagedUpdateReconciliation,
    #[error("identity {0:?} never produced its first qualified envelope")]
    IncompleteInitialization(XnasIdentityV1),
    #[error("identity {0:?} has no explicit terminal EOF-tail quarantine")]
    MissingEofTailQuarantine(XnasIdentityV1),
    #[error("identity {identity:?} failed terminal whole-book reconciliation: {source}")]
    BookInternalConsistency {
        identity: XnasIdentityV1,
        source: BookTransactionErrorV1,
    },
}

/// Source-bound, explicitly non-consumable diagnostic for a replay that reached
/// physical EOF but failed a terminal scientific invariant.
#[derive(Debug, Serialize)]
pub struct XnasTerminalDisqualificationV1 {
    schema: &'static str,
    build: XnasReplayBuildIdentityV1,
    source: CanonicalReadReceiptV1,
    config: XnasReplayConfigV1,
    counts: XnasReplayCountsV1,
    identities: Vec<XnasIdentityReplayReceiptV1>,
    committed_observation_chain_sha256: Sha256DigestV1,
    reason: XnasTerminalDisqualificationReasonV1,
    authority: &'static str,
}

impl XnasTerminalDisqualificationV1 {
    pub const fn schema(&self) -> &'static str {
        self.schema
    }
    pub const fn build(&self) -> &XnasReplayBuildIdentityV1 {
        &self.build
    }
    pub const fn source(&self) -> &CanonicalReadReceiptV1 {
        &self.source
    }
    pub const fn config(&self) -> XnasReplayConfigV1 {
        self.config
    }
    pub const fn counts(&self) -> &XnasReplayCountsV1 {
        &self.counts
    }
    pub fn identities(&self) -> &[XnasIdentityReplayReceiptV1] {
        &self.identities
    }
    pub const fn committed_observation_chain_sha256(&self) -> Sha256DigestV1 {
        self.committed_observation_chain_sha256
    }
    pub const fn reason(&self) -> &XnasTerminalDisqualificationReasonV1 {
        &self.reason
    }
    pub const fn authority(&self) -> &'static str {
        self.authority
    }
}

impl std::fmt::Display for XnasTerminalDisqualificationV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.reason.fmt(formatter)
    }
}

/// Pre-EOF failure context. The opened source and DBN metadata were verified at
/// open, but the bytes were not completely decoded or terminally re-hashed.
#[derive(Debug)]
pub struct XnasReplayPrefixFailureV1 {
    build: XnasReplayBuildIdentityV1,
    source: hft_mbo_event_contract::SourceDescriptorV1,
    metadata_binding: XnasDailyMetadataBindingV1,
    config: XnasReplayConfigV1,
    counts: XnasReplayCountsV1,
    decoded_records: u64,
    bytes_consumed: u64,
    global_receive_watermark_ns: Option<u64>,
    cause: Box<XnasReplayErrorV1>,
    authority: &'static str,
}

impl XnasReplayPrefixFailureV1 {
    pub const fn build(&self) -> &XnasReplayBuildIdentityV1 {
        &self.build
    }
    pub const fn source(&self) -> &hft_mbo_event_contract::SourceDescriptorV1 {
        &self.source
    }
    pub const fn metadata_binding(&self) -> &XnasDailyMetadataBindingV1 {
        &self.metadata_binding
    }
    pub const fn config(&self) -> XnasReplayConfigV1 {
        self.config
    }
    pub const fn counts(&self) -> &XnasReplayCountsV1 {
        &self.counts
    }
    pub const fn decoded_records(&self) -> u64 {
        self.decoded_records
    }
    pub const fn bytes_consumed(&self) -> u64 {
        self.bytes_consumed
    }
    pub const fn global_receive_watermark_ns(&self) -> Option<u64> {
        self.global_receive_watermark_ns
    }
    pub const fn cause(&self) -> &XnasReplayErrorV1 {
        &self.cause
    }
    pub const fn authority(&self) -> &'static str {
        self.authority
    }
}

impl std::fmt::Display for XnasReplayPrefixFailureV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "pre-EOF replay failure after {} decoded records: {}",
            self.decoded_records, self.cause
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentityLifecycleV1 {
    Uninitialized,
    AwaitingFirstQualifiedEnvelope,
    Valid,
    Invalid,
    Recovering,
    InvalidAfterEofTail,
}

struct IdentityReplayStateV1 {
    symbol: Arc<str>,
    initial_clear_control: Option<RawMboEventV1>,
    lifecycle: IdentityLifecycleV1,
    open: Option<OpenEnvelopeV1>,
    book: ExactBookProjectorV1,
    last_trusted_ts_recv: Option<u64>,
    ever_valid: bool,
    first_qualified_effective_available_ns: Option<u64>,
    first_qualified_terminal_source_ordinal: Option<u64>,
    last_effective_available_ns: Option<u64>,
    last_terminal_source_ordinal: Option<u64>,
    eof_tail_quarantine: Option<XnasEofTailQuarantineV1>,
    reset_boundary_quarantines: Vec<XnasResetBoundaryQuarantineV1>,
    semantic_quarantines: Vec<XnasSemanticQuarantineIncidentV1>,
    active_semantic_quarantine: Option<usize>,
    recovery_qualifications: Vec<XnasRecoveryQualificationV1>,
    rejected_record_quarantines: Vec<XnasRejectedRecordQuarantineV1>,
    validity_epochs: Vec<XnasValidityEpochV1>,
    active_validity_epoch: Option<ActiveValidityEpochV1>,
    active_recovery_reset_ordinal: Option<u64>,
}

/// Owning strict XNAS replay. Updates remain explicitly staged until `finish`
/// returns the source-bound EOF receipt and a later publication transaction
/// seals the staged generation.
pub struct StrictXnasReplayV1 {
    build: XnasReplayBuildIdentityV1,
    input: StrictMboEventIteratorV1,
    binding: XnasDailyMetadataBindingV1,
    source_digest: Sha256DigestV1,
    config: XnasReplayConfigV1,
    identities: BTreeMap<XnasIdentityV1, IdentityReplayStateV1>,
    next_raw_ordinal: u64,
    global_receive_watermark_ns: Option<u64>,
    counts: XnasReplayCountsV1,
    committed_observation_chain_sha256: Sha256DigestV1,
    committed_observation_encoding: Vec<u8>,
    eof: bool,
    failed: bool,
}

impl StrictXnasReplayV1 {
    pub fn from_strict_stream(
        input: StrictMboEventIteratorV1,
        config: XnasReplayConfigV1,
    ) -> Result<Self, XnasReplayErrorV1> {
        let binding = input
            .xnas_historical_source()
            .cloned()
            .ok_or(XnasReplayErrorV1::MissingXnasMetadataBinding)?;
        let source_digest = binding.source_object_sha256();
        if input.source().logical.canonical_sha256 != source_digest {
            return Err(XnasReplayErrorV1::MetadataSourceMismatch);
        }
        let mut identities = BTreeMap::new();
        for instrument in binding.instruments() {
            let identity = XnasIdentityV1::new(instrument.publisher_id, instrument.instrument_id);
            if identities.contains_key(&identity) {
                return Err(XnasReplayErrorV1::DuplicateMetadataIdentity(identity));
            }
            identities.insert(
                identity,
                IdentityReplayStateV1 {
                    symbol: Arc::from(instrument.symbol.as_str()),
                    initial_clear_control: None,
                    lifecycle: IdentityLifecycleV1::Uninitialized,
                    open: None,
                    book: ExactBookProjectorV1::new(
                        source_digest,
                        identity,
                        config.snapshot_depth.get(),
                    )?,
                    last_trusted_ts_recv: None,
                    ever_valid: false,
                    first_qualified_effective_available_ns: None,
                    first_qualified_terminal_source_ordinal: None,
                    last_effective_available_ns: None,
                    last_terminal_source_ordinal: None,
                    eof_tail_quarantine: None,
                    reset_boundary_quarantines: Vec::new(),
                    semantic_quarantines: Vec::new(),
                    active_semantic_quarantine: None,
                    recovery_qualifications: Vec::new(),
                    rejected_record_quarantines: Vec::new(),
                    validity_epochs: Vec::new(),
                    active_validity_epoch: None,
                    active_recovery_reset_ordinal: None,
                },
            );
        }
        Ok(Self {
            build: XnasReplayBuildIdentityV1::current(),
            input,
            binding,
            source_digest,
            config,
            identities,
            next_raw_ordinal: 1,
            global_receive_watermark_ns: None,
            counts: XnasReplayCountsV1::default(),
            committed_observation_chain_sha256: initial_committed_observation_chain(
                source_digest,
                config,
            ),
            committed_observation_encoding: Vec::with_capacity(1_024),
            eof: false,
            failed: false,
        })
    }

    /// Run the only load-bearing path through verified EOF without exposing
    /// intermediate success-typed book updates.
    pub fn run_to_eof(mut self) -> Result<XnasReplayReceiptV1, XnasReplayErrorV1> {
        while let Some(update) = self.next_quarantined_update() {
            if let Err(cause) = update {
                return Err(self.into_prefix_failure(cause));
            }
        }
        self.finish()
    }

    /// Run to verified EOF while retaining only envelopes selected by raw
    /// ordinal. The selected diagnostic values are returned only if the full
    /// source, replay populations, and every private book reconcile.
    pub fn run_to_eof_with_selected_ordinals(
        mut self,
        selected: &std::collections::BTreeSet<u64>,
    ) -> Result<XnasReplayRunV1, XnasReplayErrorV1> {
        let mut traces = Vec::new();
        let mut selected_roles = selected
            .iter()
            .copied()
            .map(|ordinal| (ordinal, Vec::new()))
            .collect::<BTreeMap<_, Vec<XnasSelectedOrdinalRoleV1>>>();
        while let Some(update) = self.next_quarantined_update() {
            let update = match update {
                Ok(update) => update,
                Err(cause) => return Err(self.into_prefix_failure(cause)),
            };
            if !selected.is_empty()
                && (selected.contains(&update.witness_source_ordinal())
                    || update
                        .events()
                        .iter()
                        .any(|event| selected.contains(&event.event().raw().raw_ordinal)))
            {
                let trace_index = u64::try_from(traces.len())
                    .map_err(|_| XnasReplayErrorV1::CounterInvariant("trace index overflow"))?;
                let identity = update.envelope.identity();
                let terminal_source_ordinal = update.envelope.terminal_source_ordinal();
                for event in update.events() {
                    if let Some(roles) = selected_roles.get_mut(&event.event().raw().raw_ordinal) {
                        roles.push(XnasSelectedOrdinalRoleV1::CompletedEnvelopeMember {
                            identity,
                            trace_index,
                            terminal_source_ordinal,
                        });
                    }
                }
                if let Some(roles) = selected_roles.get_mut(&update.witness_source_ordinal()) {
                    roles.push(XnasSelectedOrdinalRoleV1::ClosureWitness {
                        identity,
                        trace_index,
                        terminal_source_ordinal,
                    });
                }
                traces.push(XnasReplayTraceV1::from_staged(update));
            }
        }
        let receipt = self.finish()?;
        for identity_receipt in receipt.identities() {
            let identity = identity_receipt.identity();
            if let Some(initial) = identity_receipt.initial_clear_control() {
                if let Some(roles) = selected_roles.get_mut(&initial.raw_ordinal) {
                    roles.push(XnasSelectedOrdinalRoleV1::InitialClearControl { identity });
                }
            }
            for reset in identity_receipt.reset_boundary_quarantines() {
                if let Some(roles) = selected_roles.get_mut(&reset.reset_source_ordinal()) {
                    roles.push(XnasSelectedOrdinalRoleV1::ResetBoundaryTrigger { identity });
                }
                for &ordinal in reset.quarantined_source_ordinals() {
                    if let Some(roles) = selected_roles.get_mut(&ordinal) {
                        roles.push(XnasSelectedOrdinalRoleV1::ResetBoundaryQuarantinedMember {
                            identity,
                            reset_source_ordinal: reset.reset_source_ordinal(),
                        });
                    }
                }
            }
            for (index, incident) in identity_receipt.semantic_quarantines().iter().enumerate() {
                let incident_index = u64::try_from(index)
                    .map_err(|_| XnasReplayErrorV1::CounterInvariant("incident index overflow"))?;
                for &ordinal in incident.candidate_source_ordinals() {
                    if let Some(roles) = selected_roles.get_mut(&ordinal) {
                        roles.push(XnasSelectedOrdinalRoleV1::SemanticQuarantinedMember {
                            identity,
                            incident_index,
                            detected_at_source_ordinal: incident.detected_at().raw_ordinal,
                            reason: incident.reason().clone(),
                        });
                    }
                }
                for record in incident.while_invalid_records() {
                    if let Some(roles) = selected_roles.get_mut(&record.raw().raw_ordinal) {
                        roles.push(XnasSelectedOrdinalRoleV1::SemanticQuarantinedMember {
                            identity,
                            incident_index,
                            detected_at_source_ordinal: incident.detected_at().raw_ordinal,
                            reason: record.reason().clone(),
                        });
                    }
                }
            }
            for rejected in identity_receipt.rejected_record_quarantines() {
                if let Some(roles) = selected_roles.get_mut(&rejected.raw().raw_ordinal) {
                    roles.push(XnasSelectedOrdinalRoleV1::DecodedSemanticRejection {
                        identity,
                        reason: rejected.failure().reason.clone(),
                    });
                }
            }
            if let Some(tail) = identity_receipt.eof_tail_quarantine() {
                for &ordinal in tail.source_ordinals() {
                    if let Some(roles) = selected_roles.get_mut(&ordinal) {
                        roles.push(XnasSelectedOrdinalRoleV1::EofTailQuarantinedMember {
                            identity,
                            reason: tail.reason(),
                        });
                    }
                }
            }
        }
        let decoded_records = receipt.source().decoded_records();
        let mut selected_ordinal_dispositions = Vec::with_capacity(selected_roles.len());
        for (raw_ordinal, roles) in selected_roles {
            let decoded_from_source = raw_ordinal != 0 && raw_ordinal <= decoded_records;
            if decoded_from_source {
                let primary_count = roles.iter().filter(|role| role.is_primary()).count();
                if primary_count != 1 {
                    return Err(XnasReplayErrorV1::SelectedOrdinalPrimaryDisposition {
                        raw_ordinal,
                        primary_count,
                    });
                }
            }
            selected_ordinal_dispositions.push(XnasSelectedOrdinalDispositionV1 {
                raw_ordinal,
                decoded_from_source,
                roles,
            });
        }
        Ok(XnasReplayRunV1 {
            selected_raw_ordinals: selected.iter().copied().collect(),
            selected_ordinal_dispositions,
            traces,
            receipt,
        })
    }

    fn into_prefix_failure(self, cause: XnasReplayErrorV1) -> XnasReplayErrorV1 {
        XnasReplayErrorV1::PrefixFailed(Box::new(XnasReplayPrefixFailureV1 {
            build: self.build,
            source: self.input.source().clone(),
            metadata_binding: self.binding,
            config: self.config,
            counts: self.counts,
            decoded_records: self.input.decoded_records(),
            bytes_consumed: self.input.bytes_consumed(),
            global_receive_watermark_ns: self.global_receive_watermark_ns,
            cause: Box::new(cause),
            authority: "nonconsumable_prefix_diagnostic_not_eof_verified",
        }))
    }

    fn next_quarantined_update(
        &mut self,
    ) -> Option<Result<XnasStagedBookUpdateV1, XnasReplayErrorV1>> {
        if self.failed || self.eof {
            return None;
        }
        loop {
            let next = self.input.next();
            let record = match next {
                Some(Ok(record)) => record,
                Some(Err(source)) => {
                    self.failed = true;
                    return Some(Err(XnasReplayErrorV1::Boundary(source)));
                }
                None => {
                    if let Err(error) = self.quarantine_eof_tails() {
                        self.failed = true;
                        return Some(Err(error));
                    }
                    self.eof = true;
                    return None;
                }
            };
            let accepted = match record {
                VerifiedStreamRecordV1::Accepted(event) => self.accept(*event.disposition()),
                VerifiedStreamRecordV1::Rejected(event) => self.accept_rejected(event),
            };
            match accepted {
                Ok(Some(update)) => return Some(Ok(update)),
                Ok(None) => {}
                Err(error) => {
                    self.failed = true;
                    return Some(Err(error));
                }
            }
        }
    }

    fn finish(self) -> Result<XnasReplayReceiptV1, XnasReplayErrorV1> {
        if self.failed {
            return Err(XnasReplayErrorV1::CannotFinishFailedReplay);
        }
        if !self.eof {
            return Err(XnasReplayErrorV1::CannotFinishBeforeEof);
        }

        // Seal and re-hash the physical source before reporting any terminal
        // replay-semantic disqualification. This prevents a replay error from
        // masking a concurrently changed or truncated source at clean EOF.
        let source = self.input.finish()?;
        let mut disqualification = if source.xnas_historical_source() != Some(&self.binding)
            || source.source().logical.canonical_sha256 != self.source_digest
        {
            Some(XnasTerminalDisqualificationReasonV1::SourceBindingMismatch)
        } else if source.decoded_records() != self.counts.raw_records_ingested {
            Some(XnasTerminalDisqualificationReasonV1::RecordCountMismatch {
                source_records: source.decoded_records(),
                replay_records: self.counts.raw_records_ingested,
            })
        } else if source.rejected_records() != self.counts.decoded_semantic_rejections {
            Some(
                XnasTerminalDisqualificationReasonV1::RejectedRecordCountMismatch {
                    source_rejected: source.rejected_records(),
                    replay_rejected: self.counts.decoded_semantic_rejections,
                },
            )
        } else if !self.counts.population_reconciles() || self.counts.pending_members != 0 {
            Some(XnasTerminalDisqualificationReasonV1::PopulationReconciliation)
        } else if !self.counts.quarantine_reasons_reconcile() {
            Some(XnasTerminalDisqualificationReasonV1::QuarantineReasonReconciliation)
        } else if !self.counts.semantic_population_reconciles()
            || !semantic_quarantine_ledgers_reconcile(&self.identities, &self.counts)
        {
            Some(XnasTerminalDisqualificationReasonV1::SemanticQuarantineLedgerMismatch)
        } else if !validity_epoch_ledgers_reconcile(&self.identities) {
            Some(XnasTerminalDisqualificationReasonV1::ValidityEpochLedgerMismatch)
        } else if self.counts.completed_update_envelopes != self.counts.staged_book_updates {
            Some(XnasTerminalDisqualificationReasonV1::StagedUpdateReconciliation)
        } else {
            None
        };
        if disqualification.is_none() {
            for (identity, state) in &self.identities {
                if !state.ever_valid {
                    disqualification = Some(
                        XnasTerminalDisqualificationReasonV1::IncompleteInitialization(*identity),
                    );
                    break;
                }
                if state.lifecycle != IdentityLifecycleV1::Invalid
                    && (state.eof_tail_quarantine.is_none()
                        || state.lifecycle != IdentityLifecycleV1::InvalidAfterEofTail)
                {
                    disqualification = Some(
                        XnasTerminalDisqualificationReasonV1::MissingEofTailQuarantine(*identity),
                    );
                    break;
                }
                if let Err(source) = state.book.validate_internal_consistency() {
                    disqualification = Some(
                        XnasTerminalDisqualificationReasonV1::BookInternalConsistency {
                            identity: *identity,
                            source,
                        },
                    );
                    break;
                }
            }
        }
        let identities = self
            .identities
            .into_iter()
            .map(|(identity, state)| {
                let terminal_status = if !state.ever_valid {
                    XnasTerminalIdentityStatusV1::NeverQualified
                } else {
                    match state.lifecycle {
                        IdentityLifecycleV1::Invalid => {
                            XnasTerminalIdentityStatusV1::InvalidAwaitingRecoveryAtEof
                        }
                        IdentityLifecycleV1::InvalidAfterEofTail => {
                            if state
                                .eof_tail_quarantine
                                .as_ref()
                                .expect("terminal gate proved EOF-tail quarantine")
                                .recovery_candidate
                            {
                                XnasTerminalIdentityStatusV1::InvalidAfterQuarantinedRecovery
                            } else {
                                XnasTerminalIdentityStatusV1::InvalidAfterEofTailQuarantine
                            }
                        }
                        _ => XnasTerminalIdentityStatusV1::NeverQualified,
                    }
                };
                XnasIdentityReplayReceiptV1 {
                    identity,
                    symbol: state.symbol.to_string(),
                    initial_clear_control: state.initial_clear_control,
                    terminal_status,
                    last_committed_book_state_sha256: state.book.state_digest(),
                    transition_chain_sha256: state.book.transition_chain_sha256(),
                    committed_envelopes: state.book.commit_index(),
                    first_qualified_effective_available_ns: state
                        .first_qualified_effective_available_ns,
                    first_qualified_terminal_source_ordinal: state
                        .first_qualified_terminal_source_ordinal,
                    last_effective_available_ns: state.last_effective_available_ns,
                    last_terminal_source_ordinal: state.last_terminal_source_ordinal,
                    eof_tail_quarantine: state.eof_tail_quarantine,
                    reset_boundary_quarantines: state.reset_boundary_quarantines,
                    semantic_quarantines: state.semantic_quarantines,
                    recovery_qualifications: state.recovery_qualifications,
                    rejected_record_quarantines: state.rejected_record_quarantines,
                    validity_epochs: state.validity_epochs,
                }
            })
            .collect::<Vec<_>>();
        if let Some(reason) = disqualification {
            return Err(XnasReplayErrorV1::TerminalDisqualified(Box::new(
                XnasTerminalDisqualificationV1 {
                    schema: "xnas_terminal_disqualification_v1",
                    build: self.build,
                    source,
                    config: self.config,
                    counts: self.counts,
                    identities,
                    committed_observation_chain_sha256: self.committed_observation_chain_sha256,
                    reason,
                    authority: "nonconsumable_terminal_diagnostic",
                },
            )));
        }
        Ok(XnasReplayReceiptV1 {
            schema: "xnas_replay_receipt_v1",
            build: self.build,
            source,
            config: self.config,
            counts: self.counts,
            identities,
            committed_observation_chain_sha256: self.committed_observation_chain_sha256,
            authority: "development_only_authorizes_nothing",
        })
    }

    fn accept(
        &mut self,
        disposition: EventDispositionV1,
    ) -> Result<Option<XnasStagedBookUpdateV1>, XnasReplayErrorV1> {
        let raw = disposition.event().raw();
        if raw.raw_ordinal != self.next_raw_ordinal {
            return Err(XnasReplayErrorV1::RawOrdinalMismatch {
                expected: self.next_raw_ordinal,
                actual: raw.raw_ordinal,
            });
        }
        self.next_raw_ordinal = self
            .next_raw_ordinal
            .checked_add(1)
            .ok_or(XnasReplayErrorV1::RawOrdinalOverflow)?;
        if raw.source_object_sha256 != self.source_digest {
            return Err(XnasReplayErrorV1::EventSourceMismatch(raw.raw_ordinal));
        }
        let identity = XnasIdentityV1::new(raw.publisher_id, raw.instrument_id);
        let mut state =
            self.identities
                .remove(&identity)
                .ok_or(XnasReplayErrorV1::UnmappedIdentity {
                    raw_ordinal: raw.raw_ordinal,
                    identity,
                })?;
        self.counts
            .admit_pending()
            .map_err(XnasReplayErrorV1::CounterInvariant)?;
        let result = self.accept_for_identity(&mut state, disposition);
        self.identities.insert(identity, state);
        result
    }

    fn accept_rejected(
        &mut self,
        rejected: VerifiedRejectedStreamEventV1,
    ) -> Result<Option<XnasStagedBookUpdateV1>, XnasReplayErrorV1> {
        let raw = *rejected.raw();
        if raw.raw_ordinal != self.next_raw_ordinal {
            return Err(XnasReplayErrorV1::RawOrdinalMismatch {
                expected: self.next_raw_ordinal,
                actual: raw.raw_ordinal,
            });
        }
        self.next_raw_ordinal = self
            .next_raw_ordinal
            .checked_add(1)
            .ok_or(XnasReplayErrorV1::RawOrdinalOverflow)?;
        if raw.source_object_sha256 != self.source_digest {
            return Err(XnasReplayErrorV1::EventSourceMismatch(raw.raw_ordinal));
        }
        let identity = XnasIdentityV1::new(raw.publisher_id, raw.instrument_id);
        let mut state =
            self.identities
                .remove(&identity)
                .ok_or(XnasReplayErrorV1::UnmappedIdentity {
                    raw_ordinal: raw.raw_ordinal,
                    identity,
                })?;
        self.counts
            .admit_pending()
            .map_err(XnasReplayErrorV1::CounterInvariant)?;
        self.counts
            .mark_decoded_semantic_rejection()
            .map_err(XnasReplayErrorV1::CounterInvariant)?;

        let result = if is_source_fatal_validation(&rejected.failure().reason) {
            Err(XnasReplayErrorV1::Boundary(
                StrictBoundaryErrorV1::Validation(rejected.failure().clone()),
            ))
        } else {
            self.admit_quarantinable_raw_clock(&mut state, &raw);
            let (incident_index, phase) = if state.lifecycle == IdentityLifecycleV1::Invalid {
                let incident_index = state.semantic_quarantines.len().checked_sub(1).ok_or(
                    XnasReplayErrorV1::CounterInvariant(
                        "invalid identity has no semantic quarantine incident",
                    ),
                )?;
                self.quarantine_while_invalid(
                    &mut state,
                    raw,
                    XnasQuarantineReasonV1::Validation {
                        reason: rejected.failure().reason.clone(),
                    },
                )?;
                (incident_index, XnasRejectedRecordPhaseV1::WhileInvalid)
            } else {
                let incident_index = state.semantic_quarantines.len();
                let reason = XnasQuarantineReasonV1::Validation {
                    reason: rejected.failure().reason.clone(),
                };
                self.quarantine_semantic_candidate(
                    &mut state,
                    raw,
                    reason,
                    Some(raw.raw_ordinal),
                    None,
                )?;
                (incident_index, XnasRejectedRecordPhaseV1::CandidateTrigger)
            };
            state
                .rejected_record_quarantines
                .push(XnasRejectedRecordQuarantineV1 {
                    raw,
                    failure: rejected.failure().clone(),
                    stage: rejected.stage(),
                    identity_incident_index: u64::try_from(incident_index).map_err(|_| {
                        XnasReplayErrorV1::CounterInvariant("semantic incident index overflow")
                    })?,
                    phase,
                });
            Ok(None)
        };
        self.identities.insert(identity, state);
        result
    }

    fn accept_for_identity(
        &mut self,
        state: &mut IdentityReplayStateV1,
        disposition: EventDispositionV1,
    ) -> Result<Option<XnasStagedBookUpdateV1>, XnasReplayErrorV1> {
        let raw = disposition.event().raw();
        if state.lifecycle == IdentityLifecycleV1::Uninitialized {
            if !is_exact_initial_clear(raw) {
                self.admit_quarantinable_raw_clock(state, raw);
                return self.quarantine_semantic_candidate(
                    state,
                    *raw,
                    XnasQuarantineReasonV1::InvalidInitialCondition,
                    Some(raw.raw_ordinal),
                    None,
                );
            }
            let mut next_counts = self.counts.clone();
            next_counts
                .consume_initial_control()
                .map_err(XnasReplayErrorV1::CounterInvariant)?;
            self.counts = next_counts;
            state.initial_clear_control = Some(*raw);
            state.lifecycle = IdentityLifecycleV1::AwaitingFirstQualifiedEnvelope;
            return Ok(None);
        }

        if is_exact_initial_clear(raw) {
            return self.quarantine_semantic_candidate(
                state,
                *raw,
                XnasQuarantineReasonV1::LaterInitialClearControl,
                Some(raw.raw_ordinal),
                None,
            );
        }
        let clock = self.admit_ordinary_clock(state, raw);
        if state.lifecycle == IdentityLifecycleV1::Invalid {
            if let Err(reason) = clock {
                self.quarantine_while_invalid(state, *raw, reason)?;
                return Ok(None);
            }
            if matches!(
                disposition,
                EventDispositionV1::Book(BookCommandV1::Clear(_))
            ) {
                return self.begin_recovery(state, disposition);
            }
            let reason = if matches!(disposition, EventDispositionV1::Control(_)) {
                XnasQuarantineReasonV1::UnsupportedOrdinaryControl
            } else {
                XnasQuarantineReasonV1::AwaitingRecovery
            };
            self.quarantine_while_invalid(state, *raw, reason)?;
            return Ok(None);
        }
        if let Err(reason) = clock {
            return self.quarantine_semantic_candidate(
                state,
                *raw,
                reason,
                Some(raw.raw_ordinal),
                None,
            );
        }
        if matches!(disposition, EventDispositionV1::Control(_)) {
            return self.quarantine_semantic_candidate(
                state,
                *raw,
                XnasQuarantineReasonV1::UnsupportedOrdinaryControl,
                Some(raw.raw_ordinal),
                None,
            );
        }
        if matches!(
            disposition,
            EventDispositionV1::Book(BookCommandV1::Clear(_))
        ) {
            if state
                .open
                .as_ref()
                .is_some_and(|open| open.contains_provider_payload(raw))
            {
                return self.handle_envelope_failure(
                    state,
                    *raw,
                    EnvelopeAssemblyErrorV1::ExactDuplicate,
                    None,
                );
            }
            return self.begin_recovery(state, disposition);
        }

        match state.open.take() {
            None => {
                state.open = Some(self.new_open_envelope(disposition, false));
                Ok(None)
            }
            Some(mut open) => {
                if raw.channel_id != open.channel_id() {
                    return self.handle_envelope_failure(
                        state,
                        *raw,
                        EnvelopeAssemblyErrorV1::ChannelChange,
                        Some(open),
                    );
                }
                if raw.sequence == open.current_sequence() {
                    if let Err(source) = open.append_same_block(disposition) {
                        return self.handle_envelope_failure(state, *raw, source, Some(open));
                    }
                    state.open = Some(open);
                    Ok(None)
                } else if raw.sequence < open.current_sequence() {
                    self.handle_envelope_failure(
                        state,
                        *raw,
                        EnvelopeAssemblyErrorV1::SequenceRegressionOrReuse,
                        Some(open),
                    )
                } else if !open.current_saw_last() {
                    if let Err(source) = open.append_next_block(disposition) {
                        return self.handle_envelope_failure(state, *raw, source, Some(open));
                    }
                    state.open = Some(open);
                    Ok(None)
                } else {
                    let available = self
                        .global_receive_watermark_ns
                        .expect("ordinary witness contributes a trusted receive time");
                    let ready = open.close(disposition, available).map_err(|source| {
                        XnasReplayErrorV1::Envelope {
                            raw_ordinal: raw.raw_ordinal,
                            source,
                        }
                    })?;
                    let mut next_counts = self.counts.clone();
                    let members = u64::try_from(ready.events().len()).map_err(|_| {
                        XnasReplayErrorV1::CounterInvariant("member count overflow")
                    })?;
                    next_counts
                        .complete_members(members)
                        .map_err(XnasReplayErrorV1::CounterInvariant)?;
                    checked_increment(
                        &mut next_counts.completed_update_envelopes,
                        1,
                        "completed envelope count overflow",
                    )?;
                    checked_increment(
                        &mut next_counts.venue_sequence_blocks,
                        u64::try_from(ready.sequences().len()).map_err(|_| {
                            XnasReplayErrorV1::CounterInvariant("sequence block count overflow")
                        })?,
                        "venue sequence block count overflow",
                    )?;
                    checked_increment(
                        &mut next_counts.execution_sequence_blocks,
                        ready.execution_sequence_blocks(),
                        "execution sequence block count overflow",
                    )?;
                    checked_increment(
                        &mut next_counts.execution_carriers,
                        ready.execution_carrier_count(),
                        "execution carrier count overflow",
                    )?;
                    if ready.execution_carrier_count() != 0 {
                        checked_increment(
                            &mut next_counts.execution_envelopes,
                            1,
                            "execution envelope count overflow",
                        )?;
                    }
                    let command_count =
                        u64::try_from(ready.book_commands().count()).map_err(|_| {
                            XnasReplayErrorV1::CounterInvariant("book command count overflow")
                        })?;
                    checked_increment(
                        &mut next_counts.book_commands_committed,
                        command_count,
                        "book command count overflow",
                    )?;
                    checked_increment(
                        &mut next_counts.staged_book_updates,
                        1,
                        "staged update count overflow",
                    )?;
                    if ready.is_recovery() {
                        next_counts
                            .commit_recovery_reset()
                            .map_err(XnasReplayErrorV1::CounterInvariant)?;
                    }

                    let envelope_commitment = ready.commitment(self.source_digest);
                    let envelope_sha256 = envelope_commitment.sha256();
                    let terminal_source_ordinal = ready.terminal_source_ordinal();
                    let effective_available_ns = ready.effective_available_ns();
                    let witness = *ready.witness();
                    let recovery_reset_source_ordinal = if ready.is_recovery() {
                        let reset = state.active_recovery_reset_ordinal.ok_or(
                            XnasReplayErrorV1::CounterInvariant(
                                "recovery commit has no active reset ordinal",
                            ),
                        )?;
                        if let Some(index) = state.active_semantic_quarantine {
                            let incidents = state.semantic_quarantines.get(index..).ok_or(
                                XnasReplayErrorV1::CounterInvariant(
                                    "active semantic quarantine index is invalid",
                                ),
                            )?;
                            if incidents.iter().any(|incident| incident.recovery.is_some()) {
                                return Err(XnasReplayErrorV1::CounterInvariant(
                                    "active semantic quarantine was already recovered",
                                ));
                            }
                        }
                        Some(reset)
                    } else {
                        None
                    };
                    if recovery_reset_source_ordinal.is_some()
                        == state.active_validity_epoch.is_some()
                        && (recovery_reset_source_ordinal.is_some() || state.ever_valid)
                    {
                        return Err(XnasReplayErrorV1::CounterInvariant(
                            "validity epoch lifecycle disagrees with envelope qualification",
                        ));
                    }
                    validate_valid_commit(
                        state,
                        terminal_source_ordinal,
                        witness.event().raw().raw_ordinal,
                        effective_available_ns,
                        recovery_reset_source_ordinal,
                    )?;
                    let book = match state
                        .book
                        .apply_envelope_precommitted(&ready, envelope_commitment)
                    {
                        Ok(book) => book,
                        Err(source)
                            if classify_book_failure(&source)
                                == ReplayFailureScopeV1::IdentityQuarantine =>
                        {
                            return self.quarantine_failed_ready_envelope(state, ready, source)
                        }
                        Err(source) => return Err(XnasReplayErrorV1::Book(source)),
                    };
                    self.counts = next_counts;
                    state.lifecycle = IdentityLifecycleV1::Valid;
                    if let Some(reset_source_ordinal) = recovery_reset_source_ordinal {
                        let qualification = XnasRecoveryQualificationV1 {
                            reset_source_ordinal,
                            terminal_source_ordinal,
                            witness_source_ordinal: witness.event().raw().raw_ordinal,
                            effective_available_ns,
                        };
                        if let Some(index) = state.active_semantic_quarantine.take() {
                            let incidents = state
                                .semantic_quarantines
                                .get_mut(index..)
                                .expect("recovery invariants were checked before book commit");
                            for incident in incidents {
                                incident.recovery = Some(qualification.clone());
                            }
                        }
                        state.active_recovery_reset_ordinal = None;
                        state.recovery_qualifications.push(qualification);
                    }
                    record_valid_commit(
                        state,
                        terminal_source_ordinal,
                        witness.event().raw().raw_ordinal,
                        effective_available_ns,
                        recovery_reset_source_ordinal,
                        state.book.transition_chain_sha256(),
                    );
                    let validity_epoch_index = u64::try_from(state.validity_epochs.len())
                        .map_err(|_| {
                            XnasReplayErrorV1::CounterInvariant("validity epoch index overflow")
                        })?
                        .checked_add(1)
                        .ok_or(XnasReplayErrorV1::CounterInvariant(
                            "validity epoch index overflow",
                        ))?;
                    let committed_observation_sha256 = committed_observation_digest(
                        &mut self.committed_observation_encoding,
                        self.source_digest,
                        validity_epoch_index,
                        state.symbol.as_ref(),
                        envelope_sha256,
                        &ready,
                        &book,
                    );
                    let committed_observation_chain_sha256 = next_committed_observation_chain(
                        self.committed_observation_chain_sha256,
                        committed_observation_sha256,
                    );
                    self.committed_observation_chain_sha256 = committed_observation_chain_sha256;
                    state.open = Some(self.new_open_envelope(witness, false));
                    Ok(Some(XnasStagedBookUpdateV1 {
                        source_object_sha256: self.source_digest,
                        validity_epoch_index,
                        symbol: state.symbol.clone(),
                        envelope_sha256,
                        committed_observation_sha256,
                        committed_observation_chain_sha256,
                        envelope: ready,
                        book,
                    }))
                }
            }
        }
    }

    fn admit_ordinary_clock(
        &mut self,
        state: &mut IdentityReplayStateV1,
        raw: &RawMboEventV1,
    ) -> Result<(), XnasQuarantineReasonV1> {
        if raw.flags_raw & FLAG_BAD_TS_RECV != 0 {
            return Err(XnasQuarantineReasonV1::BadReceiveTimestamp);
        }
        self.global_receive_watermark_ns = Some(
            self.global_receive_watermark_ns
                .map_or(raw.ts_recv, |current| current.max(raw.ts_recv)),
        );
        if state
            .last_trusted_ts_recv
            .is_some_and(|previous| raw.ts_recv < previous)
        {
            return Err(XnasQuarantineReasonV1::IdentityReceiveTimeRegression {
                previous: state.last_trusted_ts_recv.expect("checked Some"),
                actual: raw.ts_recv,
            });
        }
        state.last_trusted_ts_recv = Some(raw.ts_recv);
        if raw.ts_event < self.binding.session_start_ns()
            || raw.ts_event >= self.binding.session_end_ns()
        {
            return Err(XnasQuarantineReasonV1::EventOutsideSourceDay {
                ts_event: raw.ts_event,
            });
        }
        Ok(())
    }

    fn admit_quarantinable_raw_clock(
        &mut self,
        state: &mut IdentityReplayStateV1,
        raw: &RawMboEventV1,
    ) {
        if raw.flags_raw & FLAG_BAD_TS_RECV != 0
            || raw.ts_recv == hft_mbo_event_contract::UNDEF_TIMESTAMP
        {
            return;
        }
        self.global_receive_watermark_ns = Some(
            self.global_receive_watermark_ns
                .map_or(raw.ts_recv, |current| current.max(raw.ts_recv)),
        );
        if state
            .last_trusted_ts_recv
            .is_none_or(|previous| raw.ts_recv >= previous)
        {
            state.last_trusted_ts_recv = Some(raw.ts_recv);
        }
    }

    fn quarantine_while_invalid(
        &mut self,
        state: &mut IdentityReplayStateV1,
        raw: RawMboEventV1,
        reason: XnasQuarantineReasonV1,
    ) -> Result<(), XnasReplayErrorV1> {
        let incident =
            state
                .semantic_quarantines
                .last_mut()
                .ok_or(XnasReplayErrorV1::CounterInvariant(
                    "invalid identity has no semantic quarantine incident",
                ))?;
        if incident
            .candidate_source_ordinals
            .contains(&raw.raw_ordinal)
            || incident
                .while_invalid_records
                .iter()
                .any(|record| record.raw.raw_ordinal == raw.raw_ordinal)
        {
            return Err(XnasReplayErrorV1::CounterInvariant(
                "semantic quarantine recorded a duplicate source ordinal",
            ));
        }
        let mut next_counts = self.counts.clone();
        next_counts
            .quarantine_while_invalid()
            .map_err(XnasReplayErrorV1::CounterInvariant)?;
        incident
            .while_invalid_records
            .push(XnasInvalidStateQuarantinedRecordV1 { raw, reason });
        self.counts = next_counts;
        Ok(())
    }

    fn quarantine_semantic_candidate(
        &mut self,
        state: &mut IdentityReplayStateV1,
        trigger: RawMboEventV1,
        reason: XnasQuarantineReasonV1,
        offending_candidate_source_ordinal: Option<u64>,
        explicit_open: Option<OpenEnvelopeV1>,
    ) -> Result<Option<XnasStagedBookUpdateV1>, XnasReplayErrorV1> {
        let open = explicit_open.or_else(|| state.open.take());
        let mut source_ordinals = open
            .as_ref()
            .map(|value| value.source_ordinals().collect::<Vec<_>>())
            .unwrap_or_default();
        if source_ordinals.last().copied() != Some(trigger.raw_ordinal) {
            source_ordinals.push(trigger.raw_ordinal);
        }
        if source_ordinals.is_empty() || source_ordinals.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(XnasReplayErrorV1::CounterInvariant(
                "semantic quarantine ordinals are not strictly increasing",
            ));
        }
        let record_count = u64::try_from(source_ordinals.len())
            .map_err(|_| XnasReplayErrorV1::CounterInvariant("quarantine count overflow"))?;
        let incident_index = state.semantic_quarantines.len();
        let incident_index_u64 = u64::try_from(incident_index)
            .map_err(|_| XnasReplayErrorV1::CounterInvariant("semantic incident index overflow"))?;
        invalidate_active_validity_epoch(
            state,
            source_ordinals[0],
            Some(trigger.raw_ordinal),
            self.global_receive_watermark_ns,
            XnasValidityInvalidationReasonV1::SemanticQuarantine {
                incident_index: incident_index_u64,
            },
        )?;
        let mut next_counts = self.counts.clone();
        next_counts
            .quarantine_semantic_candidate(record_count)
            .map_err(XnasReplayErrorV1::CounterInvariant)?;
        state
            .active_semantic_quarantine
            .get_or_insert(incident_index);
        state
            .semantic_quarantines
            .push(XnasSemanticQuarantineIncidentV1 {
                detected_at: trigger,
                offending_candidate_source_ordinal,
                reason,
                global_receive_watermark_ns: self.global_receive_watermark_ns,
                candidate_source_ordinals: source_ordinals,
                while_invalid_records: Vec::new(),
                recovery: None,
            });
        state.open = None;
        state.lifecycle = IdentityLifecycleV1::Invalid;
        state.active_recovery_reset_ordinal = None;
        self.counts = next_counts;
        Ok(None)
    }

    fn handle_envelope_failure(
        &mut self,
        state: &mut IdentityReplayStateV1,
        trigger: RawMboEventV1,
        source: EnvelopeAssemblyErrorV1,
        open: Option<OpenEnvelopeV1>,
    ) -> Result<Option<XnasStagedBookUpdateV1>, XnasReplayErrorV1> {
        if classify_envelope_failure(source) == ReplayFailureScopeV1::ReplayFatal {
            return Err(XnasReplayErrorV1::Envelope {
                raw_ordinal: trigger.raw_ordinal,
                source,
            });
        }
        self.quarantine_semantic_candidate(
            state,
            trigger,
            XnasQuarantineReasonV1::Envelope { source },
            Some(trigger.raw_ordinal),
            open,
        )
    }

    fn quarantine_failed_ready_envelope(
        &mut self,
        state: &mut IdentityReplayStateV1,
        ready: ReadyEnvelopeTxnV1,
        source: BookTransactionErrorV1,
    ) -> Result<Option<XnasStagedBookUpdateV1>, XnasReplayErrorV1> {
        let witness = *ready.witness().event().raw();
        let offending_candidate_source_ordinal = source.offending_raw_ordinal();
        let source_ordinals = ready
            .events()
            .iter()
            .map(|event| event.event().raw().raw_ordinal)
            .collect::<Vec<_>>();
        if source_ordinals.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(XnasReplayErrorV1::CounterInvariant(
                "failed transaction quarantine ordinals are not strictly increasing",
            ));
        }
        let candidate_count = u64::try_from(source_ordinals.len())
            .map_err(|_| XnasReplayErrorV1::CounterInvariant("quarantine count overflow"))?;
        let incident_index = state.semantic_quarantines.len();
        let incident_index_u64 = u64::try_from(incident_index)
            .map_err(|_| XnasReplayErrorV1::CounterInvariant("semantic incident index overflow"))?;
        invalidate_active_validity_epoch(
            state,
            source_ordinals[0],
            Some(witness.raw_ordinal),
            self.global_receive_watermark_ns,
            XnasValidityInvalidationReasonV1::SemanticQuarantine {
                incident_index: incident_index_u64,
            },
        )?;
        let mut next_counts = self.counts.clone();
        next_counts
            .quarantine_semantic_candidate(candidate_count)
            .map_err(XnasReplayErrorV1::CounterInvariant)?;
        next_counts
            .quarantine_while_invalid()
            .map_err(XnasReplayErrorV1::CounterInvariant)?;
        state
            .active_semantic_quarantine
            .get_or_insert(incident_index);
        state
            .semantic_quarantines
            .push(XnasSemanticQuarantineIncidentV1 {
                detected_at: witness,
                offending_candidate_source_ordinal,
                reason: XnasQuarantineReasonV1::Book { source },
                global_receive_watermark_ns: self.global_receive_watermark_ns,
                candidate_source_ordinals: source_ordinals,
                while_invalid_records: vec![XnasInvalidStateQuarantinedRecordV1 {
                    raw: witness,
                    reason: XnasQuarantineReasonV1::ClosureWitnessAfterFailedTransaction,
                }],
                recovery: None,
            });
        state.open = None;
        state.lifecycle = IdentityLifecycleV1::Invalid;
        state.active_recovery_reset_ordinal = None;
        self.counts = next_counts;
        Ok(None)
    }

    fn begin_recovery(
        &mut self,
        state: &mut IdentityReplayStateV1,
        reset: EventDispositionV1,
    ) -> Result<Option<XnasStagedBookUpdateV1>, XnasReplayErrorV1> {
        let raw_ordinal = reset.event().raw().raw_ordinal;
        let mut next_counts = self.counts.clone();
        let prior_quarantine = state
            .open
            .as_ref()
            .map(|prior| prior.source_ordinals().collect::<Vec<_>>());
        if let Some(prior) = &prior_quarantine {
            next_counts
                .quarantine_reset_boundary(u64::try_from(prior.len()).map_err(|_| {
                    XnasReplayErrorV1::CounterInvariant("quarantine count overflow")
                })?)
                .map_err(XnasReplayErrorV1::CounterInvariant)?;
        }
        checked_increment(
            &mut next_counts.reset_recovery_candidates,
            1,
            "recovery candidate count overflow",
        )?;
        let first_ineligible_source_ordinal = prior_quarantine
            .as_ref()
            .and_then(|prior| prior.first().copied())
            .unwrap_or(raw_ordinal);
        invalidate_active_validity_epoch(
            state,
            first_ineligible_source_ordinal,
            Some(raw_ordinal),
            self.global_receive_watermark_ns,
            XnasValidityInvalidationReasonV1::ResetBoundary,
        )?;
        self.counts = next_counts;
        state.open = None;
        if let Some(quarantined_source_ordinals) = prior_quarantine {
            state
                .reset_boundary_quarantines
                .push(XnasResetBoundaryQuarantineV1 {
                    reset_source_ordinal: raw_ordinal,
                    quarantined_source_ordinals,
                });
        }
        state.lifecycle = IdentityLifecycleV1::Recovering;
        state.active_recovery_reset_ordinal = Some(raw_ordinal);
        state.open = Some(self.new_open_envelope(reset, true));
        Ok(None)
    }

    fn new_open_envelope(&self, event: EventDispositionV1, recovery: bool) -> OpenEnvelopeV1 {
        OpenEnvelopeV1::new(
            event,
            recovery,
            self.config.max_envelope_members.get(),
            self.config.max_sequence_blocks.get(),
        )
    }

    fn quarantine_eof_tails(&mut self) -> Result<(), XnasReplayErrorV1> {
        let mut next_counts = self.counts.clone();
        let mut tails = Vec::new();
        for (&identity, state) in &self.identities {
            if let Some(open) = &state.open {
                let member_count = u64::try_from(open.len()).map_err(|_| {
                    XnasReplayErrorV1::CounterInvariant("EOF quarantine count overflow")
                })?;
                next_counts
                    .quarantine_eof_tail(member_count)
                    .map_err(XnasReplayErrorV1::CounterInvariant)?;
                tails.push((
                    identity,
                    XnasEofTailQuarantineV1 {
                        first_source_ordinal: open.first_source_ordinal(),
                        last_source_ordinal: open.last_source_ordinal(),
                        member_count,
                        source_ordinals: open.source_ordinals().collect(),
                        reason: if open.current_saw_last() {
                            XnasEofTailReasonV1::TerminalCandidateWithoutWitness
                        } else {
                            XnasEofTailReasonV1::NonterminalEnvelope
                        },
                        recovery_candidate: open.is_recovery(),
                    },
                ));
            }
        }
        for (identity, tail) in tails {
            let state = self
                .identities
                .get_mut(&identity)
                .expect("tail identity came from this map");
            invalidate_active_validity_epoch(
                state,
                tail.first_source_ordinal,
                None,
                self.global_receive_watermark_ns,
                XnasValidityInvalidationReasonV1::EofTail {
                    reason: tail.reason,
                },
            )?;
            state.open = None;
            state.eof_tail_quarantine = Some(tail);
            state.lifecycle = IdentityLifecycleV1::InvalidAfterEofTail;
        }
        self.counts = next_counts;
        Ok(())
    }
}

fn checked_increment(
    value: &mut u64,
    amount: u64,
    reason: &'static str,
) -> Result<(), XnasReplayErrorV1> {
    *value = value
        .checked_add(amount)
        .ok_or(XnasReplayErrorV1::CounterInvariant(reason))?;
    Ok(())
}

fn validate_valid_commit(
    state: &IdentityReplayStateV1,
    terminal_source_ordinal: u64,
    witness_source_ordinal: u64,
    effective_available_ns: u64,
    recovery_reset_source_ordinal: Option<u64>,
) -> Result<(), XnasReplayErrorV1> {
    if witness_source_ordinal <= terminal_source_ordinal {
        return Err(XnasReplayErrorV1::CounterInvariant(
            "qualification witness must follow its terminal member",
        ));
    }
    if recovery_reset_source_ordinal.is_some_and(|reset| reset > terminal_source_ordinal) {
        return Err(XnasReplayErrorV1::CounterInvariant(
            "recovery reset must belong to the qualified envelope",
        ));
    }
    match state.active_validity_epoch.as_ref() {
        Some(epoch) => {
            if recovery_reset_source_ordinal.is_some() {
                return Err(XnasReplayErrorV1::CounterInvariant(
                    "an active validity epoch cannot be recovery-qualified again",
                ));
            }
            if terminal_source_ordinal <= epoch.last_committed_terminal_source_ordinal
                || effective_available_ns < epoch.last_effective_available_ns
            {
                return Err(XnasReplayErrorV1::CounterInvariant(
                    "validity epoch commit order is inconsistent",
                ));
            }
        }
        None if state.ever_valid && recovery_reset_source_ordinal.is_none() => {
            return Err(XnasReplayErrorV1::CounterInvariant(
                "a previously invalidated identity requires explicit recovery",
            ));
        }
        None => {}
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn record_valid_commit(
    state: &mut IdentityReplayStateV1,
    terminal_source_ordinal: u64,
    witness_source_ordinal: u64,
    effective_available_ns: u64,
    recovery_reset_source_ordinal: Option<u64>,
    transition_chain_sha256: Sha256DigestV1,
) {
    if let Some(epoch) = state.active_validity_epoch.as_mut() {
        epoch.last_committed_terminal_source_ordinal = terminal_source_ordinal;
        epoch.last_effective_available_ns = effective_available_ns;
        epoch.last_transition_chain_sha256 = transition_chain_sha256;
    } else {
        state.active_validity_epoch = Some(ActiveValidityEpochV1 {
            qualification: XnasValidityEpochQualificationV1 {
                terminal_source_ordinal,
                witness_source_ordinal,
                effective_available_ns,
                recovery_reset_source_ordinal,
            },
            last_committed_terminal_source_ordinal: terminal_source_ordinal,
            last_effective_available_ns: effective_available_ns,
            last_transition_chain_sha256: transition_chain_sha256,
        });
    }
    state.ever_valid = true;
    state
        .first_qualified_effective_available_ns
        .get_or_insert(effective_available_ns);
    state
        .first_qualified_terminal_source_ordinal
        .get_or_insert(terminal_source_ordinal);
    state.last_effective_available_ns = Some(effective_available_ns);
    state.last_terminal_source_ordinal = Some(terminal_source_ordinal);
}

fn invalidate_active_validity_epoch(
    state: &mut IdentityReplayStateV1,
    first_ineligible_source_ordinal: u64,
    detected_at_source_ordinal: Option<u64>,
    global_receive_watermark_ns: Option<u64>,
    reason: XnasValidityInvalidationReasonV1,
) -> Result<(), XnasReplayErrorV1> {
    let Some(epoch) = state.active_validity_epoch.as_ref() else {
        return Ok(());
    };
    if first_ineligible_source_ordinal <= epoch.last_committed_terminal_source_ordinal
        || first_ineligible_source_ordinal < epoch.qualification.witness_source_ordinal
        || detected_at_source_ordinal
            .is_some_and(|detected| detected < first_ineligible_source_ordinal)
    {
        return Err(XnasReplayErrorV1::CounterInvariant(
            "validity invalidation boundary is inconsistent",
        ));
    }
    let active = state
        .active_validity_epoch
        .take()
        .expect("active validity epoch was checked");
    state.validity_epochs.push(XnasValidityEpochV1 {
        qualification: active.qualification,
        last_committed_terminal_source_ordinal: active.last_committed_terminal_source_ordinal,
        last_effective_available_ns: active.last_effective_available_ns,
        last_committed_book_state_sha256: state.book.state_digest(),
        last_transition_chain_sha256: active.last_transition_chain_sha256,
        invalidation: XnasValidityInvalidationV1 {
            first_ineligible_source_ordinal,
            detected_at_source_ordinal,
            global_receive_watermark_ns,
            reason,
        },
    });
    Ok(())
}

fn is_source_fatal_validation(reason: &ValidationReasonV1) -> bool {
    reason.boundary_class() == ValidationBoundaryClassV1::SourceStreamFatal
        || matches!(
            reason,
            ValidationReasonV1::PublisherSpecificPolicyRequired(_)
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplayFailureScopeV1 {
    IdentityQuarantine,
    ReplayFatal,
}

/// Decide whether an envelope failure belongs to one market-data identity or
/// invalidates the source/replay boundary itself.
///
/// This match is intentionally exhaustive. Adding an assembly error must fail
/// compilation until its failure owner has been chosen explicitly.
const fn classify_envelope_failure(source: EnvelopeAssemblyErrorV1) -> ReplayFailureScopeV1 {
    match source {
        EnvelopeAssemblyErrorV1::BlockTimestampMismatch
        | EnvelopeAssemblyErrorV1::ExactDuplicate
        | EnvelopeAssemblyErrorV1::LastToNonLast
        | EnvelopeAssemblyErrorV1::ChannelChange
        | EnvelopeAssemblyErrorV1::SequenceRegressionOrReuse
        | EnvelopeAssemblyErrorV1::ReceiveTimeChangedBeforeTerminal => {
            ReplayFailureScopeV1::IdentityQuarantine
        }
        EnvelopeAssemblyErrorV1::SourceChange
        | EnvelopeAssemblyErrorV1::IdentityChange
        | EnvelopeAssemblyErrorV1::NonIncreasingSourceOrdinal
        | EnvelopeAssemblyErrorV1::WrongSameBlockSequence
        | EnvelopeAssemblyErrorV1::MemberLimit { .. }
        | EnvelopeAssemblyErrorV1::SequenceBlockLimit { .. }
        | EnvelopeAssemblyErrorV1::NotTerminal
        | EnvelopeAssemblyErrorV1::AvailabilityBeforeEndpoint
        | EnvelopeAssemblyErrorV1::CountOverflow => ReplayFailureScopeV1::ReplayFatal,
    }
}

/// Decide whether a book transaction failure is attributable to one identity
/// candidate or proves a replay/invariant failure.
///
/// Resource limits and internal arithmetic/reconciliation failures remain
/// fatal: quarantining them would silently select away high-activity data or a
/// software defect. This match is exhaustive for compile-time drift control.
const fn classify_book_failure(source: &BookTransactionErrorV1) -> ReplayFailureScopeV1 {
    match source {
        BookTransactionErrorV1::DuplicateAdd { .. }
        | BookTransactionErrorV1::MissingModify { .. }
        | BookTransactionErrorV1::ModifySideMismatch { .. }
        | BookTransactionErrorV1::MissingCancel { .. }
        | BookTransactionErrorV1::CancelIdentityMismatch { .. }
        | BookTransactionErrorV1::OverCancel { .. }
        | BookTransactionErrorV1::LockedOrCrossedEndpoint { .. } => {
            ReplayFailureScopeV1::IdentityQuarantine
        }
        BookTransactionErrorV1::ZeroSnapshotDepth
        | BookTransactionErrorV1::SourceMismatch
        | BookTransactionErrorV1::IdentityMismatch
        | BookTransactionErrorV1::MemberOrdinalNotIncreasing
        | BookTransactionErrorV1::UnexpectedClear
        | BookTransactionErrorV1::InvalidRecoveryClear
        | BookTransactionErrorV1::LevelArithmeticOverflow
        | BookTransactionErrorV1::LevelAggregateUnderflow
        | BookTransactionErrorV1::LevelPopulationMismatch
        | BookTransactionErrorV1::ZeroRestingOrder
        | BookTransactionErrorV1::InternalReconciliationOverflow
        | BookTransactionErrorV1::InternalLevelStateMismatch
        | BookTransactionErrorV1::InternalLockedOrCrossedState
        | BookTransactionErrorV1::CommitIndexOverflow
        | BookTransactionErrorV1::ResetEpochOverflow
        | BookTransactionErrorV1::CountOverflow => ReplayFailureScopeV1::ReplayFatal,
    }
}

#[cfg(test)]
mod replay_failure_scope_tests {
    use super::*;

    #[test]
    fn every_envelope_failure_has_the_intended_scope() {
        let identity_quarantine = [
            EnvelopeAssemblyErrorV1::BlockTimestampMismatch,
            EnvelopeAssemblyErrorV1::ExactDuplicate,
            EnvelopeAssemblyErrorV1::LastToNonLast,
            EnvelopeAssemblyErrorV1::ChannelChange,
            EnvelopeAssemblyErrorV1::SequenceRegressionOrReuse,
            EnvelopeAssemblyErrorV1::ReceiveTimeChangedBeforeTerminal,
        ];
        let replay_fatal = [
            EnvelopeAssemblyErrorV1::SourceChange,
            EnvelopeAssemblyErrorV1::IdentityChange,
            EnvelopeAssemblyErrorV1::NonIncreasingSourceOrdinal,
            EnvelopeAssemblyErrorV1::WrongSameBlockSequence,
            EnvelopeAssemblyErrorV1::MemberLimit { limit: 1 },
            EnvelopeAssemblyErrorV1::SequenceBlockLimit { limit: 1 },
            EnvelopeAssemblyErrorV1::NotTerminal,
            EnvelopeAssemblyErrorV1::AvailabilityBeforeEndpoint,
            EnvelopeAssemblyErrorV1::CountOverflow,
        ];

        for source in identity_quarantine {
            assert_eq!(
                classify_envelope_failure(source),
                ReplayFailureScopeV1::IdentityQuarantine
            );
        }
        for source in replay_fatal {
            assert_eq!(
                classify_envelope_failure(source),
                ReplayFailureScopeV1::ReplayFatal
            );
        }
    }

    #[test]
    fn every_book_failure_has_the_intended_scope() {
        let identity_quarantine = [
            BookTransactionErrorV1::DuplicateAdd {
                order_id: 1,
                raw_ordinal: 1,
            },
            BookTransactionErrorV1::MissingModify {
                order_id: 1,
                raw_ordinal: 1,
            },
            BookTransactionErrorV1::ModifySideMismatch {
                order_id: 1,
                raw_ordinal: 1,
            },
            BookTransactionErrorV1::MissingCancel {
                order_id: 1,
                raw_ordinal: 1,
            },
            BookTransactionErrorV1::CancelIdentityMismatch {
                order_id: 1,
                raw_ordinal: 1,
            },
            BookTransactionErrorV1::OverCancel {
                order_id: 1,
                raw_ordinal: 1,
                resting: 1,
                cancelled: 2,
            },
            BookTransactionErrorV1::LockedOrCrossedEndpoint {
                best_bid: 2,
                best_ask: 1,
            },
        ];
        let replay_fatal = [
            BookTransactionErrorV1::ZeroSnapshotDepth,
            BookTransactionErrorV1::SourceMismatch,
            BookTransactionErrorV1::IdentityMismatch,
            BookTransactionErrorV1::MemberOrdinalNotIncreasing,
            BookTransactionErrorV1::UnexpectedClear,
            BookTransactionErrorV1::InvalidRecoveryClear,
            BookTransactionErrorV1::LevelArithmeticOverflow,
            BookTransactionErrorV1::LevelAggregateUnderflow,
            BookTransactionErrorV1::LevelPopulationMismatch,
            BookTransactionErrorV1::ZeroRestingOrder,
            BookTransactionErrorV1::InternalReconciliationOverflow,
            BookTransactionErrorV1::InternalLevelStateMismatch,
            BookTransactionErrorV1::InternalLockedOrCrossedState,
            BookTransactionErrorV1::CommitIndexOverflow,
            BookTransactionErrorV1::ResetEpochOverflow,
            BookTransactionErrorV1::CountOverflow,
        ];

        for source in &identity_quarantine {
            assert_eq!(
                classify_book_failure(source),
                ReplayFailureScopeV1::IdentityQuarantine
            );
        }
        for source in &replay_fatal {
            assert_eq!(
                classify_book_failure(source),
                ReplayFailureScopeV1::ReplayFatal
            );
        }
    }
}

fn semantic_quarantine_ledgers_reconcile(
    identities: &BTreeMap<XnasIdentityV1, IdentityReplayStateV1>,
    counts: &XnasReplayCountsV1,
) -> bool {
    let mut candidate_records = 0_u64;
    let mut while_invalid_records = 0_u64;
    let mut incidents = 0_u64;
    let mut recovery_qualifications = 0_u64;
    let mut rejected_records = 0_u64;
    let mut quarantined_ordinals = BTreeSet::new();
    let mut semantic_ordinals = BTreeSet::new();

    for (identity, state) in identities {
        let mut identity_semantic_ordinals = BTreeSet::new();
        let active_start = state.active_semantic_quarantine;
        for (index, incident) in state.semantic_quarantines.iter().enumerate() {
            incidents = match incidents.checked_add(1) {
                Some(value) => value,
                None => return false,
            };
            let candidate = match u64::try_from(incident.candidate_source_ordinals.len()) {
                Ok(value) => value,
                Err(_) => return false,
            };
            let invalid = match u64::try_from(incident.while_invalid_records.len()) {
                Ok(value) => value,
                Err(_) => return false,
            };
            candidate_records = match candidate_records.checked_add(candidate) {
                Some(value) => value,
                None => return false,
            };
            while_invalid_records = match while_invalid_records.checked_add(invalid) {
                Some(value) => value,
                None => return false,
            };
            if incident.candidate_source_ordinals.is_empty()
                || incident
                    .candidate_source_ordinals
                    .windows(2)
                    .any(|pair| pair[0] >= pair[1])
                || incident
                    .while_invalid_records
                    .windows(2)
                    .any(|pair| pair[0].raw.raw_ordinal >= pair[1].raw.raw_ordinal)
                || incident.while_invalid_records.first().is_some_and(|first| {
                    first.raw.raw_ordinal
                        <= *incident
                            .candidate_source_ordinals
                            .last()
                            .expect("checked nonempty")
                })
            {
                return false;
            }
            for &ordinal in &incident.candidate_source_ordinals {
                if !identity_semantic_ordinals.insert(ordinal)
                    || !semantic_ordinals.insert(ordinal)
                    || !quarantined_ordinals.insert(ordinal)
                {
                    return false;
                }
            }
            for record in &incident.while_invalid_records {
                let ordinal = record.raw.raw_ordinal;
                if !identity_semantic_ordinals.insert(ordinal)
                    || !semantic_ordinals.insert(ordinal)
                    || !quarantined_ordinals.insert(ordinal)
                {
                    return false;
                }
            }
            let should_be_active = active_start.is_some_and(|start| index >= start);
            if should_be_active == incident.recovery.is_some() {
                return false;
            }
            if let Some(recovery) = &incident.recovery {
                if !state.recovery_qualifications.contains(recovery) {
                    return false;
                }
            }
        }
        recovery_qualifications = match recovery_qualifications.checked_add(
            match u64::try_from(state.recovery_qualifications.len()) {
                Ok(value) => value,
                Err(_) => return false,
            },
        ) {
            Some(value) => value,
            None => return false,
        };
        for rejected in &state.rejected_record_quarantines {
            rejected_records = match rejected_records.checked_add(1) {
                Some(value) => value,
                None => return false,
            };
            let incident_index = match usize::try_from(rejected.identity_incident_index) {
                Ok(value) => value,
                Err(_) => return false,
            };
            let incident = match state.semantic_quarantines.get(incident_index) {
                Some(value) => value,
                None => return false,
            };
            let phase_matches = match rejected.phase {
                XnasRejectedRecordPhaseV1::CandidateTrigger => incident
                    .candidate_source_ordinals
                    .contains(&rejected.raw.raw_ordinal),
                XnasRejectedRecordPhaseV1::WhileInvalid => incident
                    .while_invalid_records
                    .iter()
                    .any(|record| record.raw.raw_ordinal == rejected.raw.raw_ordinal),
            };
            if rejected.raw.raw_ordinal != rejected.failure.raw_ordinal
                || rejected.raw.source_object_sha256 != rejected.failure.source_object_sha256
                || XnasIdentityV1::new(rejected.raw.publisher_id, rejected.raw.instrument_id)
                    != *identity
                || !identity_semantic_ordinals.contains(&rejected.raw.raw_ordinal)
                || !phase_matches
            {
                return false;
            }
        }
        for reset in &state.reset_boundary_quarantines {
            for &ordinal in &reset.quarantined_source_ordinals {
                if !quarantined_ordinals.insert(ordinal) {
                    return false;
                }
            }
        }
        if let Some(tail) = &state.eof_tail_quarantine {
            for &ordinal in &tail.source_ordinals {
                if !quarantined_ordinals.insert(ordinal) {
                    return false;
                }
            }
        }
    }

    let quarantined_record_count = match u64::try_from(quarantined_ordinals.len()) {
        Ok(value) => value,
        Err(_) => return false,
    };
    candidate_records == counts.semantic_candidate_quarantined_records
        && while_invalid_records == counts.semantic_while_invalid_quarantined_records
        && incidents == counts.semantic_quarantine_incidents
        && rejected_records == counts.decoded_semantic_rejections
        && quarantined_record_count == counts.quarantined_records
        && recovery_qualifications
            == counts
                .private_book_resets
                .checked_sub(counts.initial_clear_controls)
                .unwrap_or(u64::MAX)
}

fn validity_epoch_ledgers_reconcile(
    identities: &BTreeMap<XnasIdentityV1, IdentityReplayStateV1>,
) -> bool {
    for state in identities.values() {
        if state.active_validity_epoch.is_some()
            || state.ever_valid == state.validity_epochs.is_empty()
        {
            return false;
        }
        if !state.ever_valid {
            if state.first_qualified_effective_available_ns.is_some()
                || state.first_qualified_terminal_source_ordinal.is_some()
                || state.last_effective_available_ns.is_some()
                || state.last_terminal_source_ordinal.is_some()
                || !state.recovery_qualifications.is_empty()
            {
                return false;
            }
            continue;
        }

        let first = match state.validity_epochs.first() {
            Some(value) => value,
            None => return false,
        };
        let last = match state.validity_epochs.last() {
            Some(value) => value,
            None => return false,
        };
        if state.first_qualified_terminal_source_ordinal
            != Some(first.qualification.terminal_source_ordinal)
            || state.first_qualified_effective_available_ns
                != Some(first.qualification.effective_available_ns)
            || state.last_terminal_source_ordinal
                != Some(last.last_committed_terminal_source_ordinal)
            || state.last_effective_available_ns != Some(last.last_effective_available_ns)
            || last.last_committed_book_state_sha256 != state.book.state_digest()
            || last.last_transition_chain_sha256 != state.book.transition_chain_sha256()
        {
            return false;
        }

        let mut recovery_index = 0_usize;
        let mut previous_invalidation: Option<&XnasValidityInvalidationV1> = None;
        for (index, epoch) in state.validity_epochs.iter().enumerate() {
            let qualification = &epoch.qualification;
            let invalidation = &epoch.invalidation;
            if qualification.witness_source_ordinal <= qualification.terminal_source_ordinal
                || epoch.last_committed_terminal_source_ordinal
                    < qualification.terminal_source_ordinal
                || epoch.last_effective_available_ns < qualification.effective_available_ns
                || invalidation.first_ineligible_source_ordinal
                    <= epoch.last_committed_terminal_source_ordinal
                || invalidation.first_ineligible_source_ordinal
                    < qualification.witness_source_ordinal
                || invalidation
                    .detected_at_source_ordinal
                    .is_some_and(|detected| detected < invalidation.first_ineligible_source_ordinal)
            {
                return false;
            }
            if let Some(previous) = previous_invalidation {
                if qualification.terminal_source_ordinal < previous.first_ineligible_source_ordinal
                    || previous
                        .detected_at_source_ordinal
                        .is_some_and(|detected| qualification.terminal_source_ordinal < detected)
                {
                    return false;
                }
            }

            match qualification.recovery_reset_source_ordinal {
                Some(reset_source_ordinal) => {
                    let recovery = match state.recovery_qualifications.get(recovery_index) {
                        Some(value) => value,
                        None => return false,
                    };
                    if reset_source_ordinal != recovery.reset_source_ordinal
                        || qualification.terminal_source_ordinal != recovery.terminal_source_ordinal
                        || qualification.witness_source_ordinal != recovery.witness_source_ordinal
                        || qualification.effective_available_ns != recovery.effective_available_ns
                    {
                        return false;
                    }
                    recovery_index += 1;
                }
                None if index != 0 => return false,
                None => {}
            }

            match &invalidation.reason {
                XnasValidityInvalidationReasonV1::SemanticQuarantine { incident_index } => {
                    let incident_index = match usize::try_from(*incident_index) {
                        Ok(value) => value,
                        Err(_) => return false,
                    };
                    let incident = match state.semantic_quarantines.get(incident_index) {
                        Some(value) => value,
                        None => return false,
                    };
                    if incident.candidate_source_ordinals.first().copied()
                        != Some(invalidation.first_ineligible_source_ordinal)
                        || invalidation.detected_at_source_ordinal
                            != Some(incident.detected_at.raw_ordinal)
                    {
                        return false;
                    }
                }
                XnasValidityInvalidationReasonV1::ResetBoundary => {
                    if invalidation.detected_at_source_ordinal.is_none() {
                        return false;
                    }
                }
                XnasValidityInvalidationReasonV1::EofTail { reason } => {
                    let tail = match &state.eof_tail_quarantine {
                        Some(value) => value,
                        None => return false,
                    };
                    if invalidation.detected_at_source_ordinal.is_some()
                        || invalidation.first_ineligible_source_ordinal != tail.first_source_ordinal
                        || reason != &tail.reason
                    {
                        return false;
                    }
                }
            }
            previous_invalidation = Some(invalidation);
        }
        if recovery_index != state.recovery_qualifications.len() {
            return false;
        }
    }
    true
}

fn initial_committed_observation_chain(
    source_digest: Sha256DigestV1,
    config: XnasReplayConfigV1,
) -> Sha256DigestV1 {
    let mut hasher = Sha256::new();
    hasher.update(b"hft.xnas_committed_observation_chain.seed.v2\0");
    hasher.update(source_digest.as_bytes());
    hasher.update(
        u64::try_from(config.snapshot_depth.get())
            .expect("usize always fits u64 on supported targets")
            .to_le_bytes(),
    );
    hasher.update(
        u64::try_from(config.max_envelope_members.get())
            .expect("usize always fits u64 on supported targets")
            .to_le_bytes(),
    );
    hasher.update(
        u64::try_from(config.max_sequence_blocks.get())
            .expect("usize always fits u64 on supported targets")
            .to_le_bytes(),
    );
    Sha256DigestV1::from_bytes(hasher.finalize().into())
}

fn next_committed_observation_chain(
    prior: Sha256DigestV1,
    committed_observation: Sha256DigestV1,
) -> Sha256DigestV1 {
    let mut hasher = Sha256::new();
    hasher.update(b"hft.xnas_committed_observation_chain.v2\0");
    hasher.update(prior.as_bytes());
    hasher.update(committed_observation.as_bytes());
    Sha256DigestV1::from_bytes(hasher.finalize().into())
}

fn committed_observation_digest(
    encoding: &mut Vec<u8>,
    source_digest: Sha256DigestV1,
    validity_epoch_index: u64,
    symbol: &str,
    envelope_sha256: Sha256DigestV1,
    envelope: &ReadyEnvelopeTxnV1,
    book: &XnasBookCommitV1,
) -> Sha256DigestV1 {
    encoding.clear();
    encoding.extend_from_slice(b"hft.xnas_committed_observation.v2\0");
    encoding.extend_from_slice(source_digest.as_bytes());
    encoding.extend_from_slice(&envelope.identity().publisher_id().to_le_bytes());
    encoding.extend_from_slice(&envelope.identity().instrument_id().to_le_bytes());
    encoding.extend_from_slice(&validity_epoch_index.to_le_bytes());
    encoding.extend_from_slice(
        &u64::try_from(symbol.len())
            .expect("usize always fits u64 on supported targets")
            .to_le_bytes(),
    );
    encoding.extend_from_slice(symbol.as_bytes());
    encoding.extend_from_slice(envelope_sha256.as_bytes());
    encoding.extend_from_slice(&envelope.terminal_sequence().to_le_bytes());
    encoding.extend_from_slice(&envelope.terminal_source_ordinal().to_le_bytes());
    encoding.extend_from_slice(&event_semantic_tag(envelope.witness()));
    encode_raw_event(encoding, envelope.witness().event().raw());
    encoding.extend_from_slice(&envelope.endpoint_ns().to_le_bytes());
    encoding.extend_from_slice(&envelope.witness_ts_recv().to_le_bytes());
    encoding.extend_from_slice(&envelope.effective_available_ns().to_le_bytes());
    encoding.extend_from_slice(&envelope.closure_confirmation_delay_ns().to_le_bytes());
    encoding.extend_from_slice(&envelope.execution_sequence_blocks().to_le_bytes());
    encoding.extend_from_slice(&envelope.execution_carrier_count().to_le_bytes());
    encoding.push(u8::from(envelope.is_recovery()));
    encoding.extend_from_slice(&book.commit_index().to_le_bytes());
    encoding.extend_from_slice(&book.reset_epoch().to_le_bytes());
    encoding.extend_from_slice(&book.book_commands_committed().to_le_bytes());
    encoding.push(u8::from(book.exact_endpoint_state_changed()));
    encoding.extend_from_slice(book.transition_chain_sha256().as_bytes());
    encode_book_snapshot(encoding, book.snapshot());
    Sha256DigestV1::from_bytes(Sha256::digest(encoding.as_slice()).into())
}

fn encode_book_snapshot(encoding: &mut Vec<u8>, snapshot: &XnasBookSnapshotV1) {
    for levels in [snapshot.bids(), snapshot.asks()] {
        encoding.extend_from_slice(
            &u64::try_from(levels.len())
                .expect("usize always fits u64 on supported targets")
                .to_le_bytes(),
        );
        for level in levels {
            encoding.extend_from_slice(&level.price_raw().to_le_bytes());
            encoding.extend_from_slice(&level.aggregate_size().to_le_bytes());
            encoding.extend_from_slice(&level.order_count().to_le_bytes());
        }
    }
    encoding.extend_from_slice(&snapshot.live_orders().to_le_bytes());
}

#[cfg(test)]
fn committed_observation_digest_streaming_reference(
    source_digest: Sha256DigestV1,
    validity_epoch_index: u64,
    symbol: &str,
    envelope_sha256: Sha256DigestV1,
    envelope: &ReadyEnvelopeTxnV1,
    book: &XnasBookCommitV1,
) -> Sha256DigestV1 {
    let mut hasher = Sha256::new();
    hasher.update(b"hft.xnas_committed_observation.v2\0");
    hasher.update(source_digest.as_bytes());
    hasher.update(envelope.identity().publisher_id().to_le_bytes());
    hasher.update(envelope.identity().instrument_id().to_le_bytes());
    hasher.update(validity_epoch_index.to_le_bytes());
    hasher.update(
        u64::try_from(symbol.len())
            .expect("usize always fits u64 on supported targets")
            .to_le_bytes(),
    );
    hasher.update(symbol.as_bytes());
    hasher.update(envelope_sha256.as_bytes());
    hasher.update(envelope.terminal_sequence().to_le_bytes());
    hasher.update(envelope.terminal_source_ordinal().to_le_bytes());
    envelope::hash_event_semantics(&mut hasher, envelope.witness());
    envelope::hash_raw_event(&mut hasher, envelope.witness().event().raw());
    hasher.update(envelope.endpoint_ns().to_le_bytes());
    hasher.update(envelope.witness_ts_recv().to_le_bytes());
    hasher.update(envelope.effective_available_ns().to_le_bytes());
    hasher.update(envelope.closure_confirmation_delay_ns().to_le_bytes());
    hasher.update(envelope.execution_sequence_blocks().to_le_bytes());
    hasher.update(envelope.execution_carrier_count().to_le_bytes());
    hasher.update([u8::from(envelope.is_recovery())]);
    hasher.update(book.commit_index().to_le_bytes());
    hasher.update(book.reset_epoch().to_le_bytes());
    hasher.update(book.book_commands_committed().to_le_bytes());
    hasher.update([u8::from(book.exact_endpoint_state_changed())]);
    hasher.update(book.transition_chain_sha256().as_bytes());
    for levels in [book.snapshot().bids(), book.snapshot().asks()] {
        hasher.update(
            u64::try_from(levels.len())
                .expect("usize always fits u64 on supported targets")
                .to_le_bytes(),
        );
        for level in levels {
            hasher.update(level.price_raw().to_le_bytes());
            hasher.update(level.aggregate_size().to_le_bytes());
            hasher.update(level.order_count().to_le_bytes());
        }
    }
    hasher.update(book.snapshot().live_orders().to_le_bytes());
    Sha256DigestV1::from_bytes(hasher.finalize().into())
}

fn is_exact_initial_clear(raw: &RawMboEventV1) -> bool {
    raw.action_raw == ACTION_CLEAR
        && raw.side_raw == SIDE_NONE
        && raw.channel_id == 0
        && raw.sequence == 0
        && raw.order_id == 0
        && raw.price_raw == UNDEF_PRICE
        && raw.size_raw == 0
        && raw.ts_in_delta == 0
        && raw.flags_raw == FLAG_BAD_TS_RECV
}

#[derive(Debug, thiserror::Error)]
pub enum XnasReplayErrorV1 {
    #[error("{0}")]
    PrefixFailed(Box<XnasReplayPrefixFailureV1>),
    #[error("strict stream has no same-file XNAS daily metadata binding")]
    MissingXnasMetadataBinding,
    #[error("strict stream source and XNAS metadata binding disagree")]
    MetadataSourceMismatch,
    #[error("XNAS metadata contains duplicate identity {0:?}")]
    DuplicateMetadataIdentity(XnasIdentityV1),
    #[error(transparent)]
    Boundary(#[from] StrictBoundaryErrorV1),
    #[error(transparent)]
    Book(#[from] BookTransactionErrorV1),
    #[error("raw ordinal mismatch: expected={expected}, actual={actual}")]
    RawOrdinalMismatch { expected: u64, actual: u64 },
    #[error("raw ordinal overflow")]
    RawOrdinalOverflow,
    #[error("event source digest mismatch at raw ordinal {0}")]
    EventSourceMismatch(u64),
    #[error("unmapped identity {identity:?} at raw ordinal {raw_ordinal}")]
    UnmappedIdentity {
        raw_ordinal: u64,
        identity: XnasIdentityV1,
    },
    #[error("envelope assembly failed at raw ordinal {raw_ordinal}: {source}")]
    Envelope {
        raw_ordinal: u64,
        #[source]
        source: EnvelopeAssemblyErrorV1,
    },
    #[error("replay counter invariant failed: {0}")]
    CounterInvariant(&'static str),
    #[error("selected decoded raw ordinal {raw_ordinal} has {primary_count} primary replay dispositions; expected exactly one")]
    SelectedOrdinalPrimaryDisposition {
        raw_ordinal: u64,
        primary_count: usize,
    },
    #[error("a failed replay cannot produce a success receipt")]
    CannotFinishFailedReplay,
    #[error("replay must reach EOF before it can produce a success receipt")]
    CannotFinishBeforeEof,
    #[error(
        "the revalidation pass must be drained to EOF before it can produce an equivalence receipt"
    )]
    CannotFinishRevalidationBeforeEof,
    #[error("a failed revalidation pass cannot be resumed")]
    CannotContinueFailedRevalidation,
    #[error("terminal revalidation receipt differs from the qualification receipt")]
    RevalidationReceiptMismatch {
        qualification: Arc<XnasReplayReceiptV1>,
        revalidation: Arc<XnasReplayReceiptV1>,
    },
    #[error("{0}")]
    TerminalDisqualified(Box<XnasTerminalDisqualificationV1>),
}

impl XnasReplayErrorV1 {
    pub fn root_cause(&self) -> &Self {
        match self {
            Self::PrefixFailed(diagnostic) => diagnostic.cause().root_cause(),
            other => other,
        }
    }

    pub fn prefix_failure(&self) -> Option<&XnasReplayPrefixFailureV1> {
        match self {
            Self::PrefixFailed(diagnostic) => Some(diagnostic),
            _ => None,
        }
    }
}

#[cfg(test)]
mod committed_observation_digest_tests {
    use super::*;
    use hft_mbo_event_contract::{
        classify_full_order_book, validate_raw_event, BoundPublisherPolicyV1, LogicalSourceV1,
        OpenedReplicaV1, OpenedRepresentationV1, PublisherPolicyIdV1, SourceDescriptorV1,
        ACTION_ADD, ACTION_FILL, ACTION_TRADE, EXPECTED_MBO_RECORD_SIZE_BYTES, EXPECTED_MBO_RTYPE,
        FLAG_LAST, SIDE_ASK, SIDE_BID,
    };

    const SOURCE: Sha256DigestV1 = Sha256DigestV1::from_bytes([19; 32]);

    fn policy() -> BoundPublisherPolicyV1 {
        BoundPublisherPolicyV1::bind(
            PublisherPolicyIdV1::XnasItchHistorical,
            &SourceDescriptorV1 {
                logical: LogicalSourceV1 {
                    catalog_release_id: "test".into(),
                    catalog_object_id: "test".into(),
                    canonical_path: "/test.dbn".into(),
                    canonical_sha256: SOURCE,
                    canonical_bytes: 1,
                    dbn_version: 1,
                    dbn_ts_out: false,
                    dataset: "XNAS.ITCH".into(),
                    schema: "mbo".into(),
                },
                opened: OpenedReplicaV1 {
                    configured_path: "/test.dbn".into(),
                    opened_path: "/test.dbn".into(),
                    representation: OpenedRepresentationV1::CanonicalObject,
                    opened_sha256: SOURCE,
                    opened_bytes: 1,
                },
            },
        )
        .unwrap()
    }

    fn disposition(raw: RawMboEventV1) -> EventDispositionV1 {
        classify_full_order_book(validate_raw_event(raw).unwrap(), &policy()).unwrap()
    }

    fn member() -> RawMboEventV1 {
        RawMboEventV1 {
            source_object_sha256: SOURCE,
            raw_ordinal: 1,
            subordinal: 0,
            rtype: EXPECTED_MBO_RTYPE,
            record_size_bytes: EXPECTED_MBO_RECORD_SIZE_BYTES,
            publisher_id: 2,
            instrument_id: 101,
            ts_event: 1_000,
            ts_recv: 2_000,
            ts_in_delta: 0,
            channel_id: 0,
            sequence: 10,
            order_id: 1,
            price_raw: 100_000_000_000,
            size_raw: 100,
            flags_raw: FLAG_LAST,
            action_raw: ACTION_ADD,
            side_raw: SIDE_BID,
        }
    }

    fn witness() -> RawMboEventV1 {
        RawMboEventV1 {
            source_object_sha256: SOURCE,
            raw_ordinal: 2,
            subordinal: 0,
            rtype: EXPECTED_MBO_RTYPE,
            record_size_bytes: EXPECTED_MBO_RECORD_SIZE_BYTES,
            publisher_id: 2,
            instrument_id: 101,
            ts_event: 1_100,
            ts_recv: 2_100,
            ts_in_delta: 0,
            channel_id: 0,
            sequence: 11,
            order_id: 0,
            price_raw: 101_000_000_000,
            size_raw: 10,
            flags_raw: FLAG_LAST,
            action_raw: ACTION_TRADE,
            side_raw: SIDE_ASK,
        }
    }

    fn digests_for(
        symbol: &str,
        snapshot_depth: usize,
        witness_raw: RawMboEventV1,
    ) -> (Sha256DigestV1, Sha256DigestV1) {
        let envelope = ReadyEnvelopeTxnV1::synthetic_for_book_test(
            vec![disposition(member())],
            disposition(witness_raw),
            false,
        );
        let mut book =
            ExactBookProjectorV1::new(SOURCE, envelope.identity(), snapshot_depth).unwrap();
        let commit = book.apply_envelope(&envelope).unwrap();
        let envelope_sha256 = envelope.commitment(SOURCE).sha256();
        (
            committed_observation_digest(
                &mut Vec::new(),
                SOURCE,
                1,
                symbol,
                envelope_sha256,
                &envelope,
                &commit,
            ),
            committed_observation_digest_streaming_reference(
                SOURCE,
                1,
                symbol,
                envelope_sha256,
                &envelope,
                &commit,
            ),
        )
    }

    fn digest_for(witness_raw: RawMboEventV1) -> Sha256DigestV1 {
        let (buffered, streaming) = digests_for("TEST", 10, witness_raw);
        assert_eq!(buffered, streaming);
        buffered
    }

    #[test]
    fn buffered_observation_encoding_matches_streaming_reference() {
        let variants = [
            ("N", 1, witness()),
            (
                "NVDA",
                2,
                RawMboEventV1 {
                    size_raw: 11,
                    ..witness()
                },
            ),
            (
                "LONGER-SYNTHETIC-SYMBOL",
                10,
                RawMboEventV1 {
                    action_raw: ACTION_FILL,
                    side_raw: SIDE_BID,
                    ..witness()
                },
            ),
        ];
        for (symbol, depth, witness_raw) in variants {
            let (buffered, streaming) = digests_for(symbol, depth, witness_raw);
            assert_eq!(buffered, streaming);
        }
    }

    #[test]
    fn every_exposed_witness_payload_family_changes_the_observation_digest() {
        let base = witness();
        let variants = vec![
            RawMboEventV1 {
                raw_ordinal: base.raw_ordinal + 1,
                ..base
            },
            RawMboEventV1 {
                ts_event: base.ts_event + 1,
                ..base
            },
            RawMboEventV1 {
                ts_recv: base.ts_recv + 1,
                ..base
            },
            RawMboEventV1 {
                ts_in_delta: 1,
                ..base
            },
            RawMboEventV1 {
                channel_id: 1,
                ..base
            },
            RawMboEventV1 {
                sequence: base.sequence + 1,
                ..base
            },
            RawMboEventV1 {
                order_id: 99,
                ..base
            },
            RawMboEventV1 {
                price_raw: base.price_raw + 1,
                ..base
            },
            RawMboEventV1 {
                size_raw: base.size_raw + 1,
                ..base
            },
            RawMboEventV1 {
                flags_raw: 0,
                ..base
            },
            RawMboEventV1 {
                action_raw: ACTION_FILL,
                ..base
            },
            RawMboEventV1 {
                side_raw: SIDE_BID,
                ..base
            },
        ];

        let baseline = digest_for(base);
        let changed = variants
            .into_iter()
            .map(digest_for)
            .collect::<BTreeSet<_>>();
        assert_eq!(changed.len(), 12);
        assert!(changed.iter().all(|digest| *digest != baseline));
    }
}
