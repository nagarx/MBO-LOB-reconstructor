//! Single-symbol LOB reconstructor.
//!
//! High-performance implementation using:
//! - BTreeMap for sorted price levels
//! - ahash HashMap for fast order lookups
//! - Cached best bid/ask to avoid recomputation
//! - Minimal allocations on hot path

use ahash::AHashMap;
use std::collections::BTreeMap;

use crate::error::{Result, TlobError};
use crate::types::{Action, BookConsistency, LobState, MboMessage, Order, Side};

/// How to handle crossed quotes (bid >= ask) when they occur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CrossedQuotePolicy {
    /// Allow crossed quotes (default) - just track in stats
    #[default]
    Allow,

    /// Return the last valid state when a crossed quote would occur
    UseLastValid,

    /// Return an error when a crossed quote would occur
    Error,

    /// Skip the update entirely (don't change the book state)
    SkipUpdate,
}

/// Configuration for LOB reconstructor behavior.
#[derive(Debug, Clone)]
pub struct LobConfig {
    /// Number of price levels to track
    pub levels: usize,

    /// How to handle crossed quotes
    pub crossed_quote_policy: CrossedQuotePolicy,

    /// Whether to validate messages before processing
    pub validate_messages: bool,

    /// Whether to log warnings for consistency issues
    pub log_warnings: bool,
}

impl Default for LobConfig {
    fn default() -> Self {
        Self {
            levels: 10,
            crossed_quote_policy: CrossedQuotePolicy::Allow,
            validate_messages: true,
            log_warnings: true,
        }
    }
}

impl LobConfig {
    /// Create a new config with specified number of levels.
    pub fn new(levels: usize) -> Self {
        Self {
            levels,
            ..Default::default()
        }
    }

    /// Set crossed quote handling policy.
    pub fn with_crossed_quote_policy(mut self, policy: CrossedQuotePolicy) -> Self {
        self.crossed_quote_policy = policy;
        self
    }

    /// Enable/disable message validation.
    pub fn with_validation(mut self, validate: bool) -> Self {
        self.validate_messages = validate;
        self
    }

    /// Enable/disable warning logs.
    pub fn with_logging(mut self, log: bool) -> Self {
        self.log_warnings = log;
        self
    }
}

/// Single-symbol LOB reconstructor.
///
/// This maintains the full order book state and processes MBO messages
/// to keep it updated. Design goals:
/// - Fast message processing (target: <20 μs per message)
/// - Minimal memory footprint
/// - Accurate state tracking
/// - Easy to test and debug
#[derive(Debug, Clone)]
pub struct LobReconstructor {
    /// Configuration
    config: LobConfig,

    /// Bid orders: price -> (order_id -> size)
    /// BTreeMap keeps prices sorted (highest first for bids)
    bids: BTreeMap<i64, AHashMap<u64, u32>>,

    /// Ask orders: price -> (order_id -> size)
    /// BTreeMap keeps prices sorted (lowest first for asks)
    asks: BTreeMap<i64, AHashMap<u64, u32>>,

    /// Order tracking: order_id -> Order
    /// Fast lookup for modify/cancel/trade operations
    orders: AHashMap<u64, Order>,

    /// Cached best bid (highest bid price)
    best_bid: Option<i64>,

    /// Cached best ask (lowest ask price)
    best_ask: Option<i64>,

    /// Statistics (for monitoring)
    stats: LobStats,

    /// Last valid LOB state (for UseLastValid policy)
    last_valid_state: Option<LobState>,
}

/// Statistics for monitoring LOB health.
#[derive(Debug, Clone, Default)]
pub struct LobStats {
    /// Total messages processed
    pub messages_processed: u64,

    /// Number of active orders
    pub active_orders: usize,

    /// Number of price levels (bid side)
    pub bid_levels: usize,

    /// Number of price levels (ask side)
    pub ask_levels: usize,

    /// Total errors encountered
    pub errors: u64,

    /// Number of crossed quotes detected (bid >= ask)
    pub crossed_quotes: u64,

    /// Number of locked quotes detected (bid == ask)
    pub locked_quotes: u64,

    /// Last timestamp processed (nanoseconds since epoch)
    pub last_timestamp: Option<i64>,
}

