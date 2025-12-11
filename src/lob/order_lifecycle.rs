//! Order Lifecycle Tracking for MBO Data
//!
//! This module tracks individual orders through their complete lifecycle:
//! Add → [Modify]* → Cancel|Fill
//!
//! # Research Reference
//!
//! > "MBO features provide orthogonal information to LOB features...
//! >  ensemble improves F1 by 12 pp" — Zhang et al. (2021)
//!
//! # Key Design Decisions
//!
//! ## Handling Pre-Existing Orders (Critical)
//!
//! MBO data streams often start mid-session, meaning orders may have been placed
//! **before** the data window begins. When we see a Modify/Cancel/Fill for an
//! unknown order, we have two choices:
//!
//! 1. **Ignore** - Lose tracking information (current approach in `LobReconstructor`)
//! 2. **Infer** - Create a "synthetic" lifecycle with `origin = Inferred`
//!
//! This module uses approach #2 for lifecycle tracking while maintaining accuracy:
//! - Inferred orders are clearly marked (`OrderOrigin::Inferred`)
//! - Statistics track inferred vs observed orders separately
//! - Features can filter by origin if needed
//!
//! ## Memory Management
//!
//! Order lifecycles can accumulate memory over time. This module provides:
//! - Configurable retention of completed orders
//! - Automatic cleanup of old completed lifecycles
//! - Statistics on memory usage
//!
//! # Example
//!
//! ```ignore
//! use mbo_lob_reconstructor::lob::order_lifecycle::{OrderLifecycleTracker, OrderLifecycleConfig};
//!
//! let config = OrderLifecycleConfig::default();
//! let mut tracker = OrderLifecycleTracker::new(config);
//!
//! // Process messages
//! for msg in messages {
//!     if let Some(event) = tracker.process_message(&msg) {
//!         match event {
//!             LifecycleEvent::Created(lifecycle) => println!("New order: {}", lifecycle.order_id),
//!             LifecycleEvent::Modified { .. } => println!("Order modified"),
//!             LifecycleEvent::Completed(lifecycle) => {
//!                 println!("Order {} completed after {:?}", 
//!                          lifecycle.order_id, 
//!                          lifecycle.time_alive_ns());
//!             }
//!         }
//!     }
//! }
//!
//! // Get statistics
//! let stats = tracker.stats();
//! println!("Observed: {}, Inferred: {}", stats.observed_orders, stats.inferred_orders);
//! ```

use crate::types::{Action, MboMessage, Side};
use ahash::AHashMap;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for order lifecycle tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderLifecycleConfig {
    /// Maximum number of completed lifecycles to retain in memory.
    ///
    /// Older completed lifecycles are evicted when this limit is exceeded.
    /// Set to 0 to disable retention (only track active orders).
    /// Default: 10_000
    pub max_completed_retention: usize,

    /// Whether to track modifications (increases memory usage).
    ///
    /// If false, only tracks creation and completion.
    /// Default: true
    pub track_modifications: bool,

    /// Whether to infer lifecycles for pre-existing orders.
    ///
    /// If true, creates synthetic lifecycles when we see Modify/Cancel/Fill
    /// for an unknown order (common at start of data stream).
    /// Default: true
    pub infer_pre_existing: bool,

    /// Maximum modifications to track per order.
    ///
    /// Prevents unbounded memory growth for heavily modified orders.
    /// Default: 100
    pub max_modifications_per_order: usize,
}

impl Default for OrderLifecycleConfig {
    fn default() -> Self {
        Self {
            max_completed_retention: 10_000,
            track_modifications: true,
            infer_pre_existing: true,
            max_modifications_per_order: 100,
        }
    }
}

impl OrderLifecycleConfig {
    /// Create config with no completed retention (minimal memory).
    pub fn minimal() -> Self {
        Self {
            max_completed_retention: 0,
            track_modifications: false,
            infer_pre_existing: true,
            max_modifications_per_order: 0,
        }
    }

    /// Create config for research (full tracking).
    pub fn research() -> Self {
        Self {
            max_completed_retention: 100_000,
            track_modifications: true,
            infer_pre_existing: true,
            max_modifications_per_order: 1000,
        }
    }

    /// Set maximum completed retention.
    pub fn with_max_completed(mut self, max: usize) -> Self {
        self.max_completed_retention = max;
        self
    }

    /// Enable or disable modification tracking.
    pub fn with_modification_tracking(mut self, enabled: bool) -> Self {
        self.track_modifications = enabled;
        self
    }

    /// Enable or disable inference of pre-existing orders.
    pub fn with_inference(mut self, enabled: bool) -> Self {
        self.infer_pre_existing = enabled;
        self
    }
}

// ============================================================================
// Order Origin
// ============================================================================

/// How we learned about this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderOrigin {
    /// We observed the Add message for this order.
    Observed,

    /// Order existed before our data window started.
    ///
    /// We inferred its existence from a Modify/Cancel/Fill message.
    /// The `creation_timestamp` will be the timestamp of that first message.
    Inferred,
}

// ============================================================================
// Order Modification
// ============================================================================

/// A single modification to an order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderModification {
    /// Timestamp of the modification.
    pub timestamp: i64,

    /// Price before modification.
    pub old_price: i64,

    /// Price after modification.
    pub new_price: i64,

    /// Size before modification.
    pub old_size: u32,

    /// Size after modification.
    pub new_size: u32,

    /// Sequence number of this modification (1-indexed).
    pub modification_number: u32,
}

