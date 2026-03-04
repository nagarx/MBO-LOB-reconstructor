//! Parquet writer for raw MBO (Market-By-Order) event data.
//!
//! Buffers [`MboMessage`] events in a column-oriented batch, flushing to
//! Parquet row groups when the batch reaches the configured capacity.
//! This captures every individual order event before LOB reconstruction.

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

use crate::error::{Result, TlobError};
use crate::types::MboMessage;

use super::batch::MboBatch;
use super::schema::mbo_event_schema;
use super::{ExportConfig, ParquetExportStats};

/// Writes [`MboMessage`] events to a Parquet file.
///
/// # Usage
///
/// ```ignore
/// let config = ExportConfig::default();
/// let meta = HashMap::new();
/// let mut writer = MboEventWriter::new("events.parquet", &config, meta)?;
///
/// for msg in mbo_messages {
///     writer.write_event(&msg)?;
/// }
///
/// let stats = writer.finish()?;
/// println!("Wrote {} events", stats.rows_written);
/// ```
pub struct MboEventWriter {
    writer: ArrowWriter<File>,
    schema: arrow::datatypes::Schema,
    batch: MboBatch,

    rows_written: u64,
    row_groups: u64,
}

impl MboEventWriter {
    /// Create a new writer targeting `path`.
    ///
    /// `extra_metadata` is merged into the Parquet file-level metadata alongside
    /// the schema-level metadata (source, schema_version, units, etc.).
    pub fn new(
        path: impl AsRef<Path>,
        config: &ExportConfig,
        extra_metadata: HashMap<String, String>,
    ) -> Result<Self> {
        let mut schema = mbo_event_schema();

        let mut merged_meta = schema.metadata().clone();
        for (k, v) in extra_metadata {
            merged_meta.insert(k, v);
        }
        schema = schema.with_metadata(merged_meta);

        let file = File::create(path.as_ref()).map_err(|e| {
            TlobError::Generic(format!(
                "Failed to create MBO Parquet file '{}': {e}",
                path.as_ref().display()
            ))
        })?;

        let props = WriterProperties::builder()
            .set_compression(config.compression)
            .set_max_row_group_size(config.batch_size)
            .build();

        let writer = ArrowWriter::try_new(file, Arc::new(schema.clone()), Some(props))
            .map_err(|e| TlobError::Generic(format!("Failed to create MBO ArrowWriter: {e}")))?;

        let batch = MboBatch::new(config.batch_size);

        Ok(Self {
            writer,
            schema,
            batch,
            rows_written: 0,
            row_groups: 0,
        })
    }

    /// Write a single MBO event.
    pub fn write_event(&mut self, msg: &MboMessage) -> Result<()> {
        self.batch.push(msg);
        self.rows_written += 1;

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
            .map_err(|e| TlobError::Generic(format!("Failed to close MBO Parquet writer: {e}")))?;

        Ok(ParquetExportStats {
            rows_written: self.rows_written,
            rows_seen: self.rows_written,
            row_groups: self.row_groups,
        })
    }

    /// Number of rows written so far.
    pub fn rows_written(&self) -> u64 {
        self.rows_written
    }

    fn flush_batch(&mut self) -> Result<()> {
        let record_batch = self.batch.flush(&self.schema)?;
        self.writer.write(&record_batch).map_err(|e| {
            TlobError::Generic(format!("Failed to write MBO row group: {e}"))
        })?;
        self.row_groups += 1;
        Ok(())
    }
}

/// Builder for [`MboEventWriter`] with a fluent API.
///
/// ```ignore
/// let writer = MboEventWriter::builder("events.parquet")
///     .batch_size(65536)
///     .compression(Compression::SNAPPY)
///     .date("2025-02-03")
///     .symbol("NVDA")
///     .build()?;
/// ```
pub struct MboEventWriterBuilder {
    path: std::path::PathBuf,
    config: ExportConfig,
    metadata: HashMap<String, String>,
}

impl MboEventWriterBuilder {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            config: ExportConfig::default(),
            metadata: HashMap::new(),
        }
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

    pub fn build(self) -> Result<MboEventWriter> {
        MboEventWriter::new(self.path, &self.config, self.metadata)
    }
}

impl MboEventWriter {
    pub fn builder(path: impl AsRef<Path>) -> MboEventWriterBuilder {
        MboEventWriterBuilder::new(path)
    }
}
