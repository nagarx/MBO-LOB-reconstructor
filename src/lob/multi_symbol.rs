//! Multi-symbol LOB manager.
//!
//! Manages multiple LOB reconstructors, one per symbol.
//! Designed for processing multiple stocks simultaneously.

use super::reconstructor::LobReconstructor;
use crate::error::{Result, TlobError};
use crate::types::{LobState, MboMessage};
use ahash::AHashMap;

/// Multi-symbol LOB manager.
///
/// This manages a collection of LOB reconstructors, one per symbol.
/// Useful for:
/// - Processing multiple stocks (NVDA, TSLA, AAPL, etc.)
/// - Portfolio-level analysis
/// - Cross-asset strategies
///
/// # Example
/// ```
/// use mbo_lob_reconstructor::MultiSymbolLob;
///
/// let mut multi_lob = MultiSymbolLob::new(10);
///
/// // Add symbols
/// multi_lob.add_symbol("NVDA");
/// multi_lob.add_symbol("TSLA");
///
/// // Process messages for different symbols
/// // ... process NVDA messages
/// // ... process TSLA messages
///
/// // Get state for specific symbol
/// let nvda_state = multi_lob.get_state("NVDA");
/// ```
pub struct MultiSymbolLob {
    /// Number of levels for each LOB
    levels: usize,

    /// Map of symbol -> LOB reconstructor
    lobs: AHashMap<String, LobReconstructor>,

    /// Statistics
    stats: MultiSymbolStats,
}

/// Statistics for multi-symbol processing.
#[derive(Debug, Clone, Default)]
pub struct MultiSymbolStats {
    /// Total symbols tracked
    pub symbol_count: usize,

    /// Total messages processed across all symbols
    pub total_messages: u64,

    /// Messages per symbol
    pub messages_per_symbol: AHashMap<String, u64>,
}

impl MultiSymbolLob {
    /// Create a new multi-symbol LOB manager.
    ///
    /// # Arguments
    /// * `levels` - Number of price levels for each symbol
    pub fn new(levels: usize) -> Self {
        Self {
            levels,
            lobs: AHashMap::new(),
            stats: MultiSymbolStats::default(),
        }
    }

    /// Add a new symbol to track.
    ///
    /// # Arguments
    /// * `symbol` - Symbol name (e.g., "NVDA", "TSLA")
    ///
    /// # Returns
    /// Ok if added, error if symbol already exists
    pub fn add_symbol(&mut self, symbol: impl Into<String>) -> Result<()> {
        let symbol = symbol.into();

        if self.lobs.contains_key(&symbol) {
            return Err(TlobError::generic(format!(
                "Symbol {} already exists",
                symbol
            )));
        }

        self.lobs
            .insert(symbol.clone(), LobReconstructor::new(self.levels));
        self.stats.symbol_count = self.lobs.len();
        self.stats.messages_per_symbol.insert(symbol, 0);

        Ok(())
    }

    /// Remove a symbol from tracking.
    pub fn remove_symbol(&mut self, symbol: &str) -> Result<()> {
        if self.lobs.remove(symbol).is_none() {
            return Err(TlobError::SymbolNotFound(symbol.to_string()));
        }

        self.stats.symbol_count = self.lobs.len();
        self.stats.messages_per_symbol.remove(symbol);

        Ok(())
    }

    /// Process a message for a specific symbol.
    ///
    /// # Arguments
    /// * `symbol` - Symbol name
    /// * `msg` - MBO message to process
    ///
    /// # Returns
    /// Current LOB state for the symbol after processing
    pub fn process_message(&mut self, symbol: &str, msg: &MboMessage) -> Result<LobState> {
        let lob = self
            .lobs
            .get_mut(symbol)
            .ok_or_else(|| TlobError::SymbolNotFound(symbol.to_string()))?;

        let state = lob.process_message(msg)?;

        // Update statistics
        self.stats.total_messages += 1;
        *self
            .stats
            .messages_per_symbol
            .entry(symbol.to_string())
            .or_insert(0) += 1;

        Ok(state)
    }

    /// Get current LOB state for a symbol (without processing a message).
    pub fn get_state(&self, symbol: &str) -> Result<LobState> {
        let lob = self
            .lobs
            .get(symbol)
            .ok_or_else(|| TlobError::SymbolNotFound(symbol.to_string()))?;

        Ok(lob.get_lob_state())
    }

    /// Reset LOB state for a specific symbol.
    pub fn reset_symbol(&mut self, symbol: &str) -> Result<()> {
        let lob = self
            .lobs
            .get_mut(symbol)
            .ok_or_else(|| TlobError::SymbolNotFound(symbol.to_string()))?;

        lob.reset();

        Ok(())
    }

    /// Reset all symbols.
    pub fn reset_all(&mut self) {
        for lob in self.lobs.values_mut() {
            lob.reset();
        }

        self.stats.total_messages = 0;
        for count in self.stats.messages_per_symbol.values_mut() {
            *count = 0;
        }
    }

    /// Get list of all tracked symbols.
    pub fn symbols(&self) -> Vec<&str> {
        self.lobs.keys().map(|s| s.as_str()).collect()
    }

    /// Get number of tracked symbols.
    pub fn symbol_count(&self) -> usize {
        self.lobs.len()
    }

    /// Get statistics.
    pub fn stats(&self) -> &MultiSymbolStats {
        &self.stats
    }

    /// Get statistics for a specific symbol.
    pub fn symbol_stats(&self, symbol: &str) -> Result<&crate::lob::reconstructor::LobStats> {
        let lob = self
            .lobs
            .get(symbol)
            .ok_or_else(|| TlobError::SymbolNotFound(symbol.to_string()))?;

        Ok(lob.stats())
    }