impl OrderModification {
    /// Check if this was a price change.
    #[inline]
    pub fn is_price_change(&self) -> bool {
        self.old_price != self.new_price
    }

    /// Check if this was a size change.
    #[inline]
    pub fn is_size_change(&self) -> bool {
        self.old_size != self.new_size
    }

    /// Get the price change (new - old).
    #[inline]
    pub fn price_change(&self) -> i64 {
        self.new_price - self.old_price
    }

    /// Get the size change (new - old), can be negative.
    #[inline]
    pub fn size_change(&self) -> i32 {
        self.new_size as i32 - self.old_size as i32
    }
}

// ============================================================================
// Terminal State
// ============================================================================

/// How an order's lifecycle ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalState {
    /// Order was cancelled (full or partial that resulted in removal).
    Cancelled,

    /// Order was fully filled.
    Filled,

    /// Order was partially filled then cancelled.
    PartialFillThenCancel,

    /// Order is still active (not terminal).
    Active,
}

// ============================================================================
// Order Lifecycle
// ============================================================================

/// Complete lifecycle of a single order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderLifecycle {
    /// Unique order identifier.
    pub order_id: u64,

    /// Order side.
    pub side: Side,

    /// How we learned about this order.
    pub origin: OrderOrigin,

    /// Original price when order was created/first seen.
    pub original_price: i64,

    /// Original size when order was created/first seen.
    pub original_size: u32,

    /// Current/final price.
    pub current_price: i64,

    /// Current/final size.
    pub current_size: u32,

    /// Timestamp when order was created/first seen.
    pub creation_timestamp: i64,

    /// Timestamp of last activity.
    pub last_activity_timestamp: i64,

    /// Timestamp when order was completed (cancelled or filled).
    pub completion_timestamp: Option<i64>,

    /// How the order ended.
    pub terminal_state: TerminalState,

    /// Total volume filled from this order.
    pub total_filled: u64,

    /// Number of fill events.
    pub fill_count: u32,

    /// Modification history (if tracking enabled).
    pub modifications: Vec<OrderModification>,

    /// Number of modifications (tracked even if history not stored).
    pub modification_count: u32,
}

impl OrderLifecycle {
    /// Create a new lifecycle for an observed Add message.
    fn new_observed(msg: &MboMessage) -> Self {
        let ts = msg.timestamp.unwrap_or(0);
        Self {
            order_id: msg.order_id,
            side: msg.side,
            origin: OrderOrigin::Observed,
            original_price: msg.price,
            original_size: msg.size,
            current_price: msg.price,
            current_size: msg.size,
            creation_timestamp: ts,
            last_activity_timestamp: ts,
            completion_timestamp: None,
            terminal_state: TerminalState::Active,
            total_filled: 0,
            fill_count: 0,
            modifications: Vec::new(),
            modification_count: 0,
        }
    }

    /// Create a new lifecycle for an inferred pre-existing order.
    ///
    /// Used when we see Modify/Cancel/Fill for an unknown order.
    fn new_inferred(msg: &MboMessage) -> Self {
        let ts = msg.timestamp.unwrap_or(0);
        Self {
            order_id: msg.order_id,
            side: msg.side,
            origin: OrderOrigin::Inferred,
            // For inferred orders, we use the message's price/size as "original"
            // This is our best guess - the actual original may have been different
            original_price: msg.price,
            original_size: msg.size,
            current_price: msg.price,
            current_size: msg.size,
            creation_timestamp: ts,
            last_activity_timestamp: ts,
            completion_timestamp: None,
            terminal_state: TerminalState::Active,
            total_filled: 0,
            fill_count: 0,
            modifications: Vec::new(),
            modification_count: 0,
        }
    }

    /// Time the order was alive (creation to completion or last activity).
    pub fn time_alive_ns(&self) -> i64 {
        let end_ts = self.completion_timestamp.unwrap_or(self.last_activity_timestamp);
        end_ts.saturating_sub(self.creation_timestamp)
    }

    /// Time the order was alive in seconds.
    pub fn time_alive_seconds(&self) -> f64 {
        self.time_alive_ns() as f64 / 1_000_000_000.0
    }

    /// Check if the order is still active.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.terminal_state == TerminalState::Active
    }

    /// Check if the order was observed (vs inferred).
    #[inline]
    pub fn is_observed(&self) -> bool {
        self.origin == OrderOrigin::Observed
    }

    /// Get fill rate (filled / original size).
    pub fn fill_rate(&self) -> f64 {
        if self.original_size == 0 {
            return 0.0;
        }
        self.total_filled as f64 / self.original_size as f64
    }

    /// Check if the order had any price modifications.
    pub fn had_price_change(&self) -> bool {
        self.original_price != self.current_price
    }

    /// Check if the order had any size modifications (excluding fills).
    pub fn had_size_modification(&self) -> bool {
        self.modification_count > 0
    }

    /// Get average time between modifications (if any).
    pub fn avg_modification_interval_ns(&self) -> Option<f64> {
        if self.modifications.len() < 2 {
            return None;
        }

        let mut total_interval = 0i64;
        let mut prev_ts = self.creation_timestamp;

        for m in &self.modifications {
            total_interval += m.timestamp.saturating_sub(prev_ts);
            prev_ts = m.timestamp;
        }

        Some(total_interval as f64 / self.modifications.len() as f64)
    }
}

