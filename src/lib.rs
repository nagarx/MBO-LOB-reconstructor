//! # MBO-LOB-Reconstructor
//!
//! Source-bound MBO decoding and exact XNAS order-book reconstruction.
//!
//! The strict API binds opened bytes to an expected source identity, validates
//! XNAS event and envelope semantics, and emits transactionally committed book
//! observations plus terminal custody receipts. The pathname-only compatibility
//! loader was removed because physical EOF alone cannot prove a complete source
//! object. Qualified file replay starts at [`StrictDbnLoaderV1`].
//!
//! ## Module Overview
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`types`] | Core types: `MboMessage`, `LobState`, `Action`, `Side`, `MAX_LOB_LEVELS` |
//! | [`lob`] | LOB reconstruction: `LobReconstructor`, `MultiSymbolLob`, `PriceLevel` |
//! | [`statistics`] | Descriptive LOB statistics: `DayStats`, `RunningStats` |
//! | [`analytics`] | Advanced analytics: `DepthStats`, `MarketImpact`, `LiquidityMetrics` |
//! | [`loader`] | Source-bound DBN loading (requires `databento` feature) |
//! | [`dbn_bridge`] | Databento format conversion (requires `databento` feature) |
//! | [`warnings`] | Warning tracking: `WarningTracker`, `Warning`, `WarningCategory` |
//! | [`constants`] | Domain constants: price/time conversion, financial units, numerical precision |
//!
//! ## Feature Flags
//!
//! | Feature | Default | Description |
//! |---------|---------|-------------|
//! | `databento` | ✅ | Enable Databento DBN file support |

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod analytics;
pub mod constants;
pub mod error;
pub mod lob;
pub mod source;
pub mod statistics;
pub mod types;
pub mod warnings;

#[cfg(feature = "databento")]
#[cfg_attr(docsrs, doc(cfg(feature = "databento")))]
mod xnas;

#[cfg(feature = "databento")]
#[cfg_attr(docsrs, doc(cfg(feature = "databento")))]
pub mod dbn_bridge;

#[cfg(feature = "databento")]
#[cfg_attr(docsrs, doc(cfg(feature = "databento")))]
mod canonical_dbn;

#[cfg(feature = "databento")]
#[cfg_attr(docsrs, doc(cfg(feature = "databento")))]
pub mod loader;

// Re-exports - Constants
pub use constants::{
    BASIS_POINTS_PER_UNIT, DIVISION_GUARD_EPS, NANODOLLARS_PER_DOLLAR, NANODOLLARS_PER_DOLLAR_F64,
    NS_PER_DAY, NS_PER_HOUR, NS_PER_MILLISECOND, NS_PER_MINUTE, NS_PER_SECOND, NS_PER_SECOND_F64,
};

// Re-exports - Core types
pub use error::{Result, TlobError};
pub use types::{Action, BookConsistency, LobState, MboMessage, Order, Side, MAX_LOB_LEVELS};

// Re-exports - LOB reconstruction
pub use lob::{CrossedQuotePolicy, LobConfig, LobReconstructor, LobStats, MultiSymbolLob};

// Re-export the exact canonical event vocabulary through the reconstructor
// facade so downstream adapters cannot accidentally resolve an independently
// moving copy of the semantic contract.
pub use hft_mbo_event_contract::{
    AggressorSideV1, EventDispositionV1, ExecutionCarrierV1, LogicalSourceV1, Sha256DigestV1,
    CANONICAL_MBO_EVENT_CONTRACT_ID, CANONICAL_MBO_EVENT_CONTRACT_SHA256,
    CANONICAL_MBO_EVENT_SCHEMA_VERSION, XNAS_ITCH_HISTORICAL_PUBLISHER_IDS_V1,
};

// Re-exports - LobStats wire-format (Phase M M.A.5: envelope wrapper + schema version).
// Exposed at crate root for external consumers reading `_reconstruction_stats.json`.
pub use lob::reconstructor::{LobStatsExportEnvelope, LOB_STATS_SCHEMA_VERSION};

// Re-exports - Day Boundary Detection
pub use lob::{DayBoundary, DayBoundaryConfig, DayBoundaryDetector, DayBoundaryStats};

// Re-exports - descriptive statistics. Normalization belongs to the extractor.
pub use statistics::{DayStats, RunningStats};

// Re-exports - Analytics
pub use analytics::{DepthStats, LiquidityMetrics, MarketImpact};

// Re-exports - Warnings
pub use warnings::{
    Warning, WarningCategory, WarningSummary, WarningTracker, WarningTrackerConfig,
};

// Re-exports - Source abstraction
pub use source::{MarketDataSource, SourceMetadata, VecSource};

// Re-exports - Databento support (feature-gated)
#[cfg(feature = "databento")]
pub use dbn_bridge::DbnBridge;

#[cfg(feature = "databento")]
pub use canonical_dbn::CanonicalProjectionErrorV1;

#[cfg(feature = "databento")]
pub use xnas::{
    BookTransactionErrorV1, StrictXnasReplayV1, XnasBookCommitV1, XnasBookLevelV1,
    XnasBookSnapshotV1, XnasCommittedObservationAccumulatorV1, XnasCommittedObservationClosureV1,
    XnasEnvelopeErrorV1, XnasEofTailQuarantineV1, XnasEofTailReasonV1, XnasIdentityReplayReceiptV1,
    XnasIdentityV1, XnasInvalidStateQuarantinedRecordV1, XnasObservationAccountingErrorV1,
    XnasPendingEnvelopeObservationV1, XnasQualifiedReplayPlanV1, XnasQuarantineReasonV1,
    XnasRecoveryQualificationV1, XnasRejectedRecordPhaseV1, XnasRejectedRecordQuarantineV1,
    XnasReplayBuildIdentityV1, XnasReplayConfigV1, XnasReplayCountsV1,
    XnasReplayEquivalenceReceiptV1, XnasReplayErrorV1, XnasReplayPrefixFailureV1,
    XnasReplayProbeRequestErrorV1, XnasReplayProbeRequestV1, XnasReplayReceiptV1,
    XnasReplayRevalidationPassV1, XnasReplayRunV1, XnasReplayTraceV1,
    XnasResetBoundaryQuarantineV1, XnasSelectedOrdinalDispositionV1, XnasSelectedOrdinalRoleV1,
    XnasSemanticQuarantineIncidentV1, XnasTerminalDisqualificationReasonV1,
    XnasTerminalDisqualificationV1, XnasTerminalIdentityStatusV1,
    XnasUnboundDevelopmentReplayPlanV1, XnasValidityEpochQualificationV1, XnasValidityEpochV1,
    XnasValidityInvalidationReasonV1, XnasValidityInvalidationV1,
};

#[cfg(feature = "databento")]
pub use loader::{
    CanonicalReadReceiptV1, CanonicalSourceExpectationV1, StrictBoundaryErrorV1, StrictDbnLoaderV1,
    StrictMboEventIteratorV1, VerifiedRejectedStreamEventV1, VerifiedRejectionStageV1,
    VerifiedStreamEventV1, VerifiedStreamRecordV1, XnasDailyMetadataBindingV1,
    XnasDailyMetadataExpectationV1, XnasExpectedInstrumentIdentityV1,
    XnasPolicyBoundInstrumentIdentityV1, IO_BUFFER_SIZE,
};
