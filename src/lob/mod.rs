//! Limit Order Book (LOB) reconstruction module.
//!
//! This module provides high-performance LOB reconstruction from MBO (Market-By-Order)
//! events. It converts authoritative order events (Add, Modify, Cancel, Clear) into
//! aggregated price level snapshots.
//!
//! # Core Components
//!
//! | Type | Description |
//! |------|-------------|
//! | [`LobReconstructor`] | Single-symbol LOB reconstructor |
//! | [`MultiSymbolLob`] | Multi-symbol manager |
//! | [`LobConfig`] | Configuration options |
//! | [`LobStats`] | Processing statistics |
//! | [`CrossedQuotePolicy`] | How to handle crossed quotes |
//! | [`PriceLevel`] | Orders at a price with cached aggregate size (O(1) queries) |
//!
//! # Usage Pattern
//!
//! ## Standard Usage
//!
//! ```ignore
//! use mbo_lob_reconstructor::{LobReconstructor, LobConfig};
//!
//! // Create with default config (10 levels, skip explicit no-op controls)
//! let mut lob = LobReconstructor::new(10);
//!
//! // Process messages
//! for msg in messages {
//!     let state = lob.process_message(&msg)?;
//!     // state contains current LOB snapshot
//! }
//!
//! // Check statistics
//! println!("Processed: {}", lob.stats().messages_processed);
//! println!("No-op controls skipped: {}", lob.stats().noop_controls_skipped);
//! ```
//!
//! ## High-Performance Zero-Allocation Pattern
//!
//! For maximum throughput, reuse a single `LobState` buffer:
//!
//! ```ignore
//! use mbo_lob_reconstructor::{LobReconstructor, LobState};
//!
//! let mut lob = LobReconstructor::new(10);
//! let mut state = LobState::new(10);  // Reused across all iterations
//!
//! for msg in messages {
//!     lob.process_message_into(&msg, &mut state)?;  // Zero heap allocations
//!     // Use state.mid_price(), state.spread(), etc.
//! }
//! ```
//!
//! # Multi-Day Processing
//!
//! ```ignore
//! // Day 1
//! for msg in day1 {
//!     lob.process_message(&msg)?;
//! }
//! let day1_stats = lob.stats().clone();
//!
//! // Reset for Day 2 (clears stats)
//! lob.full_reset();
//!
//! // Day 2
//! for msg in day2 {
//!     lob.process_message(&msg)?;
//! }
//! ```
//!
//! # Compatibility controls
//!
//! Only explicit `Action::None` records are skipped as no-op controls. Field
//! shapes are never used to hide malformed order commands or aggregate trades.
//! The former message-at-a-time queue, lifecycle, and trade-aggregation modules
//! are not part of the v1 crate surface: they cannot retain strict envelope
//! custody, causal availability, or publisher-qualified queue policy.

pub mod day_boundary;
mod multi_symbol;
pub mod price_level;
pub mod reconstructor;

pub use day_boundary::{DayBoundary, DayBoundaryConfig, DayBoundaryDetector, DayBoundaryStats};
pub use multi_symbol::MultiSymbolLob;
pub use price_level::PriceLevel;
pub use reconstructor::{CrossedQuotePolicy, LobConfig, LobReconstructor, LobStats};