// ============================================================================
// Lifecycle Event
// ============================================================================

/// Event emitted when a lifecycle changes.
#[derive(Debug, Clone)]
pub enum LifecycleEvent {
    /// A new order was created.
    Created(OrderLifecycle),

    /// An order was modified.
    Modified {
        order_id: u64,
        modification: OrderModification,
    },

    /// A fill occurred.
    PartialFill {
        order_id: u64,
        fill_size: u32,
        fill_price: i64,
        timestamp: i64,
    },

    /// An order completed (cancelled or fully filled).
    Completed(OrderLifecycle),
}

// ============================================================================
// Lifecycle Statistics
// ============================================================================

/// Statistics about order lifecycle tracking.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LifecycleStats {
    /// Total orders observed (Add messages).
    pub observed_orders: u64,

    /// Total orders inferred (pre-existing).
    pub inferred_orders: u64,

    /// Total modifications recorded.
    pub total_modifications: u64,

    /// Total fills recorded.
    pub total_fills: u64,

    /// Orders cancelled.
    pub cancelled_orders: u64,

    /// Orders fully filled.
    pub filled_orders: u64,

    /// Orders partial-filled then cancelled.
    pub partial_then_cancelled: u64,

    /// Currently active orders.
    pub active_orders: u64,

    /// Completed orders retained in memory.
    pub completed_retained: u64,

    /// Completed orders evicted due to retention limit.
    pub completed_evicted: u64,

    /// Modifications dropped due to per-order limit.
    pub modifications_dropped: u64,

    /// Messages skipped (system messages, etc.).
    pub messages_skipped: u64,
}

impl LifecycleStats {
    /// Total orders tracked (observed + inferred).
    pub fn total_orders(&self) -> u64 {
        self.observed_orders + self.inferred_orders
    }

    /// Observation rate (observed / total).
    pub fn observation_rate(&self) -> f64 {
        let total = self.total_orders();
        if total == 0 {
            return 0.0;
        }
        self.observed_orders as f64 / total as f64
    }

    /// Fill rate (filled / (filled + cancelled)).
    pub fn overall_fill_rate(&self) -> f64 {
        let completed = self.filled_orders + self.cancelled_orders + self.partial_then_cancelled;
        if completed == 0 {
            return 0.0;
        }
        (self.filled_orders + self.partial_then_cancelled) as f64 / completed as f64
    }
}

// ============================================================================
// Order Lifecycle Tracker
// ============================================================================

/// Tracks order lifecycles from MBO messages.
pub struct OrderLifecycleTracker {
    config: OrderLifecycleConfig,

    /// Active orders: order_id -> lifecycle
    active: AHashMap<u64, OrderLifecycle>,

    /// Recently completed orders (for analysis)
    completed: VecDeque<OrderLifecycle>,

    /// Statistics
    stats: LifecycleStats,
}

impl OrderLifecycleTracker {
    /// Create a new tracker with the given configuration.
    pub fn new(config: OrderLifecycleConfig) -> Self {
        Self {
            config,
            active: AHashMap::new(),
            completed: VecDeque::new(),
            stats: LifecycleStats::default(),
        }
    }

    /// Process an MBO message and return any lifecycle event.
    pub fn process_message(&mut self, msg: &MboMessage) -> Option<LifecycleEvent> {
        // Skip system messages
        if msg.order_id == 0 || msg.size == 0 || msg.price <= 0 {
            self.stats.messages_skipped += 1;
            return None;
        }

        // Skip non-directional orders
        if msg.side == Side::None {
            self.stats.messages_skipped += 1;
            return None;
        }

        match msg.action {
            Action::Add => self.handle_add(msg),
            Action::Modify => self.handle_modify(msg),
            Action::Cancel => self.handle_cancel(msg),
            Action::Trade | Action::Fill => self.handle_fill(msg),
            Action::Clear | Action::None => {
                // Clear and None actions don't affect individual order lifecycles
                self.stats.messages_skipped += 1;
                None
            }
        }
    }

    /// Handle an Add message.
    fn handle_add(&mut self, msg: &MboMessage) -> Option<LifecycleEvent> {
        // Check if order already exists (exchange may reuse IDs)
        if self.active.contains_key(&msg.order_id) {
            // Treat as modify
            return self.handle_modify(msg);
        }

        let lifecycle = OrderLifecycle::new_observed(msg);
        self.active.insert(msg.order_id, lifecycle.clone());
        self.stats.observed_orders += 1;
        self.stats.active_orders += 1;

        Some(LifecycleEvent::Created(lifecycle))
    }

