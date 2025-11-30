# MBO-LOB-Reconstructor

[![Crates.io](https://img.shields.io/crates/v/mbo-lob-reconstructor.svg)](https://crates.io/crates/mbo-lob-reconstructor)
[![Documentation](https://docs.rs/mbo-lob-reconstructor/badge.svg)](https://docs.rs/mbo-lob-reconstructor)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Build Status](https://github.com/nagarx/MBO-LOB-reconstructor/workflows/CI/badge.svg)](https://github.com/nagarx/MBO-LOB-reconstructor/actions)

**High-performance MBO → LOB reconstruction and analytics for deep learning preprocessing.**

Convert Market-By-Order (MBO) data streams into Limit Order Book (LOB) snapshots with enriched analytics, designed specifically as a preprocessing step for deep learning models (DeepLOB, TLOB, Transformers, CNN-LSTM, etc.).

## ✨ Features

- **🚀 High Performance**: Process ~1M messages/second on modern hardware
- **📊 MBO → LOB Reconstruction**: Convert order-level events to aggregated price levels
- **🔬 Enriched Analytics**: Microprice, VWAP, depth imbalance, spread metrics
- **✅ Book Consistency Validation**: Detect and handle crossed/locked quotes
- **📈 Statistics Tracking**: Per-day statistics with Welford's algorithm for normalization
- **🎯 ML-Ready**: NormalizationParams, DayStats, and feature extraction utilities
- **📦 Databento Support**: Native support for compressed DBN files (.dbn.zst)

## 📦 Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
mbo-lob-reconstructor = "0.1"
```

### Feature Flags

| Feature | Default | Description |
|---------|---------|-------------|
| `databento` | ✅ | Enable Databento DBN file support |

To disable Databento support (smaller binary):

```toml
[dependencies]
mbo-lob-reconstructor = { version = "0.1", default-features = false }
```

## 🚀 Quick Start

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

### Advanced Analytics

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

### Handling Crossed Quotes

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

## 📊 LOB State Analytics

The `LobState` struct provides rich analytics out of the box:

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

## 📈 Statistics for ML Normalization

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

## 🏗️ Architecture

```
mbo_lob_reconstructor/
├── types.rs          # Core types: MboMessage, LobState, Action, Side
├── error.rs          # Error types and Result alias
├── lob/
│   ├── reconstructor.rs  # Main LobReconstructor
│   └── multi_symbol.rs   # Multi-symbol support
├── dbn_bridge.rs     # Databento format conversion
├── loader.rs         # DBN file streaming
├── statistics.rs     # DayStats, RunningStats, NormalizationParams
└── analytics.rs      # DepthStats, MarketImpact, LiquidityMetrics
```

## ⚡ Performance

Benchmarked on real NVIDIA MBO data (17.8M messages):

| Metric | Value |
|--------|-------|
| **Throughput** | ~974,000 msg/s |
| **Latency** | ~1 μs/message |
| **Data Quality** | 100% valid snapshots |
| **Memory** | Efficient streaming (no full load) |

## 🧪 Testing

```bash
# Run all tests
cargo test

# Run with real data (integration tests)
cargo test --release

# Run benchmarks
cargo bench
```

## 🤝 Use Cases

- **Deep Learning Preprocessing**: Prepare LOB data for DeepLOB, TLOB, Transformers
- **Feature Engineering**: Extract market microstructure features
- **Data Validation**: Detect and handle data quality issues
- **Normalization**: Generate consistent normalization parameters across datasets
- **Research**: Analyze order book dynamics and market microstructure

## 📚 Related Work

This library is designed to work with:

- [DeepLOB](https://arxiv.org/abs/1808.03668) - Deep Learning for Limit Order Books
- [TLOB](https://arxiv.org/abs/2211.10587) - Transformer-based LOB models
- [Databento](https://databento.com/) - High-quality market data

## 📄 License

MIT License - see [LICENSE](LICENSE) for details.

## 🙏 Acknowledgments

- Databento for the DBN format specification
- The quantitative finance research community

---

**Made with ❤️ for the quant ML community**

