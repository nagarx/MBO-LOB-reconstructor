# MBO-LOB-Reconstructor

[![Build Status](https://github.com/nagarx/MBO-LOB-reconstructor/workflows/CI/badge.svg)](https://github.com/nagarx/MBO-LOB-reconstructor/actions)
[![Rust](https://img.shields.io/badge/rust-1.82%2B-blue.svg)](https://www.rust-lang.org/)

High-performance MBO to LOB reconstruction and analytics for deep learning preprocessing.

Convert Market-By-Order (MBO) data streams into Limit Order Book (LOB) snapshots with enriched analytics, designed specifically as a preprocessing step for deep learning models (DeepLOB, TLOB, Transformers, CNN-LSTM, etc.).

> **Pipeline scope (2026-06-02).** This module is part of an **intraday trading research pipeline** — an experiment-first platform for discovering and validating *any* profitable **intraday** trading edge (no overnight positions), across approach classes (microstructure/HFT, scalping, intraday momentum, intraday statistical arbitrage, …) and instruments (equities, futures, same-day options). The pipeline *originated* as a high-frequency NVDA MBO/LOB microstructure system — that origin explains the "HFT" / "LOB" / "MBO" naming here — and that microstructure-direction program is now one (largely-closed) track among many. **Names are historical; the mission is general.** This module's role: the Rust ingestion front-end — reconstructs limit-order-book state (`LobState`) from raw Market-By-Order `.dbn.zst` events (~1M msg/s, BBO 99.17%); the order-book source feeding feature extraction. For the full mission + approach taxonomy + capability-readiness boundary, see root `CLAUDE.md` §Research Scope & Charter (+ `CROSS_ASSET_OFI_FINDINGS_AND_ISSUES_2026_06_01.md` §9).

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
| `legacy-iterator-api` | Yes | Gates the **deprecated** legacy `iter_messages()` API (implies `databento`). New code should use `iter_messages_typed()` instead; scheduled for removal in the next MAJOR release (0.3.0; calendar removal 2026-10-29) |
| `export` | No | Enable Apache Parquet export for raw LOB snapshots and MBO events (+ TOML config for CLI) |

## Quick Start

### Basic LOB Reconstruction

```rust
use mbo_lob_reconstructor::{LobReconstructor, MboMessage, Action, Side};
use mbo_lob_reconstructor::constants::NANODOLLARS_PER_DOLLAR_F64;

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
println!("Best Bid: ${:.2}", state.best_bid.unwrap() as f64 / NANODOLLARS_PER_DOLLAR_F64);
println!("Best Ask: ${:.2}", state.best_ask.unwrap() as f64 / NANODOLLARS_PER_DOLLAR_F64);
```

### Load from Databento DBN Files

```rust
use mbo_lob_reconstructor::{DbnLoader, LobReconstructor, DayStats, NormalizationParams};

// Load compressed DBN file
let loader = DbnLoader::new("data/NVDA.mbo.dbn.zst")?;

let mut lob = LobReconstructor::new(10);
let mut day_stats = DayStats::new("2025-02-03");

// Process all messages — preferred typed API (yields Result<MboMessage, BoundaryError>,
// so decode/convert failures surface as typed errors at the boundary)
let mut iter = loader.iter_messages_typed()?;
for msg_result in &mut iter {
    let msg = msg_result?;
    let state = lob.process_message(&msg)?;
    day_stats.update(&state);

    // Use enriched analytics
    if let Some(microprice) = state.microprice() {
        println!("Microprice: ${:.4}", microprice);
    }
}

// Distinguish clean EOF from a torn (mid-record) stream
let stats = iter.finalize();
assert!(stats.is_clean_eof(), "torn DBN: mid_record_eof={}", stats.mid_record_eof);

// Get normalization parameters for ML (standalone usage).
// Note: feature-extractor-MBO-LOB provides its own NormalizationParams
// with market_structure_zscore strategy for the full pipeline.
let norm_params = NormalizationParams::from_day_stats(&day_stats, 10);
norm_params.save_json("normalization.json")?;
```

> **Legacy API note**: the older `iter_messages()` (yielding `MboMessage` directly) is
> `#[deprecated]` and gated behind the default-on `legacy-iterator-api` feature. It is
> scheduled for removal in the next MAJOR release (0.3.0; calendar 2026-10-29) — do not
> write new code against it.

## Using with Feature Extractor

This library is designed to work with [feature-extractor-MBO-LOB](https://github.com/nagarx/feature-extractor-MBO-LOB) for complete ML preprocessing pipelines.

### Combined Usage (Recommended)

The extractor is a **9-crate Cargo workspace**; its facade crate is `hft-extractor`, which
depends on this library via git tag (`crates/hft-extractor/Cargo.toml`:
`mbo-lob-reconstructor = { git = ..., tag = "v0.2.1" }`, with a `.cargo/config.toml` path
override inside the monorepo). The **production path** is the config-driven `export_dataset`
CLI:

```bash
cd feature-extractor-MBO-LOB
cargo run --release --features parallel --bin export_dataset -- --config configs/<name>.toml
```

For programmatic use, the facade's entry points are `DatasetConfig` (TOML → components) and
`Pipeline` (messages → LOB reconstruction → sampling → features → sequences):

```rust
use hft_extractor::config::DatasetConfig;

// Config-driven construction (preferred)
let config = DatasetConfig::load_toml("configs/nvda_98feat.toml")?;
let layout = config.build_layout()?;
let mut pipeline = config.build_pipeline(&layout); // Uses MBO-LOB-reconstructor internally

// Process one day of MBO data through the complete pipeline
let output = pipeline.process("data/SYMBOL.mbo.dbn.zst")?;
println!("Processed {} messages", output.messages_processed);
println!("Generated {} sequences", output.sequences.len());
```

Multi-day parallel processing lives in `hft_extractor::batch` (`BatchProcessor`,
`process_files_parallel` — feature `parallel`); NPY export in `hft-export-pipeline`
(`ExportPipeline`, re-exported by the facade).

> **Archived-API note**: earlier revisions of this section showed a
> `feature_extractor::prelude` / `PipelineBuilder` / `FeatureExtractor` fluent API. That API
> exists only in the extractor's **archived monolith**
> (`feature-extractor-MBO-LOB/archive/monolith-v1/` — kept for historical reference, not
> compiled) and is absent from the live workspace. Do not write new code against it.

### Manual Component Control

For fine-grained control, drive this library's zero-allocation API directly and hand the
resulting `LobState` to the extractor workspace's live components — `FeatureEngine`
(`hft-feature-core`), `FeatureLayoutBuilder` (`hft-feature-layout`), and `SequenceBuilder`
(`hft-sequence-engine`, all re-exported by the `hft_extractor` facade):

```rust
use mbo_lob_reconstructor::{DbnLoader, LobReconstructor, LobConfig, LobState, CrossedQuotePolicy};

// Configure LOB reconstruction
let lob_config = LobConfig::new(10)
    .with_crossed_quote_policy(CrossedQuotePolicy::UseLastValid);
let mut reconstructor = LobReconstructor::with_config(lob_config);

// Reusable buffer for zero-allocation processing
let mut lob_state = LobState::new(10);

// Process messages (typed iterator: decode/convert failures surface as BoundaryError)
let loader = DbnLoader::new("data/SYMBOL.mbo.dbn.zst")?;
let mut iter = loader.iter_messages_typed()?;
for msg_result in &mut iter {
    let msg = msg_result?;

    // Zero-allocation LOB update
    reconstructor.process_message_into(&msg, &mut lob_state)?;

    // `lob_state` is now ready for downstream feature computation:
    // feed it to the extractor's FeatureEngine, then push the feature
    // vector into SequenceBuilder (`push_arc` + `try_build_sequence`).
    // The canonical wiring of this exact loop is
    // feature-extractor-MBO-LOB/crates/hft-extractor/src/pipeline.rs
    // (`Pipeline::process_messages`).
}
let stats = iter.finalize();
assert!(stats.is_clean_eof(), "torn DBN: mid_record_eof={}", stats.mid_record_eof);
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
            LifecycleEvent::Modified { order_id, .. } => println!("Modified: {}", order_id),
            LifecycleEvent::PartialFill { order_id, fill_size, .. } => {
                println!("Partial fill: {} ({} shares)", order_id, fill_size)
            }
            LifecycleEvent::Completed(lc) => println!("Order {} done in {:?}", lc.order_id, lc.time_alive_ns()),
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
                 trade.size, trade.price_f64(), trade.aggressor_side);
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
// - SkipUpdate: Return last valid state when crossed (same as UseLastValid)
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
statistical analysis (e.g., the `MBO-LOB-analyzer` Python repository). This captures the
unbiased LOB state before any feature extraction transforms.

### CLI

```bash
# Export all days in a directory
cargo run --release --features export --bin export_to_parquet -- \
    --input data/XNAS_ITCH/NVDA/mbo_2025-02-03_to_2026-01-07/ \
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

| File | Scope | Description |
|------|-------|-------------|
| `{date}_lob_snapshots.parquet` | per day | Full LOB state at each message (~7M rows/day) |
| `{date}_mbo_events.parquet` | per day | Raw MBO messages (order_id, action, side, price, size). Omitted with `--no-mbo`. |
| `{date}_reconstruction_stats.json` | per day | Schema-versioned reconstruction / provenance stats — a `LobStatsExportEnvelope` (`{ schema_version, stats }`) written atomically (tmp + fsync + rename). Re-exported at the crate root and read by external consumers. |
| `_export_summary.json` | per run | Run-level summary across all days: totals, throughput, and per-category skipped-row anomaly counts (fail-loud observability per the monorepo development rule "hft-rules §8 — Data Integrity & Error Policy"; that rules file lives at the monorepo root, not in this repo). |

### Data Contract

- **Prices**: `Int64` in nanodollars (divide by 1e9 for dollars)
- **Sizes**: `UInt32` in shares
- **Timestamps**: `Int64` nanoseconds since epoch
- **Parquet schema version**: `1.0` (embedded in file metadata)
- **Reconstruction-stats JSON**: carries its **own** `LOB_STATS_SCHEMA_VERSION` — independent of the Parquet version above (bumped on any change to the stats envelope). See `CHANGELOG.md` / `src/lob/reconstructor.rs` for the current value.

See `src/export/schema.rs` for the authoritative Parquet column definitions.

## Architecture

```
mbo_lob_reconstructor/
    lib.rs            # Crate root: module declarations, feature gates, re-exports
    types.rs          # Core types: MboMessage, LobState, Action, Side, MAX_LOB_LEVELS
    error.rs          # Error types (TlobError, 13 variants) and Result alias
    constants.rs      # Domain constants: NANODOLLARS_PER_DOLLAR, NS_PER_SECOND, BASIS_POINTS_PER_UNIT, etc.
    lob/
        mod.rs              # LOB module hub, re-exports
        reconstructor.rs    # LobReconstructor, process_message_into(), reduce_or_remove_order()
        price_level.rs      # PriceLevel with O(1) size caching
        multi_symbol.rs     # Multi-symbol support
        queue_position.rs   # QueuePositionTracker (FIFO tracking, handle_order_reduction())
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
    loader/
        mod.rs        # DBN file streaming (typed iterator API preferred; zero-copy; auto-detects compression)
        error.rs      # BoundaryError — typed error domain for the loader yield path
    hotstore.rs       # HotStoreManager: decompressed file cache (~30% faster)
    statistics.rs     # DayStats, RunningStats, NormalizationParams
    analytics.rs      # DepthStats, MarketImpact, LiquidityMetrics
    warnings.rs       # WarningTracker, WarningCategory, WarningTrackerConfig
    bin/
        export_to_parquet.rs      # CLI: export LOB snapshots + MBO events to Parquet
        decompress_to_hot_store.rs # CLI: pre-decompress .dbn.zst files for faster loading
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

## Auxiliary Documentation

Living references (kept current — read these for module behavior):

- **`ARCHITECTURE.md`** — data flow, ingestion APIs, module map, core types, export system, CLI binaries.
- **`CODEBASE.md`** — deeper per-module technical reference + integration patterns.
- **`WARNINGS.md`** — `WarningCategory` taxonomy + real-market data-quality edge-case catalog (start here when debugging a preprocessing anomaly).
- **`CHANGELOG.md`** — versioned change history (schema-version constants, feature-flag deprecations).
- **`CLAUDE.md`** — agent session orientation.

Dated historical snapshots (point-in-time audits/plans — **not** current architecture; do not treat as authoritative for present behavior): `BACKBONE_AUDIT_VALIDATED_2026_04.md`, `FOUNDATION_INTEGRITY_PLAN_2026_05.md`.

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
