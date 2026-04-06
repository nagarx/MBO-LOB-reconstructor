//! Parquet writer for LOB snapshot data.
//!
//! Buffers [`LobState`] snapshots in a column-oriented batch, flushing to
//! Parquet row groups when the batch reaches the configured capacity.
//! Supports optional downsampling and derived-field computation.

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

use crate::error::{Result, TlobError};
use crate::types::LobState;

use super::batch::LobBatch;
use super::schema::lob_snapshot_schema;
use super::{DownsampleStrategy, ExportConfig, ParquetExportStats};

/// Writes [`LobState`] snapshots to a Parquet file.
///
/// # Usage
///
/// ```ignore
/// let config = ExportConfig::default();
/// let meta = HashMap::new();
/// let mut writer = LobSnapshotWriter::new("snapshots.parquet", &config, meta)?;
///
/// for state in lob_states {
///     writer.write_snapshot(&state)?;
/// }
///
/// let stats = writer.finish()?;
/// println!("Wrote {} rows", stats.rows_written);
/// ```
pub struct LobSnapshotWriter {
    writer: ArrowWriter<File>,
    schema: arrow::datatypes::Schema,
    batch: LobBatch,
    config: ExportConfig,

    // Downsample state
    rows_seen: u64,
    rows_written: u64,
    row_groups: u64,
    last_written_ts: Option<i64>,
}

impl LobSnapshotWriter {
    /// Create a new writer targeting `path`.
    ///
    /// `extra_metadata` is merged into the Parquet file-level metadata alongside
    /// the schema-level metadata (date, symbol, lob_levels, etc.).
    pub fn new(
        path: impl AsRef<Path>,
        config: &ExportConfig,
        extra_metadata: HashMap<String, String>,
    ) -> Result<Self> {
        config.validate()?;
        let levels = config.effective_levels();
        let mut schema = lob_snapshot_schema(levels, config.include_derived);

        let mut merged_meta = schema.metadata().clone();
        merged_meta.insert("lob_levels".into(), levels.to_string());
        for (k, v) in extra_metadata {
            merged_meta.insert(k, v);
        }
        schema = schema.with_metadata(merged_meta);

        let file = File::create(path.as_ref()).map_err(|e| {
            TlobError::Generic(format!(
                "Failed to create Parquet file '{}': {e}",
                path.as_ref().display()
            ))
        })?;

        let props = WriterProperties::builder()
            .set_compression(config.compression)
            .set_max_row_group_size(config.batch_size)
            .build();

        let writer = ArrowWriter::try_new(file, Arc::new(schema.clone()), Some(props))
            .map_err(|e| TlobError::Generic(format!("Failed to create ArrowWriter: {e}")))?;

        let batch = LobBatch::new(config.batch_size, levels, config.include_derived);

        Ok(Self {
            writer,
            schema,
            batch,
            config: config.clone(),
            rows_seen: 0,
            rows_written: 0,
            row_groups: 0,
            last_written_ts: None,
        })
    }

    /// Write a single LOB snapshot.
    ///
    /// The snapshot may be skipped if it does not pass the downsampling filter.
    pub fn write_snapshot(&mut self, state: &LobState) -> Result<()> {
        self.rows_seen += 1;

        if !self.should_write(state) {
            return Ok(());
        }

        self.batch.push(state);
        self.rows_written += 1;
        self.last_written_ts = state.timestamp;

        if self.batch.is_full() {
            self.flush_batch()?;
        }
        Ok(())
    }

    /// Flush remaining buffered rows and close the Parquet file.
    ///
    /// Returns export statistics. This **must** be called to produce a valid
    /// Parquet file (the footer is written here).
    pub fn finish(mut self) -> Result<ParquetExportStats> {
        if self.batch.len() > 0 {
            self.flush_batch()?;
        }

        self.writer
            .close()
            .map_err(|e| TlobError::Generic(format!("Failed to close Parquet writer: {e}")))?;

        Ok(ParquetExportStats {
            rows_written: self.rows_written,
            rows_seen: self.rows_seen,
            row_groups: self.row_groups,
        })
    }

    /// Number of rows written so far (after downsampling).
    pub fn rows_written(&self) -> u64 {
        self.rows_written
    }

    /// Number of rows seen so far (before downsampling).
    pub fn rows_seen(&self) -> u64 {
        self.rows_seen
    }

    fn flush_batch(&mut self) -> Result<()> {
        let record_batch = self.batch.flush(&self.schema)?;
        self.writer
            .write(&record_batch)
            .map_err(|e| TlobError::Generic(format!("Failed to write row group: {e}")))?;
        self.row_groups += 1;
        Ok(())
    }

    fn should_write(&self, state: &LobState) -> bool {
        let strategy = match &self.config.downsample {
            Some(ds) => &ds.strategy,
            None => return true,
        };

        match strategy {
            DownsampleStrategy::None => true,
            DownsampleStrategy::EveryN(n) => {
                let n = *n;
                if n == 0 {
                    return true;
                }
                (self.rows_seen - 1) % (n as u64) == 0
            }
            DownsampleStrategy::MinIntervalNs(min_ns) => {
                let ts = match state.timestamp {
                    Some(t) => t,
                    None => return true,
                };
                match self.last_written_ts {
                    None => true,
                    Some(last) => (ts - last) as u64 >= *min_ns,
                }
            }
        }
    }
}

/// Builder for [`LobSnapshotWriter`] with a fluent API.
///
/// ```ignore
/// let writer = LobSnapshotWriter::builder("output.parquet")
///     .levels(10)
///     .include_derived(true)
///     .compression(Compression::SNAPPY)
///     .date("2025-02-03")
///     .symbol("NVDA")
///     .build()?;
/// ```
pub struct LobSnapshotWriterBuilder {
    path: std::path::PathBuf,
    config: ExportConfig,
    metadata: HashMap<String, String>,
}

impl LobSnapshotWriterBuilder {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            config: ExportConfig::default(),
            metadata: HashMap::new(),
        }
    }

    pub fn levels(mut self, levels: usize) -> Self {
        self.config.levels = levels;
        self
    }

    pub fn include_derived(mut self, include: bool) -> Self {
        self.config.include_derived = include;
        self
    }

    pub fn batch_size(mut self, size: usize) -> Self {
        self.config.batch_size = size;
        self
    }

    pub fn compression(mut self, compression: Compression) -> Self {
        self.config.compression = compression;
        self
    }

    pub fn date(mut self, date: &str) -> Self {
        self.metadata.insert("date".into(), date.into());
        self
    }

    pub fn symbol(mut self, symbol: &str) -> Self {
        self.metadata.insert("symbol".into(), symbol.into());
        self
    }

    pub fn metadata(mut self, key: &str, value: &str) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    pub fn build(self) -> Result<LobSnapshotWriter> {
        LobSnapshotWriter::new(self.path, &self.config, self.metadata)
    }
}

impl LobSnapshotWriter {
    pub fn builder(path: impl AsRef<Path>) -> LobSnapshotWriterBuilder {
        LobSnapshotWriterBuilder::new(path)
    }
}
