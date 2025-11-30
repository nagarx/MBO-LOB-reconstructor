//! Core data types for MBO messages and LOB state.
//!
//! These types are designed to be:
//! - Memory efficient (use smallest types possible)
//! - Cache-friendly (aligned, packed where appropriate)
//! - Zero-copy where possible
//! - Compatible with Databento's MBO format

use serde::{Deserialize, Serialize};

/// MBO action type (what happened to the order)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)] // Explicit representation for efficiency
pub enum Action {
    /// Add new order to book
    Add = b'A',
    /// Modify existing order
    Modify = b'M',
    /// Cancel/remove order
    Cancel = b'C',
    /// Trade execution (full or partial fill)
    Trade = b'T',
    /// Fill (alternative trade representation)
    Fill = b'F',
}

impl Action {
    /// Parse action from a byte (Databento format).
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            b'A' => Some(Action::Add),
            b'M' => Some(Action::Modify),
            b'C' => Some(Action::Cancel),
            b'T' => Some(Action::Trade),
            b'F' => Some(Action::Fill),
            _ => None,
        }
    }

    /// Convert to byte representation.
    pub fn to_byte(self) -> u8 {
        self as u8
    }
}

/// Order side (bid or ask)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Side {
    /// Buy order (bid)
    Bid = b'B',
    /// Sell order (ask)
    Ask = b'A',
    /// Non-directional (used for some trade types)
    None = b'N',
}

impl Side {
    /// Parse side from a byte.
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            b'B' => Some(Side::Bid),
            b'A' => Some(Side::Ask),
            b'N' => Some(Side::None),
            _ => None,
        }
    }

    /// Convert to byte representation.
    pub fn to_byte(self) -> u8 {
        self as u8
    }

    /// Check if this is a bid.
    #[inline(always)]
    pub fn is_bid(self) -> bool {
        matches!(self, Side::Bid)
    }

    /// Check if this is an ask.
    #[inline(always)]
    pub fn is_ask(self) -> bool {
        matches!(self, Side::Ask)
    }
}

/// Market By Order (MBO) message.
///
/// This represents a single order book event. All fields use fixed-size types
/// for predictable memory layout and cache efficiency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MboMessage {
    /// Unique order identifier
    pub order_id: u64,

    /// Order action (add, modify, cancel, trade)
    pub action: Action,

    /// Order side (bid or ask)
    pub side: Side,

    /// Price in fixed-point format (divide by 1e9 for dollars)
    /// Using i64 to match Databento format
    pub price: i64,

    /// Order size in shares/contracts
    pub size: u32,

    /// Timestamp (nanoseconds since epoch)
    /// Optional - not always needed for LOB reconstruction
    pub timestamp: Option<i64>,
}

impl MboMessage {
    /// Create a new MBO message.
    pub fn new(order_id: u64, action: Action, side: Side, price: i64, size: u32) -> Self {
        Self {
            order_id,
            action,
            side,
            price,
            size,
            timestamp: None,
        }
    }

    /// Create with timestamp.
    pub fn with_timestamp(mut self, timestamp: i64) -> Self {
        self.timestamp = Some(timestamp);
        self
    }

    /// Get price as floating point dollars.
    #[inline]
    pub fn price_as_f64(&self) -> f64 {
        self.price as f64 / 1e9
    }

    /// Validate the message fields.
    pub fn validate(&self) -> crate::error::Result<()> {
        use crate::error::TlobError;

        if self.order_id == 0 {
            return Err(TlobError::InvalidOrderId(0));
        }

        if self.price <= 0 {
            return Err(TlobError::InvalidPrice(self.price));
        }

        if self.size == 0 {
            return Err(TlobError::InvalidSize(0));
        }

        Ok(())
    }
}

/// Order information stored in LOB.
///
/// Minimal representation to save memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Order {
    pub side: Side,
    pub price: i64,
    pub size: u32,
}