impl LobReconstructor {
    /// Create a new LOB reconstructor.
    ///
    /// # Arguments
    /// * `levels` - Number of price levels to track (e.g., 10)
    ///
    /// # Example
    /// ```
    /// use mbo_lob_reconstructor::LobReconstructor;
    ///
    /// let lob = LobReconstructor::new(10);
    /// ```
    pub fn new(levels: usize) -> Self {
        Self::with_config(LobConfig::new(levels))
    }

    /// Create a new LOB reconstructor with custom configuration.
    ///
    /// # Arguments
    /// * `config` - Configuration for LOB behavior
    ///
    /// # Example
    /// ```
    /// use mbo_lob_reconstructor::{LobReconstructor, LobConfig, CrossedQuotePolicy};
    ///
    /// let config = LobConfig::new(10)
    ///     .with_crossed_quote_policy(CrossedQuotePolicy::UseLastValid);
    /// let lob = LobReconstructor::with_config(config);
    /// ```
    pub fn with_config(config: LobConfig) -> Self {
        Self {
            config,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            orders: AHashMap::new(),
            best_bid: None,
            best_ask: None,
            stats: LobStats::default(),
            last_valid_state: None,
        }
    }

    /// Get the number of price levels being tracked.
    #[inline]
    pub fn levels(&self) -> usize {
        self.config.levels
    }

    /// Get a reference to the current configuration.
    #[inline]
    pub fn config(&self) -> &LobConfig {
        &self.config
    }

    /// Process a single MBO message and update LOB state.
    ///
    /// This is the main entry point for LOB updates. It:
    /// 1. Validates the message (if enabled in config)
    /// 2. Updates internal state based on action
    /// 3. Updates cached best prices
    /// 4. Checks for book consistency (crossed/locked quotes)
    /// 5. Applies crossed quote policy
    /// 6. Returns current LOB snapshot
    ///
    /// # Arguments
    /// * `msg` - The MBO message to process
    ///
    /// # Returns
    /// Current LOB state after processing the message
    ///
    /// # Errors
    /// Returns error if message is invalid or causes inconsistent state
    /// (depending on configuration)
    #[inline]
    pub fn process_message(&mut self, msg: &MboMessage) -> Result<LobState> {
        // Validate message (if enabled)
        if self.config.validate_messages {
            msg.validate()?;
        }

        // Process based on action
        match msg.action {
            Action::Add => self.add_order(msg)?,
            Action::Modify => self.modify_order(msg)?,
            Action::Cancel => self.cancel_order(msg)?,
            Action::Trade | Action::Fill => self.process_trade(msg)?,
        }

        // Update statistics
        self.stats.messages_processed += 1;
        self.stats.active_orders = self.orders.len();
        self.stats.bid_levels = self.bids.len();
        self.stats.ask_levels = self.asks.len();

        // Track timestamp
        if let Some(ts) = msg.timestamp {
            self.stats.last_timestamp = Some(ts);
        }

        // Update best prices
        self.update_best_prices();

        // Check for book consistency and apply policy
        let consistency = self.check_consistency();
        self.track_consistency(consistency);

        // Apply crossed quote policy
        self.apply_crossed_quote_policy(consistency, msg.timestamp)
    }