    /// Handle a Modify message.
    fn handle_modify(&mut self, msg: &MboMessage) -> Option<LifecycleEvent> {
        let ts = msg.timestamp.unwrap_or(0);

        // Get or create lifecycle
        let lifecycle = if let Some(lc) = self.active.get_mut(&msg.order_id) {
            lc
        } else {
            // Pre-existing order - infer if configured
            if !self.config.infer_pre_existing {
                return None;
            }

            let inferred = OrderLifecycle::new_inferred(msg);
            self.active.insert(msg.order_id, inferred);
            self.stats.inferred_orders += 1;
            self.stats.active_orders += 1;

            // The first message for an inferred order doesn't produce a modification
            // since we have no "before" state
            return Some(LifecycleEvent::Created(
                self.active.get(&msg.order_id).unwrap().clone(),
            ));
        };

        // Create modification record
        let modification = OrderModification {
            timestamp: ts,
            old_price: lifecycle.current_price,
            new_price: msg.price,
            old_size: lifecycle.current_size,
            new_size: msg.size,
            modification_number: lifecycle.modification_count + 1,
        };

        // Update lifecycle
        lifecycle.current_price = msg.price;
        lifecycle.current_size = msg.size;
        lifecycle.last_activity_timestamp = ts;
        lifecycle.modification_count += 1;

        // Track modification history if enabled
        if self.config.track_modifications
            && lifecycle.modifications.len() < self.config.max_modifications_per_order
        {
            lifecycle.modifications.push(modification.clone());
        } else if self.config.track_modifications {
            self.stats.modifications_dropped += 1;
        }

        self.stats.total_modifications += 1;

        Some(LifecycleEvent::Modified {
            order_id: msg.order_id,
            modification,
        })
    }

    /// Handle a Cancel message.
    fn handle_cancel(&mut self, msg: &MboMessage) -> Option<LifecycleEvent> {
        let ts = msg.timestamp.unwrap_or(0);

        // Get or create lifecycle
        let lifecycle = if let Some(lc) = self.active.get_mut(&msg.order_id) {
            lc
        } else {
            // Pre-existing order - infer if configured
            if !self.config.infer_pre_existing {
                return None;
            }

            let mut inferred = OrderLifecycle::new_inferred(msg);
            // For a cancel, the order is immediately completed
            inferred.completion_timestamp = Some(ts);
            inferred.terminal_state = TerminalState::Cancelled;
            inferred.current_size = 0;

            self.stats.inferred_orders += 1;
            self.stats.cancelled_orders += 1;

            // Store in completed (not active since it's already done)
            self.store_completed(inferred.clone());

            return Some(LifecycleEvent::Completed(inferred));
        };

        // Update lifecycle
        lifecycle.last_activity_timestamp = ts;
        lifecycle.completion_timestamp = Some(ts);

        // Determine terminal state
        if lifecycle.total_filled > 0 {
            lifecycle.terminal_state = TerminalState::PartialFillThenCancel;
            self.stats.partial_then_cancelled += 1;
        } else {
            lifecycle.terminal_state = TerminalState::Cancelled;
            self.stats.cancelled_orders += 1;
        }
        lifecycle.current_size = 0;

        // Move from active to completed
        let completed = self.active.remove(&msg.order_id).unwrap();
        self.stats.active_orders -= 1;
        self.store_completed(completed.clone());

        Some(LifecycleEvent::Completed(completed))
    }

    /// Handle a Fill/Trade message.
    fn handle_fill(&mut self, msg: &MboMessage) -> Option<LifecycleEvent> {
        let ts = msg.timestamp.unwrap_or(0);

        // Get or create lifecycle
        let lifecycle = if let Some(lc) = self.active.get_mut(&msg.order_id) {
            lc
        } else {
            // Pre-existing order - infer if configured
            if !self.config.infer_pre_existing {
                return None;
            }

            // For a fill, we need to be careful:
            // - We don't know the original size
            // - We'll treat msg.size as what's being filled
            // - If it's a full fill, the order completes
            let mut inferred = OrderLifecycle::new_inferred(msg);
            inferred.total_filled = msg.size as u64;
            inferred.fill_count = 1;
            inferred.current_size = 0; // Assume full fill for inferred
            inferred.completion_timestamp = Some(ts);
            inferred.terminal_state = TerminalState::Filled;

            self.stats.inferred_orders += 1;
            self.stats.total_fills += 1;
            self.stats.filled_orders += 1;

            self.store_completed(inferred.clone());

            return Some(LifecycleEvent::Completed(inferred));
        };

        // Update lifecycle
        lifecycle.last_activity_timestamp = ts;
        lifecycle.total_filled += msg.size as u64;
        lifecycle.fill_count += 1;
        self.stats.total_fills += 1;

        // Calculate remaining size
        let filled_size = msg.size.min(lifecycle.current_size);
        lifecycle.current_size = lifecycle.current_size.saturating_sub(filled_size);

        // Check if fully filled
        if lifecycle.current_size == 0 {
            lifecycle.completion_timestamp = Some(ts);
            lifecycle.terminal_state = TerminalState::Filled;
            self.stats.filled_orders += 1;

            // Move from active to completed
            let completed = self.active.remove(&msg.order_id).unwrap();
            self.stats.active_orders -= 1;
            self.store_completed(completed.clone());

            return Some(LifecycleEvent::Completed(completed));
        }

        Some(LifecycleEvent::PartialFill {
            order_id: msg.order_id,
            fill_size: filled_size,
            fill_price: msg.price,
            timestamp: ts,
        })
    }

    /// Store a completed lifecycle, respecting retention limits.
    fn store_completed(&mut self, lifecycle: OrderLifecycle) {
        if self.config.max_completed_retention == 0 {
            return;
        }

        self.completed.push_back(lifecycle);
        self.stats.completed_retained += 1;

        // Evict old entries if over limit
        while self.completed.len() > self.config.max_completed_retention {
            self.completed.pop_front();
            self.stats.completed_evicted += 1;
            self.stats.completed_retained -= 1;
        }
    }

