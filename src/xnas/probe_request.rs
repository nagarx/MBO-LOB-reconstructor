//! Shared, deny-unknown-fields wire request for nonpublishing XNAS probes.

use super::{
    StrictXnasReplayV1, XnasQualifiedReplayPlanV1, XnasReplayConfigV1, XnasReplayErrorV1,
    XnasUnboundDevelopmentReplayPlanV1,
};
use crate::loader::{
    CanonicalSourceExpectationV1, StrictBoundaryErrorV1, StrictDbnLoaderV1,
    XnasDailyMetadataExpectationV1,
};
use hft_mbo_event_contract::{
    validate_accepted_catalog_release_v1, LogicalSourceV1, PublisherPolicyIdV1, Sha256DigestV1,
    SourceIdentityErrorV1,
};
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XnasReplayProbeRequestV1 {
    custody_projection_path: PathBuf,
    storage_root_path: PathBuf,
    catalog_release_id: String,
    catalog_storage_root_id: String,
    custody_projection_schema: String,
    custody_projection_file_sha256: Sha256DigestV1,
    custody_projection_content_sha256: Sha256DigestV1,
    canonical_profile_sha256: Sha256DigestV1,
    embedded_per_file_tsv_sha256: Sha256DigestV1,
    evidence_manifest_sha256: Sha256DigestV1,
    terminal_validation_receipt_sha256: Sha256DigestV1,
    terminal_validation_status: String,
    relative_path: String,
    compressed_sha256: Sha256DigestV1,
    compressed_bytes: u64,
    expected_records: u64,
    metadata_start_ns: u64,
    metadata_end_ns: u64,
    requested_symbols_preview: String,
    requested_symbols_sha256: Sha256DigestV1,
    symbols_n: u64,
    active_instruments_n: u64,
    provenance_tier: String,
    provider_manifest_relative_path: String,
    provider_manifest_sha256: Sha256DigestV1,
    provider_job_id: String,
    provider_declared_data_file_count: u64,
    dbn_version: u8,
    dbn_ts_out: bool,
    dataset: String,
    schema: String,
    snapshot_depth: usize,
    max_envelope_members: usize,
    max_sequence_blocks: usize,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum XnasReplayProbeRequestErrorV1 {
    #[error("probe request is outside strict historical XNAS MBO v1: {0}")]
    InvalidStrictProfile(&'static str),
    #[error(transparent)]
    SourceIdentity(#[from] SourceIdentityErrorV1),
    #[error(transparent)]
    Boundary(#[from] StrictBoundaryErrorV1),
    #[error(transparent)]
    Replay(#[from] XnasReplayErrorV1),
}

impl XnasReplayProbeRequestV1 {
    pub fn validate(&self) -> Result<(), XnasReplayProbeRequestErrorV1> {
        if !self.custody_projection_path.is_absolute() {
            return Err(XnasReplayProbeRequestErrorV1::InvalidStrictProfile(
                "custody_projection_path must be absolute",
            ));
        }
        if !self.storage_root_path.is_absolute() {
            return Err(XnasReplayProbeRequestErrorV1::InvalidStrictProfile(
                "storage_root_path must be absolute",
            ));
        }
        if self.dataset != "XNAS.ITCH"
            || self.schema != "mbo"
            || self.dbn_version != 1
            || self.dbn_ts_out
        {
            return Err(XnasReplayProbeRequestErrorV1::InvalidStrictProfile(
                "dataset/schema/dbn_version/ts_out",
            ));
        }
        if self.snapshot_depth == 0
            || self.max_envelope_members == 0
            || self.max_sequence_blocks == 0
        {
            return Err(XnasReplayProbeRequestErrorV1::InvalidStrictProfile(
                "replay resource bounds must be nonzero",
            ));
        }
        self.logical_source()?.validate_strict()?;
        Ok(())
    }

    /// Require a valid request to match one catalog release admitted by the
    /// root event-contract authority.
    pub fn validate_admitted(&self) -> Result<(), XnasReplayProbeRequestErrorV1> {
        self.validate()?;
        validate_accepted_catalog_release_v1(&self.logical_source()?)?;
        Ok(())
    }

    pub fn open_admitted_strict_replay(
        &self,
        expected_metadata: XnasDailyMetadataExpectationV1,
    ) -> Result<StrictXnasReplayV1, XnasReplayProbeRequestErrorV1> {
        self.validate_admitted()?;
        let expectation = self.expectation()?;
        let stream = StrictDbnLoaderV1::open_xnas_expected(expectation, expected_metadata)?;
        Ok(StrictXnasReplayV1::from_strict_stream(
            stream,
            self.replay_config()?,
        )?)
    }

    /// Diagnostic/nonpublishing compatibility path without an independent
    /// partition expectation. Production carrier code must use
    /// [`Self::open_admitted_strict_replay`] or [`Self::qualify_xnas`].
    pub fn open_admitted_strict_replay_unbound_development(
        &self,
    ) -> Result<StrictXnasReplayV1, XnasReplayProbeRequestErrorV1> {
        self.validate_admitted()?;
        let stream =
            StrictDbnLoaderV1::open(self.expectation()?, PublisherPolicyIdV1::XnasItchHistorical)?;
        Ok(StrictXnasReplayV1::from_strict_stream(
            stream,
            self.replay_config()?,
        )?)
    }

    pub fn qualify_xnas(
        &self,
        expected_metadata: XnasDailyMetadataExpectationV1,
    ) -> Result<XnasQualifiedReplayPlanV1, XnasReplayProbeRequestErrorV1> {
        self.validate_admitted()?;
        Ok(XnasQualifiedReplayPlanV1::qualify_xnas(
            self.expectation()?,
            expected_metadata,
            self.replay_config()?,
        )?)
    }

    /// Production qualification for a catalog-predeclared date and symbol.
    /// The vendor-local instrument ID is derived only from the exact
    /// hash-verified DBN metadata and is then frozen as the pass-two
    /// expectation.
    pub fn qualify_xnas_catalog_bound(
        &self,
        expected_session_date: &str,
        expected_symbol: &str,
    ) -> Result<XnasQualifiedReplayPlanV1, XnasReplayProbeRequestErrorV1> {
        self.validate_admitted()?;
        let logical = self.logical_source()?;
        let derived_date = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(
            logical.metadata_start_ns,
        ))
        .map_err(|_| {
            XnasReplayProbeRequestErrorV1::InvalidStrictProfile("metadata_start_ns")
        })?
        .date()
        .to_string();
        if expected_session_date != derived_date
            || expected_symbol != logical.requested_symbols_preview
            || logical.symbols_n != 1
            || logical.active_instruments_n != 1
        {
            return Err(XnasReplayProbeRequestErrorV1::InvalidStrictProfile(
                "catalog-bound session date and symbol intent",
            ));
        }
        let stream = StrictDbnLoaderV1::open(
            self.expectation()?,
            PublisherPolicyIdV1::XnasItchHistorical,
        )?;
        let replay = StrictXnasReplayV1::from_strict_stream(stream, self.replay_config()?)?;
        Ok(XnasQualifiedReplayPlanV1::qualify_catalog_bound(
            replay,
            expected_session_date,
            expected_symbol,
        )?)
    }

    /// Diagnostic/nonpublishing compatibility path. It deliberately lacks an
    /// independent partition expectation and is named so it cannot be mistaken
    /// for the production boundary.
    pub fn qualify_xnas_unbound_development(
        &self,
    ) -> Result<XnasUnboundDevelopmentReplayPlanV1, XnasReplayProbeRequestErrorV1> {
        self.validate_admitted()?;
        let stream =
            StrictDbnLoaderV1::open(self.expectation()?, PublisherPolicyIdV1::XnasItchHistorical)?;
        Ok(
            StrictXnasReplayV1::from_strict_stream(stream, self.replay_config()?)?
                .qualify_unbound_development()?,
        )
    }

    pub const fn expected_records(&self) -> u64 {
        self.expected_records
    }

    pub const fn snapshot_depth(&self) -> usize {
        self.snapshot_depth
    }

    pub const fn compressed_sha256(&self) -> Sha256DigestV1 {
        self.compressed_sha256
    }

    /// Project the exact catalog/source identity carried by this request.
    ///
    /// This does not imply admission. Selection owners may use the projection
    /// to predeclare their source denominator before opening any source, then
    /// call [`Self::validate_admitted`] (or a replay constructor) before use.
    pub fn logical_source(&self) -> Result<LogicalSourceV1, SourceIdentityErrorV1> {
        let logical = LogicalSourceV1 {
            catalog_release_id: self.catalog_release_id.clone(),
            catalog_storage_root_id: self.catalog_storage_root_id.clone(),
            custody_projection_schema: self.custody_projection_schema.clone(),
            custody_projection_file_sha256: self.custody_projection_file_sha256,
            custody_projection_content_sha256: self.custody_projection_content_sha256,
            canonical_profile_sha256: self.canonical_profile_sha256,
            embedded_per_file_tsv_sha256: self.embedded_per_file_tsv_sha256,
            evidence_manifest_sha256: self.evidence_manifest_sha256,
            terminal_validation_receipt_sha256: self.terminal_validation_receipt_sha256,
            terminal_validation_status: self.terminal_validation_status.clone(),
            relative_path: self.relative_path.clone(),
            compressed_sha256: self.compressed_sha256,
            compressed_bytes: self.compressed_bytes,
            expected_records: self.expected_records,
            metadata_start_ns: self.metadata_start_ns,
            metadata_end_ns: self.metadata_end_ns,
            requested_symbols_preview: self.requested_symbols_preview.clone(),
            requested_symbols_sha256: self.requested_symbols_sha256,
            symbols_n: self.symbols_n,
            active_instruments_n: self.active_instruments_n,
            provenance_tier: self.provenance_tier.clone(),
            provider_manifest_relative_path: self.provider_manifest_relative_path.clone(),
            provider_manifest_sha256: self.provider_manifest_sha256,
            provider_job_id: self.provider_job_id.clone(),
            provider_declared_data_file_count: self.provider_declared_data_file_count,
            dbn_version: self.dbn_version,
            dbn_ts_out: self.dbn_ts_out,
            dataset: self.dataset.clone(),
            schema: self.schema.clone(),
        };
        logical.validate_strict()?;
        Ok(logical)
    }

    fn expectation(&self) -> Result<CanonicalSourceExpectationV1, XnasReplayProbeRequestErrorV1> {
        Ok(CanonicalSourceExpectationV1::new(
            self.logical_source()?,
            self.custody_projection_path.clone(),
            self.storage_root_path.clone(),
        )?)
    }

    fn replay_config(&self) -> Result<XnasReplayConfigV1, XnasReplayProbeRequestErrorV1> {
        let nonzero = |value, field| {
            NonZeroUsize::new(value)
                .ok_or(XnasReplayProbeRequestErrorV1::InvalidStrictProfile(field))
        };
        Ok(XnasReplayConfigV1::new(
            nonzero(self.snapshot_depth, "snapshot_depth")?,
            nonzero(self.max_envelope_members, "max_envelope_members")?,
            nonzero(self.max_sequence_blocks, "max_sequence_blocks")?,
        ))
    }
}
