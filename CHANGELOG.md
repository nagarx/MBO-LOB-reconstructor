# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2025-01-XX

### Added

#### Core LOB Reconstruction
- `LobReconstructor` - High-performance single-symbol LOB reconstruction
- `MultiSymbolLob` - Multi-symbol LOB management
- `LobConfig` - Configurable LOB behavior
- `CrossedQuotePolicy` - Four policies for handling crossed quotes (Allow, UseLastValid, Error, SkipUpdate)

#### Core Types
- `MboMessage` - Market-By-Order message representation
- `LobState` - LOB snapshot with enriched analytics
- `Action` - Order actions (Add, Modify, Cancel, Trade, Fill)
- `Side` - Order side (Bid, Ask, None)
- `BookConsistency` - Book state validation (Valid, Empty, Crossed, Locked)

#### Enriched Analytics on LobState
- `mid_price()` - Average of best bid and ask
- `spread()` / `spread_bps()` - Spread in dollars and basis points
- `microprice()` - Volume-weighted mid-price
- `vwap_bid(n)` / `vwap_ask(n)` - VWAP for top N levels
- `weighted_mid(n)` - VWAP-based mid-price
- `depth_imbalance()` - Normalized volume imbalance [-1, 1]
- `total_bid_volume()` / `total_ask_volume()` - Total volume per side
- `active_bid_levels()` / `active_ask_levels()` - Count of non-empty levels
- `check_consistency()` - Book state validation

#### Statistics for ML
- `RunningStats` - Online mean/std computation using Welford's algorithm
- `DayStats` - Per-day statistics tracking for all LOB metrics
- `NormalizationParams` - Z-score normalization parameters with save/load

#### Advanced Analytics
- `DepthStats` - Per-side depth statistics (volume, VWAP, concentration, price range)
- `MarketImpact` - Order execution simulation with slippage analysis
- `LiquidityMetrics` - Combined book analysis (spread, imbalance, pressure)

#### Databento Support (feature-gated)
- `DbnLoader` - Streaming loader for compressed DBN files
- `DbnBridge` - Databento MboMsg to internal MboMessage conversion
- `LoaderStats` - File loading statistics

#### Error Handling
- `TlobError` - Comprehensive error types
- `CrossedQuote` / `LockedQuote` - Specific errors for book consistency issues

### Performance
- ~974,000 messages/second throughput
- ~1 μs latency per message
- 100% data quality on real NVIDIA MBO data (17.8M messages)

### Testing
- 89 unit tests
- 19 integration tests with real market data
- 8 edge case tests for robustness
- 3 documentation tests

