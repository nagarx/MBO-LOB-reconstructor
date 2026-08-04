//! Exact two-pass replay boundary for downstream feature staging.
//!
//! Pass one proves that the configured source reaches the strict terminal gate.
//! Pass two reopens the same caller-expectation-bound object and exposes committed envelope
//! observations for private downstream staging. The second terminal receipt must
//! equal the qualification receipt field-for-field at the typed value level before
//! a caller can receive [`XnasReplayEquivalenceReceiptV1`]. This proves replay
//! equivalence, not vendor-semantic correctness or publication eligibility.

use super::{
    StrictXnasReplayV1, XnasBookCommitV1, XnasIdentityV1, XnasReplayErrorV1, XnasReplayReceiptV1,
    XnasReplayTraceV1,
};
use crate::loader::{CanonicalSourceExpectationV1, StrictDbnLoaderV1};
use hft_mbo_event_contract::{EventDispositionV1, PublisherPolicyIdV1, Sha256DigestV1};
use serde::Serialize;
use std::sync::Arc;

/// A source/configuration pair that completed one strict terminal replay.
///
/// The plan is reusable so consumers can retry a failed private staging pass.
/// Every retry reopens, pre-hashes, decodes, post-hashes, and terminally
/// reconciles the exact configured source again.
#[derive(Debug, Clone)]
#[must_use = "a qualified plan must be revalidated while staging or deliberately discarded"]
pub struct XnasQualifiedReplayPlanV1 {
    qualification: Arc<XnasReplayReceiptV1>,
}

impl XnasQualifiedReplayPlanV1 {
    pub fn qualification_receipt(&self) -> &XnasReplayReceiptV1 {
        self.qualification.as_ref()
    }

    /// Open a second, exact replay of the same source and configuration.
    ///
    /// No observation returned by this pass is publication-qualified. A caller
    /// must stage derived state privately and require
    /// [`XnasReplayRevalidationPassV1::finish`] before promotion.
    pub fn open_revalidation_pass(
        &self,
    ) -> Result<XnasReplayRevalidationPassV1, XnasReplayErrorV1> {
        let source_receipt = self.qualification.source();
        let descriptor = source_receipt.source();
        let expectation = CanonicalSourceExpectationV1::new(
            descriptor.logical.clone(),
            source_receipt.expected_records(),
        );
        let stream = StrictDbnLoaderV1::open(
            expectation,
            &descriptor.opened.configured_path,
            PublisherPolicyIdV1::XnasItchHistorical,
        )?;
        let replay = StrictXnasReplayV1::from_strict_stream(stream, self.qualification.config())?;
        Ok(XnasReplayRevalidationPassV1 {
            qualification: self.qualification.clone(),
            replay: Some(replay),
            reached_eof: false,
            failed: false,
        })
    }
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
    pub fn qualify(self) -> Result<XnasQualifiedReplayPlanV1, XnasReplayErrorV1> {
        Ok(XnasQualifiedReplayPlanV1 {
            qualification: Arc::new(self.run_to_eof()?),
        })
    }
}
