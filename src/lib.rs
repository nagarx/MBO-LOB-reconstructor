//! # MBO-LOB-Reconstructor
//!
//! High-performance MBO → LOB reconstruction and analytics for deep learning preprocessing.
//!
//! This library converts Market-By-Order (MBO) data streams into Limit Order Book (LOB)
//! snapshots with enriched analytics, designed specifically as a preprocessing step for
//! deep learning models (DeepLOB, TLOB, Transformers, CNN-LSTM, etc.).
//!
//! ## Features
//!
//! - **🚀 High Performance**: Process ~1M messages/second on modern hardware
//! - **📊 MBO → LOB Reconstruction**: Convert order-level events to aggregated price levels
//! - **🔬 Enriched Analytics**: Microprice, VWAP, depth imbalance, spread metrics
//! - **✅ Book Consistency Validation**: Detect and handle crossed/locked quotes
//! - **📈 Statistics Tracking**: Per-day statistics with Welford's algorithm
//! - **🎯 ML-Ready**: NormalizationParams, DayStats, and feature extraction utilities
//! - **📦 Databento Support**: Native support for compressed DBN files
//!
//! ### Performance Optimizations
//!
//! - **Zero-Allocation API**: Use `process_message_into()` to reuse `LobState` buffers
//! - **Stack-Allocated `LobState`**: Fixed-size arrays instead of `Vec` (~30-50% faster)
//! - **O(1) Price Level Size**: `PriceLevel` caches aggregate size (no more `values().sum()`)
//! - **Zero-Copy DBN Parsing**: `MessageIterator` extracts fields without cloning
//!
//! ## Quick Start
//!
//! ### Basic LOB Reconstruction
//!
//! ```rust
//! use mbo_lob_reconstructor::{LobReconstructor, MboMessage, Action, Side};
//!
//! // Create LOB reconstructor with 10 price levels
//! let mut lob = LobReconstructor::new(10);
//!
//! // Process MBO messages
//! let msg = MboMessage::new(
//!     1001,                    // order_id
//!     Action::Add,             // action
//!     Side::Bid,               // side
//!     100_000_000_000,         // price ($100.00 in fixed-point)
//!     100,                     // size
//! );
//!
//! let state = lob.process_message(&msg).unwrap();
//!
//! // Access LOB state
//! if let Some(bid) = state.best_bid {
//!     println!("Best Bid: ${:.2}", bid as f64 / 1e9);
//! }
//! ```
//!
//! ### Load from Databento DBN Files
//!
//! ```ignore
//! use mbo_lob_reconstructor::{DbnLoader, LobReconstructor, DayStats, NormalizationParams};
//!
//! // Load compressed DBN file
//! let loader = DbnLoader::new("data/NVDA.mbo.dbn.zst")?
//!     .skip_invalid(true);
//!
//! let mut lob = LobReconstructor::new(10);
//! let mut day_stats = DayStats::new("2025-02-03");
//!
//! // Process all messages
//! for msg in loader.iter_messages()? {
//!     let state = lob.process_message(&msg)?;
//!     day_stats.update(&state);
//! }
//!
//! // Get normalization parameters for ML
//! let norm_params = NormalizationParams::from_day_stats(&day_stats, 10);
//! norm_params.save_json("normalization.json")?;
//! ```
//!
//! ### Advanced Analytics
//!
//! ```rust
//! use mbo_lob_reconstructor::{LobState, DepthStats, MarketImpact, LiquidityMetrics, Side};
//!
//! // Create a sample state for demonstration
//! let mut state = LobState::new(5);
//! // LobState uses fixed-size arrays (not Vec) for zero-allocation performance
//! state.bid_prices[0] = 100_000_000_000;  // $100.00
//! state.bid_prices[1] = 99_990_000_000;   // $99.99
//! state.bid_sizes[0] = 100;
//! state.bid_sizes[1] = 200;
//! state.ask_prices[0] = 100_010_000_000;  // $100.01
//! state.ask_prices[1] = 100_020_000_000;  // $100.02
//! state.ask_sizes[0] = 150;
//! state.ask_sizes[1] = 100;
//! state.best_bid = Some(100_000_000_000);
//! state.best_ask = Some(100_010_000_000);
//!
//! // Per-side depth statistics
//! let bid_stats = DepthStats::from_lob_state(&state, Side::Bid);
//! println!("Bid VWAP: ${:.4}", bid_stats.weighted_avg_price);
//!
//! // Market impact simulation
//! let impact = MarketImpact::simulate_buy(&state, 100);
//! println!("Slippage: {:.2} bps", impact.slippage_bps);
//!
//! // Combined liquidity metrics
//! let metrics = LiquidityMetrics::from_lob_state(&state);
//! println!("Spread: {:.2} bps", metrics.spread_bps);
//! ```
//!
//! ### High-Performance Zero-Allocation Processing
//!
//! For maximum throughput (real-time or batch processing), use the zero-allocation API:
//!
//! ```ignore
//! use mbo_lob_reconstructor::{LobReconstructor, LobState, DbnLoader};
//!
//! // Create reconstructor and reusable state buffer
//! let mut lob = LobReconstructor::new(10);
//! let mut state = LobState::new(10);  // Stack-allocated, reused across all messages
//!
//! // Load and process
//! let loader = DbnLoader::new("data/NVDA.mbo.dbn.zst")?;
//!
//! for msg in loader.iter_messages()? {
//!     // Zero-allocation: fills existing state buffer in-place
//!     lob.process_message_into(&msg, &mut state)?;
//!
//!     // Use state without any heap allocation overhead
//!     if let Some(mid) = state.mid_price() {
//!         // ... process mid-price
//!     }
//! }
//! ```
//!
//! ## Module Overview
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`types`] | Core types: `MboMessage`, `LobState`, `Action`, `Side`, `MAX_LOB_LEVELS` |
//! | [`lob`] | LOB reconstruction: `LobReconstructor`, `MultiSymbolLob`, `PriceLevel` |
//! | [`statistics`] | ML statistics: `DayStats`, `RunningStats`, `NormalizationParams` |
//! | [`analytics`] | Advanced analytics: `DepthStats`, `MarketImpact`, `LiquidityMetrics` |
//! | [`loader`] | DBN file loading (requires `databento` feature) |
//! | [`dbn_bridge`] | Databento format conversion (requires `databento` feature) |
//! | [`warnings`] | Warning tracking: `WarningTracker`, `Warning`, `WarningCategory` |
//!
//! ## Feature Flags
//!
//! | Feature | Default | Description |
//! |---------|---------|-------------|
//! | `databento` | ✅ | Enable Databento DBN file support |

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod analytics;
pub mod error;
pub mod lob;
pub mod source;
pub mod statistics;
pub mod types;
pub mod warnings;