    // ========================================================================
    // Query Methods
    // ========================================================================

    /// Get an active order's lifecycle.
    pub fn get_active(&self, order_id: u64) -> Option<&OrderLifecycle> {
        self.active.get(&order_id)
    }

    /// Get all active lifecycles.
    pub fn active_orders(&self) -> impl Iterator<Item = &OrderLifecycle> {
        self.active.values()
    }

    /// Get recently completed lifecycles.
    pub fn completed_orders(&self) -> impl Iterator<Item = &OrderLifecycle> {
        self.completed.iter()
    }

    /// Get the N most recently completed orders.
    pub fn recent_completed(&self, n: usize) -> impl Iterator<Item = &OrderLifecycle> {
        self.completed.iter().rev().take(n)
    }

    /// Get statistics.
    pub fn stats(&self) -> &LifecycleStats {
        &self.stats
    }

    /// Get current memory usage estimate (in entries, not bytes).
    pub fn memory_usage(&self) -> (usize, usize) {
        (self.active.len(), self.completed.len())
    }

    /// Reset the tracker.
    pub fn reset(&mut self) {
        self.active.clear();
        self.completed.clear();
        self.stats = LifecycleStats::default();
    }

    // ========================================================================
    // Aggregate Features
    // ========================================================================

    /// Calculate aggregate features for active orders.
    pub fn active_order_features(&self) -> ActiveOrderFeatures {
        let mut features = ActiveOrderFeatures::default();

        for lc in self.active.values() {
            features.total_count += 1;

            if lc.is_observed() {
                features.observed_count += 1;
            } else {
                features.inferred_count += 1;
            }

            if lc.side == Side::Bid {
                features.bid_count += 1;
            } else {
                features.ask_count += 1;
            }

            if lc.modification_count > 0 {
                features.modified_count += 1;
                features.total_modifications += lc.modification_count as u64;
            }

            if lc.fill_count > 0 {
                features.partial_filled_count += 1;
            }

            let age = lc.time_alive_ns();
            features.total_age_ns += age as u64;
            features.max_age_ns = features.max_age_ns.max(age as u64);
        }

        if features.total_count > 0 {
            features.avg_age_ns = features.total_age_ns / features.total_count as u64;
        }

        features
    }

    /// Calculate recent completion statistics.
    pub fn recent_completion_stats(&self, n: usize) -> CompletionStats {
        let mut stats = CompletionStats::default();

        for lc in self.completed.iter().rev().take(n) {
            stats.total += 1;

            match lc.terminal_state {
                TerminalState::Cancelled => stats.cancelled += 1,
                TerminalState::Filled => stats.filled += 1,
                TerminalState::PartialFillThenCancel => stats.partial_then_cancel += 1,
                TerminalState::Active => {} // Shouldn't happen
            }

            stats.total_time_alive_ns += lc.time_alive_ns() as u64;
            stats.total_fill_rate += lc.fill_rate();

            if lc.is_observed() {
                stats.observed += 1;
            }
        }

        if stats.total > 0 {
            stats.avg_time_alive_ns = stats.total_time_alive_ns / stats.total as u64;
            stats.avg_fill_rate = stats.total_fill_rate / stats.total as f64;
        }

        stats
    }
}

/// Aggregate features for active orders.
#[derive(Debug, Clone, Default)]
pub struct ActiveOrderFeatures {
    pub total_count: u32,
    pub observed_count: u32,
    pub inferred_count: u32,
    pub bid_count: u32,
    pub ask_count: u32,
    pub modified_count: u32,
    pub partial_filled_count: u32,
    pub total_modifications: u64,
    pub total_age_ns: u64,
    pub avg_age_ns: u64,
    pub max_age_ns: u64,
}

impl ActiveOrderFeatures {
    /// Get bid/ask imbalance in active orders.
    pub fn side_imbalance(&self) -> f64 {
        let total = self.bid_count + self.ask_count;
        if total == 0 {
            return 0.0;
        }
        (self.bid_count as f64 - self.ask_count as f64) / total as f64
    }

    /// Get observation rate for active orders.
    pub fn observation_rate(&self) -> f64 {
        if self.total_count == 0 {
            return 0.0;
        }
        self.observed_count as f64 / self.total_count as f64
    }

    /// Get modification rate (orders with at least one modification).
    pub fn modification_rate(&self) -> f64 {
        if self.total_count == 0 {
            return 0.0;
        }
        self.modified_count as f64 / self.total_count as f64
    }

    /// Get average modifications per modified order.
    pub fn avg_modifications(&self) -> f64 {
        if self.modified_count == 0 {
            return 0.0;
        }
        self.total_modifications as f64 / self.modified_count as f64
    }
}

/// Statistics for recently completed orders.
#[derive(Debug, Clone, Default)]
pub struct CompletionStats {
    pub total: u32,
    pub cancelled: u32,
    pub filled: u32,
    pub partial_then_cancel: u32,
    pub observed: u32,
    pub total_time_alive_ns: u64,
    pub avg_time_alive_ns: u64,
    pub total_fill_rate: f64,
    pub avg_fill_rate: f64,
}

impl CompletionStats {
    /// Get cancellation rate.
    pub fn cancel_rate(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.cancelled as f64 / self.total as f64
    }

