//! Exact two-pass replay boundary for downstream feature staging.
//!
//! Pass one proves that the configured source reaches the strict terminal gate.
//! Pass two reopens the same caller-expectation-bound object and exposes committed envelope
//! observations for private downstream staging. The second terminal receipt must
//! equal the qualification receipt field-for-field at the typed value level before
//! a caller can receive [`XnasReplayEquivalenceReceiptV1`]. This proves replay
//! equivalence, not vendor-semantic correctness or publication eligibility.

use super::{
    initial_committed_observation_chain, next_committed_observation_chain, StrictXnasReplayV1,
    XnasBookCommitV1, XnasIdentityV1, XnasReplayConfigV1, XnasReplayErrorV1, XnasReplayReceiptV1,
    XnasReplayTraceV1,
};
use crate::loader::{
    CanonicalSourceExpectationV1, StrictBoundaryErrorV1, StrictDbnLoaderV1,
    XnasDailyMetadataExpectationV1,
};
use hft_mbo_event_contract::{EventDispositionV1, PublisherPolicyIdV1, Sha256DigestV1};
use serde::Serialize;
use std::sync::Arc;
use thiserror::Error;

/// Consumer-side closure over the exact pass-two observation population.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XnasCommittedObservationClosureV1 {
    observations_consumed: u64,
    terminal_chain_sha256: Sha256DigestV1,
}

impl XnasCommittedObservationClosureV1 {
    pub const fn observations_consumed(self) -> u64 {
        self.observations_consumed
    }

    pub const fn terminal_chain_sha256(self) -> Sha256DigestV1 {
        self.terminal_chain_sha256
    }
}

/// Reconstructor-owned verifier for the observations a consumer actually saw.
///
/// Merely retaining the final upstream chain value would not detect a consumer
/// that skipped an intermediate observation. This accumulator recomputes every
/// link and closes against the terminal receipt.
pub struct XnasCommittedObservationAccumulatorV1 {
    qualification_receipt: XnasReplayReceiptV1,
    source_digest: Sha256DigestV1,
    chain: Sha256DigestV1,
    observations_consumed: u64,
    last_source_ordinal: Option<u64>,
    last_effective_available_ns: Option<u64>,
}

impl XnasCommittedObservationAccumulatorV1 {
    pub fn new(receipt: &XnasReplayReceiptV1) -> Self {
        let source_digest = receipt.source().source().logical.compressed_sha256;
        let config = receipt.config();
        Self {
            qualification_receipt: receipt.clone(),
            source_digest,
            chain: initial_committed_observation_chain(source_digest, config),
            observations_consumed: 0,
            last_source_ordinal: None,
            last_effective_available_ns: None,
        }
    }

    pub fn observe(
        &mut self,
        observation: &XnasPendingEnvelopeObservationV1,
    ) -> Result<(), XnasObservationAccountingErrorV1> {
        if observation.source_object_sha256() != self.source_digest {
            return Err(XnasObservationAccountingErrorV1::SourceMismatch);
        }
        if self
            .last_source_ordinal
            .is_some_and(|last| observation.first_source_ordinal() <= last)
        {
            return Err(XnasObservationAccountingErrorV1::OrdinalRegression {
                previous_last: self.last_source_ordinal.expect("checked as Some"),
                observed_first: observation.first_source_ordinal(),
            });
        }
        if self
            .last_effective_available_ns
            .is_some_and(|last| observation.effective_available_ns() < last)
        {
            return Err(XnasObservationAccountingErrorV1::AvailabilityRegression {
                previous: self.last_effective_available_ns.expect("checked as Some"),
                observed: observation.effective_available_ns(),
            });
        }
        let expected = next_committed_observation_chain(
            self.chain,
            observation.committed_observation_sha256(),
        );
        if observation.committed_observation_chain_sha256() != expected {
            return Err(XnasObservationAccountingErrorV1::ChainMismatch);
        }
        self.chain = expected;
        self.observations_consumed = self
            .observations_consumed
            .checked_add(1)
            .ok_or(XnasObservationAccountingErrorV1::CountOverflow)?;
        self.last_source_ordinal = Some(observation.last_source_ordinal());
        self.last_effective_available_ns = Some(observation.effective_available_ns());
        Ok(())
    }

