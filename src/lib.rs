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
//! state.bid_prices = vec![100_000_000_000, 99_990_000_000, 0, 0, 0];
//! state.bid_sizes = vec![100, 200, 0, 0, 0];
//! state.ask_prices = vec![100_010_000_000, 100_020_000_000, 0, 0, 0];
//! state.ask_sizes = vec![150, 100, 0, 0, 0];
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
//! ## Module Overview
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`types`] | Core types: `MboMessage`, `LobState`, `Action`, `Side` |
//! | [`lob`] | LOB reconstruction: `LobReconstructor`, `MultiSymbolLob` |
//! | [`statistics`] | ML statistics: `DayStats`, `RunningStats`, `NormalizationParams` |
//! | [`analytics`] | Advanced analytics: `DepthStats`, `MarketImpact`, `LiquidityMetrics` |
//! | [`loader`] | DBN file loading (requires `databento` feature) |
//! | [`dbn_bridge`] | Databento format conversion (requires `databento` feature) |
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
pub mod statistics;
pub mod types;
pub mod warnings;

#[cfg(feature = "databento")]
#[cfg_attr(docsrs, doc(cfg(feature = "databento")))]
pub mod dbn_bridge;

#[cfg(feature = "databento")]
#[cfg_attr(docsrs, doc(cfg(feature = "databento")))]
pub mod loader;

// Re-exports - Core types
pub use error::{Result, TlobError};
pub use types::{Action, BookConsistency, LobState, MboMessage, Order, Side};

// Re-exports - LOB reconstruction
pub use lob::{CrossedQuotePolicy, LobConfig, LobReconstructor, LobStats, MultiSymbolLob};

// Re-exports - Statistics for ML
pub use statistics::{DayStats, NormalizationParams, RunningStats};

// Re-exports - Analytics
pub use analytics::{DepthStats, LiquidityMetrics, MarketImpact};

// Re-exports - Warnings
pub use warnings::{
    Warning, WarningCategory, WarningSummary, WarningTracker, WarningTrackerConfig,
};

// Re-exports - Databento support (feature-gated)
#[cfg(feature = "databento")]
pub use dbn_bridge::DbnBridge;

#[cfg(feature = "databento")]
pub use loader::{is_valid_order, DbnLoader, LoaderStats};
