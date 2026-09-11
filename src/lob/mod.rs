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
//! By default (`LobConfig::skip_system_messages = true`) the reconstructor skips
//! records for which `MboMessage::is_heartbeat()` is true: the field shape
//! `order_id == 0 || size == 0 || price <= 0` on any action EXCEPT `Action::Clear`
//! and `Action::TradeAggregate`, which are never skipped — a Clear resets the book
//! and a trade print is counted (a book no-op). The count is tracked in
//! `LobStats::system_messages_skipped`, which on the measured corpus is a structural
//! 0 since rung 4A. The default validation gate (`validate_messages = true`) runs
//! `MboMessage::validate_admission()`, not `validate()`.

pub mod day_boundary;
mod multi_symbol;
pub mod order_lifecycle;
pub mod price_level;
pub mod queue_position;
pub mod reconstructor;
// RETAINED, PRIVATE, AND DELIBERATELY UNUSED pending the ladder's `trade_aggregator` commit.
// The un-export (2026-09-07) made the module private, and dead_code IMMEDIATELY FIRED on 9
// items -- which is the point: while `pub use` stood, those items were externally reachable so
// dead_code COULD NOT fire, and the waiver's whole safety argument ("safe ONLY because the
// module has zero code consumers") was invisible to the compiler and had to be asserted by a
// human. It is now mechanically checked on every build.
// This allow is therefore a RECORD, not a silencing: CI runs `cargo clippy --all-features --
// -D warnings` (ci.yml:102), so without it the 9 warnings break the build. ⛔ DELETE THIS
// ATTRIBUTE when the trade_aggregator commit lands -- if the module is then still dead, the
// warnings returning is the correct signal that it should be archived rather than fixed.
// The module is NOT deleted: hft-rules §0, archive never delete -- it is the comparison
// subject for whatever replaces it. See src/lib.rs for why the re-export went.
#[allow(dead_code)]
mod trade_aggregator;

pub use day_boundary::{DayBoundary, DayBoundaryConfig, DayBoundaryDetector, DayBoundaryStats};
pub use multi_symbol::MultiSymbolLob;
pub use order_lifecycle::{
    ActiveOrderFeatures, CompletionStats, LifecycleEvent, LifecycleStats, OrderLifecycle,
    OrderLifecycleConfig, OrderLifecycleTracker, OrderModification, OrderOrigin, TerminalState,
};
pub use price_level::PriceLevel;
pub use queue_position::{
    PositionChange, PositionChangeReason, QueuePositionConfig, QueuePositionInfo,
    QueuePositionTracker, QueueStats,
};
pub use reconstructor::{CrossedQuotePolicy, LobConfig, LobReconstructor, LobStats};
// UN-EXPORTED 2026-09-07 with the crate-root re-export above it; see src/lib.rs.
