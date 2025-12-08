//! Limit Order Book (LOB) reconstruction module.
//!
//! This module provides high-performance LOB reconstruction from MBO (Market-By-Order)
//! events. It converts individual order events (Add, Modify, Cancel, Trade) into
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
//! // Create with default config (10 levels, skip system messages)
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
//! println!("System messages skipped: {}", lob.stats().system_messages_skipped);
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
//! # System Messages
//!
//! DBN/MBO data contains system messages (order_id=0, heartbeats, status updates)
//! that are not valid orders. By default, `LobConfig::skip_system_messages = true`
//! causes these to be silently skipped. The count is tracked in
//! `LobStats::system_messages_skipped`.

mod multi_symbol;
pub mod price_level;
pub mod reconstructor;

pub use multi_symbol::MultiSymbolLob;
pub use price_level::PriceLevel;
pub use reconstructor::{CrossedQuotePolicy, LobConfig, LobReconstructor, LobStats};
