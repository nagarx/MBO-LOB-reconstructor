//! Parquet export for raw LOB snapshots and MBO events.
//!
//! Exports `LobState` snapshots and `MboMessage` events to Apache Parquet files,
//! providing an unbiased data source for downstream statistical analysis before
//! any feature extraction transforms (sampling, normalization, labeling).
//!
//! # Data Contract
//!
//! - **Schema version**: `1.0` -- any breaking change requires a version bump
//! - **Price unit**: nanodollars (`i64`, divide by 1e9 for dollars)
//! - **Timestamp unit**: nanoseconds since epoch
//! - **Size unit**: shares
//! - Parquet file-level metadata encodes all units and provenance
//!
//! # Usage
//!
//! ```ignore
//! use mbo_lob_reconstructor::export::{ExportConfig, LobSnapshotWriter, MboEventWriter};
//!
//! let config = ExportConfig::default();
//! let mut lob_writer = LobSnapshotWriter::new("lob.parquet", &config, metadata)?;
//! lob_writer.write_snapshot(&state)?;
//! let stats = lob_writer.finish()?;
//! ```

pub mod schema;

mod batch;
pub mod lob_writer;
pub mod mbo_writer;

pub use lob_writer::LobSnapshotWriter;
pub use mbo_writer::MboEventWriter;

use parquet::basic::Compression;

use crate::types::MAX_LOB_LEVELS;

/// Schema version embedded in every exported Parquet file.
/// Bump on any breaking schema change.
pub const SCHEMA_VERSION: &str = "1.0";

/// Default number of rows buffered before flushing to a Parquet row group.
pub const DEFAULT_BATCH_SIZE: usize = 65_536;

/// Configuration for Parquet export.
///
/// All fields have sensible defaults via [`ExportConfig::default()`].
/// Per RULE.md Section 5: every threshold and behavior is configurable.
#[derive(Debug, Clone)]
pub struct ExportConfig {
    /// Number of LOB levels to export (clamped to [`MAX_LOB_LEVELS`]).
    pub levels: usize,

    /// Include derived analytics columns (mid_price, spread, etc.).
    pub include_derived: bool,

    /// Also export MBO events to a separate Parquet file.
    pub include_mbo_events: bool,

    /// Rows per Parquet row group (controls memory vs I/O tradeoff).
    pub batch_size: usize,

    /// Parquet compression codec.
    pub compression: Compression,

    /// Optional downsampling strategy for LOB snapshots.
    pub downsample: Option<DownsampleConfig>,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            levels: 10,
            include_derived: true,
            include_mbo_events: true,
            batch_size: DEFAULT_BATCH_SIZE,
            compression: Compression::SNAPPY,
            downsample: None,
        }
    }
}

impl ExportConfig {
    /// Clamp `levels` to the compile-time maximum.
    pub fn effective_levels(&self) -> usize {
        self.levels.min(MAX_LOB_LEVELS)
    }

    /// Validate configuration values.
    pub fn validate(&self) -> crate::error::Result<()> {
        if self.levels == 0 {
            return Err(crate::error::TlobError::InvalidConfig(
                "ExportConfig.levels must be at least 1".into(),
            ));
        }
        if self.levels > crate::types::MAX_LOB_LEVELS {
            return Err(crate::error::TlobError::InvalidConfig(format!(
                "ExportConfig.levels must be <= {} (got {})",
                crate::types::MAX_LOB_LEVELS,
                self.levels
            )));
        }
        if self.batch_size == 0 {
            return Err(crate::error::TlobError::InvalidConfig(
                "ExportConfig.batch_size must be > 0".into(),
            ));
        }
        Ok(())
    }
}

/// Downsampling configuration for LOB snapshot export.
#[derive(Debug, Clone)]
pub struct DownsampleConfig {
    pub strategy: DownsampleStrategy,
}

/// Strategy for reducing the number of exported LOB snapshots.
#[derive(Debug, Clone)]
pub enum DownsampleStrategy {
    /// Export every snapshot (no downsampling).
    None,
    /// Export every N-th snapshot.
    EveryN(usize),
    /// Export at most one snapshot per N nanoseconds.
    MinIntervalNs(u64),
}

/// Statistics returned after completing a Parquet export.
#[derive(Debug, Clone)]
pub struct ParquetExportStats {
    /// Total rows written to the file.
    pub rows_written: u64,
    /// Total rows seen (before downsampling).
    pub rows_seen: u64,
    /// Number of row groups flushed.
    pub row_groups: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_export_config_validate_valid() {
        ExportConfig::default().validate().unwrap();
    }

    #[test]
    fn test_export_config_validate_zero_levels() {
        let mut config = ExportConfig::default();
        config.levels = 0;
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("at least 1"));
    }

    #[test]
    fn test_export_config_validate_excessive_levels() {
        let mut config = ExportConfig::default();
        config.levels = crate::types::MAX_LOB_LEVELS + 1;
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be <="));
    }

    #[test]
    fn test_export_config_validate_zero_batch_size() {
        let mut config = ExportConfig::default();
        config.batch_size = 0;
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("batch_size"));
    }

    #[test]
    fn test_export_config_validate_max_levels_boundary() {
        let mut config = ExportConfig::default();
        config.levels = crate::types::MAX_LOB_LEVELS;
        config.validate().unwrap(); // MAX_LOB_LEVELS is valid
    }
}