/// Book consistency status after validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookConsistency {
    /// Book is valid: best_bid < best_ask
    Valid,
    /// Book is empty (no quotes on one or both sides)
    Empty,
    /// Book is locked: best_bid == best_ask (unusual but can occur)
    Locked,
    /// Book is crossed: best_bid > best_ask (invalid state)
    Crossed,
}

impl BookConsistency {
    /// Returns true if the book state is valid for trading/analysis.
    #[inline]
    pub fn is_valid(&self) -> bool {
        matches!(self, BookConsistency::Valid)
    }

    /// Returns true if the book is crossed (invalid state).
    #[inline]
    pub fn is_crossed(&self) -> bool {
        matches!(self, BookConsistency::Crossed)
    }

    /// Returns true if the book is locked (bid == ask).
    #[inline]
    pub fn is_locked(&self) -> bool {
        matches!(self, BookConsistency::Locked)
    }

    /// Returns true if the book is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        matches!(self, BookConsistency::Empty)
    }
}

/// LOB state snapshot.
///
/// This represents the current state of the order book at N levels.
/// Arrays are used instead of vectors for fixed-size allocation.
#[derive(Debug, Clone, PartialEq)]
pub struct LobState {
    /// Bid prices (highest to lowest)
    pub bid_prices: Vec<i64>,

    /// Bid sizes (corresponding to bid_prices)
    pub bid_sizes: Vec<u32>,

    /// Ask prices (lowest to highest)
    pub ask_prices: Vec<i64>,

    /// Ask sizes (corresponding to ask_prices)
    pub ask_sizes: Vec<u32>,

    /// Best bid price (cached)
    pub best_bid: Option<i64>,

    /// Best ask price (cached)
    pub best_ask: Option<i64>,

    /// Number of levels
    pub levels: usize,

    /// Timestamp of this snapshot (nanoseconds since epoch)
    pub timestamp: Option<i64>,

    /// Message sequence number that produced this state
    pub sequence: u64,
}

impl LobState {
    /// Create a new empty LOB state with specified number of levels.
    pub fn new(levels: usize) -> Self {
        Self {
            bid_prices: vec![0; levels],
            bid_sizes: vec![0; levels],
            ask_prices: vec![0; levels],
            ask_sizes: vec![0; levels],
            best_bid: None,
            best_ask: None,
            levels,
            timestamp: None,
            sequence: 0,
        }
    }

    // =========================================================================
    // Book Consistency Validation
    // =========================================================================

    /// Check book consistency (whether bid < ask).
    ///
    /// This is a critical validation for market structure integrity.
    /// A crossed book (bid >= ask) indicates either:
    /// - Data quality issues
    /// - Market halt/unusual conditions
    /// - Reconstruction errors
    ///
    /// # Returns
    /// - `BookConsistency::Valid` if best_bid < best_ask
    /// - `BookConsistency::Empty` if either side has no quotes
    /// - `BookConsistency::Locked` if best_bid == best_ask
    /// - `BookConsistency::Crossed` if best_bid > best_ask
    #[inline]
    pub fn check_consistency(&self) -> BookConsistency {
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

    /// Returns true if the book is in a valid state (bid < ask).
    #[inline]
    pub fn is_consistent(&self) -> bool {
        self.check_consistency().is_valid()
    }

    /// Returns true if the book is crossed (bid > ask) - invalid state.
    #[inline]
    pub fn is_crossed(&self) -> bool {
        self.check_consistency().is_crossed()
    }

    /// Returns true if the book is locked (bid == ask).
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.check_consistency().is_locked()
    }