    /// Get full fill rate.
    pub fn full_fill_rate(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.filled as f64 / self.total as f64
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(order_id: u64, action: Action, side: Side, price: i64, size: u32, ts: i64) -> MboMessage {
        MboMessage {
            order_id,
            action,
            side,
            price,
            size,
            timestamp: Some(ts),
        }
    }

    // ------------------------------------------------------------------------
    // Configuration Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_config_default() {
        let config = OrderLifecycleConfig::default();
        assert_eq!(config.max_completed_retention, 10_000);
        assert!(config.track_modifications);
        assert!(config.infer_pre_existing);
    }

    #[test]
    fn test_config_minimal() {
        let config = OrderLifecycleConfig::minimal();
        assert_eq!(config.max_completed_retention, 0);
        assert!(!config.track_modifications);
    }

    #[test]
    fn test_config_research() {
        let config = OrderLifecycleConfig::research();
        assert_eq!(config.max_completed_retention, 100_000);
        assert!(config.track_modifications);
    }

    // ------------------------------------------------------------------------
    // Basic Lifecycle Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_simple_add_cancel() {
        let mut tracker = OrderLifecycleTracker::new(OrderLifecycleConfig::default());

        // Add order
        let add_msg = make_msg(1, Action::Add, Side::Bid, 100_000_000_000, 100, 1000);
        let event = tracker.process_message(&add_msg);

        assert!(matches!(event, Some(LifecycleEvent::Created(_))));
        assert_eq!(tracker.stats().observed_orders, 1);
        assert_eq!(tracker.stats().active_orders, 1);

        // Cancel order
        let cancel_msg = make_msg(1, Action::Cancel, Side::Bid, 100_000_000_000, 100, 2000);
        let event = tracker.process_message(&cancel_msg);

        assert!(matches!(event, Some(LifecycleEvent::Completed(_))));
        if let Some(LifecycleEvent::Completed(lc)) = event {
            assert_eq!(lc.order_id, 1);
            assert_eq!(lc.terminal_state, TerminalState::Cancelled);
            assert_eq!(lc.time_alive_ns(), 1000);
            assert!(lc.is_observed());
        }