    /// Check book consistency and return the status.
    #[inline]
    fn check_consistency(&self) -> BookConsistency {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => {
                if bid < ask {
                    BookConsistency::Valid
                } else if bid == ask {
                    BookConsistency::Locked
                } else {
                    BookConsistency::Crossed
                }
            }
            _ => BookConsistency::Empty,
        }
    }

    /// Track consistency in statistics and optionally log.
    #[inline]
    fn track_consistency(&mut self, consistency: BookConsistency) {
        match consistency {
            BookConsistency::Valid => {
                // Update last valid state
                let state = self.get_lob_state();
                self.last_valid_state = Some(state);
            }
            BookConsistency::Crossed => {
                self.stats.crossed_quotes += 1;
                if self.config.log_warnings {
                    if let (Some(bid), Some(ask)) = (self.best_bid, self.best_ask) {
                        log::warn!(
                            "Crossed quote detected: bid={:.4} > ask={:.4} (message #{})",
                            bid as f64 / 1e9,
                            ask as f64 / 1e9,
                            self.stats.messages_processed
                        );
                    }
                }
            }
            BookConsistency::Locked => {
                self.stats.locked_quotes += 1;
                if self.config.log_warnings {
                    if let Some(bid) = self.best_bid {
                        log::debug!(
                            "Locked quote detected: bid=ask={:.4} (message #{})",
                            bid as f64 / 1e9,
                            self.stats.messages_processed
                        );
                    }
                }
            }
            BookConsistency::Empty => {}
        }
    }

    /// Apply the configured crossed quote policy.
    #[inline]
    fn apply_crossed_quote_policy(
        &self,
        consistency: BookConsistency,
        timestamp: Option<i64>,
    ) -> Result<LobState> {
        // For valid or empty states, always return current state
        if consistency == BookConsistency::Valid || consistency == BookConsistency::Empty {
            return Ok(self.get_lob_state_with_metadata(timestamp));
        }

        // For crossed or locked states, apply policy
        match self.config.crossed_quote_policy {
            CrossedQuotePolicy::Allow => {
                // Return the crossed/locked state as-is
                Ok(self.get_lob_state_with_metadata(timestamp))
            }
            CrossedQuotePolicy::UseLastValid => {
                // Return the last known valid state
                Ok(self
                    .last_valid_state
                    .clone()
                    .unwrap_or_else(|| self.get_lob_state_with_metadata(timestamp)))
            }
            CrossedQuotePolicy::Error => {
                // Return an error
                if let (Some(bid), Some(ask)) = (self.best_bid, self.best_ask) {
                    if bid > ask {
                        Err(TlobError::CrossedQuote(bid, ask))
                    } else {
                        Err(TlobError::LockedQuote(bid, ask))
                    }
                } else {
                    Ok(self.get_lob_state_with_metadata(timestamp))
                }
            }
            CrossedQuotePolicy::SkipUpdate => {
                // Return the last valid state (same as UseLastValid in effect)
                Ok(self
                    .last_valid_state
                    .clone()
                    .unwrap_or_else(|| self.get_lob_state_with_metadata(timestamp)))
            }
        }
    }

    /// Check if the current book state is consistent (bid < ask).
    ///
    /// # Returns
    /// - `true` if book is valid (bid < ask) or empty
    /// - `false` if book is crossed (bid > ask) or locked (bid == ask)
    #[inline]
    pub fn is_consistent(&self) -> bool {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => bid < ask,
            _ => true, // Empty book is considered consistent
        }
    }

    /// Check if the book is crossed (bid > ask).
    #[inline]
    pub fn is_crossed(&self) -> bool {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => bid > ask,
            _ => false,
        }
    }

    /// Add a new order to the book.
    #[inline]
    fn add_order(&mut self, msg: &MboMessage) -> Result<()> {
        // Check if order already exists
        if self.orders.contains_key(&msg.order_id) {
            // Some exchanges reuse order IDs, treat as modify
            return self.modify_order(msg);
        }

        // Add to appropriate side
        let price_level = match msg.side {
            Side::Bid => self.bids.entry(msg.price).or_default(),
            Side::Ask => self.asks.entry(msg.price).or_default(),
            Side::None => {
                // Non-directional orders are ignored
                return Ok(());
            }
        };

        // Insert order at price level
        price_level.insert(msg.order_id, msg.size);

        // Track order
        self.orders.insert(
            msg.order_id,
            Order {
                side: msg.side,
                price: msg.price,
                size: msg.size,
            },
        );

        Ok(())
    }

    /// Modify an existing order.
    #[inline]
    fn modify_order(&mut self, msg: &MboMessage) -> Result<()> {
        // Get existing order
        let old_order = match self.orders.get(&msg.order_id) {
            Some(order) => *order,
            None => {
                // Order not found, treat as add
                return self.add_order(msg);
            }
        };

        // Remove old order
        self.remove_order_internal(msg.order_id, &old_order)?;

        // Add as new order
        self.add_order(msg)?;

        Ok(())
    }

    /// Cancel (remove) an order from the book.
    #[inline]
    fn cancel_order(&mut self, msg: &MboMessage) -> Result<()> {
        // Get existing order
        let order = match self.orders.get(&msg.order_id) {
            Some(order) => *order,
            None => {
                // Order not found, might have been already cancelled
                // This is not necessarily an error in real markets
                return Ok(());
            }
        };

        // Remove order
        self.remove_order_internal(msg.order_id, &order)?;

        Ok(())
    }

    /// Process a trade (execution).
    ///
    /// Trade reduces order size or removes it completely.
    #[inline]
    fn process_trade(&mut self, msg: &MboMessage) -> Result<()> {
        // Get existing order
        let mut order = match self.orders.get(&msg.order_id) {
            Some(order) => *order,
            None => {
                // Trade for unknown order, ignore
                return Ok(());
            }
        };

        // Get price level
        let price_level = match order.side {
            Side::Bid => self.bids.get_mut(&order.price),
            Side::Ask => self.asks.get_mut(&order.price),
            Side::None => return Ok(()),
        };

        let price_level = match price_level {
            Some(level) => level,
            None => {
                // Price level doesn't exist, inconsistent state
                self.stats.errors += 1;
                return Err(TlobError::InconsistentState(format!(
                    "Price level {} not found for order {}",
                    order.price, msg.order_id
                )));
            }
        };

        // Get order at price level
        let order_size = match price_level.get_mut(&msg.order_id) {
            Some(size) => size,
            None => {
                // Order not at price level, inconsistent state
                self.stats.errors += 1;
                return Err(TlobError::InconsistentState(format!(
                    "Order {} not found at price level {}",
                    msg.order_id, order.price
                )));
            }
        };

        // Reduce order size
        if msg.size >= *order_size {
            // Full fill - remove order
            price_level.remove(&msg.order_id);

            // Remove empty price level
            if price_level.is_empty() {
                match order.side {
                    Side::Bid => {
                        self.bids.remove(&order.price);
                    }
                    Side::Ask => {
                        self.asks.remove(&order.price);
                    }
                    Side::None => {}
                }
            }

            // Remove from order tracking
            self.orders.remove(&msg.order_id);
        } else {
            // Partial fill - reduce size
            *order_size -= msg.size;
            order.size -= msg.size;
            self.orders.insert(msg.order_id, order);
        }

        Ok(())
    }

    /// Internal helper to remove an order.
    #[inline(always)]
    fn remove_order_internal(&mut self, order_id: u64, order: &Order) -> Result<()> {
        // Get price level
        let price_level = match order.side {
            Side::Bid => self.bids.get_mut(&order.price),
            Side::Ask => self.asks.get_mut(&order.price),
            Side::None => return Ok(()),
        };

        if let Some(price_level) = price_level {
            // Remove order from price level
            price_level.remove(&order_id);

            // Remove empty price level
            if price_level.is_empty() {
                match order.side {
                    Side::Bid => {
                        self.bids.remove(&order.price);
                    }
                    Side::Ask => {
                        self.asks.remove(&order.price);
                    }
                    Side::None => {}
                }
            }
        }

        // Remove from order tracking
        self.orders.remove(&order_id);

        Ok(())
    }

    /// Update cached best bid and ask prices.
    ///
    /// BTreeMap keeps prices sorted, so we can efficiently get min/max.
    #[inline(always)]
    fn update_best_prices(&mut self) {
        // Best bid = highest bid price (BTreeMap iter is ascending, use last)
        self.best_bid = self.bids.keys().next_back().copied();

        // Best ask = lowest ask price (BTreeMap iter is ascending, use first)
        self.best_ask = self.asks.keys().next().copied();
    }

    /// Get current LOB state snapshot (without metadata).
    ///
    /// This creates a snapshot of the top N levels on each side.
    /// For most use cases, prefer `get_lob_state_with_metadata` which includes
    /// timestamp and sequence information.
    #[inline]
    pub fn get_lob_state(&self) -> LobState {
        self.get_lob_state_with_metadata(None)
    }

    /// Get current LOB state snapshot with metadata.
    ///
    /// This creates a snapshot of the top N levels on each side,
    /// including timestamp and message sequence number.
    ///
    /// # Arguments
    /// * `timestamp` - Optional timestamp from the message that triggered this snapshot
    #[inline]
    pub fn get_lob_state_with_metadata(&self, timestamp: Option<i64>) -> LobState {
        let levels = self.config.levels;
        let mut state = LobState::new(levels);

        // Best prices
        state.best_bid = self.best_bid;
        state.best_ask = self.best_ask;

        // Metadata
        state.timestamp = timestamp.or(self.stats.last_timestamp);
        state.sequence = self.stats.messages_processed;

        // Bid side (top N, highest to lowest)
        for (i, (&price, orders)) in self.bids.iter().rev().take(levels).enumerate() {
            state.bid_prices[i] = price;
            state.bid_sizes[i] = orders.values().sum();
        }

        // Ask side (top N, lowest to highest)
        for (i, (&price, orders)) in self.asks.iter().take(levels).enumerate() {
            state.ask_prices[i] = price;
            state.ask_sizes[i] = orders.values().sum();
        }

        state
    }

    /// Reset the LOB to empty state.
    pub fn reset(&mut self) {
        self.bids.clear();
        self.asks.clear();
        self.orders.clear();
        self.best_bid = None;
        self.best_ask = None;
        self.stats = LobStats::default();
        self.last_valid_state = None;
    }

    /// Get current statistics.
    pub fn stats(&self) -> &LobStats {
        &self.stats
    }

    /// Get number of active orders.
    pub fn order_count(&self) -> usize {
        self.orders.len()
    }

    /// Get number of price levels on bid side.
    pub fn bid_levels(&self) -> usize {
        self.bids.len()
    }

    /// Get number of price levels on ask side.
    pub fn ask_levels(&self) -> usize {
        self.asks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_message(
        order_id: u64,
        action: Action,
        side: Side,
        price_dollars: f64,
        size: u32,
    ) -> MboMessage {
        MboMessage::new(order_id, action, side, (price_dollars * 1e9) as i64, size)
    }

    #[test]
    fn test_new_lob() {
        let lob = LobReconstructor::new(10);
        assert_eq!(lob.order_count(), 0);
        assert_eq!(lob.bid_levels(), 0);
        assert_eq!(lob.ask_levels(), 0);
    }

    #[test]
    fn test_add_bid_order() {
        let mut lob = LobReconstructor::new(10);

        let msg = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        let state = lob.process_message(&msg).unwrap();

        assert_eq!(lob.order_count(), 1);
        assert_eq!(lob.bid_levels(), 1);
        assert_eq!(state.best_bid, Some(100_000_000_000));
        assert_eq!(state.bid_prices[0], 100_000_000_000);
        assert_eq!(state.bid_sizes[0], 100);
    }

    #[test]
    fn test_add_ask_order() {
        let mut lob = LobReconstructor::new(10);

        let msg = create_test_message(1, Action::Add, Side::Ask, 100.01, 200);
        let state = lob.process_message(&msg).unwrap();

        assert_eq!(lob.order_count(), 1);
        assert_eq!(lob.ask_levels(), 1);
        assert_eq!(state.best_ask, Some(100_010_000_000));
        assert_eq!(state.ask_prices[0], 100_010_000_000);
        assert_eq!(state.ask_sizes[0], 200);
    }

    #[test]
    fn test_bid_ask_spread() {
        let mut lob = LobReconstructor::new(10);

        // Add bid
        let bid = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        lob.process_message(&bid).unwrap();

        // Add ask
        let ask = create_test_message(2, Action::Add, Side::Ask, 100.01, 200);
        let state = lob.process_message(&ask).unwrap();

        // Check spread
        assert!(state.is_valid());
        assert!((state.mid_price().unwrap() - 100.005).abs() < 1e-6);
        assert!((state.spread().unwrap() - 0.01).abs() < 1e-6);
    }

    #[test]
    fn test_cancel_order() {
        let mut lob = LobReconstructor::new(10);

        // Add order
        let add = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        lob.process_message(&add).unwrap();
        assert_eq!(lob.order_count(), 1);

        // Cancel order
        let cancel = create_test_message(1, Action::Cancel, Side::Bid, 100.0, 100);
        lob.process_message(&cancel).unwrap();
        assert_eq!(lob.order_count(), 0);
        assert_eq!(lob.bid_levels(), 0);
    }

    #[test]
    fn test_modify_order() {
        let mut lob = LobReconstructor::new(10);

        // Add order
        let add = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        lob.process_message(&add).unwrap();

        // Modify order (change price)
        let modify = create_test_message(1, Action::Modify, Side::Bid, 100.01, 150);
        let state = lob.process_message(&modify).unwrap();

        assert_eq!(lob.order_count(), 1);
        assert_eq!(state.best_bid, Some(100_010_000_000));
        assert_eq!(state.bid_sizes[0], 150);
    }

    #[test]
    fn test_trade_partial_fill() {
        let mut lob = LobReconstructor::new(10);

        // Add order
        let add = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        lob.process_message(&add).unwrap();

        // Partial fill (50 shares)
        let trade = create_test_message(1, Action::Trade, Side::Bid, 100.0, 50);
        let state = lob.process_message(&trade).unwrap();

        assert_eq!(lob.order_count(), 1); // Order still exists
        assert_eq!(state.bid_sizes[0], 50); // Reduced size
    }

    #[test]
    fn test_trade_full_fill() {
        let mut lob = LobReconstructor::new(10);

        // Add order
        let add = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        lob.process_message(&add).unwrap();

        // Full fill
        let trade = create_test_message(1, Action::Trade, Side::Bid, 100.0, 100);
        lob.process_message(&trade).unwrap();

        assert_eq!(lob.order_count(), 0); // Order removed
        assert_eq!(lob.bid_levels(), 0); // Price level removed
    }

    #[test]
    fn test_multiple_orders_same_price() {
        let mut lob = LobReconstructor::new(10);

        // Add multiple orders at same price
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();
        lob.process_message(&create_test_message(2, Action::Add, Side::Bid, 100.0, 200))
            .unwrap();
        lob.process_message(&create_test_message(3, Action::Add, Side::Bid, 100.0, 300))
            .unwrap();

        let state = lob.get_lob_state();

        assert_eq!(lob.order_count(), 3);
        assert_eq!(lob.bid_levels(), 1); // All at same price
        assert_eq!(state.bid_sizes[0], 600); // Aggregated size
    }

    #[test]
    fn test_reset() {
        let mut lob = LobReconstructor::new(10);

        // Add some orders
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();
        lob.process_message(&create_test_message(2, Action::Add, Side::Ask, 100.01, 200))
            .unwrap();

        assert_eq!(lob.order_count(), 2);

        // Reset
        lob.reset();

        assert_eq!(lob.order_count(), 0);
        assert_eq!(lob.bid_levels(), 0);
        assert_eq!(lob.ask_levels(), 0);
    }

    #[test]
    fn test_statistics() {
        let mut lob = LobReconstructor::new(10);

        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();
        lob.process_message(&create_test_message(2, Action::Add, Side::Ask, 100.01, 200))
            .unwrap();

        let stats = lob.stats();
        assert_eq!(stats.messages_processed, 2);
        assert_eq!(stats.active_orders, 2);
        assert_eq!(stats.bid_levels, 1);
        assert_eq!(stats.ask_levels, 1);
    }

    // =========================================================================
    // Crossed Quote Policy Tests
    // =========================================================================

    #[test]
    fn test_crossed_quote_detection() {
        let mut lob = LobReconstructor::new(10);

        // Add valid bid
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();

        // Add ask BELOW bid (creates crossed quote)
        lob.process_message(&create_test_message(2, Action::Add, Side::Ask, 99.99, 200))
            .unwrap();

        // Book should be crossed
        assert!(lob.is_crossed());
        assert!(!lob.is_consistent());

        // Stats should show crossed quote
        let stats = lob.stats();
        assert_eq!(stats.crossed_quotes, 1);
    }

    #[test]
    fn test_locked_quote_detection() {
        let mut lob = LobReconstructor::new(10);

        // Add bid
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();

        // Add ask at SAME price as bid (creates locked quote)
        lob.process_message(&create_test_message(2, Action::Add, Side::Ask, 100.0, 200))
            .unwrap();

        // Stats should show locked quote
        let stats = lob.stats();
        assert_eq!(stats.locked_quotes, 1);
    }

    #[test]
    fn test_crossed_quote_policy_allow() {
        let config = LobConfig::new(10)
            .with_crossed_quote_policy(CrossedQuotePolicy::Allow)
            .with_logging(false);
        let mut lob = LobReconstructor::with_config(config);

        // Create crossed book
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();
        let state = lob
            .process_message(&create_test_message(2, Action::Add, Side::Ask, 99.99, 200))
            .unwrap();

        // Should return the crossed state (Allow policy)
        assert!(state.is_crossed());
        assert_eq!(state.best_bid, Some(100_000_000_000)); // $100.00
        assert_eq!(state.best_ask, Some(99_990_000_000)); // $99.99
    }

    #[test]
    fn test_crossed_quote_policy_use_last_valid() {
        let config = LobConfig::new(10)
            .with_crossed_quote_policy(CrossedQuotePolicy::UseLastValid)
            .with_logging(false);
        let mut lob = LobReconstructor::with_config(config);

        // Create a valid book first (bid < ask)
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();
        let valid_state = lob
            .process_message(&create_test_message(2, Action::Add, Side::Ask, 100.01, 200))
            .unwrap();

        // Verify we have a valid state
        assert!(!valid_state.is_crossed());
        assert!(valid_state.is_valid());

        // Cancel the ask to prepare for crossed quote
        lob.process_message(&create_test_message(
            2,
            Action::Cancel,
            Side::Ask,
            100.01,
            200,
        ))
        .unwrap();

        // Try to create crossed book (ask below bid)
        let state_after_cross = lob
            .process_message(&create_test_message(3, Action::Add, Side::Ask, 99.99, 200))
            .unwrap();

        // Should return the last valid state (not crossed)
        assert!(!state_after_cross.is_crossed());
        // The returned state should be the last valid one
        assert_eq!(state_after_cross.best_ask, Some(100_010_000_000)); // $100.01 from valid state
    }

    #[test]
    fn test_crossed_quote_policy_error() {
        let config = LobConfig::new(10)
            .with_crossed_quote_policy(CrossedQuotePolicy::Error)
            .with_logging(false);
        let mut lob = LobReconstructor::with_config(config);

        // Add valid bid
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();

        // Try to create crossed book - should return error
        let result =
            lob.process_message(&create_test_message(2, Action::Add, Side::Ask, 99.99, 200));

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::error::TlobError::CrossedQuote(_, _)
        ));
    }

    #[test]
    fn test_config_builder() {
        let config = LobConfig::new(5)
            .with_crossed_quote_policy(CrossedQuotePolicy::UseLastValid)
            .with_validation(false)
            .with_logging(false);

        assert_eq!(config.levels, 5);
        assert_eq!(
            config.crossed_quote_policy,
            CrossedQuotePolicy::UseLastValid
        );
        assert!(!config.validate_messages);
        assert!(!config.log_warnings);
    }

    #[test]
    fn test_lob_with_config() {
        let config = LobConfig::new(5);
        let lob = LobReconstructor::with_config(config);

        assert_eq!(lob.levels(), 5);
    }

    #[test]
    fn test_stats_track_crossed_and_locked() {
        let config = LobConfig::new(10).with_logging(false);
        let mut lob = LobReconstructor::with_config(config);

        // Add bid
        lob.process_message(&create_test_message(1, Action::Add, Side::Bid, 100.0, 100))
            .unwrap();

        // Create locked quote
        lob.process_message(&create_test_message(2, Action::Add, Side::Ask, 100.0, 200))
            .unwrap();
        assert_eq!(lob.stats().locked_quotes, 1);

        // Cancel the ask
        lob.process_message(&create_test_message(
            2,
            Action::Cancel,
            Side::Ask,
            100.0,
            200,
        ))
        .unwrap();

        // Create crossed quote
        lob.process_message(&create_test_message(3, Action::Add, Side::Ask, 99.99, 200))
            .unwrap();
        assert_eq!(lob.stats().crossed_quotes, 1);

        // Total counts
        assert_eq!(lob.stats().locked_quotes, 1);
        assert_eq!(lob.stats().crossed_quotes, 1);
    }
}
