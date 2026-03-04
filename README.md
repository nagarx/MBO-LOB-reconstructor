# MBO-LOB-Reconstructor

[![Build Status](https://github.com/nagarx/MBO-LOB-reconstructor/workflows/CI/badge.svg)](https://github.com/nagarx/MBO-LOB-reconstructor/actions)
[![Rust](https://img.shields.io/badge/rust-1.83%2B-blue.svg)](https://www.rust-lang.org/)

High-performance MBO to LOB reconstruction and analytics for deep learning preprocessing.

Convert Market-By-Order (MBO) data streams into Limit Order Book (LOB) snapshots with enriched analytics, designed specifically as a preprocessing step for deep learning models (DeepLOB, TLOB, Transformers, CNN-LSTM, etc.).

## Features

- **High Performance**: Process approximately 1M messages/second on modern hardware
- **MBO to LOB Reconstruction**: Convert order-level events to aggregated price levels
- **Temporal Fields**: Time delta, triggering action/side for time-sensitive features (FI-2010 u6-u9)
- **Enriched Analytics**: Microprice, VWAP, depth imbalance, spread metrics
- **Book Consistency Validation**: Detect and handle crossed/locked quotes
- **Statistics Tracking**: Per-day statistics with Welford's algorithm for normalization
- **ML-Ready**: NormalizationParams, DayStats, and feature extraction utilities
- **Databento Support**: Native support for compressed DBN files (.dbn.zst)

### Composable Tracking Modules

Additional standalone modules that can be used alongside LOB reconstruction:

- **Queue Position Tracking**: FIFO queue position, volume ahead (for execution probability)
- **Order Lifecycle Tracking**: Track orders through Add→Modify→Cancel/Fill lifecycle
- **Day Boundary Detection**: Automatic trading day detection for train/test splits
- **Trade Aggregation**: Aggregate fills into trades with aggressor side detection

## Feature Flags

| Feature | Default | Description |
|---------|---------|-------------|
| `databento` | Yes | Enable Databento DBN file support |
| `export` | No | Enable Apache Parquet export for raw LOB snapshots and MBO events |

## Quick Start

### Basic LOB Reconstruction

```rust
use mbo_lob_reconstructor::{LobReconstructor, MboMessage, Action, Side};

// Create LOB reconstructor with 10 price levels
let mut lob = LobReconstructor::new(10);

// Process MBO messages
let msg = MboMessage::new(
    1001,                    // order_id
    Action::Add,             // action
    Side::Bid,               // side
    100_000_000_000,         // price ($100.00 in fixed-point)
    100,                     // size
);

let state = lob.process_message(&msg)?;

// Access LOB state
println!("Best Bid: ${:.2}", state.best_bid.unwrap() as f64 / 1e9);
println!("Best Ask: ${:.2}", state.best_ask.unwrap() as f64 / 1e9);
```

### Load from Databento DBN Files

```rust
use mbo_lob_reconstructor::{DbnLoader, LobReconstructor, DayStats};

// Load compressed DBN file
let loader = DbnLoader::new("data/NVDA.mbo.dbn.zst")?
    .skip_invalid(true);

let mut lob = LobReconstructor::new(10);
let mut day_stats = DayStats::new("2025-02-03");

// Process all messages
for msg in loader.iter_messages()? {
    let state = lob.process_message(&msg)?;
    day_stats.update(&state);
    
    // Use enriched analytics
    if let Some(microprice) = state.microprice() {
        println!("Microprice: ${:.4}", microprice);
    }
}

// Get normalization parameters for ML
let norm_params = NormalizationParams::from_day_stats(&day_stats, 10);
norm_params.save_json("normalization.json")?;
```

## Using with Feature Extractor

This library is designed to work with [feature-extractor-MBO-LOB](https://github.com/nagarx/feature-extractor-MBO-LOB) for complete ML preprocessing pipelines.

### Combined Usage (Recommended)

```rust
use feature_extractor::prelude::*;

fn main() -> Result<()> {
    // Build pipeline with fluent API
    let mut pipeline = PipelineBuilder::new()
        .lob_levels(10)           // Uses MBO-LOB-reconstructor internally
        .with_derived_features()  // +8 derived features
        .window(100, 10)          // 100 snapshots per sequence
        .event_sampling(1000)     // Sample every 1000 events
        .build()?;

    // Process MBO data through complete pipeline
    let output = pipeline.process("data/SYMBOL.mbo.dbn.zst")?;

    println!("Processed {} messages", output.messages_processed);
    println!("Generated {} sequences", output.sequences_generated);

    // Export to NumPy for Python/PyTorch
    let exporter = NumpyExporter::new("output/");
    exporter.export_day("2025-02-03", &output)?;

    Ok(())
}
```

### Manual Component Control

For fine-grained control, use the components directly:

```rust
use mbo_lob_reconstructor::{DbnLoader, LobReconstructor, LobConfig, LobState, CrossedQuotePolicy};
use feature_extractor::{FeatureExtractor, FeatureConfig, SequenceBuilder, SequenceConfig};
use std::sync::Arc;

// Configure LOB reconstruction
let lob_config = LobConfig::new(10)
    .with_crossed_quote_policy(CrossedQuotePolicy::UseLastValid);
let mut reconstructor = LobReconstructor::with_config(lob_config);

// Configure feature extraction
let feature_config = FeatureConfig::default().with_derived(true);
let mut extractor = FeatureExtractor::with_config(feature_config.clone());

// Configure sequence building (feature count auto-computed)
let seq_config = SequenceConfig::from_feature_config(100, 10, &feature_config);
let mut sequence_builder = SequenceBuilder::with_config(seq_config);

// Reusable buffers for zero-allocation processing
let mut lob_state = LobState::new(10);
let mut feature_buffer = Vec::with_capacity(feature_config.feature_count());

// Collect sequences during streaming (IMPORTANT: avoids buffer eviction)
let mut sequences = Vec::new();

// Process messages
let loader = DbnLoader::new("data/SYMBOL.mbo.dbn.zst")?;
for msg in loader.iter_messages()? {
    // Zero-allocation LOB update
    reconstructor.process_message_into(&msg, &mut lob_state)?;
    
    // Zero-allocation feature extraction
    extractor.extract_into(&lob_state, &mut feature_buffer)?;
    
    // Wrap in Arc for zero-copy sharing
    let features = Arc::new(std::mem::take(&mut feature_buffer));
    feature_buffer = Vec::with_capacity(feature_config.feature_count());
    
    // Push with Arc (8-byte clone instead of 672-byte clone)
    sequence_builder.push_arc(msg.timestamp.unwrap_or(0) as u64, features)?;
    
    // IMPORTANT: Build sequences during streaming to avoid buffer eviction
    if let Some(seq) = sequence_builder.try_build_sequence() {
        sequences.push(seq);
    }
}

println!("Generated {} sequences", sequences.len());
```

> **Note**: Using `try_build_sequence()` during streaming is critical. The deprecated
> `generate_all_sequences()` method only returns sequences from the buffer's current
> contents (default 1000 snapshots), potentially losing earlier data.

## Advanced Analytics

```rust
use mbo_lob_reconstructor::{DepthStats, MarketImpact, LiquidityMetrics, Side};

// Per-side depth statistics
let bid_stats = DepthStats::from_lob_state(&state, Side::Bid);
println!("Bid VWAP: ${:.4}", bid_stats.weighted_avg_price);
println!("Bid concentration: {:.2}%", bid_stats.concentration_ratio * 100.0);

// Market impact simulation
let impact = MarketImpact::simulate_buy(&state, 1000);
println!("Slippage for 1K shares: {:.2} bps", impact.slippage_bps);
println!("Levels consumed: {}", impact.levels_consumed);

// Combined liquidity metrics
let metrics = LiquidityMetrics::from_lob_state(&state);
println!("Spread: {:.2} bps", metrics.spread_bps);
println!("Book pressure: {:.4}", metrics.book_pressure());
```

## Composable Tracking Modules

These modules are **standalone** - they process `MboMessage` independently and can be used alongside or without LOB reconstruction.

### Queue Position Tracking

```rust
use mbo_lob_reconstructor::{QueuePositionTracker, QueuePositionConfig};

let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());

for msg in messages {
    tracker.process_message(&msg);
    
    if let Some(info) = tracker.queue_position(msg.order_id) {
        println!("Order {} at position {} with {} volume ahead",
                 msg.order_id, info.position, info.volume_ahead);
    }
}
```

### Order Lifecycle Tracking

```rust
use mbo_lob_reconstructor::{OrderLifecycleTracker, OrderLifecycleConfig, LifecycleEvent};

let mut tracker = OrderLifecycleTracker::new(OrderLifecycleConfig::default());

for msg in messages {
    if let Some(event) = tracker.process_message(&msg) {
        match event {
            LifecycleEvent::Created(lc) => println!("New order: {}", lc.order_id),
            LifecycleEvent::Completed(lc) => println!("Order {} done in {:?}", lc.order_id, lc.time_alive_ns()),
            _ => {}
        }
    }
}
```

### Trade Aggregation with Aggressor Detection

```rust
use mbo_lob_reconstructor::{TradeAggregator, TradeAggregatorConfig};

let mut aggregator = TradeAggregator::new(TradeAggregatorConfig::default());

for msg in messages {
    if let Some(trade) = aggregator.process_message(&msg) {
        println!("Trade: {} @ ${:.2} (aggressor: {:?})",
                 trade.size, trade.price as f64 / 1e9, trade.aggressor_side);
    }
}

// Buy vs sell pressure
let imbalance = aggregator.trade_imbalance();  // [-1.0, 1.0]
```

## Handling Crossed Quotes

```rust
use mbo_lob_reconstructor::{LobReconstructor, LobConfig, CrossedQuotePolicy};

// Configure crossed quote handling
let config = LobConfig::new(10)
    .with_crossed_quote_policy(CrossedQuotePolicy::UseLastValid)
    .with_logging(false);

let mut lob = LobReconstructor::with_config(config);

// Available policies:
// - Allow: Return crossed state as-is (track in stats)
// - UseLastValid: Return last valid state when crossed
// - Error: Return error on crossed quote
// - SkipUpdate: Skip updates that would cause crossing
```

## LOB State Analytics

The `LobState` struct provides rich analytics:

| Method | Description |
|--------|-------------|
| `mid_price()` | Average of best bid and ask |
| `spread()` | Difference between best ask and bid |
| `spread_bps()` | Spread in basis points |
| `microprice()` | Volume-weighted mid-price |
| `vwap_bid(n)` / `vwap_ask(n)` | VWAP for top N levels |
| `weighted_mid(n)` | VWAP-based mid-price |
| `depth_imbalance()` | Normalized volume imbalance [-1, 1] |
| `total_bid_volume()` / `total_ask_volume()` | Total volume per side |
| `active_bid_levels()` / `active_ask_levels()` | Count of non-empty levels |
| `check_consistency()` | Book state: Valid, Empty, Crossed, Locked |

### Temporal Fields (FI-2010 u6-u9)

| Field/Method | Description |
|--------------|-------------|
| `delta_ns` | Time since last LOB update (nanoseconds) |
| `delta_seconds()` | Time delta in seconds |
| `event_intensity()` | Events per second (1/Δt) |
| `triggering_action` | Action that caused this state (Add, Cancel, Trade, etc.) |
| `triggering_side` | Side affected (Bid, Ask) |
| `is_trade_event()` | Check if triggered by Trade/Fill |
| `is_add_event()` | Check if triggered by Add |
| `is_cancel_event()` | Check if triggered by Cancel |

## Statistics for ML Normalization

### DayStats

Track per-day statistics for proper normalization:

```rust
use mbo_lob_reconstructor::DayStats;

let mut stats = DayStats::new("2025-02-03");

// Update with each LOB snapshot
for state in lob_states {
    stats.update(&state);
}

// Access statistics
println!("Valid snapshots: {}", stats.valid_snapshots);
println!("Data quality: {:.2}%", stats.data_quality_ratio() * 100.0);
println!("Mid-price mean: ${:.4}", stats.mid_price.mean);
println!("Mid-price std: ${:.4}", stats.mid_price.std());
```

### NormalizationParams

Generate and persist normalization parameters:

```rust
use mbo_lob_reconstructor::NormalizationParams;

// Create from day stats
let params = NormalizationParams::from_day_stats(&day_stats, 10);

// Save/load for consistent normalization
params.save_json("norm_params.json")?;
let loaded = NormalizationParams::load_json("norm_params.json")?;

// Apply normalization
let normalized = params.normalize(value, feature_idx);
let denormalized = params.denormalize(normalized, feature_idx);
```

## Parquet Export (feature: `export`)

Export raw LOB snapshots and MBO events to Apache Parquet files for downstream
statistical analysis (e.g., `raw-lob-analyzer`). This captures the unbiased
LOB state before any feature extraction transforms.

### CLI

```bash
# Export all days in a directory
cargo run --release --features export --bin export_to_parquet -- \
    --input data/NVDA_2025-02-03_to_2026-01-07/ \
    --output data/exports/raw_lob/ \
    --symbol NVDA --levels 10

# Export with downsampling (every 100th snapshot)
cargo run --release --features export --bin export_to_parquet -- \
    --input data/NVDA/ --output data/exports/raw_lob_sampled/ \
    --symbol NVDA --downsample-every 100
```

### Programmatic API

```rust
use mbo_lob_reconstructor::export::{ExportConfig, LobSnapshotWriter, MboEventWriter};

let config = ExportConfig::default();
let mut lob_writer = LobSnapshotWriter::builder("snapshots.parquet")
    .levels(10)
    .include_derived(true)
    .date("2025-02-03")
    .symbol("NVDA")
    .build()?;

// Write each LOB state as it comes from the reconstructor
lob_writer.write_snapshot(&lob_state)?;

// Finalize (writes Parquet footer)
let stats = lob_writer.finish()?;
println!("Wrote {} snapshots", stats.rows_written);
```

### Output Files (per day)

| File | Description |
|------|-------------|
| `{date}_lob_snapshots.parquet` | Full LOB state at each message (~7M rows/day) |
| `{date}_mbo_events.parquet` | Raw MBO messages (order_id, action, side, price, size) |

### Data Contract

- **Prices**: `Int64` in nanodollars (divide by 1e9 for dollars)
- **Sizes**: `UInt32` in shares
- **Timestamps**: `Int64` nanoseconds since epoch
- **Schema version**: `1.0` (embedded in file metadata)

See `src/export/schema.rs` for the authoritative column definitions.

## Architecture

```
mbo_lob_reconstructor/
    types.rs          # Core types: MboMessage, LobState, Action, Side, MAX_LOB_LEVELS
    error.rs          # Error types and Result alias
    lob/
        reconstructor.rs    # LobReconstructor, process_message_into()
        price_level.rs      # PriceLevel with O(1) size caching
        multi_symbol.rs     # Multi-symbol support
        queue_position.rs   # QueuePositionTracker (FIFO tracking)
        order_lifecycle.rs  # OrderLifecycleTracker (Add→Modify→Cancel/Fill)
        day_boundary.rs     # DayBoundaryDetector (trading day detection)
        trade_aggregator.rs # TradeAggregator (fills→trades, aggressor side)
    export/               # Parquet export (requires `export` feature)
        mod.rs            # ExportConfig, DownsampleConfig, re-exports
        schema.rs         # Arrow schema definitions (LOB + MBO)
        lob_writer.rs     # LobSnapshotWriter: LobState -> Parquet
        mbo_writer.rs     # MboEventWriter: MboMessage -> Parquet
        batch.rs          # Column-oriented row batching
    source.rs         # MarketDataSource trait, DbnSource, VecSource
    dbn_bridge.rs     # Databento format conversion
    loader.rs         # DBN file streaming (zero-copy message iteration)
    statistics.rs     # DayStats, RunningStats, NormalizationParams
    analytics.rs      # DepthStats, MarketImpact, LiquidityMetrics
    warnings.rs       # WarningTracker, WarningCategory
```

### Key Optimizations

| Optimization | Description |
|-------------|-------------|
| `LobState` fixed arrays | Stack-allocated arrays instead of `Vec` (no heap per snapshot) |
| `process_message_into()` | Zero-allocation LOB update into existing buffer |
| `PriceLevel` O(1) cache | Cached total_size eliminates O(n) sum per level |
| Zero-copy message iteration | `DbnLoader` avoids cloning `MboMsg` |

## Performance

Benchmarked on real NVIDIA MBO data (17.8M messages):

| Metric | Value |
|--------|-------|
| Throughput | ~974,000 msg/s |
| Latency | ~1 microsecond/message |
| Data Quality | 100% valid snapshots |
| Memory | Efficient streaming (no full load) |

> **Note**: The primary bottleneck is **zstd decompression** (single-threaded per file stream),
> not LOB reconstruction. For multi-day batch processing, consider the `BatchProcessor` in
> `feature-extractor-MBO-LOB` which parallelizes across files, or pre-decompress files to
> uncompressed `.dbn` format for ~5-10× throughput improvement.

## Testing

```bash
# Run all tests (default features)
cargo test

# Run all tests including Parquet export
cargo test --features export

# Run export tests only
cargo test --features export --test export_test

# Run with real data (integration tests)
cargo test --release

# Run benchmarks
cargo bench
```

## Use Cases

- Deep Learning Preprocessing: Prepare LOB data for DeepLOB, TLOB, Transformers
- Feature Engineering: Extract market microstructure features
- Data Validation: Detect and handle data quality issues
- Normalization: Generate consistent normalization parameters across datasets
- Research: Analyze order book dynamics and market microstructure

## Related Libraries

- [feature-extractor-MBO-LOB](https://github.com/nagarx/feature-extractor-MBO-LOB) - Feature extraction and ML pipeline

## Related Work

This library is designed to work with:

- [DeepLOB](https://arxiv.org/abs/1808.03668) - Deep Learning for Limit Order Books
- [TLOB](https://arxiv.org/abs/2211.10587) - Transformer-based LOB models
- [Databento](https://databento.com/) - High-quality market data

## License

Proprietary - All Rights Reserved. See [LICENSE](LICENSE) for details.

No permission is granted to use, copy, modify, or distribute this software.