        assert_eq!(tracker.stats().cancelled_orders, 1);
        assert_eq!(tracker.stats().active_orders, 0);
    }

    #[test]
    fn test_simple_add_fill() {
        let mut tracker = OrderLifecycleTracker::new(OrderLifecycleConfig::default());

        // Add order
        let add_msg = make_msg(1, Action::Add, Side::Ask, 100_000_000_000, 100, 1000);
        tracker.process_message(&add_msg);

        // Full fill
        let fill_msg = make_msg(1, Action::Fill, Side::Ask, 100_000_000_000, 100, 2000);
        let event = tracker.process_message(&fill_msg);

        assert!(matches!(event, Some(LifecycleEvent::Completed(_))));
        if let Some(LifecycleEvent::Completed(lc)) = event {
            assert_eq!(lc.terminal_state, TerminalState::Filled);
            assert_eq!(lc.total_filled, 100);
            assert_eq!(lc.fill_count, 1);
            assert!((lc.fill_rate() - 1.0).abs() < 0.001);
        }

        assert_eq!(tracker.stats().filled_orders, 1);
    }

    #[test]
    fn test_partial_fill_then_cancel() {
        let mut tracker = OrderLifecycleTracker::new(OrderLifecycleConfig::default());

        // Add order
        let add_msg = make_msg(1, Action::Add, Side::Bid, 100_000_000_000, 100, 1000);
        tracker.process_message(&add_msg);

        // Partial fill
        let fill_msg = make_msg(1, Action::Fill, Side::Bid, 100_000_000_000, 40, 2000);
        let event = tracker.process_message(&fill_msg);

        assert!(matches!(event, Some(LifecycleEvent::PartialFill { .. })));

        // Check lifecycle state
        let lc = tracker.get_active(1).unwrap();
        assert_eq!(lc.current_size, 60);
        assert_eq!(lc.total_filled, 40);

        // Cancel remaining
        let cancel_msg = make_msg(1, Action::Cancel, Side::Bid, 100_000_000_000, 60, 3000);
        let event = tracker.process_message(&cancel_msg);

        if let Some(LifecycleEvent::Completed(lc)) = event {
            assert_eq!(lc.terminal_state, TerminalState::PartialFillThenCancel);
            assert_eq!(lc.total_filled, 40);
            assert!((lc.fill_rate() - 0.4).abs() < 0.001);
        }

        assert_eq!(tracker.stats().partial_then_cancelled, 1);
    }

    #[test]
    fn test_multiple_partial_fills() {
        let mut tracker = OrderLifecycleTracker::new(OrderLifecycleConfig::default());

        // Add order
        let add_msg = make_msg(1, Action::Add, Side::Ask, 100_000_000_000, 100, 1000);
        tracker.process_message(&add_msg);

        // Three partial fills
        tracker.process_message(&make_msg(1, Action::Fill, Side::Ask, 100_000_000_000, 30, 2000));
        tracker.process_message(&make_msg(1, Action::Fill, Side::Ask, 100_000_000_000, 30, 3000));

        let lc = tracker.get_active(1).unwrap();
        assert_eq!(lc.current_size, 40);
        assert_eq!(lc.fill_count, 2);

        // Final fill
        let event = tracker.process_message(&make_msg(1, Action::Fill, Side::Ask, 100_000_000_000, 40, 4000));

        if let Some(LifecycleEvent::Completed(lc)) = event {
            assert_eq!(lc.terminal_state, TerminalState::Filled);
            assert_eq!(lc.fill_count, 3);
            assert_eq!(lc.total_filled, 100);
        }
    }

    // ------------------------------------------------------------------------
    // Modification Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_order_modification() {
        let mut tracker = OrderLifecycleTracker::new(OrderLifecycleConfig::default());

        // Add order
        let add_msg = make_msg(1, Action::Add, Side::Bid, 100_000_000_000, 100, 1000);
        tracker.process_message(&add_msg);

        // Modify order (price change)
        let modify_msg = make_msg(1, Action::Modify, Side::Bid, 101_000_000_000, 100, 2000);
        let event = tracker.process_message(&modify_msg);

        assert!(matches!(event, Some(LifecycleEvent::Modified { .. })));
        if let Some(LifecycleEvent::Modified { modification, .. }) = event {
            assert!(modification.is_price_change());
            assert!(!modification.is_size_change());
            assert_eq!(modification.price_change(), 1_000_000_000);
        }

        let lc = tracker.get_active(1).unwrap();
        assert_eq!(lc.modification_count, 1);
        assert_eq!(lc.modifications.len(), 1);
        assert!(lc.had_price_change());
    }

    #[test]
    fn test_modification_limit() {
        let config = OrderLifecycleConfig::default()
            .with_modification_tracking(true);
        let mut config = config;
        config.max_modifications_per_order = 3;

        let mut tracker = OrderLifecycleTracker::new(config);

        // Add order
        tracker.process_message(&make_msg(1, Action::Add, Side::Bid, 100_000_000_000, 100, 1000));

        // 5 modifications
        for i in 1..=5 {
            tracker.process_message(&make_msg(
                1,
                Action::Modify,
                Side::Bid,
                100_000_000_000 + i * 1_000_000_000,
                100,
                1000 + i * 1000,
            ));
        }

        let lc = tracker.get_active(1).unwrap();
        assert_eq!(lc.modification_count, 5);
        assert_eq!(lc.modifications.len(), 3); // Limited

        assert_eq!(tracker.stats().modifications_dropped, 2);
    }

    // ------------------------------------------------------------------------
    // Pre-Existing Order Tests (Critical)
    // ------------------------------------------------------------------------

    #[test]
    fn test_inferred_order_on_cancel() {
        let mut tracker = OrderLifecycleTracker::new(OrderLifecycleConfig::default());

        // Cancel for unknown order (pre-existing)
        let cancel_msg = make_msg(999, Action::Cancel, Side::Bid, 100_000_000_000, 50, 1000);
        let event = tracker.process_message(&cancel_msg);

        assert!(matches!(event, Some(LifecycleEvent::Completed(_))));
        if let Some(LifecycleEvent::Completed(lc)) = event {
            assert_eq!(lc.origin, OrderOrigin::Inferred);
            assert_eq!(lc.terminal_state, TerminalState::Cancelled);
        }

        assert_eq!(tracker.stats().inferred_orders, 1);
        assert_eq!(tracker.stats().observed_orders, 0);
    }

    #[test]
    fn test_inferred_order_on_fill() {
        let mut tracker = OrderLifecycleTracker::new(OrderLifecycleConfig::default());

        // Fill for unknown order (pre-existing)
        let fill_msg = make_msg(999, Action::Fill, Side::Ask, 100_000_000_000, 50, 1000);
        let event = tracker.process_message(&fill_msg);

        assert!(matches!(event, Some(LifecycleEvent::Completed(_))));
        if let Some(LifecycleEvent::Completed(lc)) = event {
            assert_eq!(lc.origin, OrderOrigin::Inferred);
            assert_eq!(lc.terminal_state, TerminalState::Filled);
        }

        assert_eq!(tracker.stats().inferred_orders, 1);
    }

    #[test]
    fn test_inferred_order_on_modify() {
        let mut tracker = OrderLifecycleTracker::new(OrderLifecycleConfig::default());

        // Modify for unknown order (pre-existing)
        let modify_msg = make_msg(999, Action::Modify, Side::Bid, 100_000_000_000, 50, 1000);
        let event = tracker.process_message(&modify_msg);

        // Should create a new lifecycle
        assert!(matches!(event, Some(LifecycleEvent::Created(_))));
        if let Some(LifecycleEvent::Created(lc)) = event {
            assert_eq!(lc.origin, OrderOrigin::Inferred);
            assert!(lc.is_active());
        }

        assert_eq!(tracker.stats().inferred_orders, 1);
        assert_eq!(tracker.stats().active_orders, 1);
    }

    #[test]
    fn test_inference_disabled() {
        let config = OrderLifecycleConfig::default().with_inference(false);
        let mut tracker = OrderLifecycleTracker::new(config);

        // Cancel for unknown order - should be ignored
        let cancel_msg = make_msg(999, Action::Cancel, Side::Bid, 100_000_000_000, 50, 1000);
        let event = tracker.process_message(&cancel_msg);

        assert!(event.is_none());
        assert_eq!(tracker.stats().inferred_orders, 0);
    }

    // ------------------------------------------------------------------------
    // System Message Handling
    // ------------------------------------------------------------------------

    #[test]
    fn test_system_messages_skipped() {
        let mut tracker = OrderLifecycleTracker::new(OrderLifecycleConfig::default());

        // order_id = 0
        let msg1 = make_msg(0, Action::Add, Side::Bid, 100_000_000_000, 100, 1000);
        assert!(tracker.process_message(&msg1).is_none());

        // size = 0
        let msg2 = make_msg(1, Action::Add, Side::Bid, 100_000_000_000, 0, 1000);
        assert!(tracker.process_message(&msg2).is_none());

        // price <= 0
        let msg3 = make_msg(1, Action::Add, Side::Bid, 0, 100, 1000);
        assert!(tracker.process_message(&msg3).is_none());

        // Side::None
        let msg4 = make_msg(1, Action::Add, Side::None, 100_000_000_000, 100, 1000);
        assert!(tracker.process_message(&msg4).is_none());

        assert_eq!(tracker.stats().messages_skipped, 4);
    }

    // ------------------------------------------------------------------------
    // Retention Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_completed_retention_limit() {
        let config = OrderLifecycleConfig::default().with_max_completed(3);
        let mut tracker = OrderLifecycleTracker::new(config);

        // Add and cancel 5 orders
        for i in 1..=5u64 {
            tracker.process_message(&make_msg(i, Action::Add, Side::Bid, 100_000_000_000, 100, i as i64 * 1000));
            tracker.process_message(&make_msg(i, Action::Cancel, Side::Bid, 100_000_000_000, 100, i as i64 * 1000 + 500));
        }

        // Should only retain 3
        let (active, completed) = tracker.memory_usage();
        assert_eq!(active, 0);
        assert_eq!(completed, 3);

        assert_eq!(tracker.stats().completed_evicted, 2);

        // Verify we kept the most recent
        let recent: Vec<_> = tracker.recent_completed(10).collect();
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].order_id, 5);
        assert_eq!(recent[1].order_id, 4);
        assert_eq!(recent[2].order_id, 3);
    }

    #[test]
    fn test_no_retention() {
        let config = OrderLifecycleConfig::minimal();
        let mut tracker = OrderLifecycleTracker::new(config);

        // Add and cancel
        tracker.process_message(&make_msg(1, Action::Add, Side::Bid, 100_000_000_000, 100, 1000));
        tracker.process_message(&make_msg(1, Action::Cancel, Side::Bid, 100_000_000_000, 100, 2000));

        let (_, completed) = tracker.memory_usage();
        assert_eq!(completed, 0);
    }

    // ------------------------------------------------------------------------
    // Feature Calculation Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_active_order_features() {
        let mut tracker = OrderLifecycleTracker::new(OrderLifecycleConfig::default());

        // Add some orders
        tracker.process_message(&make_msg(1, Action::Add, Side::Bid, 100_000_000_000, 100, 1000));
        tracker.process_message(&make_msg(2, Action::Add, Side::Ask, 101_000_000_000, 50, 2000));
        tracker.process_message(&make_msg(3, Action::Add, Side::Bid, 99_000_000_000, 75, 3000));

        // Modify one
        tracker.process_message(&make_msg(1, Action::Modify, Side::Bid, 100_500_000_000, 100, 4000));

        let features = tracker.active_order_features();

        assert_eq!(features.total_count, 3);
        assert_eq!(features.observed_count, 3);
        assert_eq!(features.bid_count, 2);
        assert_eq!(features.ask_count, 1);
        assert_eq!(features.modified_count, 1);

        // Imbalance: (2 - 1) / 3 = 0.333...
        assert!((features.side_imbalance() - 0.333).abs() < 0.01);
    }

    #[test]
    fn test_completion_stats() {
        let mut tracker = OrderLifecycleTracker::new(OrderLifecycleConfig::default());

        // Add and complete some orders
        for i in 1..=5u64 {
            tracker.process_message(&make_msg(i, Action::Add, Side::Bid, 100_000_000_000, 100, i as i64 * 1000));

            if i % 2 == 0 {
                // Fill
                tracker.process_message(&make_msg(i, Action::Fill, Side::Bid, 100_000_000_000, 100, i as i64 * 1000 + 500));
            } else {
                // Cancel
                tracker.process_message(&make_msg(i, Action::Cancel, Side::Bid, 100_000_000_000, 100, i as i64 * 1000 + 500));
            }
        }

        let stats = tracker.recent_completion_stats(10);

        assert_eq!(stats.total, 5);
        assert_eq!(stats.filled, 2);
        assert_eq!(stats.cancelled, 3);
        assert!((stats.full_fill_rate() - 0.4).abs() < 0.01);
    }

    // ------------------------------------------------------------------------
    // Reset Test
    // ------------------------------------------------------------------------

    #[test]
    fn test_reset() {
        let mut tracker = OrderLifecycleTracker::new(OrderLifecycleConfig::default());

        tracker.process_message(&make_msg(1, Action::Add, Side::Bid, 100_000_000_000, 100, 1000));
        tracker.process_message(&make_msg(1, Action::Cancel, Side::Bid, 100_000_000_000, 100, 2000));

        assert!(tracker.stats().total_orders() > 0);

        tracker.reset();

        assert_eq!(tracker.stats().total_orders(), 0);
        assert_eq!(tracker.memory_usage(), (0, 0));
    }
}