    pub fn finish(
        self,
        equivalence: &XnasReplayEquivalenceReceiptV1,
    ) -> Result<XnasCommittedObservationClosureV1, XnasObservationAccountingErrorV1> {
        let receipt = equivalence.exact_receipt();
        if receipt != &self.qualification_receipt {
            return Err(XnasObservationAccountingErrorV1::ReceiptBindingMismatch);
        }
        if self.observations_consumed != receipt.counts().completed_update_envelopes {
            return Err(XnasObservationAccountingErrorV1::ObservationCountMismatch {
                expected: receipt.counts().completed_update_envelopes,
                observed: self.observations_consumed,
            });
        }
        if self.chain != receipt.committed_observation_chain_sha256() {
            return Err(XnasObservationAccountingErrorV1::TerminalChainMismatch);
        }
        Ok(XnasCommittedObservationClosureV1 {
            observations_consumed: self.observations_consumed,
            terminal_chain_sha256: self.chain,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum XnasObservationAccountingErrorV1 {
    #[error("observation source digest differs from the qualified receipt")]
    SourceMismatch,
    #[error("observation ordinals overlap or regress: previous_last={previous_last}, observed_first={observed_first}")]
    OrdinalRegression {
        previous_last: u64,
        observed_first: u64,
    },
    #[error("observation causal availability regressed: previous={previous}, observed={observed}")]
    AvailabilityRegression { previous: u64, observed: u64 },
    #[error("observation rolling-chain link does not match the independently recomputed link")]
    ChainMismatch,
    #[error("observation count overflow")]
    CountOverflow,
    #[error("completed equivalence receipt differs from the qualification receipt bound to this accumulator")]
    ReceiptBindingMismatch,
    #[error("observation population mismatch: expected={expected}, observed={observed}")]
    ObservationCountMismatch { expected: u64, observed: u64 },
    #[error("recomputed terminal observation chain differs from the receipt")]
    TerminalChainMismatch,
}

/// A source/configuration pair that completed one strict terminal replay.
///
/// The plan is reusable so consumers can retry a failed private staging pass.
/// Every retry reopens, pre-hashes, decodes, post-hashes, and terminally
/// reconciles the exact configured source again.
#[derive(Debug, Clone)]
#[must_use = "a qualified plan must be revalidated while staging or deliberately discarded"]
pub struct XnasQualifiedReplayPlanV1 {
    qualification: Arc<XnasReplayReceiptV1>,
    expected_metadata: XnasDailyMetadataExpectationV1,
}

impl XnasQualifiedReplayPlanV1 {
    /// Qualify one admitted historical XNAS object with the crate-owned
    /// publisher policy and an independently declared metadata denominator.
    ///
    /// This constructor is crate-private so external callers must enter through
    /// [`super::XnasReplayProbeRequestV1::qualify_xnas`], which independently
    /// enforces catalog-release admission before any source is opened.
    pub(crate) fn qualify_xnas(
        expectation: CanonicalSourceExpectationV1,
        expected_metadata: XnasDailyMetadataExpectationV1,
        config: XnasReplayConfigV1,
    ) -> Result<Self, XnasReplayErrorV1> {
        let stream = StrictDbnLoaderV1::open_xnas_expected(expectation, expected_metadata.clone())?;
        let replay = StrictXnasReplayV1::from_strict_stream(stream, config)?;
        Ok(Self {
            qualification: Arc::new(replay.run_to_eof()?),
            expected_metadata,
        })
    }

    /// Qualify an exact catalog-selected source while treating the DBN
    /// instrument ID as vendor-observed provenance, not caller-authored
    /// scientific intent.  The source/date/symbol denominator is validated by
    /// the request boundary before this call.  Pass one must reach EOF; its
    /// hash-bound metadata then becomes the mandatory expectation for pass two.
    pub(crate) fn qualify_catalog_bound(
        replay: StrictXnasReplayV1,
        expected_session_date: &str,
        expected_symbol: &str,
    ) -> Result<Self, XnasReplayErrorV1> {
        let qualification = replay.run_to_eof()?;
        let binding = qualification
            .source()
            .xnas_historical_source()
            .ok_or(XnasReplayErrorV1::MissingXnasMetadataBinding)?;
        if binding.session_date() != expected_session_date
            || binding.instruments().len() != 1
            || binding.instruments()[0].symbol != expected_symbol
        {
            return Err(XnasReplayErrorV1::CatalogIntentMismatch);
        }
        let expected_metadata = XnasDailyMetadataExpectationV1::from_verified_binding(binding)?;
        Ok(Self {
            qualification: Arc::new(qualification),
            expected_metadata,
        })
    }

    pub fn qualification_receipt(&self) -> &XnasReplayReceiptV1 {
        self.qualification.as_ref()
    }

    pub const fn metadata_expectation(&self) -> &XnasDailyMetadataExpectationV1 {
        &self.expected_metadata
    }

    /// Open a second, exact replay of the same source and configuration.
    ///
    /// No observation returned by this pass is publication-qualified. A caller
    /// must stage derived state privately and require
    /// [`XnasReplayRevalidationPassV1::finish`] before promotion.
    pub fn open_revalidation_pass(
        &self,
    ) -> Result<XnasReplayRevalidationPassV1, XnasReplayErrorV1> {
        open_revalidation_pass(&self.qualification, Some(&self.expected_metadata))
    }
}

/// A diagnostic-only replay plan that has no independently declared metadata
/// denominator.
///
/// This is a distinct type so it cannot be passed to production feature
/// carrier APIs that require [`XnasQualifiedReplayPlanV1`].
#[derive(Debug, Clone)]
#[must_use = "a development replay plan must be revalidated or deliberately discarded"]
pub struct XnasUnboundDevelopmentReplayPlanV1 {
    qualification: Arc<XnasReplayReceiptV1>,
}

impl XnasUnboundDevelopmentReplayPlanV1 {
    pub fn qualification_receipt(&self) -> &XnasReplayReceiptV1 {
        self.qualification.as_ref()
    }

    pub fn open_revalidation_pass(
        &self,
    ) -> Result<XnasReplayRevalidationPassV1, XnasReplayErrorV1> {
        open_revalidation_pass(&self.qualification, None)
    }
}

fn open_revalidation_pass(
    qualification: &Arc<XnasReplayReceiptV1>,
    expected_metadata: Option<&XnasDailyMetadataExpectationV1>,
) -> Result<XnasReplayRevalidationPassV1, XnasReplayErrorV1> {
    let descriptor = qualification.source().source();
    let expectation = CanonicalSourceExpectationV1::new(
        descriptor.logical.clone(),
        descriptor.opened.custody_projection_path.clone().into(),
        descriptor.opened.storage_root_path.clone().into(),
    )
    .map_err(StrictBoundaryErrorV1::SourceIdentity)?;
    let stream = match expected_metadata {
        Some(expected) => StrictDbnLoaderV1::open_xnas_expected(expectation, expected.clone())?,
        None => StrictDbnLoaderV1::open(expectation, PublisherPolicyIdV1::XnasItchHistorical)?,
    };
    let replay = StrictXnasReplayV1::from_strict_stream(stream, qualification.config())?;
    Ok(XnasReplayRevalidationPassV1 {
        qualification: qualification.clone(),
        replay: Some(replay),
        reached_eof: false,
        failed: false,
    })
}

/// One committed envelope observed during the still-unverified second pass.
///
/// This deliberately does not implement `Serialize` or expose its inner
/// success trace. Downstream code may inspect it to build a private staged
/// generation, but an equivalence receipt is minted only after terminal replay.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "pending observations are non-publishable until the pass returns an equivalence receipt"]
pub struct XnasPendingEnvelopeObservationV1(XnasReplayTraceV1);

impl XnasPendingEnvelopeObservationV1 {
    pub const fn source_object_sha256(&self) -> Sha256DigestV1 {
        self.0.source_object_sha256()
    }
    pub const fn validity_epoch_index(&self) -> u64 {
        self.0.validity_epoch_index()
    }
    pub const fn identity(&self) -> XnasIdentityV1 {
        self.0.identity()
    }
    pub fn symbol(&self) -> &str {
        self.0.symbol()
    }
    pub const fn envelope_sha256(&self) -> Sha256DigestV1 {
        self.0.envelope_sha256()
    }
    pub const fn committed_observation_sha256(&self) -> Sha256DigestV1 {
        self.0.committed_observation_sha256()
    }
    pub const fn committed_observation_chain_sha256(&self) -> Sha256DigestV1 {
        self.0.committed_observation_chain_sha256()
    }
    pub fn ordered_distinct_sequences(&self) -> &[u32] {
        self.0.ordered_distinct_sequences()
    }
    /// The only once-consumed analytical/member population for this envelope.
    pub fn events(&self) -> &[EventDispositionV1] {
        self.0.events()
    }
    pub fn first_source_ordinal(&self) -> u64 {
        self.0.first_source_ordinal()
    }
    pub fn last_source_ordinal(&self) -> u64 {
        self.0.last_source_ordinal()
    }
    pub const fn terminal_sequence(&self) -> u32 {
        self.0.terminal_sequence()
    }
    pub const fn terminal_source_ordinal(&self) -> u64 {
        self.0.terminal_source_ordinal()
    }
    /// Closure evidence only; never feed this as a current-envelope member.
    pub const fn witness(&self) -> &EventDispositionV1 {
        self.0.witness()
    }
    pub const fn witness_source_ordinal(&self) -> u64 {
        self.0.witness_source_ordinal()
    }
    pub const fn endpoint_ns(&self) -> u64 {
        self.0.endpoint_ns()
    }
    pub const fn witness_ts_recv(&self) -> u64 {
        self.0.witness_ts_recv()
    }
    pub const fn effective_available_ns(&self) -> u64 {
        self.0.effective_available_ns()
    }
    pub const fn closure_confirmation_delay_ns(&self) -> u64 {
        self.0.closure_confirmation_delay_ns()
    }
    pub const fn execution_sequence_blocks(&self) -> u64 {
        self.0.execution_sequence_blocks()
    }
    pub const fn execution_carriers(&self) -> u64 {
        self.0.execution_carriers()
    }
    pub const fn is_recovery(&self) -> bool {
        self.0.is_recovery()
    }
    pub const fn book(&self) -> &XnasBookCommitV1 {
        self.0.book()
    }
}

/// The second pass that supplies private, pending envelope observations.
#[must_use = "the pass must reach EOF and finish before staged output can be promoted"]
pub struct XnasReplayRevalidationPassV1 {
    qualification: Arc<XnasReplayReceiptV1>,
    replay: Option<StrictXnasReplayV1>,
    reached_eof: bool,
    failed: bool,
}

impl XnasReplayRevalidationPassV1 {
    /// Return the next atomically committed envelope. `Ok(None)` occurs only
    /// after physical EOF and open-tail quarantine; failures are fused and every
    /// later call returns `CannotContinueFailedRevalidation`.
    pub fn next_observation(
        &mut self,
    ) -> Result<Option<XnasPendingEnvelopeObservationV1>, XnasReplayErrorV1> {
        if self.failed {
            return Err(XnasReplayErrorV1::CannotContinueFailedRevalidation);
        }
        if self.reached_eof {
            return Ok(None);
        }
        let replay = self
            .replay
            .as_mut()
            .expect("an active revalidation pass owns its replay");
        match replay.next_quarantined_update() {
            Some(Ok(update)) => Ok(Some(XnasPendingEnvelopeObservationV1(
                XnasReplayTraceV1::from_staged(update),
            ))),
            Some(Err(cause)) => {
                let failed = self
                    .replay
                    .take()
                    .expect("the replay existed when it returned an error");
                self.failed = true;
                Err(failed.into_prefix_failure(cause))
            }
            None => {
                self.reached_eof = true;
                Ok(None)
            }
        }
    }

    /// Terminally seal pass two and prove exact equality with pass one.
    pub fn finish(mut self) -> Result<XnasReplayEquivalenceReceiptV1, XnasReplayErrorV1> {
        if self.failed {
            return Err(XnasReplayErrorV1::CannotFinishFailedReplay);
        }
        if !self.reached_eof {
            return Err(XnasReplayErrorV1::CannotFinishRevalidationBeforeEof);
        }
        let replay = self
            .replay
            .take()
            .ok_or(XnasReplayErrorV1::CannotFinishFailedReplay)?;
        let revalidation = replay.finish()?;
        if revalidation != *self.qualification {
            return Err(XnasReplayErrorV1::RevalidationReceiptMismatch {
                qualification: self.qualification,
                revalidation: Arc::new(revalidation),
            });
        }
        Ok(XnasReplayEquivalenceReceiptV1 {
            schema: "xnas_replay_equivalence_receipt_v1",
            exact_receipt: self.qualification,
            verified_complete_replays: 2,
            authority:
                "development_only_exact_two_pass_replay_equivalence_not_publication_authority",
        })
    }
}

/// Software attestation that two complete replays of one source/configuration
/// produced field-for-field equal terminal receipts.
///
/// This receipt is required evidence for downstream promotion, but it is not a
/// vendor oracle, research-admission decision, or publication transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasReplayEquivalenceReceiptV1 {
    schema: &'static str,
    exact_receipt: Arc<XnasReplayReceiptV1>,
    verified_complete_replays: u8,
    authority: &'static str,
}

impl XnasReplayEquivalenceReceiptV1 {
    pub const fn schema(&self) -> &'static str {
        self.schema
    }
    /// The exact receipt value produced independently by both complete passes.
    /// It is stored once because retaining two equal O(records) ledgers adds no
    /// evidence and can triple peak custody on malformed sources.
    pub fn exact_receipt(&self) -> &XnasReplayReceiptV1 {
        self.exact_receipt.as_ref()
    }

    pub const fn verified_complete_replays(&self) -> u8 {
        self.verified_complete_replays
    }

    pub const fn authority(&self) -> &'static str {
        self.authority
    }
}

impl StrictXnasReplayV1 {
    /// Complete pass one and retain its exact terminal receipt as the plan for
    /// downstream private staging.
    pub fn qualify_unbound_development(
        self,
    ) -> Result<XnasUnboundDevelopmentReplayPlanV1, XnasReplayErrorV1> {
        Ok(XnasUnboundDevelopmentReplayPlanV1 {
            qualification: Arc::new(self.run_to_eof()?),
        })
    }
}