    /// Check if a symbol is being tracked.
    pub fn has_symbol(&self, symbol: &str) -> bool {
        self.lobs.contains_key(symbol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Action, Side};

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
    fn test_new_multi_symbol() {
        let multi_lob = MultiSymbolLob::new(10);
        assert_eq!(multi_lob.symbol_count(), 0);
    }

    #[test]
    fn test_add_symbol() {
        let mut multi_lob = MultiSymbolLob::new(10);

        multi_lob.add_symbol("NVDA").unwrap();
        assert_eq!(multi_lob.symbol_count(), 1);
        assert!(multi_lob.has_symbol("NVDA"));
    }

    #[test]
    fn test_add_duplicate_symbol() {
        let mut multi_lob = MultiSymbolLob::new(10);

        multi_lob.add_symbol("NVDA").unwrap();
        let result = multi_lob.add_symbol("NVDA");
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_symbol() {
        let mut multi_lob = MultiSymbolLob::new(10);

        multi_lob.add_symbol("NVDA").unwrap();
        multi_lob.remove_symbol("NVDA").unwrap();
        assert_eq!(multi_lob.symbol_count(), 0);
    }

    #[test]
    fn test_process_message_for_symbol() {
        let mut multi_lob = MultiSymbolLob::new(10);

        multi_lob.add_symbol("NVDA").unwrap();

        let msg = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        let state = multi_lob.process_message("NVDA", &msg).unwrap();

        assert_eq!(state.best_bid, Some(100_000_000_000));
    }

    #[test]
    fn test_process_multiple_symbols() {
        let mut multi_lob = MultiSymbolLob::new(10);

        // Add symbols
        multi_lob.add_symbol("NVDA").unwrap();
        multi_lob.add_symbol("TSLA").unwrap();

        // Process NVDA
        let nvda_msg = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        multi_lob.process_message("NVDA", &nvda_msg).unwrap();

        // Process TSLA
        let tsla_msg = create_test_message(1, Action::Add, Side::Bid, 200.0, 50);
        multi_lob.process_message("TSLA", &tsla_msg).unwrap();

        // Check states are independent
        let nvda_state = multi_lob.get_state("NVDA").unwrap();
        let tsla_state = multi_lob.get_state("TSLA").unwrap();

        assert_eq!(nvda_state.best_bid, Some(100_000_000_000));
        assert_eq!(tsla_state.best_bid, Some(200_000_000_000));
    }

    #[test]
    fn test_symbols_list() {
        let mut multi_lob = MultiSymbolLob::new(10);

        multi_lob.add_symbol("NVDA").unwrap();
        multi_lob.add_symbol("TSLA").unwrap();
        multi_lob.add_symbol("AAPL").unwrap();

        let mut symbols = multi_lob.symbols();
        symbols.sort(); // HashMap order is not guaranteed

        assert_eq!(symbols, vec!["AAPL", "NVDA", "TSLA"]);
    }

    #[test]
    fn test_reset_symbol() {
        let mut multi_lob = MultiSymbolLob::new(10);

        multi_lob.add_symbol("NVDA").unwrap();

        // Add order
        let msg = create_test_message(1, Action::Add, Side::Bid, 100.0, 100);
        multi_lob.process_message("NVDA", &msg).unwrap();

        // Reset
        multi_lob.reset_symbol("NVDA").unwrap();

        let state = multi_lob.get_state("NVDA").unwrap();
        assert_eq!(state.best_bid, None);
    }

    #[test]
    fn test_reset_all() {
        let mut multi_lob = MultiSymbolLob::new(10);

        multi_lob.add_symbol("NVDA").unwrap();
        multi_lob.add_symbol("TSLA").unwrap();

        // Add orders
        multi_lob
            .process_message(
                "NVDA",
                &create_test_message(1, Action::Add, Side::Bid, 100.0, 100),
            )
            .unwrap();
        multi_lob
            .process_message(
                "TSLA",
                &create_test_message(1, Action::Add, Side::Bid, 200.0, 50),
            )
            .unwrap();

        // Reset all
        multi_lob.reset_all();

        let nvda_state = multi_lob.get_state("NVDA").unwrap();
        let tsla_state = multi_lob.get_state("TSLA").unwrap();

        assert_eq!(nvda_state.best_bid, None);
        assert_eq!(tsla_state.best_bid, None);
    }

    #[test]
    fn test_statistics() {
        let mut multi_lob = MultiSymbolLob::new(10);

        multi_lob.add_symbol("NVDA").unwrap();
        multi_lob.add_symbol("TSLA").unwrap();

        // Process messages
        multi_lob
            .process_message(
                "NVDA",
                &create_test_message(1, Action::Add, Side::Bid, 100.0, 100),
            )
            .unwrap();
        multi_lob
            .process_message(
                "NVDA",
                &create_test_message(2, Action::Add, Side::Ask, 100.01, 200),
            )
            .unwrap();
        multi_lob
            .process_message(
                "TSLA",
                &create_test_message(1, Action::Add, Side::Bid, 200.0, 50),
            )
            .unwrap();

        let stats = multi_lob.stats();
        assert_eq!(stats.symbol_count, 2);
        assert_eq!(stats.total_messages, 3);
        assert_eq!(stats.messages_per_symbol.get("NVDA"), Some(&2));
        assert_eq!(stats.messages_per_symbol.get("TSLA"), Some(&1));
    }
}