#[cfg(feature = "databento")]
#[cfg_attr(docsrs, doc(cfg(feature = "databento")))]
pub mod dbn_bridge;

#[cfg(feature = "databento")]
#[cfg_attr(docsrs, doc(cfg(feature = "databento")))]
pub mod hotstore;

#[cfg(feature = "databento")]
#[cfg_attr(docsrs, doc(cfg(feature = "databento")))]
pub mod loader;

// Re-exports - Core types
pub use error::{Result, TlobError};
pub use types::{Action, BookConsistency, LobState, MboMessage, Order, Side, MAX_LOB_LEVELS};

// Re-exports - LOB reconstruction
pub use lob::{CrossedQuotePolicy, LobConfig, LobReconstructor, LobStats, MultiSymbolLob};

// Re-exports - Day Boundary Detection
pub use lob::{DayBoundary, DayBoundaryConfig, DayBoundaryDetector, DayBoundaryStats};

// Re-exports - Trade Aggregation
pub use lob::{Fill, Trade, TradeAggregator, TradeAggregatorConfig};

// Re-exports - Statistics for ML
pub use statistics::{DayStats, NormalizationParams, RunningStats};

// Re-exports - Analytics
pub use analytics::{DepthStats, LiquidityMetrics, MarketImpact};

// Re-exports - Warnings
pub use warnings::{
    Warning, WarningCategory, WarningSummary, WarningTracker, WarningTrackerConfig,
};

// Re-exports - Source abstraction
pub use source::{MarketDataSource, SourceMetadata, VecSource};

#[cfg(feature = "databento")]
pub use source::DbnSource;

// Re-exports - Databento support (feature-gated)
#[cfg(feature = "databento")]
pub use dbn_bridge::DbnBridge;

#[cfg(feature = "databento")]
pub use hotstore::{HotStoreConfig, HotStoreManager};

#[cfg(feature = "databento")]
pub use loader::{is_valid_order, DbnLoader, LoaderStats, IO_BUFFER_SIZE};