    /// Validate book consistency and return an error if invalid.
    ///
    /// # Returns
    /// - `Ok(())` if book is valid
    /// - `Err(TlobError::CrossedQuote)` if book is crossed
    /// - `Err(TlobError::LockedQuote)` if book is locked
    pub fn validate_consistency(&self) -> crate::error::Result<()> {
        use crate::error::TlobError;

        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => {
                if bid > ask {
                    Err(TlobError::CrossedQuote(bid, ask))
                } else if bid == ask {
                    Err(TlobError::LockedQuote(bid, ask))
                } else {
                    Ok(())
                }
            }
            _ => Ok(()), // Empty book is considered valid
        }
    }

    // =========================================================================
    // Basic Analytics (already exist)
    // =========================================================================

    /// Calculate mid-price (average of best bid and ask).
    #[inline]
    pub fn mid_price(&self) -> Option<f64> {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => {
                let bid_f = bid as f64 / 1e9;
                let ask_f = ask as f64 / 1e9;
                Some((bid_f + ask_f) / 2.0)
            }
            _ => None,
        }
    }

    /// Calculate spread (difference between best ask and best bid).
    #[inline]
    pub fn spread(&self) -> Option<f64> {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => {
                let bid_f = bid as f64 / 1e9;
                let ask_f = ask as f64 / 1e9;
                Some(ask_f - bid_f)
            }
            _ => None,
        }
    }

    /// Check if LOB has valid state (at least one bid and one ask).
    #[inline]
    pub fn is_valid(&self) -> bool {
        self.best_bid.is_some() && self.best_ask.is_some()
    }

    /// Get spread in basis points (bps).
    #[inline]
    pub fn spread_bps(&self) -> Option<f64> {
        match (self.mid_price(), self.spread()) {
            (Some(mid), Some(spread)) if mid > 0.0 => Some((spread / mid) * 10000.0),
            _ => None,
        }
    }

    // =========================================================================
    // Enriched Analytics (NEW)
    // =========================================================================

    /// Calculate microprice (volume-weighted mid-price).
    ///
    /// Microprice = (bid_price * ask_size + ask_price * bid_size) / (bid_size + ask_size)
    ///
    /// This provides a better estimate of "fair value" than simple mid-price
    /// by incorporating the relative sizes at the best levels.
    ///
    /// # Returns
    /// - `Some(microprice)` in dollars if both sides have valid quotes
    /// - `None` if either side is empty or total size is zero
    #[inline]
    pub fn microprice(&self) -> Option<f64> {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => {
                let bid_size = self.bid_sizes.first().copied().unwrap_or(0) as f64;
                let ask_size = self.ask_sizes.first().copied().unwrap_or(0) as f64;
                let total_size = bid_size + ask_size;

                if total_size > 0.0 {
                    let bid_f = bid as f64 / 1e9;
                    let ask_f = ask as f64 / 1e9;
                    Some((bid_f * ask_size + ask_f * bid_size) / total_size)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Calculate total bid volume across all levels.
    #[inline]
    pub fn total_bid_volume(&self) -> u64 {
        self.bid_sizes.iter().map(|&s| s as u64).sum()
    }

    /// Calculate total ask volume across all levels.
    #[inline]
    pub fn total_ask_volume(&self) -> u64 {
        self.ask_sizes.iter().map(|&s| s as u64).sum()
    }

    /// Calculate depth imbalance (normalized difference between bid and ask volume).
    ///
    /// Imbalance = (bid_volume - ask_volume) / (bid_volume + ask_volume)
    ///
    /// Range: [-1.0, 1.0]
    /// - Positive: more volume on bid side (buying pressure)
    /// - Negative: more volume on ask side (selling pressure)
    /// - Zero: balanced book
    ///
    /// # Returns
    /// - `Some(imbalance)` if total volume > 0
    /// - `None` if book is empty
    #[inline]
    pub fn depth_imbalance(&self) -> Option<f64> {
        let bid_vol = self.total_bid_volume() as f64;
        let ask_vol = self.total_ask_volume() as f64;
        let total = bid_vol + ask_vol;

        if total > 0.0 {
            Some((bid_vol - ask_vol) / total)
        } else {
            None
        }
    }

    /// Calculate VWAP for the bid side (top N levels).
    ///
    /// VWAP = Σ(price * size) / Σ(size)
    ///
    /// # Arguments
    /// - `n_levels`: Number of levels to include (clamped to available levels)
    ///
    /// # Returns
    /// - `Some(vwap)` in dollars if bid side has volume
    /// - `None` if no bid volume
    #[inline]
    pub fn vwap_bid(&self, n_levels: usize) -> Option<f64> {
        let n = n_levels.min(self.levels);
        let mut total_value: f64 = 0.0;
        let mut total_size: u64 = 0;

        for i in 0..n {
            let price = self.bid_prices[i];
            let size = self.bid_sizes[i] as u64;
            if price > 0 && size > 0 {
                total_value += (price as f64 / 1e9) * (size as f64);
                total_size += size;
            }
        }

        if total_size > 0 {
            Some(total_value / total_size as f64)
        } else {
            None
        }
    }

    /// Calculate VWAP for the ask side (top N levels).
    ///
    /// VWAP = Σ(price * size) / Σ(size)
    ///
    /// # Arguments
    /// - `n_levels`: Number of levels to include (clamped to available levels)
    ///
    /// # Returns
    /// - `Some(vwap)` in dollars if ask side has volume
    /// - `None` if no ask volume
    #[inline]
    pub fn vwap_ask(&self, n_levels: usize) -> Option<f64> {
        let n = n_levels.min(self.levels);
        let mut total_value: f64 = 0.0;
        let mut total_size: u64 = 0;

        for i in 0..n {
            let price = self.ask_prices[i];
            let size = self.ask_sizes[i] as u64;
            if price > 0 && size > 0 {
                total_value += (price as f64 / 1e9) * (size as f64);
                total_size += size;
            }
        }

        if total_size > 0 {
            Some(total_value / total_size as f64)
        } else {
            None
        }
    }

    /// Calculate weighted mid-price using VWAP from both sides.
    ///
    /// This is the average of bid VWAP and ask VWAP for top N levels.
    ///
    /// # Arguments
    /// - `n_levels`: Number of levels to include on each side
    ///
    /// # Returns
    /// - `Some(weighted_mid)` in dollars if both sides have volume
    /// - `None` if either side is empty
    #[inline]
    pub fn weighted_mid(&self, n_levels: usize) -> Option<f64> {
        match (self.vwap_bid(n_levels), self.vwap_ask(n_levels)) {
            (Some(bid_vwap), Some(ask_vwap)) => Some((bid_vwap + ask_vwap) / 2.0),
            _ => None,
        }
    }

    /// Get number of active bid levels (with non-zero size).
    #[inline]
    pub fn active_bid_levels(&self) -> usize {
        self.bid_sizes.iter().filter(|&&s| s > 0).count()
    }

    /// Get number of active ask levels (with non-zero size).
    #[inline]
    pub fn active_ask_levels(&self) -> usize {
        self.ask_sizes.iter().filter(|&&s| s > 0).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Action and Side tests
    // =========================================================================

    #[test]
    fn test_action_from_byte() {
        assert_eq!(Action::from_byte(b'A'), Some(Action::Add));
        assert_eq!(Action::from_byte(b'M'), Some(Action::Modify));
        assert_eq!(Action::from_byte(b'C'), Some(Action::Cancel));
        assert_eq!(Action::from_byte(b'T'), Some(Action::Trade));
        assert_eq!(Action::from_byte(b'F'), Some(Action::Fill));
        assert_eq!(Action::from_byte(b'X'), None);
    }

    #[test]
    fn test_action_to_byte() {
        assert_eq!(Action::Add.to_byte(), b'A');
        assert_eq!(Action::Modify.to_byte(), b'M');
        assert_eq!(Action::Cancel.to_byte(), b'C');
        assert_eq!(Action::Trade.to_byte(), b'T');
        assert_eq!(Action::Fill.to_byte(), b'F');
    }

    #[test]
    fn test_side_checks() {
        assert!(Side::Bid.is_bid());
        assert!(!Side::Ask.is_bid());
        assert!(Side::Ask.is_ask());
        assert!(!Side::Bid.is_ask());
        assert!(!Side::None.is_bid());
        assert!(!Side::None.is_ask());
    }

    #[test]
    fn test_side_from_byte() {
        assert_eq!(Side::from_byte(b'B'), Some(Side::Bid));
        assert_eq!(Side::from_byte(b'A'), Some(Side::Ask));
        assert_eq!(Side::from_byte(b'N'), Some(Side::None));
        assert_eq!(Side::from_byte(b'X'), None);
    }

    // =========================================================================
    // MboMessage tests
    // =========================================================================

    #[test]
    fn test_mbo_message_price_conversion() {
        let msg = MboMessage::new(
            123,
            Action::Add,
            Side::Bid,
            100_000_000_000, // $100.00
            100,
        );

        assert_eq!(msg.price_as_f64(), 100.0);
    }

    #[test]
    fn test_mbo_message_with_timestamp() {
        let msg = MboMessage::new(123, Action::Add, Side::Bid, 100_000_000_000, 100)
            .with_timestamp(1234567890_000_000_000);

        assert_eq!(msg.timestamp, Some(1234567890_000_000_000));
    }

    #[test]
    fn test_mbo_message_validation() {
        let msg = MboMessage::new(123, Action::Add, Side::Bid, 100_000_000_000, 100);

        assert!(msg.validate().is_ok());

        // Invalid: zero order_id
        let invalid = MboMessage::new(0, Action::Add, Side::Bid, 100_000_000_000, 100);
        assert!(invalid.validate().is_err());

        // Invalid: zero price
        let invalid = MboMessage::new(123, Action::Add, Side::Bid, 0, 100);
        assert!(invalid.validate().is_err());

        // Invalid: negative price
        let invalid = MboMessage::new(123, Action::Add, Side::Bid, -100, 100);
        assert!(invalid.validate().is_err());

        // Invalid: zero size
        let invalid = MboMessage::new(123, Action::Add, Side::Bid, 100_000_000_000, 0);
        assert!(invalid.validate().is_err());
    }

    // =========================================================================
    // BookConsistency tests
    // =========================================================================

    #[test]
    fn test_book_consistency_valid() {
        let consistency = BookConsistency::Valid;
        assert!(consistency.is_valid());
        assert!(!consistency.is_crossed());
        assert!(!consistency.is_locked());
        assert!(!consistency.is_empty());
    }

    #[test]
    fn test_book_consistency_crossed() {
        let consistency = BookConsistency::Crossed;
        assert!(!consistency.is_valid());
        assert!(consistency.is_crossed());
        assert!(!consistency.is_locked());
        assert!(!consistency.is_empty());
    }

    #[test]
    fn test_book_consistency_locked() {
        let consistency = BookConsistency::Locked;
        assert!(!consistency.is_valid());
        assert!(!consistency.is_crossed());
        assert!(consistency.is_locked());
        assert!(!consistency.is_empty());
    }

    #[test]
    fn test_book_consistency_empty() {
        let consistency = BookConsistency::Empty;
        assert!(!consistency.is_valid());
        assert!(!consistency.is_crossed());
        assert!(!consistency.is_locked());
        assert!(consistency.is_empty());
    }

    // =========================================================================
    // LobState Consistency Validation tests
    // =========================================================================

    #[test]
    fn test_lob_state_consistency_valid() {
        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000); // $100.00
        state.best_ask = Some(100_010_000_000); // $100.01

        assert_eq!(state.check_consistency(), BookConsistency::Valid);
        assert!(state.is_consistent());
        assert!(!state.is_crossed());
        assert!(!state.is_locked());
        assert!(state.validate_consistency().is_ok());
    }

    #[test]
    fn test_lob_state_consistency_crossed() {
        let mut state = LobState::new(10);
        state.best_bid = Some(100_010_000_000); // $100.01 (bid > ask!)
        state.best_ask = Some(100_000_000_000); // $100.00

        assert_eq!(state.check_consistency(), BookConsistency::Crossed);
        assert!(!state.is_consistent());
        assert!(state.is_crossed());
        assert!(!state.is_locked());

        let err = state.validate_consistency();
        assert!(err.is_err());
        assert!(matches!(
            err.unwrap_err(),
            crate::error::TlobError::CrossedQuote(_, _)
        ));
    }

    #[test]
    fn test_lob_state_consistency_locked() {
        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000); // $100.00
        state.best_ask = Some(100_000_000_000); // $100.00 (same as bid!)

        assert_eq!(state.check_consistency(), BookConsistency::Locked);
        assert!(!state.is_consistent());
        assert!(!state.is_crossed());
        assert!(state.is_locked());

        let err = state.validate_consistency();
        assert!(err.is_err());
        assert!(matches!(
            err.unwrap_err(),
            crate::error::TlobError::LockedQuote(_, _)
        ));
    }

    #[test]
    fn test_lob_state_consistency_empty() {
        let state = LobState::new(10);

        assert_eq!(state.check_consistency(), BookConsistency::Empty);
        assert!(!state.is_consistent());
        assert!(!state.is_crossed());
        assert!(!state.is_locked());
        // Empty book should not produce an error
        assert!(state.validate_consistency().is_ok());
    }

    #[test]
    fn test_lob_state_consistency_partial_empty() {
        // Only bid side filled
        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000);
        assert_eq!(state.check_consistency(), BookConsistency::Empty);

        // Only ask side filled
        let mut state = LobState::new(10);
        state.best_ask = Some(100_010_000_000);
        assert_eq!(state.check_consistency(), BookConsistency::Empty);
    }

    // =========================================================================
    // LobState Basic Analytics tests
    // =========================================================================

    #[test]
    fn test_lob_state_mid_price() {
        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000); // $100.00
        state.best_ask = Some(100_010_000_000); // $100.01

        let mid = state.mid_price().unwrap();
        assert!((mid - 100.005).abs() < 1e-6);
    }

    #[test]
    fn test_lob_state_mid_price_empty() {
        let state = LobState::new(10);
        assert!(state.mid_price().is_none());
    }

    #[test]
    fn test_lob_state_spread() {
        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000); // $100.00
        state.best_ask = Some(100_010_000_000); // $100.01

        let spread = state.spread().unwrap();
        assert!((spread - 0.01).abs() < 1e-6);
    }

    #[test]
    fn test_lob_state_spread_bps() {
        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000); // $100.00
        state.best_ask = Some(100_010_000_000); // $100.01

        let spread_bps = state.spread_bps().unwrap();
        assert!((spread_bps - 1.0).abs() < 0.01); // ~1 bps
    }

    // =========================================================================
    // LobState Enriched Analytics tests
    // =========================================================================

    #[test]
    fn test_lob_state_microprice() {
        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000); // $100.00
        state.best_ask = Some(100_020_000_000); // $100.02
        state.bid_sizes[0] = 100;
        state.ask_sizes[0] = 100;

        // Equal sizes: microprice should equal mid-price
        let microprice = state.microprice().unwrap();
        let mid = state.mid_price().unwrap();
        assert!((microprice - mid).abs() < 1e-6);
    }

    #[test]
    fn test_lob_state_microprice_weighted() {
        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000); // $100.00
        state.best_ask = Some(100_020_000_000); // $100.02
        state.bid_sizes[0] = 100; // Small bid
        state.ask_sizes[0] = 300; // Large ask

        // More volume on ask side: microprice should be closer to bid
        // microprice = (100.00 * 300 + 100.02 * 100) / 400 = 100.005
        let microprice = state.microprice().unwrap();
        assert!((microprice - 100.005).abs() < 1e-6);
    }

    #[test]
    fn test_lob_state_microprice_empty() {
        let state = LobState::new(10);
        assert!(state.microprice().is_none());

        // Has prices but no sizes
        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000);
        state.best_ask = Some(100_010_000_000);
        assert!(state.microprice().is_none());
    }

    #[test]
    fn test_lob_state_total_volume() {
        let mut state = LobState::new(3);
        state.bid_sizes = vec![100, 200, 50];
        state.ask_sizes = vec![150, 100, 75];

        assert_eq!(state.total_bid_volume(), 350);
        assert_eq!(state.total_ask_volume(), 325);
    }

    #[test]
    fn test_lob_state_depth_imbalance() {
        let mut state = LobState::new(3);

        // Balanced book
        state.bid_sizes = vec![100, 100, 100];
        state.ask_sizes = vec![100, 100, 100];
        let imbalance = state.depth_imbalance().unwrap();
        assert!((imbalance - 0.0).abs() < 1e-6);

        // More bids (positive imbalance)
        state.bid_sizes = vec![200, 100, 100];
        state.ask_sizes = vec![100, 100, 100];
        let imbalance = state.depth_imbalance().unwrap();
        assert!(imbalance > 0.0);
        assert!((imbalance - 0.142857).abs() < 0.001); // (400-300)/700

        // More asks (negative imbalance)
        state.bid_sizes = vec![100, 100, 100];
        state.ask_sizes = vec![200, 200, 200];
        let imbalance = state.depth_imbalance().unwrap();
        assert!(imbalance < 0.0);
        // (300-600)/900 = -300/900 = -1/3 ≈ -0.333
        assert!((imbalance - (-0.333333)).abs() < 0.001);
    }

    #[test]
    fn test_lob_state_depth_imbalance_empty() {
        let state = LobState::new(3);
        assert!(state.depth_imbalance().is_none());
    }

    #[test]
    fn test_lob_state_vwap() {
        let mut state = LobState::new(3);

        // Bid side: 100 @ $100, 200 @ $99, 50 @ $98
        state.bid_prices = vec![100_000_000_000, 99_000_000_000, 98_000_000_000];
        state.bid_sizes = vec![100, 200, 50];

        // VWAP = (100*100 + 99*200 + 98*50) / (100+200+50) = (10000+19800+4900) / 350 = 99.14...
        let vwap_all = state.vwap_bid(3).unwrap();
        assert!((vwap_all - 99.142857).abs() < 0.001);

        // VWAP for top 2 levels = (100*100 + 99*200) / 300 = 99.333...
        let vwap_2 = state.vwap_bid(2).unwrap();
        assert!((vwap_2 - 99.333333).abs() < 0.001);

        // VWAP for top 1 level = 100
        let vwap_1 = state.vwap_bid(1).unwrap();
        assert!((vwap_1 - 100.0).abs() < 1e-6);
    }

    #[test]
    fn test_lob_state_vwap_empty() {
        let state = LobState::new(3);
        assert!(state.vwap_bid(3).is_none());
        assert!(state.vwap_ask(3).is_none());
    }

    #[test]
    fn test_lob_state_weighted_mid() {
        let mut state = LobState::new(2);

        // Bid: 100 @ $100, 100 @ $99 → VWAP = 99.5
        state.bid_prices = vec![100_000_000_000, 99_000_000_000];
        state.bid_sizes = vec![100, 100];

        // Ask: 100 @ $101, 100 @ $102 → VWAP = 101.5
        state.ask_prices = vec![101_000_000_000, 102_000_000_000];
        state.ask_sizes = vec![100, 100];

        // Weighted mid = (99.5 + 101.5) / 2 = 100.5
        let weighted_mid = state.weighted_mid(2).unwrap();
        assert!((weighted_mid - 100.5).abs() < 1e-6);
    }

    #[test]
    fn test_lob_state_active_levels() {
        let mut state = LobState::new(5);
        state.bid_sizes = vec![100, 0, 50, 0, 0];
        state.ask_sizes = vec![100, 200, 0, 0, 50];

        assert_eq!(state.active_bid_levels(), 2);
        assert_eq!(state.active_ask_levels(), 3);
    }

    #[test]
    fn test_lob_state_new_fields() {
        let mut state = LobState::new(10);

        // Test timestamp and sequence fields
        assert!(state.timestamp.is_none());
        assert_eq!(state.sequence, 0);

        state.timestamp = Some(1234567890_000_000_000);
        state.sequence = 42;

        assert_eq!(state.timestamp, Some(1234567890_000_000_000));
        assert_eq!(state.sequence, 42);
    }
}
