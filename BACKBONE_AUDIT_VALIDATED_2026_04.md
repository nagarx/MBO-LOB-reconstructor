# BACKBONE AUDIT — VALIDATED FINDINGS (2026-04)

> **Status**: Read-only audit + validation. Zero code changes. Findings catalogued for the engineering team to consume when designing fixes.
>
> **Scope**: `MBO-LOB-reconstructor/` (the heart of the pipeline) plus the join points with `feature-extractor-MBO-LOB/`, `mbo-statistical-profiler/`, and `opra-statistical-profiler/` where data flows or contracts cross repository boundaries.
>
> **Reading order**: Executive Summary (§0) → Verification Matrix (§1) → Active bugs (§2) → Verified-in-code, empirically-inactive (§3) → Refuted claims (§4 — important: read this before doing any work, so we do not chase phantoms) → C4 re-derivation catalog (§5) → Dead code architectural pathology (§6) → Documentation drift (§7) → Methodological lessons (§8) → Appendices.
>
> **Provenance**: Two independent passes were conducted against ground-truth code:
> 1. **Discovery pass** — 7 parallel adversarial agents, each scoped to a distinct module, looking for bugs.
> 2. **Validation pass** — 7 parallel independent validators, each given the discovery findings verbatim and instructed to verify against code AND empirical data (real `_reconstruction_stats.json` across 234 NVDA days, real DBN flag distribution from 4 sample days totaling 61M records).
>
> **Every code-level claim in this document has been verified by at least one of the validation agents** (file:line, formula, mechanism). Empirical claims have been verified against real production data where possible; where they are pending verification (e.g., NEW-1's question of whether NVDA DBN files actually contain non-MBO records mid-stream), they are explicitly marked "Pending" in the §1 verification matrix and listed as P0 verifications in §9. Where a discovery-pass claim was overstated or refuted, it is explicitly marked.

---

## §0 — Executive Summary

The reconstructor's **core book-state mutation logic is sound**. The identified bugs cluster at three boundaries: (a) DBN parse → MboMessage bridge, (b) reconstructor → consumer event surface, and (c) time/DST handling. The audit also surfaced a substantial **dead-code architectural pathology** (~7,093 LOC of publicly re-exported but unused-in-production modules — see §6 for the per-module breakdown) that does not corrupt features but actively misleads consumers and future audits.

### What changed from the discovery-pass smoking-gun ranking

The discovery pass identified 9 "smoking guns." The validation pass produced this corrected severity table:

| ID | Discovery rank | Validated rank | Why changed |
|----|----------------|----------------|-------------|
| SG-1 (OFI Clear poisoning) | CRITICAL #1 | **REFUTED** | Mechanism does not exist in production — Action::Clear is filtered before reaching the reconstructor |
| SG-2 (Modify-of-missing) | CRITICAL | LOW–MEDIUM | Code path verified, but it is a reasonable warmup recovery heuristic; the real defect is missing observability |
| SG-3 (Allow crossed quotes) | HIGH | LOW (empirically) | Code claim verified, but 0 crossed quotes across 234 days / 2.77B messages |
| SG-4 (DBN flags dropped) | CRITICAL | LOW for SNAPSHOT, MEDIUM for the broader debt | Code claim verified, but 0 SNAPSHOT records in batch-historical NVDA data |
| SG-5 (silent iterator truncation) | CRITICAL | HIGH (confirmed) | Production caller `pipeline.rs:107` does not opt in to `skip_invalid(true)`; mid-record EOF aliases to clean EOF |
| SG-6 (executed_pressure count vs volume) | HIGH | MEDIUM | Real definitional mismatch with research literature |
| SG-7 (TimeBasedSampler DST) | HIGH | HIGH (confirmed) | Fires on ~70% of trading days; all 15+ production TOML configs use static `-5` |
| SG-8 (phantom cancels for features 51/55/89) | HIGH | LOW (refuted for the named features) | The named features do not consume `triggering_action`; only the `mbo-statistical-profiler` (research tool) is affected |
| SG-9 (MultiSymbolLob asymmetric reset) | HIGH | LOW (blast radius zero) | Asymmetry verified, but module has zero downstream consumers |

**Headline lesson**: Of the 9 discovery-pass CRITICAL/HIGH findings, only **ONE (SG-7) stands at exactly the claimed severity**. Two more (SG-5, SG-6) remain serious bugs but were downgraded by one tier (CRITICAL→HIGH and HIGH→MEDIUM respectively). The remaining six were either refuted (SG-1, SG-8), verified-but-empirically-inactive on this dataset (SG-3, SG-4), or substantially downgraded (SG-2, SG-9). One additional claim from the discovery pass — that `begin_day(...)` does not exist anywhere in production — was literally wrong (the hook is fully implemented in two profiler crates with ~18 production overrides), though the spirit of the underlying concern is real: the **feature-extractor pipeline** lacks a parallel `begin_day` hook, which is exactly the C4 anti-pattern captured in §5.

### Newly discovered issues (validation pass)

The validation also surfaced **9 new findings** that the discovery pass missed:

- **NEW-1 (HIGH)**: Silent stream termination on non-MBO records — strictly stronger version of SG-5.
- **NEW-2 (MEDIUM)**: `LobStats.errors` field is dead code (always reads 0, but exported as a stat).
- **NEW-3 (MEDIUM)**: `LoaderStats.bytes_read` is dead code (`progress()` always returns 0%).
- **NEW-4 (MEDIUM)**: `state.sequence` cross-repo documentation drift (analyzer claims it is the exchange sequence; producer writes a local message counter).
- **NEW-5 (MEDIUM)**: `system_messages_skipped` is silently always 0 in production (consumer pre-filter empties the metric).
- **NEW-6 (LOW)**: `delta_ns = 0` for identical-timestamp clusters (HFT bursts), not just backwards-time.
- **NEW-7 (MEDIUM)**: `MboWindow.first_ts` is not updated on eviction; `duration_seconds()` overestimates and `trade_rate_*` underestimates by a stable factor that compounds with F-004's count-vs-volume drift.
- **NEW-8 (DOCUMENTATION)**: Both the docstring and the discovery pass got LobState/MboMessage sizes wrong. Manually computed values: LobState ≈ 576 B, MboMessage ≈ 40-48 B (the test asserts only the loose range `500 < x < 700`).
- **NEW-9 (LOW)**: `weighted_mid_price` collapse-when-one-side-is-zero exists in TWO sites, not one (`MBO-LOB-reconstructor/src/types.rs:594-612` AND `feature-extractor-MBO-LOB/crates/hft-feature-core/src/derived.rs:115-123`).

### Top three things that actually need fixing

1. **SG-7 (TimeBasedSampler DST)** — fires on ~165/234 EDT trading days (≈70.5%) in the canonical NVDA 11-month dataset (2025-02-03 to 2026-01-07; DST window 2025-03-09 to 2025-11-01). Every EDT day's market-open boundary is shifted by 1 hour. Every time-based feature is biased.
2. **NEW-1 / SG-5 / F-024 (silent stream termination cluster)** — `pipeline.rs:107` does not call `.skip_invalid(true)`, so any non-MBO record OR any `DbnBridge::convert` failure mid-stream causes silent partial-day exports. There is no log, no metric, no signal to the operator. (Phase-2 verifier surfaced F-024 as a SECOND silent-EOF mode at the bridge layer — same Result-typed-Item fix addresses both.)
3. **SG-6 / F-004 (executed_pressure definitional mismatch)** — feature 86 is event-count-difference, not signed-volume as in Cont/Bouchaud literature. Same definitional drift in features 88, 89, AND (per Phase-2 verifier N-17) features 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59. **12 NEW affected features** discovered by Phase-2 enumeration check.

**Phase-2 addition**: **F-029 (volatility 10% filter)** — silently drops returns >10%/sample, erasing circuit-breaker-triggering events from features 106, 107, 109, 110, 111. Severity HIGH but only fires during high-volatility regimes (today: NVDA's 2025-2026 dataset has had no circuit-breaker events on NVDA itself). Treat as P1 #3 in §9 because the bug WOULD activate during exactly the events vol features most need to capture.

### Open Decisions Required Before Remediation

Five findings present multiple valid remediation options without a clear engineering-decision default. Each requires a team decision before implementation work can begin. Surfacing them upfront prevents stop-and-restart cycles during the remediation phase.

| Finding | Decision Required | Options | Considerations |
|---------|-------------------|---------|----------------|
| **F-004 / SG-6** (executed_pressure) | Should features 48-59, 86, 88, 89 be volume-weighted (per Cont/Bouchaud literature), count-based (current), or both? | (a) Migrate all to volume-weighted; (b) Keep count-based and document; (c) Expose both as separate features | (a) requires `MboWindow` schema change to add `total_*_volume_*` counters; (b) is no-op + doc; (c) doubles feature count. Decision affects 15 features and may change downstream model training. |
| **F-006** (features 68/69 hardcoded 0.0) | Implement queue tracking or formally deprecate features 68/69? | (a) Wire `QueuePositionTracker` into production path; (b) Mark features deprecated in `pipeline_contract.toml` and drop columns; (c) Mark deprecated but keep zero-emit columns for back-compat | (a) requires re-enabling the perf-disabled `queue_tracking` flag (~10× slower per CLAUDE.md); (b) breaks downstream consumers reading these columns; (c) hides the rot but is a smaller blast radius. |
| **F-009** (state.sequence cross-repo drift) | How to fix the `state.sequence` semantic mismatch? | (a) Rename the column to `events_processed` (semantic name); (b) Thread the actual DBN `sequence` through `MboMessage` and add a separate column; (c) Fix only the analyzer documentation | (a) breaks downstream Parquet readers; (b) requires bridge change + new column + backfill; (c) leaves the misleading column name in place. |
| **F-010 + NEW-5** (`system_messages_skipped` always 0) | Where should the system-message-skip counter live? | (a) Move to consumer side (feature-extractor); (b) Remove the consumer pre-filter so the reconstructor sees and counts; (c) Document the asymmetry as a known limitation | (a) duplicates the filter logic; (b) defeats the optimization that the consumer pre-filter provides; (c) is the cheapest but makes the stat permanently misleading. |
| **F-015** (microprice degenerate one-side-zero collapse) | Is the Stoikov collapse-to-zero-size-side a feature or a bug? | (a) Document as the Stoikov convention (no code change); (b) Return `None` when either size is 0 (defensive); (c) Use `(bid + ask) / 2` fallback when either size is 0 | Today the path is not exercised in production (per F-015 narrative). Decision matters for any future code that calls `update_order_size(.., 0)`. |

**Plus a meta-decision**: the §6 dead-code framing — is `MBO-LOB-reconstructor` (a) a standalone library for general use, or (b) the upstream half of an in-house pipeline? The audit document explicitly punts this in §6 because it is a product/strategy question, but the engineering team needs to answer it before deciding how to handle the ~7,093 LOC dead-code surface.

**Note on this table**: where the corresponding §2 narrative expresses a preference (e.g., F-004's "Surface area for redesign" leans toward volume-weighted alignment with Cont/Bouchaud; F-006's surface area implies the queue tracking should be implemented properly, not deprecated), the team can use that as a default starting point without re-litigating from scratch. The "Open Decisions" framing means decisions are needed before remediation begins, NOT that the audit has no opinion.

Everything else is either lower-severity, latent, empirically inactive in this dataset, or architectural cleanup.

---

## §1 — Verification Matrix (Quick Reference)

Every claim, its verification status, and the validation method.

### Active production bugs (highest priority)

| ID | Title | Severity | Status | Verified by | Empirical |
|----|-------|----------|--------|-------------|-----------|
| SG-7 | TimeBasedSampler re-derives `day_epoch_ns`; static `utc_offset_hours` ignores DST | HIGH | VERIFIED | Code trace + 15 production TOML configs all hardcode `-5` | ~165/234 EDT trading days (≈70.5%) in NVDA 2025-2026 dataset |
| NEW-1 | Silent stream termination on non-MBO records | HIGH | VERIFIED | `decode_record::<MboMsg>()` returns `Err::Conversion`, default `skip_invalid=false` returns `None` indistinguishable from EOF | Pending — depends on whether NVDA DBN files contain non-MBO records |
| SG-5 | `MessageIterator` silently truncates on decode error or mid-record EOF | HIGH | VERIFIED | `loader.rs:362-374`; dbn library's `silence_eof_error` aliases mid-record EOF | Production caller `pipeline.rs:107` doesn't opt in to skip_invalid |
| SG-6 | `executed_pressure` (feature 86) is event-count not signed-volume | MEDIUM | VERIFIED | `signals/mod.rs:159-163` + `window.rs:107-112` | Cited literature (Cont 2014, Bouchaud 2018) defines volume-weighted |
| NEW-7 | `MboWindow.first_ts` not updated on eviction | MEDIUM | VERIFIED | `window.rs:78-80` only sets `first_ts` on empty-buffer push | Affects all `trade_rate_*` after buffer fills |
| LB-3 | `fill_lob_state_with_temporal` clears `[0..levels]`, leaves stale data at `[levels..MAX_LOB_LEVELS]` (TODAY-ACTIVE in export batch path) | MEDIUM | VERIFIED + TODAY-ACTIVE | `LobBatch::push` (`export/batch.rs:92`) iterates `0..self.levels` (writer config), not `0..state.levels` | Triggered when `LobSnapshotWriter` is configured with different levels than the state |
| NEW-2 | `LobStats.errors` field is dead code (defined and exported but never incremented) | MEDIUM | VERIFIED | Grep across `MBO-LOB-reconstructor/src/` found definition + serialization sites, zero increment sites | Operator reading `_reconstruction_stats.json` sees `errors: 0` always |
| NEW-3 | `LoaderStats.bytes_read` dead; `progress()` always returns 0% | MEDIUM | VERIFIED | `loader.rs:99, 414` | Long-running extractions show no progress signal |
| NEW-4 | `state.sequence` is `messages_processed`, exported as a column called "sequence", documented as exchange sequence | MEDIUM | VERIFIED | `reconstructor.rs:802` + `export/schema.rs:61` + `MBO-LOB-analyzer/CODEBASE.md:179` | Cross-repo contract drift |
| NEW-5 | `system_messages_skipped` is silently always 0 in production | MEDIUM | VERIFIED | Consumer pre-filter at `pipeline.rs:142-144` short-circuits before reconstructor | Stat reads 0 even when filtering occurs |
| NEW-6 | `delta_ns = 0` for identical-timestamp clusters, not just backwards-time | LOW | VERIFIED | `reconstructor.rs:783-786` strict `current > prev` | HFT bursts produce many same-ns events |
| SG-2 | Modify-of-missing-order silently becomes Add at the modify's NEW price | LOW–MEDIUM | VERIFIED but OVERSTATED | `reconstructor.rs:494-510` | No counter exists; no observability |
| LB-7 | `WarningTracker.record()` underflow on backwards wall clock | LOW | VERIFIED + clarified | `warnings.rs:329` | Release behavior: dedup history clears (less severe than discovery pass framed) |
| Features 68/69 | `avg_queue_position` and `queue_volume_ahead` hardcoded to 0.0 in production | MEDIUM | VERIFIED VERBATIM | `feature-extractor-MBO-LOB/crates/hft-feature-core/src/mbo/mod.rs:329-330` — literal source: `let avg_queue_position = 0.0; let queue_volume_ahead = 0.0;` (unconditional) | All exports emit zero for these two feature indices |
| NEW-9 | `weighted_mid_price` collapse-when-one-side-is-zero exists in TWO sites | LOW | VERIFIED | `MBO-LOB-reconstructor/src/types.rs:594-612` + `feature-extractor-MBO-LOB/crates/hft-feature-core/src/derived.rs:115-123` | Discovery pass found only the LobState site |
| SG-9 | `MultiSymbolLob::reset_symbol` uses `reset()` while `reset_all()` uses `full_reset()` — asymmetric (see F-016) | LOW (blast radius zero) | VERIFIED | `multi_symbol.rs:152-173` | Zero downstream consumers in production; bug exists in dead-code surface only. Would become MEDIUM if `MultiSymbolLob` is wired in |

### Verified in code, empirically inactive in this dataset

| ID | Title | Status | Empirical |
|----|-------|--------|-----------|
| SG-3 | Production uses `CrossedQuotePolicy::Allow` (default) | VERIFIED in code, REFUTED empirically | 0 crossed quotes / 0 locked quotes across 234 days / 2.77B messages |
| SG-4 | DBN `flags` field dropped (including `SNAPSHOT`) | VERIFIED in code, REFUTED empirically | 0 `SNAPSHOT`, 0 `MAYBE_BAD_BOOK`, 0 `TOB`, 0 `MBP`, 4 `BAD_TS_RECV` (out of 61M sampled records) |

### Documentation drift (verified deviations from doc claims)

| ID | Title | Verdict | Detail |
|----|-------|---------|--------|
| F-019 | `LobState` size doc claim vs actual | DOC OFF BY ~16 BYTES | Doc: ~560 B; actual: ~576 B; test asserts only loose `500 < x < 700`. See §7 F-019. |
| F-020 | `MboMessage` size doc claim vs actual | DOC OFF BY 8-16 BYTES | Doc: 32 B; actual: 40 B (with reorder) or 48 B (without); `Option<i64>` cannot use a niche. See §7 F-020. |

### Phase 2 additions (post-validation; from VVN1 + VVN2 verifiers)

These findings were surfaced by VV5 in less-deeply-audited modules and re-verified by Phase-2 verifiers. Two of the original 17 (N-5 schema column-name drift; N-7 MBO writer rows_seen) were REFUTED on re-verification and are not recorded as F-NNN entries.

| ID | Title | Severity | Status | Verified by | Empirical / Trigger |
|----|-------|----------|--------|-------------|---------------------|
| F-024 | `DbnBridge::convert(...)` failure aliases to clean EOF when `skip_invalid=false` (second silent-EOF mode beyond F-002) | HIGH (library API surface) / N/A (export bin uses `skip_invalid=true`) | VERIFIED | `loader.rs:380-396` | Production export bin SAFE; other library callers VULNERABLE |
| F-029 | Volatility computer silently drops returns >10% per sample (circuit-breaker events erased) | HIGH (only fires during high-volatility regimes — circuit-breaker triggers, IPO-day moves, halt-resume gaps, overnight gaps; NVDA's 2025-2026 dataset has had no such events on NVDA itself) | VERIFIED | `experimental/volatility.rs:175-210` | Affects features 106, 107, 109, 110, 111 during high-vol regimes |
| F-021 | Silent reconstructor-error swallow at `export_to_parquet.rs:411` | MEDIUM | VERIFIED | Per-day partial parquet rows silently lost | Fires on every export run |
| F-022 | Signed-to-unsigned wraparound in `MinIntervalNs` downsampler | MEDIUM (latent-but-catastrophic per Lesson 5 caveat) | VERIFIED | `lob_writer.rs:175-185` | Fires only when `MinIntervalNs(_)` configured + non-monotonic timestamps |
| F-026 | Hot-store decompress: no integrity verification + race window | MEDIUM | VERIFIED | `hotstore.rs:368-374, 388, 404` | Race window real under concurrent decompress; integrity gap real under any prior-interrupted-write |
| F-027 | `MultiLevelOfiTracker` per-level appearance/disappearance false spikes | MEDIUM | VERIFIED | `ofi/multi_level.rs:161-186` | Active when `mlofi`/`kolm_of` groups enabled (features 116-147) |
| F-028 | `SQRT_ANNUAL_TRADING_SECONDS = 2430.0` hardcoded (US-equity-RTH only) | MEDIUM | VERIFIED | `experimental/volatility.rs:29-33` | Mis-scales features 106, 107, 111 if dataset is non-equity (108-110 unaffected) |
| F-030 | `BatchProcessor` in `FailFast` mode runs ALL files before reporting failure | MEDIUM (operational) | VERIFIED | `batch.rs:549-558, 599-635` | Default error mode |
| F-023 | `pipeline.rs:159` silent `timestamp = 0` fallback | LOW (defensive gap; doesn't fire under DBN ingestion) | VERIFIED-WITH-NUANCE | `pipeline.rs:158-163` + `adapters.rs:153` | Latent; activates on synthetic inputs or non-DBN ingestion |
| F-025 | `state.levels as u8` silent truncation | LOW (latent; would activate if `MAX_LOB_LEVELS > 255`) | VERIFIED-WITH-NUANCE | `export/batch.rs:88` | Cannot fire with current `MAX_LOB_LEVELS = 20` |
| F-031 | `BatchProcessor` empty-file list silent success | LOW | VERIFIED | `batch.rs:564-574` | Operator-typo path |
| F-032 | `decompress_to_hot_store.rs` accepts ANY `.zst` (not just `.dbn.zst`) | LOW-MEDIUM | VERIFIED | `bin/decompress_to_hot_store.rs:164` vs `:182` | Operator-typo path; would silently ingest non-MBO data |
| F-033 | `extract_date` returns FIRST 8-digit run in filename | LOW | VERIFIED | `bin/export_to_parquet.rs:356-368` | Brittle under non-canonical filename patterns |
| F-034 | `EveryN(0)` silent reinterpretation + undocumented "first-row-always-written" semantic | LOW | VERIFIED-WITH-NUANCE | `lob_writer.rs:171-174` | Semantic clarity issue; no correctness bug under documented usage |

### REFUTED on re-verification (do NOT add as F-NNN)

| Phase-2 ID | Title | Why refuted |
|------------|-------|-------------|
| N-5 | "Schema column-name drift between schema (`levels`) and batch (`level_count`)" | Arrow `RecordBatch` is built positionally via `RecordBatch::try_new(schema, columns)`; the Rust struct field name `level_count` is private and never serialized. The Parquet column name is the schema's `"levels"`. No external contract drift. (At most a low-severity readability nit.) |
| N-7 | "MBO writer's `rows_seen == rows_written` is a fake metric" | `MboEventWriter` has no downsampling — every seen event is written. Setting `rows_seen = rows_written` is an honest equality, not a fabricated metric. (Reuse of `ParquetExportStats` struct with `LobSnapshotWriter` justifies the symmetric assignment.) |

### Latent (will fire under future refactors / scenarios)

| ID | Title | Status | Trigger |
|----|-------|--------|---------|
| LB-1 | `Pipeline::reset()` does not reset the LOB | VERIFIED, latent-primed | Anyone hoists `lob` from local-in-`process_messages` to a Pipeline field |
| LB-2 | Saturating arithmetic on `total_size` cache silently masks invariant violations | VERIFIED, today-masked | Sizes that overflow u32 (NVDA scale unreachable; FX/futures scale would trigger) |
| LB-4 | `BookSnapshot::bid_size`/`ask_size` returns `Some(0.0)` for empty slots | VERIFIED as trait-contract violation | Future consumer using `is_some()` to detect occupancy |
| LB-5 | `microprice` collapses to the zero-size side (NOT the opposite side as the discovery pass said) when one side has 0 size | VERIFIED, direction corrected | If `update_order_size(.., 0)` ever gets called by reconstructor (currently not exercised) |
| LB-6 | `Modify` always destroys queue position even for same-price size-only modifies | VERIFIED, cosmetic today | If a queue-position-aware feature consumer is added that reads `PriceLevel.iter()` order |

### Refuted (do not work on these — phantom mechanisms or orthogonal misattributions)

| ID | Title | Why refuted |
|----|-------|-------------|
| SG-1 | "Mid-session Action::Clear poisons consumer OFI for the rest of the day" — was the #1 ranked smoking gun | Action::Clear messages carry `order_id=0, size=0, price=0` and are filtered by `is_system_message()` at `pipeline.rs:142-144` BEFORE the reconstructor sees them. `self.reset()` is never called. `on_book_update` is never invoked with an empty book. The OFI computer never sees the false discontinuity. Additionally, even if Clear DID propagate, `OfiComputer::update()` correctly overwrites `prev_*` on empty-book passes (lines 130-138). The discovery pass got both mechanism and consequence wrong. |
| SG-8 | "Cancel-of-missing biases features 51/55/89" | The named features (`cancel_rate_ask`, `net_cancel_flow`, `cancel_asymmetry`) do not read `LobState.triggering_action`. They consume `MboOrderEvent` via `engine.on_order_event`, which reads `msg.action` directly from the raw bridge output, NOT from the post-mutation snapshot. The bug only affects `mbo-statistical-profiler/src/trackers/ofi.rs:607-613` (a research tool, not the production training pipeline). |
| "begin_day(...) does not exist anywhere in production" | Discovery-pass claim. Refuted: `begin_day` exists as a fully-implemented trait method in `mbo-statistical-profiler/src/lib.rs:80` and `opra-statistical-profiler/src/lib.rs:60`, with ~18 production overrides, 2 orchestrator call sites (`mbo-statistical-profiler/src/profiler.rs:71`, `opra-statistical-profiler/src/profiler.rs:96`), and 2+ regression-guard tests. CLAUDE.md is correct on this point. The discovery agent likely greppped only `MBO-LOB-reconstructor/` and missed both profiler crates. |

---

## §2 — Active Production Bugs (Detailed Root-Cause Analysis)

This section documents bugs that are active in production code paths and corrupting (or could corrupt) downstream data today.

### F-001 — TimeBasedSampler re-derives `day_epoch_ns`; static `utc_offset_hours` ignores DST [SG-7]

**Severity**: HIGH (active in production for ~70% of trading days)

**Status**: VERIFIED (code + 15+ production TOML configs + manual DST analysis)

**Code path**:

`feature-extractor-MBO-LOB/crates/hft-sequence-engine/src/sampling.rs:609-628`:

```rust
fn compute_market_open_ns(&self, timestamp_ns: u64) -> u64 {
    let day_epoch_ns = timestamp_ns - (timestamp_ns % NS_PER_DAY);  // RE-DERIVED per call
    let open_local_ns = 9 * NS_PER_HOUR + 30 * NS_PER_MINUTE;
    let offset_ns = (self.utc_offset_hours.unsigned_abs() as u64) * NS_PER_HOUR;
    if self.utc_offset_hours < 0 {
        day_epoch_ns + open_local_ns + offset_ns
    } else {
        day_epoch_ns + open_local_ns.saturating_sub(offset_ns)
    }
}
```

`utc_offset_hours` is a struct field set ONCE in the constructor (`sampling.rs:595-606`) and read directly. It is sourced from a static TOML field via `DatasetConfig::build_sampler` (`crates/hft-extractor/src/config.rs:513-516`). There is no `update_offset(...)` method, no per-day refresh, no setter. `Sampler::should_sample()` (`sampling.rs:660`) does not refresh it. `initialize_grid()` is called only once via `if !self.initialized`.

**Root cause** (from the roots):

The function takes `timestamp_ns` as input, derives `day_epoch_ns` from it (correctly), then combines with `self.utc_offset_hours` to compute the UTC nanoseconds of the trading day's 09:30 local time. The flaw is conceptual: `utc_offset_hours` is treated as a constant of the deployment, but the US Eastern timezone alternates between EST (UTC-5) and EDT (UTC-4) twice per year. The function has all the information needed to compute the correct offset (it has the timestamp, from which year/month/day can be derived), but it does not do so.

For an EDT day with `utc_offset_hours = -5` (the value in every production TOML):
- The computed `market_open_ns` = `day_epoch + 14:30 UTC` (correct for EST: 09:30 EST = 14:30 UTC).
- The actual EDT market open is `09:30 EDT = 13:30 UTC`.
- Off by exactly +1 hour for the entire EDT day.

The grid is then anchored to the wrong open. With a 60s sampling interval, every sample boundary on every EDT day is shifted by 1 hour. Time-based features (`session_progress`, `minutes_since_open`, `time_bucket`, `dt_seconds` indirectly) all carry this shift.

**Why the fix exists in the workspace but isn't consumed**:

`hft-statistics/src/time/regime.rs:70-83` defines `utc_offset_for_date(year, month, day) -> i32`, which correctly returns `-4` for EDT and `-5` for EST based on the second-Sunday-of-March / first-Sunday-of-November rule. This function IS used elsewhere in the codebase:

- `feature-extractor-MBO-LOB/crates/hft-feature-core/src/experimental/seasonality.rs:80-86` (DST-aware ✓)
- `feature-extractor-MBO-LOB/crates/hft-feature-core/src/signals/mod.rs:260-270` (DST-aware ✓)

But `hft-sequence-engine/Cargo.toml` does not depend on `hft-statistics`, and `TimeBasedSampler` does not consume the offset function.

**Empirical scope**:

NVDA dataset spans 2025-02-03 to 2026-01-07. DST 2025 starts March 9 (2nd Sunday March), ends November 2 (1st Sunday November). Counting trading days (US equity holidays — Jan 1, Jan 20 MLK, Feb 17 Presidents, Apr 18 Good Friday, May 26 Memorial, Jun 19 Juneteenth, Jul 4, Sep 1 Labor, Nov 27 Thanksgiving, Dec 25 — excluded):

- Total trading days: **234** (range partition: pre-DST [2025-02-03, 2025-03-08] + EDT window [2025-03-09, 2025-11-01] + post-DST [2025-11-02, 2026-01-07])
- EDT trading days: **165 (≈70.5%)**
- EST trading days: **69 (≈29.5%)**

(Validation correction: an earlier draft cited "170/242"; both numerator and denominator were off, but the 70% ratio is exact. Re-validated by VV3 against the canonical day count used elsewhere in this document.)

Every TOML in `feature-extractor-MBO-LOB/configs/*.toml` that sets `utc_offset_hours` uses `-5` (verified by `grep -rn`). Verified configs (15 in total): `e5_timebased_15s.toml:60`, `e5_timebased_30s.toml:62`, `e5_timebased_60s.toml:63`, `e5_timebased_120s.toml:63`, `nvda_xnas_98feat_timebased_5s.toml:66`, plus all 10 universality configs (hood, dkng, fang, ibkr, isrg, mrna, pep, snap, zm, crsp).

**Adjacent failure mode**:

The discovery pass also claimed that "post-market at 19:00 EST = 00:00 UTC of the next calendar day flips `day_epoch_ns` to the wrong day." This sub-claim is INCORRECT for canonical RTH+post-market windows. RTH on EST closes at 16:00 ET = 21:00 UTC, well before UTC midnight. The day-flip risk only fires for data extending past ~7 PM ET, which is not standard. The 1-hour DST shift is the real and dominant failure mode.

**What corrupts**:
- Any time-based feature whose grid is anchored to the wrong open.
- All MBO/derived features computed at the boundary of session windows.
- Any sample-stride-aligned label that uses absolute clock time.

**Surface area for redesign**:
- The simplest fix is to compute `(year, month, day)` from `timestamp_ns` inside `compute_market_open_ns` and call `utc_offset_for_date(year, month, day)` instead of reading `self.utc_offset_hours`. This requires adding `hft-statistics` as a dependency to `hft-sequence-engine`.
- A more principled fix is to introduce a `begin_day(day_index, utc_offset, day_epoch_ns)` lifecycle hook (parallel to the one that already exists in `mbo-statistical-profiler` and `opra-statistical-profiler`). This would close the C4 anti-pattern at three sites simultaneously (see §5).

**Upstream availability update (2026-04-26 freshness check)**: `hft-statistics` PR-08 (shipped 2026-04-26) provides `RthSession::UsNyseNasdaq` + chrono-tz IANA tzdata + DST-aware `utc_offset_for_date(year, month, day)` under the default-on `time-session` feature flag. **The fix infrastructure is ready**; the only blocker is `hft-sequence-engine`'s "Zero external dependencies beyond std" policy (declared at `crates/hft-sequence-engine/Cargo.toml:6, 10-11`). The engineer applying the F-001 fix needs to either (a) relax that policy and add `hft-statistics` as a dep, or (b) push the per-day offset computation upstream into `hft-extractor`'s `Pipeline` (which currently calls `pipeline.reset()` between days but does NOT pass a date or invoke any sampler-side day hook). Option (b) keeps `hft-sequence-engine` policy-clean but requires a new `Sampler::on_new_day(date)` trait method — a coordinated change with broader review surface.

---

### F-002 — Silent stream termination on non-MBO records [NEW-1, supersedes SG-5]

**Severity**: HIGH (active in production whenever a DBN file contains a non-MBO record)

**Status**: VERIFIED (code trace through reconstructor's loader and dbn library)

**Code path**:

`MBO-LOB-reconstructor/src/loader.rs:362-374`:

```rust
let dbn_msg_ref = match self.decoder.decode_record::<dbn::MboMsg>() {
    Ok(Some(msg)) => msg,
    Ok(None) => return None,            // End of file (clean EOF)
    Err(e) => {
        if self.skip_invalid {
            log::warn!("Failed to decode DBN record: {e}");
            self.stats.messages_skipped += 1;
            continue;
        } else {
            log::error!("Failed to decode DBN record: {e}");
            return None;                // Silent EOF aliasing
        }
    }
};
```

Default `skip_invalid: false` (`loader.rs:176`).

**The dbn 0.20 library's behavior** (verified at `~/.cargo/git/checkouts/dbn-2a1f7b3a87238bdb/d9fad40/`):

1. `rust/dbn/src/decode/dbn/sync.rs:307-322` — `decode<T>()` calls `rec_ref.get::<T>()`. If the record's `rtype` does not match the requested `T` (e.g., next record is a `STATUS`, `SYMBOL_MAPPING`, `SYSTEM`, `STATISTICS`, or `INSTRUMENT_DEF` while `T = MboMsg`), it returns `Err(crate::Error::conversion::<T>(...))`. So a non-MBO record produces a typed-dispatch error, not a silent skip.
2. `rust/dbn/src/error.rs:116-122` — `silence_eof_error` converts `io::ErrorKind::UnexpectedEof` to `Ok(None)`.
3. `rust/dbn/src/decode/dbn/sync.rs:331-358` — `decode_ref` calls `silence_eof_error` after `read_exact` on the length byte. So a partial-write or torn-off record at EOF is converted to `Ok(None)` — clean EOF and mid-record EOF are indistinguishable from the iterator's perspective.

**Production caller analysis**:

| Caller | Calls `.skip_invalid(true)`? | Status |
|--------|------------------------------|--------|
| `feature-extractor-MBO-LOB/crates/hft-extractor/src/pipeline.rs:107` (production extractor) | NO | Vulnerable |
| `mbo-statistical-profiler/src/profiler.rs:78` | NO | Vulnerable |
| `MBO-LOB-reconstructor/src/source.rs:362-368` (DbnSource wrapper) | NO | Vulnerable (transitively) |
| `MBO-LOB-reconstructor/src/bin/export_to_parquet.rs:384` | YES | Safe |
| `MBO-LOB-reconstructor/examples/process_nvda_single_day.rs:48` | YES | Safe |

**Root cause** (from the roots):

There are two distinct silent-failure modes that compose into one indistinguishable end-of-iteration outcome:

1. **Typed dispatch rejecting unexpected variants**: `decode_record::<dbn::MboMsg>` is a typed call. The dbn `mbo` schema is documented as MBO-only, but actual files may contain status/symbol-mapping/system records depending on schema version and gateway behavior. When such a record is encountered, the typed dispatch returns `Err(Conversion)`. With `skip_invalid=false`, the iterator returns `None` — same return as clean EOF.

2. **Mid-record EOF silenced upstream**: Even if the caller sets `skip_invalid=true`, the dbn library's `silence_eof_error` collapses `UnexpectedEof` to `Ok(None)` BEFORE returning to the loader. So a corrupt zstd block, a partial-write, or a network-truncated file all look like clean EOF.

The composition: an iterator that returns `None` for both legitimate EOF and several silent-data-loss scenarios.

**What corrupts**:
- Partial-day exports — the caller observes a clean iteration end but processed only the prefix of the file.
- Day-level statistics computed on truncated data.
- ML labels generated on partial sessions.
- Zero observability: there is no log at WARN level (only at ERROR for the `skip_invalid=false` decode-error case), and the iterator's `stats()` method (`loader.rs:402-404`) is never called by production callers anyway.

**Adjacent observability gap**: `MessageIterator::iter_messages` (`loader.rs:298-306`) consumes `self`; `LoaderStats` moves into the iterator and there is no caller-accessible API to read `messages_skipped` post-iteration. Even when `skip_invalid=true` swallows decode errors, the count is unreachable from the production pipeline.

**Surface area for redesign**:
- Change the iterator's `Item` type from `MboMessage` to `Result<MboMessage, _>`, propagating typed errors to the caller.
- Add a `finalize() -> Result<LoaderStats>` method that returns final stats AND surfaces "iteration ended due to error" vs "iteration ended at clean EOF."
- At the dbn library boundary, distinguish "clean EOF" from "torn record at EOF" — possibly by checking the file's size against the position consumed.

---

### F-003 — `MessageIterator` silently truncates on decode error or mid-record EOF [SG-5]

**Severity**: HIGH

**Status**: VERIFIED. This is essentially the same root cause as F-002, but framed at the iterator API level. They should be addressed together.

See F-002 for the full mechanism. The summary: the iterator collapses three failure modes (clean EOF, decode error with `skip_invalid=false`, mid-record EOF regardless of `skip_invalid`) into a single `None` return.

---

### F-004 — `executed_pressure` (feature 86) is event-count not signed-volume [SG-6]

**Severity**: MEDIUM (definitional mismatch with research literature; affects multiple features)

**Status**: VERIFIED (code trace + literature cross-check)

**Code path**:

`feature-extractor-MBO-LOB/crates/hft-feature-core/src/signals/mod.rs:159-163`:

```rust
// === Signal 86: executed_pressure ===
// trade_rate_ask = BUY-initiated, trade_rate_bid = SELL-initiated
// > 0 = buy pressure (bullish)
buffer[sig_range.start + 2] = trade_rate_ask - trade_rate_bid;
```

`feature-extractor-MBO-LOB/crates/hft-feature-core/src/mbo/mod.rs:124-125`:

```rust
let trade_rate_bid = medium.trade_count_bid as f64 / duration;
let trade_rate_ask = medium.trade_count_ask as f64 / duration;
```

`feature-extractor-MBO-LOB/crates/hft-feature-core/src/mbo/window.rs:107-112`:

```rust
(OrderAction::Trade, OrderSide::Bid) => { self.trade_count_bid += 1; }
(OrderAction::Trade, OrderSide::Ask) => { self.trade_count_ask += 1; }
```

**Root cause** (from the roots):

The signal code computes `trade_rate_X` as `count / duration`. The window's increment statement is unambiguously `+= 1`, not `+= event.size`. This means:

- `trade_rate_*` is **events per second**, not shares per second.
- `executed_pressure = trade_rate_ask - trade_rate_bid` is **net trade event count rate**, not signed volume.

The published microstructure literature (Cont, Stoikov & Talreja 2014 "Order book dynamics and limit order placement"; Bouchaud, Bonart, Donier & Gould 2018 "Trades, Quotes and Prices" §10-§11; Easley, López de Prado & O'Hara 2012 VPIN papers using BVC bucketed volume) all define order-flow imbalance and "executed pressure" in **signed-volume** terms.

**The evidence that this is a definitional drift, not an intentional choice**:

The same `SignalComputer` (same crate, same author trail) contains `OfiComputer::update()` at `crates/hft-feature-core/src/signals/ofi.rs:151-167`, which uses signed share quantities (`curr_bid_size as f64`, `prev_bid_size as f64`) per Cont et al. 2014 Eq. (1). So **the codebase contains both conventions in the same module** — feature 84 (`true_ofi`) is volume-based; feature 86 (`executed_pressure`) is count-based.

There is no code comment justifying the asymmetry. The contract file `contracts/pipeline_contract.toml:128` only defines `executed_pressure = 86` as an index without a formula. Documentation (`feature-extractor-MBO-LOB/CODEBASE.md:738`, `DATAFLOW_VISUALIZATION.md:280`) describes the formula as `trade_rate_ask - trade_rate_bid` without qualifying whether "rate" is count or volume.

**Adjacent affected features**:

- **Feature 88 `trade_asymmetry`** (`signals/mod.rs:172-179`): `(trade_rate_ask - trade_rate_bid) / total_trades`. Numerator and denominator share the duration term, so this is normalized count asymmetry, not volume-weighted.
- **Feature 89 `cancel_asymmetry`** (`signals/mod.rs:181-188`): purely count-based by construction. Cancels are incremented as `+= 1` in `window.rs:101-105` and never carry size.

**Adjacent bug discovered during validation (NEW-7)**:

`MboWindow.duration_seconds()` clamps to a 0.001s minimum (`window.rs:144-151`). For 100 events all at the same nanosecond (auction batch), `duration = 0.001s` and `trade_rate = 100,000 events/s`. The 0.001 floor means the *relative* dynamic range across same-nanosecond bursts is preserved but the *absolute* magnitude is compressed by ~1000× compared to instantaneous rate. For normalized features like `trade_asymmetry`, the 1000× factor cancels. For unnormalized `executed_pressure`, it does not.

**Adjacent NEW-7**: `MboWindow.first_ts` is only set on empty-buffer push (`window.rs:78-80`). After the buffer fills, `first_ts` stays anchored to a long-evicted event. `duration_seconds()` overestimates the window span; `trade_rate_*` underestimates the true rate by a stable factor. This bias is independent of the count-vs-volume issue but compounds with it.

**What corrupts**:
- Feature 86 (`executed_pressure`) — primary bias.
- Feature 88 (`trade_asymmetry`) — same bias, normalized.
- Feature 89 (`cancel_asymmetry`) — count-only by construction; cannot be made volume-weighted without a schema change to the window.
- Indirectly: any model that learned to weight these features assumes count-based semantics.

**Phase 2 enumeration extension** (from N-17 verification by VVN2):

The original F-004 narrative named features 86, 88, 89. Phase-2 verification confirmed that **9 additional MBO core features share the same definitional drift** (count-based, not volume-weighted): **48** `add_rate_bid`, **49** `add_rate_ask`, **50** `cancel_rate_bid`, **51** `cancel_rate_ask`, **52** `trade_rate_bid`, **53** `trade_rate_ask`, **54** `net_order_flow`, **55** `net_cancel_flow`, **56** `net_trade_flow`, **57** `aggressive_order_ratio`, **58** `order_flow_volatility`, **59** `flow_regime_indicator`. All derive from `MboWindow`'s `+= 1` count counters at `mbo/mod.rs:120-181`.

Two features that V5 initially flagged were REFUTED by VVN2:
- **Feature 65 `large_order_ratio`** (`mbo/mod.rs:273-278`): correctly count-based by definition — the question being asked IS "what fraction of events are large", so count is the natural unit. NOT the same drift as F-004.
- **Feature 66 `size_skewness`** (`mbo/mod.rs:283-296`): V5 claimed this was `large_order_imbalance` and count-based. VVN2 correction: feature 66 is `size_skewness` (size-aware, third moment of size distribution); the actual feature 75 IS `large_order_imbalance` and IS correctly volume-weighted at `mbo/mod.rs:463-474` (`large_bid + e.size as u64`). NEITHER 66 NOR 75 is part of this drift.

**Total enumerated affected features**: 12 (originally claimed: 14; V5's two errors were indexing). Indices: 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 86, 88, 89 (15 total when including the originally-named 86/88/89; 12 NEW per N-17).

**Surface area for redesign**:
- Add `total_trade_volume_bid/ask` AND `total_cancel_volume_bid/ask` (mirroring `total_volume_bid/ask` for adds) to `MboWindow`; compute volume-weighted rates instead of (or alongside) count-based rates.
- Decide if all 15 affected features should use the volume-weighted variant exclusively or if both conventions should be exposed.
- Document the formulation in `pipeline_contract.toml`.
- Fix `MboWindow.first_ts` post-eviction (F-012; separate from the count/volume decision but compounds with it).

---

### F-005 — `LobBatch::push` iterates writer's `levels` not state's `levels` [LB-3, today-active in export path]

**Severity**: MEDIUM (today-active in a specific reuse pattern; not catastrophic but stale data leaks to Parquet)

**Status**: VERIFIED + TODAY-ACTIVE

**Code path**:

`MBO-LOB-reconstructor/src/lob/reconstructor.rs:773, 789-794, 803`:

```rust
let levels = self.config.levels.min(MAX_LOB_LEVELS);
...
for i in 0..levels {
    state.bid_prices[i] = 0;
    state.bid_sizes[i] = 0;
    state.ask_prices[i] = 0;
    state.ask_sizes[i] = 0;
}
...
state.levels = levels;
```

`MBO-LOB-reconstructor/src/export/batch.rs:92`:

```rust
for i in 0..self.levels {
    self.bid_prices.push(state.bid_prices[i]);
    self.bid_sizes.push(state.bid_sizes[i]);
    ...
}
```

**Root cause** (from the roots):

`fill_lob_state_with_temporal` only clears positions `[0..self.config.levels]` of the 20-element fixed arrays. Positions `[self.config.levels..MAX_LOB_LEVELS]` retain whatever was there from a prior call.

`LobBatch::push` reads its level count from `self.levels` (the writer's configured count), NOT from `state.levels` (the state's actual filled depth). If the two disagree — which they will if the writer is configured for more levels than the reconstructor — the writer reads UNCLEARED positions from the state and writes them to Parquet.

The two configurations get out of sync in any of these scenarios:
1. The writer is constructed with `levels=20` but the reconstructor uses `config.levels=10`. Indices 10..20 in `state.bid_prices` are uninitialized zeros from the constructor.
2. The same `LobState` buffer is reused across two reconstructors with mismatched levels.
3. A future config change reduces `levels` mid-pipeline.

**What corrupts**:
- Parquet snapshot rows beyond actual book depth contain zeros (from the constructor) or stale data (in reuse).
- Downstream Parquet readers that iterate the full schema width see "level present at price 0 with size 0" rows for non-existent levels.

**Why this is today-active**: The discovery pass classified this as latent. The validation found that `LobBatch::push` already triggers it whenever a `LobSnapshotWriter` is configured with a different `levels` value than the `LobReconstructor`. The default (matched levels) is safe; any divergence is silent.

**Surface area for redesign**:
- Change `LobBatch::push` to iterate `0..state.levels` instead of `0..self.levels`.
- OR clear all `MAX_LOB_LEVELS` slots in `fill_lob_state_with_temporal` (simpler; small perf cost).
- Add a debug assertion: `debug_assert_eq!(state.levels, self.levels)` at the start of `push`.

---

### F-006 — Features 68 (`avg_queue_position`) and 69 (`queue_volume_ahead`) are unconditionally 0.0

**Severity**: MEDIUM (two contract features always emit zero signal)

**Status**: VERIFIED VERBATIM (clearest claim in the entire audit)

**Code path**:

`feature-extractor-MBO-LOB/crates/hft-feature-core/src/mbo/mod.rs:329-330`:

```rust
fn extract_queue(&self, book: &dyn BookSnapshot) -> [f64; 6] {
    // Features 68-69: queue position tracking (disabled by default → 0.0)
    let avg_queue_position = 0.0;
    let queue_volume_ahead = 0.0;
    ...
    [
        avg_queue_position,    // index 68
        queue_volume_ahead,    // index 69
        orders_per_level,
        level_concentration,
        depth_ticks_bid,
        depth_ticks_ask,
    ]
}
```

These two `let` bindings are unconditional. Grep confirms only **two** assignments to these names exist in the file (lines 329 and 330). There is no `if`, no `match`, no config-gated branch that ever sets a non-zero value. The `MBO-LOB-reconstructor`'s `QueuePositionTracker` (1,683 LOC, 23 unit tests, 5 integration tests) is never instantiated by the production extractor.

The comment cites "(disabled by default due to performance impact)" but production has no flag exposed. The path is dead, not gated.

**Root cause**: The feature contract documents indices 68 and 69 as `avg_queue_position` and `queue_size_ahead` (per `pipeline_contract.toml`, schema version 2.2). Production exports include columns named for these features. ML training reads them. They are always zero. Models trained on these features are training on a constant.

**Adjacent**: The same dead-code framing surrounds the legacy archived monolith at `feature-extractor-MBO-LOB/archive/monolith-v1/src/features/mbo_features/queue_features.rs`, which DID wire `QueuePositionTracker` conditionally — but that monolith is no longer compiled.

**What corrupts**:
- Two of 148 contract feature columns always emit zero.
- Models trained on these features assign them weight 0.0 (best case) or learn to use the zero signal as a noise injection (worst case).

**Surface area for redesign**:
- Either implement queue tracking (use `QueuePositionTracker` from `MBO-LOB-reconstructor`, or write a fresh integration), OR
- Mark features 68/69 as DEPRECATED in `pipeline_contract.toml` and either drop them from exports or rename to `_RESERVED_*`.

**Bonus findings from 2026-04-26 freshness check (VS3)**:
- **Dangling `include_queue_tracking` config flag**: `crates/hft-export-pipeline/src/config/features.rs:41` defines `pub include_queue_tracking: bool` (default `false` everywhere), exposed via TOML so users can write `include_queue_tracking = true` in their config. **No consumer reads this flag.** A grep across all crates finds only the field declarations — no `extract_queue` site checks it; no `MboComputer` field stores a `QueuePositionTracker`. Users who set the flag get false confidence that they enabled queue tracking when they actually didn't. This is a separate observability defect; consider either wiring it (gate the hardcoded zeros behind the flag once `QueuePositionTracker` is integrated) or removing it.
- **Contract/code naming skew**: `contracts/pipeline_contract.toml:99-101` defines feature 69 as `queue_size_ahead`, but `crates/hft-feature-core/src/mbo/mod.rs:330` uses the variable name `queue_volume_ahead`. Same feature, two names. Minor but worth aligning when remediating.

---

### F-007 — `LobStats.errors` field is dead code [NEW-2]

**Severity**: MEDIUM (silent observability failure; affects monitoring dashboards)

**Status**: VERIFIED (grep across `MBO-LOB-reconstructor/src/`)

**Root cause**:

The struct field `errors: u64` is declared on `LobStats` (in `MBO-LOB-reconstructor/src/lob/reconstructor.rs`), serialized to `_reconstruction_stats.json`, but never incremented anywhere in the codebase. Any operator reading the stats file sees `errors: 0` regardless of the actual error count.

This is the same anti-pattern as NEW-3 (`bytes_read`): a counter exists in code and contracts, but the increment site is missing. There is no compile-time check that catches this; both fields silently report zeros.

**What corrupts**:
- Data-quality monitoring dashboards report "no errors" even when errors occur.
- Manual inspection of `_reconstruction_stats.json` provides false confidence.
- Any automated alerting based on `errors > threshold` never fires.

**Surface area for redesign**:
- Either remove the dead field (and bump the stats schema), OR
- Wire it up: every `Err(_)` returned by `process_message_into` should increment `self.stats.errors` before propagating. Same for any defensive error paths in helper methods.

---

### F-008 — `LoaderStats.bytes_read` is dead code; `progress()` always returns 0% [NEW-3]

**Severity**: MEDIUM (silent observability failure)

**Status**: VERIFIED

**Code path**:

`MBO-LOB-reconstructor/src/loader.rs:99` (field declaration):

```rust
pub bytes_read: u64,
```

`MBO-LOB-reconstructor/src/loader.rs:414` (read site in `progress()`):

```rust
pub fn progress(&self) -> f64 {
    if self.stats.file_size == 0 {
        return 100.0;
    }
    (self.stats.bytes_read as f64 / self.stats.file_size as f64) * 100.0
}
```

Grep across the file: no assignment to `bytes_read` anywhere except the constructor (which sets it to 0). `MessageIterator::next()` only updates `messages_read` and `messages_skipped`.

**Root cause**:

The field is declared and read but never written. The Decoder owns the underlying byte position; exposing it would require either (a) wrapping the BufReader to count bytes consumed, or (b) querying the dbn library for current file position. Neither was implemented.

**What corrupts**:
- Long-running extractions show perpetual 0% progress.
- Stuck decompression has no observable signal.
- If `file_size == 0` (failed-stat case), `progress()` returns 100% — also misleading.

**Surface area for redesign**: Wrap the `BufReader` in a counting reader, or expose a position API on the dbn decoder.

---

### F-009 — `state.sequence` is `messages_processed` but documented and exported as exchange sequence [NEW-4]

**Severity**: MEDIUM (cross-repo contract drift)

**Status**: VERIFIED (producer + schema + consumer documentation)

**Code path**:

Producer (`MBO-LOB-reconstructor/src/lob/reconstructor.rs:802` and `998`):

```rust
state.sequence = self.stats.messages_processed;
```

Schema (`MBO-LOB-reconstructor/src/export/batch.rs:87`):

```rust
self.sequence.push(state.sequence);
```

Schema (`MBO-LOB-reconstructor/src/export/schema.rs:61`):

```rust
Field::new("sequence", DataType::UInt64, false),  // NON-nullable
```

(Validation correction: an earlier draft cited `schema.rs:62`; actual location is line 61. Line 62 in the same file declares the `levels` field. Verified by VV1.)

Consumer documentation (`MBO-LOB-analyzer/CODEBASE.md:179`): documents the `sequence` column as "Sequence number from exchange."

**Root cause** (from the roots):

The DBN `MboMsg` has a `sequence` field (the exchange's matching-engine sequence number). The reconstructor's bridge (`dbn_bridge.rs:47-65`) drops this field. The reconstructor then **invents its own** counter (`self.stats.messages_processed`, incremented at `reconstructor.rs:930`) and writes it to `state.sequence`. The export writer dutifully emits this value to a Parquet column named `sequence`. The downstream analyzer reads `sequence` and (per its documentation) treats it as the exchange sequence.

The drift has three layers:
1. **Field hijack**: a column named `sequence` carries a value with completely different semantics than what the name implies.
2. **Documentation misalignment**: the analyzer's `CODEBASE.md` describes the column as exchange-provided.
3. **Operational consequences**: any analyzer logic that assumes monotonicity-from-exchange (gap detection, dedup, replay reconciliation) is silently wrong. Within a single reconstructor run the values ARE monotonic (1, 2, 3, ...), but they reset on `full_reset()` (between days) and bear no relation to the exchange's actual sequence numbers.

**Adjacent**: System-message-skipped paths increment the snapshot's `state.sequence` from the unchanged `messages_processed` value (`reconstructor.rs:891-896`), so the snapshot inherits the PREVIOUS message's sequence. Two consecutive system-message-skipped snapshots can carry the same `sequence` value. (See NEW-5 for related observability issue.)

**What corrupts**:
- Cross-repo contract violation. Future analyzer code that relies on the documented semantic (exchange sequence) will silently produce wrong results.
- No reliable in-band gap-detection signal.

**Surface area for redesign**:
- Rename the field on `LobState` and the Parquet column to `events_processed` (semantic name).
- Thread the actual DBN `sequence` through `MboMessage` and add a separate column.
- Update `MBO-LOB-analyzer/CODEBASE.md` to match.

---

### F-010 — `system_messages_skipped` is silently always 0 in production [NEW-5]

**Severity**: MEDIUM (silent observability failure)

**Status**: VERIFIED

**Root cause**:

`feature-extractor-MBO-LOB/crates/hft-extractor/src/pipeline.rs:142-144` filters system messages BEFORE invoking the reconstructor:

```rust
if msg.is_system_message() {
    continue;
}
lob.process_message_into(&msg, &mut lob_state)?;
```

Inside the reconstructor, `process_message_into` (`reconstructor.rs:887-897`) ALSO has a system-message filter that increments `self.stats.system_messages_skipped`. Because the consumer's filter runs first, the reconstructor's filter NEVER fires in the production path. `lob.stats().system_messages_skipped` reads 0 indefinitely.

**Why this is silent**:
- Both filters use the same `MboMessage::is_system_message()` predicate (`types.rs:177-179`), so they cannot disagree on the filtering decision.
- The reconstructor's stat is exposed via `lob.stats()` and is also serialized to `_reconstruction_stats.json`.
- Operators reading the stats file see `system_messages_skipped: 0` and infer that no system messages occurred — wrong inference, since the consumer pre-filter ate them.

**What corrupts**:
- Same family as NEW-2 and NEW-3: a counter that exists, is exported, and reads zero — but for a different reason (this one IS incremented when triggered, but the trigger is short-circuited upstream).
- Any data-quality dashboard that uses this stat is misled.

**Surface area for redesign**:
- Either (a) remove the consumer's pre-filter so the reconstructor sees and counts system messages, OR (b) move the counter to the consumer side, OR (c) document that this stat is always 0 in the production path.

---

### F-011 — `delta_ns` silently saturates to 0 for identical-timestamp clusters [NEW-6]

**Severity**: LOW (silent loss of intensity signal during HFT bursts)

**Status**: VERIFIED

**Code path**:

`MBO-LOB-reconstructor/src/lob/reconstructor.rs:783-786, 994-997`:

```rust
let delta_ns = match (timestamp, previous_ts) {
    (Some(current), Some(prev)) if current > prev => (current - prev) as u64,
    _ => 0,
};
```

The strict `current > prev` comparison means `delta_ns = 0` for:
- (a) Backwards-time events (clock skew or out-of-order delivery).
- (b) Identical-timestamp events.

Identical-timestamp events are common in HFT data: multiple events stamped at the same nanosecond, the same MBO message after a system-skip path, auction batches at the same instant.

**Root cause**:

The discovery pass framed this as "saturates to 0 on backwards-time events" — that is correct but underspecified. The strict-greater-than comparison treats `current == prev` as a violation, which is overly conservative. A `delta_ns = 0` is a defensible value for same-timestamp events (it correctly conveys "zero time elapsed") but causes downstream `event_intensity()` and `delta_seconds()` to return `None` (`types.rs:398-420`), masking the burst exactly when it matters most.

**What corrupts**:
- Features that consume `event_intensity` or `delta_seconds` lose signal during high-intensity bursts.
- Burst detection / spike features get suppressed.

**Surface area for redesign**:
- Change the strict `>` to `>=`, returning `0` for same-timestamp events.
- AND/OR add a `same_timestamp_count` stat to expose the burst-cluster size.
- AND/OR have `event_intensity()` return `Some(f64::INFINITY)` for `delta_ns == 0 && current == prev`.

---

### F-012 — `MboWindow.first_ts` is not updated on eviction [NEW-7]

**Severity**: MEDIUM (silent rate underestimation in all `*_rate_*` features after buffer fills)

**Status**: VERIFIED

**Code path**:

`feature-extractor-MBO-LOB/crates/hft-feature-core/src/mbo/window.rs:71-88` (verbatim):

```rust
#[inline]
pub fn push(&mut self, event: MboEvent) {
    if self.events.len() == self.capacity {
        let old = self.events.pop_front().unwrap();
        self.decrement_counters(&old);
    }

    if self.events.is_empty() {
        self.first_ts = event.timestamp;
    }
    self.last_ts = event.timestamp;

    self.increment_counters(&event);
    self.events.push_back(event);

    self.size_cache_dirty = true;
    self.size_stats_dirty = true;
}
```

(Validation correction: an earlier draft of this code block reordered the eviction-check and empty-check operations. The actual source does eviction-check FIRST, then empty-check. The substantive bug — `first_ts` not refreshed after eviction — is unchanged. Verified verbatim by VV1.)

**Root cause** (from the roots):

`first_ts` is intended to mark "the timestamp of the oldest event currently in the window." On the first push to an empty buffer, it is correctly set. On subsequent pushes that trigger eviction, the front element is removed (lines 73-76) but the empty-check at line 78 evaluates to false (the buffer still contains capacity-1 events), so `first_ts` is not refreshed to point at the new front's timestamp. Result: `first_ts` is anchored to a long-evicted event after the buffer fills.

`duration_seconds()` (`window.rs:144-151`) computes `(last_ts - first_ts) / 1e9`. With a stale `first_ts`, this **overestimates** the window's actual time span. Consequently `count / duration` **underestimates** the true rate by a stable factor.

**Quantitative impact**: For a buffer of capacity N events that has been running for time T after fill, the true window duration is `T - first_evicted_ts`, but the computed duration is `T - first_event_ever`. As N events evict, the bias accumulates. For a 1000-event window over a 5-minute session, the bias is bounded (eviction churn is fast), but for any `_rate_*` feature, the magnitude is systematically depressed.

**Adjacent**: This compounds with F-004 (count vs volume) to make rate-based features doubly suspect. Both must be addressed for `executed_pressure`, `add_rate_*`, `cancel_rate_*`, `trade_rate_*`, `trade_asymmetry`, `cancel_asymmetry` to be accurate.

**Surface area for redesign**:
- After eviction, set `self.first_ts = self.events.front().map(|e| e.timestamp).unwrap_or(self.last_ts)`.
- Add a debug-only assertion that `last_ts >= first_ts`.

---

### F-013 — Modify-of-missing-order silently becomes Add at the modify's NEW price [SG-2, downgraded]

**Severity**: LOW–MEDIUM (the heuristic is reasonable; the missing observability is the real defect)

**Status**: VERIFIED but OVERSTATED in discovery pass

**Code path**:

`MBO-LOB-reconstructor/src/lob/reconstructor.rs:494-510`:

```rust
fn modify_order(&mut self, msg: &MboMessage) -> Result<()> {
    let old_order = match self.orders.get(&msg.order_id) {
        Some(order) => *order,
        None => {
            // Order not found, treat as add
            return self.add_order(msg);
        }
    };
    self.remove_order_internal(msg.order_id, &old_order)?;
    self.add_order(msg)?;
    Ok(())
}
```

**Root cause** (from the roots):

When a Modify arrives for an order_id absent from `self.orders`, the function falls through to `add_order(msg)` — at `msg.price`, which is the Modify's NEW target price. The recovery is reasonable for warmup scenarios (orders that pre-existed before the daily file's start), since the alternative (drop the order) corrupts every subsequent event for that order_id.

**Why the discovery pass overstated**:
- Claimed "phantom orders, BBO shift" with HIGH severity.
- The behavior is essentially "treat-as-snapshot" recovery, not a phantom-introduction bug.
- For warmup, this is the only sensible default.

**Why there is still a real defect**:
- No `modify_order_not_found` counter exists. Compare to the symmetric `cancel_order_not_found` and `trade_order_not_found` counters on `LobStats` (`reconstructor.rs:213-229`).
- The discovery pass suggested counting it from "real export stats" — that counter does not exist, so the rate is unobservable.
- Without observability, we cannot distinguish "rare warmup recovery" (acceptable) from "common feed quirk" (concerning).

**Adjacent silent fallthrough**:

`add_order` at `reconstructor.rs:461-464` does the symmetric thing: if an order_id already exists, it routes silently to `modify_order`. So Add-of-existing also fires through the same code path. Combined, you have bidirectional silent fallthrough between Add and Modify with no diagnostic.

**Surface area for redesign**:
- Add `modify_order_not_found` and `add_order_id_collision` counters.
- Decide on policy: log at INFO level for each, or aggregate into the warning system, or expose via a per-message anomaly callback.
- Document the heuristic explicitly in the reconstructor's docstring.

---

### F-014 — `WarningTracker.record()` underflow on backwards wall clock [LB-7]

**Severity**: LOW (release behavior is dedup-history-clears, not catastrophic)

**Status**: VERIFIED + clarified

**Code path**:

`MBO-LOB-reconstructor/src/warnings.rs:325-329`:

```rust
let now = warning.recorded_at;
...
recent_list.retain(|(_, ts)| now - *ts < self.config.dedupe_window_ns);
```

**Root cause**:

`now` and `*ts` are both `u64` epoch nanoseconds derived from `SystemTime::now()` (`warnings.rs:162-165`). `SystemTime::now()` returns wall-clock time; if the wall clock moves backwards (NTP correction, hypervisor live migration, manual user clock change), `now < *ts` can occur. The unsigned subtraction `now - *ts` then underflows.

**Behavior**:
- **Debug build**: panic on underflow.
- **Release build**: `0u64 - large` wraps to `u64::MAX - large + 1`, an enormous number. The retain predicate `(huge < dedupe_window_ns)` evaluates to FALSE → entries are EVICTED. Net effect: the dedup history clears for that bucket. The dedup is briefly disabled (one window of duplicate warnings) and then state recovers naturally on the next normal-time warning.

**Discovery-pass framing correction**:

The discovery pass said "silent dedup permanently disabled." The validation found that release behavior is "dedup history flushes for the affected category," after which dedup resumes normally on the next warning. Less catastrophic than framed.

**What corrupts**:
- Debug builds in production (rare): process panic.
- Release builds: brief duplicate-warning burst per backwards-clock event.

**Surface area for redesign**:
- Use `now.saturating_sub(*ts) < dedupe_window_ns`, OR
- Use `now.checked_sub(*ts).map_or(false, |d| d < dedupe_window_ns)`, OR
- Use `Instant::now()` (monotonic) instead of `SystemTime::now()` for dedup window math; reserve `SystemTime` for the human-readable timestamp display.

---

### F-015 — `weighted_mid_price` collapse-when-one-side-is-zero exists in TWO sites [NEW-9, supersedes LB-5]

**Severity**: LOW (the path is not exercised in production today, but the bug exists in two parallel implementations)

**Status**: VERIFIED, with one direction correction to the discovery pass

**Code path** (Site 1):

`MBO-LOB-reconstructor/src/types.rs:594-612` — `microprice` (verbatim):

```rust
#[inline]
pub fn microprice(&self) -> Option<f64> {
    match (self.best_bid, self.best_ask) {
        (Some(bid), Some(ask)) => {
            let bid_size = self.bid_sizes.first().copied().unwrap_or(0) as f64;
            let ask_size = self.ask_sizes.first().copied().unwrap_or(0) as f64;
            let total_size = bid_size + ask_size;

            if total_size > 0.0 {
                let bid_f = bid as f64 / NANODOLLARS_PER_DOLLAR_F64;
                let ask_f = ask as f64 / NANODOLLARS_PER_DOLLAR_F64;
                Some((bid_f * ask_size + ask_f * bid_size) / total_size)
            } else {
                None
            }
        }
        _ => None,
    }
}
```

(Validation correction: an earlier draft used `?` operator with `total_size <= 0.0` early return. The actual source uses `match` with `total_size > 0.0` positive branch. The substantive Stoikov-collapse behavior is identical either way. Verified verbatim by VV1.)

**Code path** (Site 2):

`feature-extractor-MBO-LOB/crates/hft-feature-core/src/derived.rs:115-123` — `weighted_mid_price`:

```rust
let best_bid_size = book.bid_size(0).unwrap_or(0.0);
let best_ask_size = book.ask_size(0).unwrap_or(0.0);
let total_best_size = best_bid_size + best_ask_size;
let weighted_mid_price = if total_best_size > 0.0 {
    (best_bid * best_ask_size + best_ask * best_bid_size) / total_best_size
} else {
    mid_price
};
```

**Root cause** (from the roots):

The Stoikov microprice formula weights each side's price by the OPPOSITE side's size. When one side has zero size, the formula collapses:

- If `bid_size = 0, ask_size > 0`: numerator = `bid * ask_size + ask * 0 = bid * ask_size`. Divided by `ask_size + 0 = ask_size`. Result: **bid** (NOT ask, as the discovery pass said).
- Symmetrically, if `ask_size = 0`, result is **ask**.

The microprice naturally tracks the side with depleted liquidity. Some practitioners view this as a feature (signals impending sweep); others as a bug. The behavior is the standard Stoikov microprice convention.

**Discovery-pass error**: The discovery pass said "microprice ≡ ask, when bid_size=0." The validation found this is backwards — it collapses to `bid` (the zero-size side), not `ask`.

**Why two sites matter**:
- The discovery pass flagged only the LobState site.
- The same logic exists in the feature-extractor's `derived.rs`. If the bug needs fixing, both sites must be fixed in lockstep, or the consumer-side adapter must guarantee they cannot disagree.

**Why this isn't firing today**:
- The reconstructor's input filter (`is_system_message`: `order_id == 0 || size == 0 || price <= 0`) drops messages with `size == 0` BEFORE they reach the book.
- `update_order_size(.., 0)` is not called by the reconstructor (only by `QueuePositionTracker`, which is dead code).
- So a price level with `total_size == 0` should not occur in normal flow.

**Latent triggers**:
- Future code that calls `update_order_size(.., 0)`.
- Future logic that allows zero-size orders for partial-fill semantics.
- Asymmetric levels (only one side has data) — actually a one-sided book triggers `derived.rs:72-81`'s early return before reaching the microprice path, so this path is doubly latent.

**Surface area for redesign**:
- Decide if the Stoikov collapse is intended behavior (document it) or a bug (return `None` if either size is 0).
- Unify the two implementations behind a single function.

---

### F-016 — `MultiSymbolLob::reset_symbol` uses `reset()` while `reset_all()` uses `full_reset()` [SG-9]

**Severity**: LOW (asymmetry verified, but `MultiSymbolLob` has zero downstream consumers)

**Status**: VERIFIED, blast radius zero

**Code path**:

`MBO-LOB-reconstructor/src/lob/multi_symbol.rs:151-173`:

```rust
pub fn reset_symbol(&mut self, symbol: &str) -> Result<()> {
    let lob = self.lobs.get_mut(symbol).ok_or(...)?;
    lob.reset();          // preserves stats
    Ok(())
}

pub fn reset_all(&mut self) {
    for lob in self.lobs.values_mut() {
        lob.full_reset(); // clears LobStats
    }
    self.stats.total_messages = 0;
    for count in self.stats.messages_per_symbol.values_mut() {
        *count = 0;
    }
}
```

`reset_symbol` does NOT zero `MultiSymbolStats.messages_per_symbol[symbol]` either. So per-symbol counts at the manager level diverge from per-symbol counts at the reconstructor level after any single-symbol reset.

**Why blast radius is zero**: Grep confirms `MultiSymbolLob` has no downstream consumers. Only used in `examples/multi_symbol.rs:8,16` and the in-file test module.

**What this signals**: If `MultiSymbolLob` is ever wired into production, this asymmetry becomes a HIGH-severity silent stats-leakage bug. Today it is dead code with bad ergonomics.

**Surface area for redesign**:
- Decide: either both `reset_symbol` and `reset_all` should preserve stats, OR both should clear stats. The asymmetry serves no purpose.
- If `MultiSymbolLob` is intended to be wired in, fix this BEFORE wiring.

---

### F-021 — Silent reconstructor-error swallow in production export binary [Phase 2 / N-1]

**Severity**: MEDIUM (active in every export run; partial observability via separate `_reconstruction_stats.json`)

**Status**: VERIFIED (Phase 2 verifier VVN1)

**Code path**:

`MBO-LOB-reconstructor/src/bin/export_to_parquet.rs:411` (verbatim):

```rust
for msg in loader.iter_messages()? {
    msg_count += 1;

    if let Some(ref mut mw) = mbo_writer {
        mw.write_event(&msg)?;
    }

    if lob.process_message_into(&msg, &mut lob_state).is_ok() && lob_state.is_valid() {
        lob_writer.write_snapshot(&lob_state)?;
    }
}
```

**Root cause** (from the roots):

The export driver invokes `lob.process_message_into(...).is_ok()` and discards every error variant — `TlobError::CrossedQuote`, `LockedQuote`, `OrderNotFound`, `InvalidPrice`, etc. — without log, stat increment, or surface counter. Each such error becomes a missing parquet snapshot for that message; downstream consumers cannot distinguish "no snapshot because system message filtered" from "no snapshot because reconstructor errored". Worse: the asymmetric error handling within the same loop — `mw.write_event(&msg)?` (line 408) propagates with `?` while `lob.process_message_into(...)` is silent — means MBO write failures abort the entire day, but LOB write source failures silently produce a partial parquet with no audit trail.

**Mitigating partial observability**: The reconstructor itself updates internal stats on errors (`lob.stats()` is exposed and serialized to `_reconstruction_stats.json` at line 426), so an operator who reads the stats file CAN see total error counts per category. But the export-level row-loss count (rows that would have been valid LOB snapshots had reconstruction not errored) is invisible — the operator must reconstruct it by subtraction.

**What corrupts**: Per-day parquet row counts diverge silently from per-day MBO message counts. Cross-checking via the `_reconstruction_stats.json` file is possible but not automated.

**Surface area for redesign**:
- Replace `.is_ok()` with `match` arm: `Ok(()) => { ... } Err(e) => { /* increment a per-error-kind export counter, propagate or log */ }`.
- Add a `lob_write_skipped_due_to_error` counter to `ParquetExportStats` so the operator-visible stats file surfaces this rather than requiring inference.

---

### F-022 — Signed-to-unsigned wraparound in `MinIntervalNs` downsampler [Phase 2 / N-2]

**Severity**: MEDIUM (active only when `DownsampleStrategy::MinIntervalNs(_)` is configured AND input has non-monotonic timestamps; LATENT-CATASTROPHIC under §8 Lesson 5 caveat — would silently overwrite the downsampler's threshold)

**Status**: VERIFIED (VVN1)

**Code path**:

`MBO-LOB-reconstructor/src/export/lob_writer.rs:175-185` (verbatim):

```rust
DownsampleStrategy::MinIntervalNs(min_ns) => {
    let ts = match state.timestamp {
        Some(t) => t,
        None => return true,
    };
    match self.last_written_ts {
        None => true,
        Some(last) => (ts - last) as u64 >= *min_ns,
    }
}
```

**Root cause** (from the roots):

The expression `(ts - last) as u64` performs signed `i64` subtraction. When `ts < last` (out-of-order or backwards-time event), the result is negative; the cast to `u64` wraps to a near-`u64::MAX` value that always satisfies `>= min_ns` regardless of the configured threshold. The sample is therefore unconditionally written. For a cluster of out-of-order events, all of them bypass the downsampler in a burst, producing artifactual hot-spots in the parquet output. The pattern mirrors the LB-7 `WarningTracker` underflow but in a different module.

**Adjacent silent fallback**: `state.timestamp == None → return true` at line 179 silently disables downsampling on any LOB state whose timestamp wasn't populated. Same family as F-002's silent EOF aliasing.

**What corrupts**: Downstream parquet density profile during fast burst clusters or replay-driven OOO events. Operationally non-obvious because the downsampler appears to be working correctly on monotonic inputs.

**Surface area for redesign**:
- Use `ts.saturating_sub(last).max(0) as u64`, OR
- Compare as i64: `(ts - last) >= *min_ns as i64` (with a guard for `min_ns > i64::MAX` which is impossibly large), OR
- Use `(ts as i128 - last as i128).max(0) as u64`.
- Decide what to do for the `state.timestamp == None` case — likely should also be a hard skip rather than a hard write.

---

### F-023 — `pipeline.rs:159` silent `timestamp = 0` fallback (defensive gap, latent under DBN) [Phase 2 / N-3]

**Severity**: LOW (does not fire under DBN production path; defensive-coding gap exposed only by synthetic/test inputs or upstream invariant violations)

**Status**: VERIFIED-WITH-NUANCE (VVN2)

**Code path**:

`feature-extractor-MBO-LOB/crates/hft-extractor/src/pipeline.rs:158-163` (verbatim):

```rust
// Sampling decision
let timestamp_ns = msg.timestamp.unwrap_or(0).max(0) as u64;
let ctx = SamplingContext {
    timestamp_ns,
    event_volume: msg.size,
};
```

Same construct also appears at `crates/hft-extractor/src/adapters.rs:153` (`MboOrderEvent::timestamp_ns()`).

**Root cause** (from the roots):

`Pipeline::process_messages()` and `MboOrderEvent::timestamp_ns()` silently coerce `None` and negative timestamps to `0` (UTC epoch 1970-01-01). Combined with `TimeBasedSampler`'s grid alignment to 09:30 ET, a `0` timestamp would project sampling onto a 1970-anchored grid, producing arbitrary first-sample alignment. The DBN production path at `MBO-LOB-reconstructor/src/dbn_bridge.rs:63` always sets `Some(msg.hd.ts_event as i64)`, so this fallback **does NOT fire on the production code path**. It would fire on synthetic test inputs, on a corrupted DBN message with negative `ts_event`, or on any future non-DBN data source that doesn't enforce the invariant.

**Why this is documented despite being defensive-only**: This is exactly the failure pattern §8 Lesson 5 caveat addresses — a silent fallback to a meaningless value is acceptable today only because the upstream invariant holds. If the team ever adds a non-DBN ingestion path (live streaming, simulation harness, multi-venue), this becomes active.

**Surface area for redesign**:
- Replace silent `unwrap_or(0).max(0) as u64` with either:
  - Hard fail (`expect` with a descriptive message) at the boundary; OR
  - Skip the sample with a counter increment and a one-time WARN log; OR
  - Return a typed `Result` from `Pipeline::process_messages()` so the caller can decide.

---

### F-024 — `DbnBridge::convert(...)` failure aliases to clean EOF when `skip_invalid=false` (second silent-EOF mode) [Phase 2 / N-4]

**Severity**: HIGH for library API surface; N/A for production export binary (which sets `skip_invalid=true`)

**Status**: VERIFIED (VVN1)

**Code path**:

`MBO-LOB-reconstructor/src/loader.rs:380-396` (verbatim):

```rust
match DbnBridge::convert(dbn_msg_ref) {
    Ok(mbo_msg) => { ... }
    Err(e) => {
        if self.skip_invalid {
            log::warn!("Skipping invalid message: {e}");
            self.stats.messages_skipped += 1;
            continue;
        } else {
            log::error!("Invalid message: {e}");
            return None;
        }
    }
}
```

**Root cause** (from the roots):

`MessageIterator::next()` returns `None` for THREE distinct conditions, not the two F-002 documents:
1. Clean EOF (`Ok(None)`).
2. DBN decode error with `skip_invalid=false` (F-002's "typed dispatch" case).
3. **`DbnBridge::convert` failure** (e.g., on an unknown action/side byte) with `skip_invalid=false` (this finding).

The caller (Iterator consumer) cannot distinguish them. Production callers using the default `skip_invalid=false` would end iteration on the first convert failure (e.g., a single corrupt action byte from a DBN protocol upgrade) and silently produce a truncated stream. Only a `log::error!` line distinguishes the cases — no return code, no propagation through the iterator, no `messages_skipped` increment in the `skip_invalid=false` branch.

**Production exposure analysis**: `MBO-LOB-reconstructor/src/bin/export_to_parquet.rs:384` constructs the loader with `.skip_invalid(true)`, so the production export binary IS protected — convert failures are logged and skipped. Other library callers (e.g., the sister `feature-extractor-MBO-LOB/crates/hft-extractor/src/pipeline.rs:107`) do NOT opt in to `skip_invalid(true)` and ARE vulnerable.

**Adjacent observability gap**: In the `skip_invalid=false` path, `messages_skipped` is NOT incremented (only the log line records the failure). So the failure is silently invisible in `LoaderStats` too.

**Surface area for redesign**: Same as F-002 — change `MessageIterator::Item` to `Result<MboMessage, _>` so the three cases are distinguishable. Both F-002 and F-024 should be addressed together.

---

### F-025 — `state.levels as u8` silent truncation if `MAX_LOB_LEVELS` is raised above 255 [Phase 2 / N-6]

**Severity**: LOW (latent; would become MEDIUM if `MAX_LOB_LEVELS` is increased in a future config change)

**Status**: VERIFIED-WITH-NUANCE (VVN1)

**Code path**:

`MBO-LOB-reconstructor/src/export/batch.rs:88` (verbatim):

```rust
self.level_count.push(state.levels as u8);
```

`state.levels` is declared as `usize` at `types.rs:309`. The `as u8` cast wraps modulo 256.

**Root cause** (from the roots):

`MAX_LOB_LEVELS` is currently `20` (`types.rs:30`), and `LobState::new` clamps via `.min(MAX_LOB_LEVELS)` (`types.rs:375`), so `state.levels` is always in `[0, 20]` and the `as u8` cast is safe at present. There is no compile-time enforcement of `MAX_LOB_LEVELS <= 255`, no runtime `debug_assert!`, and no `try_from` validation. If a future config or refactor raises `MAX_LOB_LEVELS` above 255 (or if the public mutable `LobState.levels` field is bypassed and set externally to a larger value), the value silently wraps.

**Adjacent observation** (cross-cutting with C4-7): The `push()` body iterates `for i in 0..self.levels` (where `self.levels` is the `LobBatch` field, NOT `state.levels`), so the array length and the declared `level_count` are decoupled. A downstream consumer reading `levels=N` and `bid_prices.length=M` may see `N ≠ M` if the writer-vs-state level configuration drifts (this is the F-005 bug, surfaced again from a different angle).

**Surface area for redesign**:
- Add `const _: () = assert!(MAX_LOB_LEVELS <= 255, "level_count column is u8");` at the type definition site.
- Use `u8::try_from(state.levels).expect(...)` instead of `as u8`.

---

### F-026 — Hot-store decompress: no integrity verification on existing files; race window between concurrent decompresses [Phase 2 / N-8]

**Severity**: MEDIUM (active under any concurrent-decompress scenario; integrity gap silent under any prior-interrupted-write scenario)

**Status**: VERIFIED (VVN1)

**Code path**:

`MBO-LOB-reconstructor/src/hotstore.rs:368-374` (verbatim):

```rust
pub fn decompress<P: AsRef<Path>>(&self, compressed_path: P) -> Result<PathBuf> {
    let compressed_path = compressed_path.as_ref();
    let decompressed_path = self.decompressed_path_for(compressed_path);

    // Check if already decompressed
    if decompressed_path.exists() {
        log::info!(
            "Decompressed file already exists: {}",
            decompressed_path.display()
        );
        return Ok(decompressed_path);
    }
    ...
```

**Root cause** (from the roots):

Two distinct silent-corruption modes share the same decompression path:

1. **No integrity verification on existing files**: `decompress()` returns immediately on `decompressed_path.exists()` without size check, checksum, or zero-length test. If a previous decompress was interrupted before `fs::rename(temp_path, decompressed_path)`, the temp file is leaked and the final file is genuinely complete (this case is safe). But if the file appears via any *other* mechanism — manual copy, partial cp, NFS half-write, prior version with a now-different layout — the hot-path silently reuses corrupt content. The `--force` CLI flag exists but does not gate this `exists()` check.

2. **Race window with no lock**: Two threads or two processes calling `decompress()` concurrently for the same input both observe `exists() == false`, both write to the same `temp_path` (`decompressed_path.with_extension("tmp")` at line 388 — a deterministic name with no PID/UUID suffix), and the last `fs::rename` wins. The losing rename either fails (clobbering the survivor's `temp_path` cleanup at line 404) or replaces a freshly-renamed file mid-read.

**Documentation contradicts code**: The doc comment at `hotstore.rs:350-354` explicitly claims "Uses atomic file operations to handle concurrent decompression attempts. If two threads try to decompress the same file, one will succeed and the other will find the file already exists." This is FALSE within the race window where both threads pass the `exists()` check before either renames.

**What corrupts**: Hot-store cached files served as if valid; downstream parsing produces undefined behavior depending on exactly which bytes were partial.

**Surface area for redesign**:
- Add a per-process unique suffix to `temp_path` (`PID + nanosecond timestamp + random` is standard).
- Use `fs::OpenOptions::new().create_new(true)` to acquire an exclusive create lock on `temp_path`; fail-and-retry if another process holds it.
- Optionally checksum the existing decompressed file against an expected manifest before serving.
- Update the doc comment at line 350-354 to reflect actual behavior.

---

### F-027 — `MultiLevelOfiTracker` emits per-level OFI on level appearance/disappearance even when no order flow occurred [Phase 2 / N-9]

**Severity**: MEDIUM (active when experimental `mlofi` or `kolm_of` feature groups are enabled — features 116-147; silent magnitudes leak into model training)

**Status**: VERIFIED (VVN2)

**Code path**:

`feature-extractor-MBO-LOB/crates/hft-feature-core/src/ofi/multi_level.rs:161-186` (per-level OFI computation, verbatim):

```rust
let size_change = curr_size as f64 - prev_size as f64;

if is_bid {
    if curr_price > prev_price || (prev_price == 0 && curr_price > 0) {
        // Price improved or level appeared: new queue
        curr_size as f64
    } else if curr_price < prev_price || (curr_price == 0 && prev_price > 0) {
        // Price worsened or level disappeared: lost queue
        -(prev_size as f64)
    } else {
        // Same price: track size change
        size_change
    }
} else {
    // Ask side (inverted: lower price = improvement)
    if curr_price < prev_price || (prev_price == 0 && curr_price > 0) {
        -(curr_size as f64)
    } else if curr_price > prev_price || (curr_price == 0 && prev_price > 0) {
        prev_size as f64
    } else {
        -size_change
    }
}
```

**Root cause** (from the roots):

`MultiLevelOfiTracker::compute_level_ofi()` treats "level appeared" and "level disappeared" as full-queue OFI events at every depth, indistinguishable from a price-improvement at L1. Because deep levels (L5+) frequently flicker into/out of the visible top-N book as orders are added/cancelled at intermediate prices — without any actual flow event at that price — the tracker emits per-level OFI spikes (`+/- prev_size` or `+/- curr_size`) whenever the L5 slot's content changes due to displacement by a new level above.

Concretely: if L5 was occupied by an order at $99.95 and a new order arrives at $99.96 (creating a new L5 and pushing the old L5 to L6), the new L5's `curr_price=99.96` differs from the old L5's `prev_price=99.95`. The tracker fires `+curr_size` as if a new queue appeared, when in reality NO order flow occurred at this level — it's the same orders, displaced one slot. (Empirical flicker rate at L5+ for NVDA was not measured; the bug is verified at the code-logic level. A future verification could quantify the per-day rate by instrumenting the tracker.)

**When this fires**: `MultiLevelOfiTracker::new(KOLM_OF_LEVELS)` drives `kolm_of` features 128-147 via `on_book_update` on every transition (`crates/hft-feature-core/src/experimental/kolm_of.rs:100`). It also drives `mlofi` features 116-127. So this fires on the production code path whenever those experimental groups are enabled, contaminating per-level OFI features 116-147 with synthetic spikes that have no underlying order flow.

**Empty-to-empty case**: Correctly returns 0 at line 157-159 (verified). The bug is the displacement-vs-flow indistinguishability.

**What corrupts**: Features 116-147 carry artifactual spikes that are statistically indistinguishable from genuine order flow at the per-feature level. Models trained on these features may learn spurious correlations between book-displacement events and labels.

**Surface area for redesign** (non-trivial): Distinguishing "true level appearance" from "displacement due to a new level above" requires comparing the entire price ladder across the transition, not just per-slot deltas. One approach: track per-order-id position changes rather than per-slot price changes (similar to the `QueuePositionTracker` design but at multiple levels).

---

### F-028 — Hardcoded `SQRT_ANNUAL_TRADING_SECONDS = 2430.0` (US-equity-RTH only) [Phase 2 / N-10]

**Severity**: MEDIUM (silent dimensional miscalibration of features 106, 107, 111 if the dataset is not US-equity-RTH; ratio features 108-110 are unaffected)

**Status**: VERIFIED (VVN2)

**Code path**:

`feature-extractor-MBO-LOB/crates/hft-feature-core/src/experimental/volatility.rs:29-33` (verbatim):

```rust
/// Annualization factor for intraday data.
///
/// Assumes ~6.5 hours of trading, 252 days per year.
/// sqrt(252 × 6.5 × 3600) ≈ 2430.
const SQRT_ANNUAL_TRADING_SECONDS: f64 = 2430.0;
```

Use sites: lines 268, 269, 284 of the same file — applied to `realized_vol_fast` (feature 106), `realized_vol_slow` (feature 107), and `vol_of_vol` (feature 111). Not parameterized by config or runtime; no test ties it to dataset characteristics.

**Root cause** (from the roots):

The constant `2430.0` is derived from `sqrt(252 trading_days × 6.5 hours × 3600 seconds)` — strictly the US equity Regular Trading Hours convention. For futures (~23 hour sessions), the correct factor is ~5,680. For FX (24h × ~252), it's ~5,901. For crypto (24h × 365), it's ~6,926. The constant is hardcoded in one place; there is no `--annualization-factor` CLI flag, no config field, no per-symbol override. The same `2430.0` ships for every market.

**What corrupts**:
- Features 106 (`realized_vol_fast`), 107 (`realized_vol_slow`), 111 (`vol_of_vol`) are silently mis-scaled by a constant ratio if the dataset is not US-equity-RTH (e.g., 2.3× wrong for a futures dataset).
- Features 108 (`vol_ratio = fast / slow`), 109 (`vol_momentum = fast_now - fast_prev`), 110 (`return_autocorr`) are dimensionless and **unaffected** because the constant cancels.

**Production impact today**: NVDA dataset is US-equity-RTH, so this fires harmlessly with the correct constant. The risk activates the moment a non-US-equity dataset is added (e.g., futures, FX, ETH). Per §8 Lesson 5 caveat: this is "latent-but-mis-calibrating" rather than catastrophic — silent feature scaling drift across datasets.

**Surface area for redesign**:
- Make `SQRT_ANNUAL_TRADING_SECONDS` a config field (`[normalization.annualization_seconds_per_year]` or similar) computed from `trading_days_per_year × hours_per_day × 3600`.
- Allow per-dataset override.
- Validate at config-load: `assert(constant > 0 && constant.is_finite())`.

---

### F-029 — Volatility computer silently drops returns >10% per sample (circuit-breaker events erased) [Phase 2 / N-11]

**Severity**: HIGH (silent under-estimation of volatility precisely during high-vol regimes — when vol features matter most)

**Status**: VERIFIED (VVN2)

**Code path**:

`feature-extractor-MBO-LOB/crates/hft-feature-core/src/experimental/volatility.rs:175-210` (verbatim):

```rust
fn update(&mut self, mid_price: f64) {
    if !mid_price.is_finite() || mid_price <= 0.0 {
        return;
    }

    self.sample_count += 1;

    if let Some(prev) = self.prev_price {
        if prev > 0.0 {
            let log_return = (mid_price / prev).ln();

            // Filter outlier returns (> 10% per sample)
            if log_return.is_finite() && log_return.abs() < 0.1 {
                self.fast_window.push(log_return);
                self.slow_window.push(log_return);

                self.returns.push_back(log_return);
                if self.returns.len() > self.returns_capacity {
                    self.returns.pop_front();
                }

                // Update vol snapshots at fixed intervals for momentum tracking
                if self.sample_count.is_multiple_of(VOL_SNAPSHOT_INTERVAL) {
                    let new_vol = self.fast_window.std();
                    if new_vol > FLOAT_CMP_EPS {
                        self.vol_history.push(new_vol);
                    }
                    self.prev_fast_vol = self.current_fast_vol;
                    self.current_fast_vol = new_vol;
                }
            }
        }
    }

    self.prev_price = Some(mid_price);
}
```

**Root cause** (from the roots):

`ExpVolatilityComputer::update()` discards every log-return whose absolute value exceeds 0.1 (~10% per inter-sample interval). The dropped return is NOT pushed to `fast_window`, `slow_window`, or `returns`. AND `prev_fast_vol`/`current_fast_vol` snapshot tracking is also gated by the filter (lines 197-204 are inside the filter branch).

**Critical**: `self.prev_price = Some(mid_price)` at line 209 runs **UNCONDITIONALLY** (outside the filter), so the next sample's log-return is computed from the post-jump price as if the jump never occurred. Result: circuit-breaker triggers, IPO-day moves, halt-resume gaps, and overnight gaps (when sampling crosses a session boundary) **would be erased** from the volatility signal whenever a single-sample return exceeds 10% — exactly the events a vol estimator most needs to capture. (NVDA's 2025-2026 dataset has had no NVDA circuit-breaker events; overnight-gap erasure would fire whenever sampling crosses a session boundary with a sufficiently large gap.)

**Threshold rationale**: The 0.1 threshold is hardcoded and not parameterized by sample interval. A per-sample 10% threshold is roughly equivalent to a daily-vol of 158% (assuming sub-second sampling), so legitimate moves trigger the filter only during truly extreme events — but those are exactly the events whose magnitude matters for risk/regime features.

**No telemetry**: There is no counter that tracks how often the filter fires, no log on filter activation. There is no way to detect that the filter is silently affecting volatility readings.

**What corrupts**:
- Features 106 (`realized_vol_fast`), 107 (`realized_vol_slow`), 109 (`vol_momentum`), 110 (`return_autocorr`), 111 (`vol_of_vol`) systematically under-estimate volatility precisely during high-vol regimes.
- Models that learn to react to these features as "volatility regime indicators" will be blind to circuit-breaker/halt-resume events.

**Surface area for redesign**:
- Replace fixed 10% threshold with a sample-interval-dependent rule (e.g., clamp at `10 × rolling_std`).
- Add a `filtered_returns_count` stat exposed via the volatility computer's snapshot.
- Decide whether dropped returns should advance the buffer (preserve windows) or skip the sample entirely (preserve return chain integrity); current code does neither cleanly.

---

### F-030 — `BatchProcessor` in `FailFast` mode runs ALL files before reporting failure [Phase 2 / N-12]

**Severity**: MEDIUM (operationally costly: a 1-day failure in a 30-day batch produces 30-day wallclock latency and discards 29 days of completed work; no data corruption)

**Status**: VERIFIED (VVN2)

**Code path**:

`feature-extractor-MBO-LOB/crates/hft-extractor/src/batch.rs:549-558` (acknowledging comment, verbatim):

```rust
/// Process multiple day files in parallel.
///
/// Each file is processed by a fresh Pipeline instance created from the
/// shared DatasetConfig. Results are collected and returned as BatchOutput.
///
/// # FailFast behavior
///
/// Rayon runs ALL files regardless of error mode. FailFast only checks
/// errors after all results are collected. The first error in file-list
/// order is returned; all successful results are discarded.
```

**Root cause** (from the roots):

`BatchProcessor::process_files()` advertises a `FailFast` error mode, but the rayon `par_iter().map(...).collect()` pattern eagerly drives ALL files to completion before any error is observed. The `FailFast` check happens in the post-collection partitioning loop (lines 599-635), so the user-visible behavior is: every file runs to completion, the first error in file-list order is returned, and ALL successful day-results are silently discarded. For an N-day batch with one bad file at index 0, the system pays the wall-clock and CPU cost of N days of work and then returns one error — the user sees nothing of the work done.

The `cancellation_token.is_cancelled()` check at line 605 fires only on EXTERNAL cancellation, not on a sibling task's error. The behavior is documented in the doc-comment as "Preserve monolith behavior", but the name `FailFast` is misleading: this is `RunAll-ReportFirstError`.

**Default error mode**: `ErrorMode::FailFast` is the default (line 139), so this affects every default invocation.

**What corrupts**: Nothing — but operator efficiency is destroyed. A 234-day production batch with one bad file at day 1 runs 234 days of CPU work, returns 1 error, and the operator must restart from the beginning.

**Surface area for redesign**:
- Either rename `FailFast` to `RunAllReportFirst` and add a true fail-fast variant (e.g., using `try_for_each` with explicit cancellation propagation), OR
- Add an `OnErrorAction` callback that's invoked at the rayon-task boundary so the operator can choose whether to cancel siblings.

---

### F-031 — `BatchProcessor` empty-file list short-circuits to "success" with zero days processed [Phase 2 / N-13]

**Severity**: LOW (no data corruption; silent no-op is operationally annoying but not safety-critical)

**Status**: VERIFIED (VVN2)

**Code path**:

`feature-extractor-MBO-LOB/crates/hft-extractor/src/batch.rs:564-574` (verbatim):

```rust
// Short-circuit for empty file list
if files.is_empty() {
    return Ok(BatchOutput {
        results: vec![],
        errors: vec![],
        elapsed: start.elapsed(),
        threads_used,
        was_cancelled: false,
        skipped_count: 0,
    });
}
```

**Root cause** (from the roots):

`BatchProcessor::process_files()` returns `Ok(BatchOutput { results: vec![], errors: vec![], ... })` when invoked with an empty file slice, without warning, logging, or invoking the progress callback's completion hook. An operator who supplies a wrong `--input-dir` or a glob that matches no files sees a "successful" run with 0 days, 0 errors, 0 features extracted, and a non-zero `elapsed` — indistinguishable from a successful no-op.

`process_paths()` at line 681 forwards directly to `process_files()`, so the same hole exists for the path-based API.

**Surface area for redesign**:
- Return a typed `BatchError::EmptyFileList` variant, OR
- Log a one-line WARN, OR
- Make the empty case configurable (`BatchConfig::allow_empty_input: bool`).

---

### F-032 — `decompress_to_hot_store` accepts ANY `.zst` file in single-file mode (not just `.dbn.zst`) [Phase 2 / N-14]

**Severity**: LOW-MEDIUM (only fires under operator typo; would silently ingest non-MBO data into the hot store)

**Status**: VERIFIED (VVN1)

**Code path**:

`MBO-LOB-reconstructor/src/bin/decompress_to_hot_store.rs:164` (single-file branch) vs `:182` (directory branch) — verbatim:

```rust
if path.is_file() {
    if path.extension().is_some_and(|e| e == "zst") {       // line 164 — accepts any .zst
        files.push(path.to_path_buf());
    }
} else if path.is_dir() {
    for entry in fs::read_dir(path).map_err(|e| { ... })? {
        ...
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.ends_with(".dbn.zst") {                  // line 182 — requires .dbn.zst
                files.push(path);
            }
        }
    }
}
```

**Root cause** (from the roots):

`find_compressed_files` uses two divergent predicates: in single-file mode it accepts any file whose `extension()` is `"zst"` (so `weights.tar.zst`, `dump.csv.zst`, `image.png.zst`, etc. all pass), while in directory mode it requires a `.dbn.zst` suffix. Passing a non-DBN `.zst` file as `--input` causes the tool to attempt zstd-decompression — which would succeed on any well-formed zstd file — and then place the decompressed bytes at `<filename>.dbn` (because `HotStoreConfig::dbn_defaults` substitutes `.dbn.zst → .dbn`, but a name like `dump.csv.zst` doesn't end in `.dbn.zst`, so the substitution falls back to the unchanged filename — see `hotstore.rs:326-332`). Silent ingestion of non-MBO data into the hot store creates path-name collisions and downstream parsing errors that don't surface until a later pipeline stage.

**Adjacent observation**: `path.extension()` returns only the LAST extension, so `foo.dbn.zst.extension() == "zst"` AND `foo.zst.extension() == "zst"` — the predicate cannot distinguish them. The directory-mode `name.ends_with(".dbn.zst")` test would be the symmetric fix.

**Surface area for redesign**: Replace single-file predicate with `name.ends_with(".dbn.zst")` for symmetry with directory mode.

---

### F-033 — `extract_date` returns FIRST 8-digit run found in filename, not the canonical date [Phase 2 / N-15]

**Severity**: LOW (brittleness, not currently broken under Databento's standard naming)

**Status**: VERIFIED (VVN1)

**Code path**:

`MBO-LOB-reconstructor/src/bin/export_to_parquet.rs:356-368` (verbatim):

```rust
fn extract_date(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    for part in name.split(&['-', '.', '_']) {
        if part.len() == 8 && part.chars().all(|c| c.is_ascii_digit()) {
            return format!("{}-{}-{}", &part[0..4], &part[4..6], &part[6..8]);
        }
    }
    name.to_string()
}
```

**Root cause** (from the roots):

`extract_date` greedily returns the first dash/dot/underscore-separated token of length 8 that is all digits. For canonical Databento filenames like `xnas-itch-20250203.mbo.dbn.zst` the first 8-digit token IS the date. But filenames such as `archive_20240101_xnas-itch-20250203.mbo.dbn.zst` (with an archive-creation prefix) or `run-12345678-xnas-itch-20250203.mbo.dbn.zst` (a job-id that happens to be 8 digits) would yield the WRONG date. The extracted date is then formatted into output filenames (`{date}_lob_snapshots.parquet`) and embedded in Parquet metadata under `"date"` (line 389) — propagating the wrong provenance silently.

There is also no validation of `YYYY ∈ [1970, 2099]`, `MM ∈ [01,12]`, `DD ∈ [01,31]`. A `"19999999"` token would format as `1999-99-99`.

**Surface area for redesign**:
- Match a stricter regex like `^xnas-itch-(\d{4})(\d{2})(\d{2})\.` and validate.
- Take date as an explicit CLI argument rather than inferring from filename.
- Log the extracted date alongside the input path so operators can sanity-check.

---

### F-034 — `EveryN` downsampler off-by-one semantic + `EveryN(0)` silent reinterpretation [Phase 2 / N-16]

**Severity**: LOW (semantic clarity issue, not correctness — the documented "every N-th" matches code behavior modulo undocumented zero handling)

**Status**: VERIFIED-WITH-NUANCE (VVN1)

**Code path**:

`MBO-LOB-reconstructor/src/export/lob_writer.rs:171-174` (verbatim):

```rust
DownsampleStrategy::EveryN(n) => {
    let n = *n;
    if n == 0 {
        return true;
    }
    (self.rows_seen - 1) % (n as u64) == 0
}
```

`rows_seen` is incremented BEFORE `should_write` is invoked (`lob_writer.rs:106`), so by the time line 174 reads it, the value is `1` for the first row. The check writes rows where `(rows_seen - 1) % n == 0`, i.e., 1-indexed rows 1, n+1, 2n+1, ... — `EveryN(2)` writes rows at 1-based indices 1, 3, 5, ...; `EveryN(5)` writes 1, 6, 11, ....

**Root cause** (from the roots):

The `DownsampleStrategy::EveryN` doc comment (`mod.rs:124-125`) says "Export every N-th snapshot" — undocumented detail: the FIRST row is always written, then every N-th thereafter. Two reasonable user mental models:
1. "Write exactly one in every block of N consecutive rows" — current code matches this.
2. "Skip N-1 rows then write" — would write rows 0, N, 2N, ... not 1, N+1, 2N+1.

The semantics are arguably correct (write the first row, then every Nth thereafter) but the doc-comment doesn't make the choice explicit.

**Adjacent**: `n == 0` at line 171 returns `true` (treats it as "write all"). But a user passing `EveryN(0)` likely meant either "write none" or it's a config error. Silent reinterpretation as `EveryN(1)` is surprising — should be a config-validation error in `ExportConfig::validate`.

**Underflow concern (latent only)**: `(rows_seen - 1)` could underflow if `should_write` is ever called when `rows_seen == 0`. Today this can't happen because the increment runs first, but a future refactor that bypasses the increment would silently wrap to `u64::MAX`.

**Surface area for redesign**:
- Document the "first row always written" semantic in the `DownsampleStrategy::EveryN` doc-comment.
- Validate `EveryN(0)` at config-load with a hard error.
- Use `rows_seen.saturating_sub(1)` for defense.

---

## §3 — Verified in Code, Empirically Inactive in This Dataset

These bugs exist in the code as the discovery pass described them, but empirical validation shows they do NOT fire on the current NVDA dataset. They become production-active only under different data shapes (multi-venue, live-streaming, etc.).

### F-017 — Production uses `CrossedQuotePolicy::Allow` (the default) [SG-3, empirically inactive]

**Severity**: LOW empirically; HIGH if data shape changes

**Status**: VERIFIED in code, REFUTED empirically

**Code path** (verified):

`feature-extractor-MBO-LOB/crates/hft-extractor/src/pipeline.rs:125`:

```rust
let mut lob = LobReconstructor::new(self.lob_levels);
```

`MBO-LOB-reconstructor/src/lob/reconstructor.rs:307-309`:

```rust
pub fn new(levels: usize) -> Self {
    Self::with_config(LobConfig::new(levels))
}
```

`LobConfig::new(levels)` (line 82-87) is `Self { levels, ..Default::default() }`. `Default` for `LobConfig` (line 68-78) sets `crossed_quote_policy: CrossedQuotePolicy::Allow`. Additionally, `CrossedQuotePolicy` derives `#[derive(Default)]` and the `Allow` variant carries `#[default]` (line 23). Two independent defaults both yield `Allow`.

Every production caller uses this default (verified): `pipeline.rs:125` (production extractor), `bin/export_to_parquet.rs:385` (production parquet binary). Examples in `examples/` also use `LobConfig::new(10)` which inherits `Allow`.

`derived.rs:88-89` computes `spread = best_ask - best_bid` directly. For a crossed quote (bid > ask): `spread < 0`. `spread_bps` (line 92-96) propagates the negative value. The microprice formula uses inverted size weights but produces a finite real value.

**Empirical** (from `data/exports/raw_lob_full/*_reconstruction_stats.json`, 234 days, 2.77B messages):
- 0 crossed_quotes
- 0 locked_quotes
- 0 days had any nonzero crossed/locked count

**Why no crossed quotes**: This dataset is single-venue XNAS.ITCH. A single venue cannot cross itself (intra-venue matching rules prevent it). Crossed quotes appear in NBBO consolidations where Venue A's bid > Venue B's ask. The bug is latent and would activate immediately if multi-venue / NBBO consolidated data is ever ingested.

**Defensive mitigation present**: `signals/mod.rs:211` writes feature 92 (`book_valid`) and feature 96 (`invalidity_delta`), giving the model a downstream signal it COULD use to mask crossed-quote samples. Whether models actually use it is configuration-dependent.

**Surface area for redesign**:
- Change the default policy to `UseLastValid` (one line change), OR
- Add a CI assertion that the production config's policy is the intended one, OR
- Wait until multi-venue data is on the roadmap, then prioritize.

---

### F-018 — DBN `flags` field dropped (including `SNAPSHOT`) [SG-4, empirically inactive]

**Severity**: LOW for SNAPSHOT empirically; MEDIUM for the architectural debt of dropping `sequence` and `MAYBE_BAD_BOOK`

**Status**: VERIFIED in code, REFUTED empirically for SNAPSHOT

**Code path** (verified):

`MBO-LOB-reconstructor/src/dbn_bridge.rs:47-65` reads only 6 fields of `dbn::MboMsg`: `order_id`, `action`, `side`, `price`, `size`, `hd.ts_event`. **Dropped**: `flags`, `channel_id`, `ts_recv`, `ts_in_delta`, `sequence`, `hd.publisher_id`, `hd.instrument_id`, `hd.length`, `hd.rtype`. Confirmed: `flags` is never accessed anywhere downstream (grep across `MBO-LOB-reconstructor/src/`, `feature-extractor-MBO-LOB/crates/`, `databento-ingest/`, `MBO-LOB-analyzer/`).

**dbn 0.20 spec** (verified at `~/.cargo/git/checkouts/dbn-2a1f7b3a87238bdb/d9fad40/`):

The flag constants are defined in `rust/dbn/src/flags.rs` as bare names (NOT `F_`-prefixed as the discovery pass wrote — that was a known-older-style nomenclature error):

| Constant | Bit | Value | Semantics |
|----------|-----|-------|-----------|
| `LAST` | 7 | 0x80 | Last record in the venue event for given `instrument_id` |
| `TOB` | 6 | 0x40 | Top-of-book record |
| `SNAPSHOT` | 5 | 0x20 | Sourced from a replay/snapshot server |
| `MBP` | 4 | 0x10 | Aggregated price-level record |
| `BAD_TS_RECV` | 3 | 0x08 | Capture timestamp suspect |
| `MAYBE_BAD_BOOK` | 2 | 0x04 | Channel had unrecoverable gap |

**Empirical** (4 NVDA day files in `data/hot_store/`, 61M records sampled):

| Day | Records | SNAPSHOT | MAYBE_BAD_BOOK | BAD_TS_RECV | LAST | TOB | MBP |
|-----|---------|----------|----------------|-------------|------|-----|-----|
| 2025-02-03 | 18,476,041 | 0 | 0 | 1 | 10,028,975 (54.28%) | 0 | 0 |
| 2025-02-07 | 13,074,533 | 0 | 0 | 1 | n/a | 0 | 0 |
| 2025-02-18 | 9,797,736 | 0 | 0 | 1 | n/a | 0 | 0 |
| 2025-03-07 | 19,697,482 | 0 | 0 | 1 | n/a | 0 | 0 |
| **Total** | **61,045,792** | **0** | **0** | **4** | — | **0** | **0** |

**SNAPSHOT count is 0 across 61M records.** The SG-4 narrative — that snapshot recovery messages double-add orders, contributing to BBO error — does not occur in this dataset. The data is batch-historical (`metadata.json` confirms `delivery: "download"`, `split_duration: "day"`, `dataset: "XNAS.ITCH"`). Databento snapshot recovery is a live-streaming concept, not a batch-historical-file artifact.

**The truly impactful dropped fields** (more important than SNAPSHOT):

1. **`sequence`** — exchange's matching-engine sequence number. Dropped means no in-band gap detection. Combined with `loader.rs` not validating sequence monotonicity, undetected packet loss silently corrupts the book.
2. **`MAYBE_BAD_BOOK`** — the venue itself signaling "we know something's wrong here." Currently silently ignored. Even though this dataset has 0 such records, if it ever fires, the pipeline keeps building features as if all is well.

**Defensive observation**: The reconstructor's existing "duplicate Add → modify_order" path (`reconstructor.rs:461-464`) would soft-degrade a snapshot-replay's repeated Add into a Modify — preventing direct double-counting in the most common case. But snapshots may bring NEW order_ids never seen in the live stream, which would be inserted fresh.

**Surface area for redesign**:
- Carry `flags` on `MboMessage` (one extra byte per message).
- Carry `sequence` on `MboMessage` (one extra u32).
- Add a counter for `MAYBE_BAD_BOOK` events; surface them as a data-quality signal.
- Add a sequence-gap detector in the loader.

---

## §3.5 — Honest Disposition: The "BBO 99.17% Accuracy" Question

The discovery-phase user prompt explicitly named "BBO accuracy 99.17%" (i.e., 0.83% wrong) and asked the audit to identify reconstructor-side contributors. The audit does NOT have a definitive answer; this section documents what we know, what we ruled out, and the candidate explanations we cannot disambiguate without additional investigation.

### What we ruled out

- **SG-1 (Action::Clear OFI poisoning) — REFUTED**. Even if it had been real, it would not have affected BBO accuracy directly (it would have affected OFI). The mechanism doesn't exist in production; see R-001.
- **SG-3 (Allow crossed-quote policy) — Empirically inactive**. Cross-validated against 234 NVDA `_reconstruction_stats.json` files: **0 crossed_quotes, 0 locked_quotes, 0 days had any nonzero count**. Single-venue XNAS.ITCH cannot self-cross. So crossed-quote propagation contributes **0%** of the 0.83% on this dataset.
- **SG-4 (`SNAPSHOT` flag dropped) — Empirically inactive**. 0 SNAPSHOT records across 61M sampled NVDA messages. Snapshot-replay is a live-streaming concept; batch-historical files do not contain it. So snapshot-replay double-add contributes **0%** of the 0.83% on this dataset.

### What could plausibly contribute (but we did not quantify)

In order of estimated likelihood-to-contribute (highest first):

1. **Upstream data quality**. Per `MBO-LOB-reconstructor/CLAUDE.md`, "~0.5% cancel_order_not_found" is documented as "normal" — a cancel that arrives for an order_id the reconstructor hasn't seen. This is a feed quirk, not a reconstructor bug, and is comparable in magnitude to the 0.83% number. **The most likely single contributor is upstream**.

2. **F-013 (Modify-of-missing → silent Add at NEW price)**. If the modify carries a price near or at the BBO, this introduces a phantom order at that price level, potentially shifting the BBO. No counter exists (per F-013 narrative); rate is unobservable. Probably small (Modifies of missing orders are rarer than Cancels of missing orders by spec) but non-zero.

3. **F-018 (DBN `sequence` field dropped → no in-band gap detection)**. If a packet is dropped in transit, the reconstructor cannot detect it; subsequent state mutations operate on a corrupt book. The Databento standard datasets are usually gap-free, but undetected gaps cannot be ruled out.

4. **F-021 (silent reconstructor-error swallow at `export_to_parquet.rs:411`)**. Errors that occur during reconstruction (e.g., `OrderNotFound`, `InvalidPrice`) silently produce a "no snapshot" row. This affects per-row availability, not BBO correctness per se — but if the BBO accuracy metric is computed from missing-row patterns, this could contribute.

5. **N-1's adjacent observation: asymmetric error handling in the same loop**. MBO writer propagates with `?`, LOB writer is silent. If a day file has any reconstructor error, the operator sees a complete MBO parquet but a partial LOB parquet, producing apparent BBO drift purely from the row-count asymmetry.

### What we cannot determine from this audit

- **The source of the "99.17%" number itself**. Is it computed against an independent ground truth (e.g., Databento's MBP-10 schema vs the reconstructor's MBO-derived BBO)? Against a second reconstructor implementation? Against documented exchange-side data quality SLAs? The audit document does not have visibility into how this metric was derived. Without the metric's denominator and computation method, we cannot allocate the 0.83% across the candidates above.

- **Per-day error distribution**. We have aggregate stats across 234 days but not per-day breakdown of where the 0.83% concentrates. If errors cluster on specific days (e.g., DST-transition days, days with halts, days with large news-driven volatility), the dominant contributor is likely different than if errors are uniformly distributed.

### Recommended verification (deferred)

If resolving the BBO puzzle becomes a priority:

1. **Re-derive the metric from raw stats** (`crossed_quotes + locked_quotes / messages_processed`) and confirm whether it actually equals 0.83% or whether the 99.17% number refers to something else (e.g., per-event BBO correctness vs an MBP-10 reference).
2. **Inspect a sample day's `_reconstruction_stats.json`** for `cancel_order_not_found`, `trade_order_not_found`, and other silent-recovery counters. Their sum is a lower bound on "messages where the reconstructor's view diverges from the ideal".
3. **Cross-check** Databento's MBP-10 schema for the same NVDA day against the reconstructor's BBO derived from MBO. Per-message divergence is the cleanest measurement.

### Honest summary

The audit ruled out the loud candidate (SG-1) and confirmed two latent candidates are empirically inactive on this dataset (SG-3, SG-4). The remaining ~0.83% BBO error is most plausibly attributed to **upstream data quality (cancel_order_not_found ~0.5%)** plus **silent error-swallow paths (F-021)** plus possibly **modify-of-missing phantom adds (F-013)**, but the audit does not have the empirical leverage to allocate the budget precisely. The fix order in §9 is correct regardless of which contributor dominates: the silent-error-handling fixes (F-002 + F-003 + F-024 + F-021) would surface the actual error rate, after which the team could re-measure BBO accuracy with proper observability.

---

## §4 — Refuted Claims (Important: Do NOT Work On These)

This section documents claims from the discovery pass that the validation refuted. They are documented in detail so future audits do not re-discover them as bugs.

### R-001 — "Mid-session Action::Clear poisons consumer OFI for the rest of the day" [SG-1, REFUTED]

**Discovery-pass severity**: CRITICAL (ranked #1 smoking gun, posited as the explanation for the OFI lag-1 r=0.006 puzzle)

**Validated verdict**: REFUTED. The mechanism does not exist in the production code path.

**The discovery-pass chain (5 links)**:
1. Reconstructor's `Action::Clear` calls `self.reset()` — empties book, preserves stats, no consumer notification.
2. Adapter `MboOrderEvent::try_from_message` returns `None` for `Action::Clear` — `engine.on_order_event` not called.
3. Pipeline still calls `engine.on_book_update(&book)` on the now-empty book.
4. `OfiComputer::update()` returns early on empty book WITHOUT resetting `prev_*` fields.
5. Subsequent Add events rebuild book; OFI compares NEW BBO vs PRE-Clear BBO → false spike.

**Validation findings on each link**:

- **Link 1**: VERIFIED. `reconstructor.rs:911-922` does call `self.reset()` (not `full_reset()`). Stats preserved. No notification mechanism (no channel, callback, or event).
- **Link 2**: VERIFIED. `adapters.rs:112-123`: `Action::Clear | Action::None => return None`.
- **Link 3**: **REFUTED — this is the broken link**. Pipeline (`pipeline.rs:142-144`) filters at the `is_system_message()` predicate FIRST:
  ```rust
  for msg in messages {
      messages_processed += 1;
      if msg.is_system_message() {
          continue;                // ← Clear is filtered HERE
      }
      lob.process_message_into(&msg, &mut lob_state)?;
      let book = LobBook(&lob_state);
      self.engine.on_book_update(&book);     // ← NEVER reached for Clear
      if let Some(event) = MboOrderEvent::try_from_message(&msg) {
          self.engine.on_order_event(&event);
      }
  }
  ```
  
  And `is_system_message()` (`MBO-LOB-reconstructor/src/types.rs:177-179`) returns `true` for `order_id == 0 || size == 0 || price <= 0`. Databento's `Action::Clear` messages carry exactly these dummy values. Confirmed by the test fixture at `MBO-LOB-reconstructor/src/lob/reconstructor.rs:1743`: `MboMessage::new(0, Action::Clear, Side::None, 0, 0)`.
  
  **Consequence**: For a Clear message, the pipeline `continue`s at line 143. `process_message_into` is never called. `self.reset()` never fires. `on_book_update` is never invoked with an empty book. The whole chain is broken at link 3.

- **Link 4**: **PARTIALLY REFUTED**. Even if Clear DID propagate (counterfactually), `OfiComputer::update()` (`feature-extractor-MBO-LOB/crates/hft-feature-core/src/signals/ofi.rs:108-138`) DOES correctly handle the empty-book case. The relevant lines:

  ```rust
  let (curr_bid, curr_ask) = match (curr_bid_price, curr_ask_price) {
      (Some(b), Some(a)) => (b, a),
      _ => {
          self.prev_bid_price = curr_bid_price;     // ← OVERWRITES with None
          self.prev_bid_size = curr_bid_size;       // ← OVERWRITES with 0
          self.prev_ask_price = curr_ask_price;
          self.prev_ask_size = curr_ask_size;
          return;
      }
  };
  ```

  The empty-book pass DOES overwrite `prev_*` to None/0 before returning. The next non-empty book hits the FIRST-CALL branch (because `prev_bid_price = None`), initializes `prev_*` from current book, and returns without OFI contribution. **No false OFI spike is produced by `OfiComputer`** even in the counterfactual scenario.

- **Link 5**: NOT APPLICABLE (since links 3 and 4 break the chain).

**Empirical confirmation**:

`MBO-LOB-analyzer/full_234day_output/01_DataQualityAnalyzer.json` reports **234/234 NVDA days have exactly 1 Clear message each**. The first timestamp of each day is around 09:00 UTC = 04:00 ET (NVDA pre-market start). The single Clear is almost certainly XNAS-ITCH's session-open marker (~09:30 ET), clearing the pre-market book before continuous trading begins. NVDA had no circuit-breaker halts in the 2025 dataset.

But even with 1 Clear/day, the chain doesn't fire because of link 3.

**The actually-real bug (different and milder)**:

The reconstructor silently drops Clear without producing any false OFI tick. The reconstructor retains stale orders from before the Clear in `self.orders`. If exchanges reuse order IDs intra-day (uncommon for NVDA-ITCH which uses unique IDs per session), this could cause data-quality drift. The actual Clear timing (~09:30 ET, session-open marker) means the affected window is pre-market, where downstream signals are typically discarded by warmup gates anyway.

**Why this matters for the OFI puzzle**:

The OFI lag-1 r=0.006 puzzle is fully attributable to:
1. **Sampling decoherence** (the explanation already in CLAUDE.md): event-based sampling at `event_count = 1000` resamples onto irregular wall-clock intervals; OFI's natural decay over wall-clock time is aliased.
2. **Genuine market efficiency**: lag-1 OFI predictive r at the order-event scale should be small (~0.01-0.05 in well-arbitraged equities like NVDA).

Even charitably assuming the discovery-pass mechanism DID fire: 1 Clear/day, ~390 samples/day at 60s sampling, magnitude 1-2× per-interval OFI std, variance contribution ≈1% of OFI variance per day. A fixed-time-of-day spike does not create lag-1 autocorrelation structure (it creates a time-bucket fixed effect, detectable by `time_regime` features).

**Methodological note**: The discovery agent compounded two errors (didn't verify the system-message filter applies to Clear; misread OFI's empty-book branch as not-resetting-prev_*) into a confident-sounding chain that doesn't match runtime behavior. Both were forgivable single-step errors that the validation pass caught by independently re-walking each link.

**Adjacent finding (for future fix consideration)**:

`MultiLevelOfiTracker` (`feature-extractor-MBO-LOB/crates/hft-feature-core/src/ofi/multi_level.rs:90-141`) DOES have the false-spike bug pattern — for each level: if `curr_bid_price = 0` (level absent) and `prev_bid_price > 0`, returns `-(prev_size as f64)` (level disappeared); on rebuild, returns `+curr_size` (level appeared). But this only fires if `on_book_update` is called with an empty book — which it isn't, because Clear is filtered upstream. **Latent bug, not active.** Worth flagging for the day someone changes the Clear-handling pattern.

---

### R-002 — "Phantom cancels bias features 51, 55, 89" [SG-8, REFUTED for the named features]

**Discovery-pass severity**: HIGH

**Validated verdict**: REFUTED for production training features; only `mbo-statistical-profiler` (research tool) is affected.

**The discovery-pass mechanism**:

When Cancel arrives for an unknown order_id, `reconstructor.rs:548-565` increments `cancel_order_not_found` and returns `Ok(())`. The book is unchanged. But upstream, `messages_processed` increments and `triggering_action = Some(Action::Cancel)` is set on the LobState (via `fill_lob_state_with_temporal` at line 808). The discovery pass claimed features 51 (`cancel_rate_ask`), 55 (`net_cancel_flow`), and 89 (`cancel_asymmetry`) read `triggering_action` and would inflate by ~0.5%.

**Why refuted**:

`triggering_action` is NOT used by any production feature in `feature-extractor-MBO-LOB`. Verified by grep: zero matches for `triggering_action` in `feature-extractor-MBO-LOB/crates/`. The cancel features are populated through a different path:

1. `pipeline.rs:154-156`: `MboOrderEvent::try_from_message(&msg)` then `engine.on_order_event(&event)`.
2. `MboOrderEvent::try_from_message` (`adapters.rs:112-133`) maps `Action::Cancel → OrderAction::Cancel` based purely on the **raw inbound message action**, with no knowledge of whether the LOB book changed.
3. `MboComputer::on_order_event` (`mbo/mod.rs:659-674`) pushes to fast/medium/slow windows.
4. `MboWindow.increment_counters` (`mbo/window.rs:90-115`) increments `cancel_count_bid`/`cancel_count_ask` purely on `OrderAction::Cancel` + side.

**Plain reframing**: Cancel-rate features measure cancellation FLOW at the exchange-feed level, not deletions from a reconstructed book. A "phantom" cancel from the reconstructor's perspective is a real cancel from the feed's perspective. Whether the LOB had the order or not is irrelevant to the feed-level cancel count.

**Where the bug IS real**:

`mbo-statistical-profiler/src/trackers/ofi.rs:607-613` DOES read `triggering_action` from `LobState`:

```rust
let action = lob_state.triggering_action.unwrap_or(Action::None);
match action {
    Action::Add => self.day_ofi_add.push(ofi),
    Action::Cancel => self.day_ofi_cancel.push(ofi),
    Action::Trade | Action::Fill => self.day_ofi_trade.push(ofi),
    _ => {}
}
```

For cancels-of-missing-orders, the OFI compute operates on an unchanged `lob_state`, so OFI = 0 for that step. Pushing zero-OFI samples to `day_ofi_cancel` slightly biases the cancel-OFI distribution toward zero. This is a real bias in the profiler tracker (a research/analysis tool), but NOT in the production feature pipeline.

**General principle that DID survive**:

The snapshot's `triggering_action` is set even when the action did not modify the book. This is a contract violation — the field name implies "the action that triggered THIS state change," not "the action of the message just processed." Any future consumer that reads `triggering_action == Cancel` and assumes "the book was modified by a cancel" inherits the bug.

**Surface area for future consideration**:
- Add a `book_modified: bool` field to `LobState`, OR
- Set `triggering_action = None` on no-op paths (cancels-of-missing, trades-of-missing, system messages), OR
- Document the field's actual semantics ("the action of the message just processed, regardless of book impact").

---

### R-003 — "begin_day(...) lifecycle hook does not exist anywhere in production" [REFUTED]

**Discovery-pass claim** (severity: implied as evidence the C4 audit was incomplete): "`grep -rn 'begin_day'` across the entire repo returns ZERO matches in production code; only documentation references and one mention inside an `hft-statistics` doc comment ... CLAUDE.md was wrong about this."

**Validated verdict**: REFUTED. CLAUDE.md is correct. The hook is fully implemented and actively used.

**The hook exists in two trait declarations**:

1. `mbo-statistical-profiler/src/lib.rs:80`:
   ```rust
   pub trait AnalysisTracker: Send {
       ...
       fn begin_day(&mut self, day_index: u32, utc_offset: i32, day_epoch_ns: i64) {}
       ...
   }
   ```
   This is the EXACT 3-arg signature CLAUDE.md describes.

2. `opra-statistical-profiler/src/lib.rs:60`:
   ```rust
   pub trait OptionsTracker {
       fn begin_day(&mut self, ctx: &DayContext);
       ...
   }
   ```

**Production overrides** (10 in mbo-statistical-profiler):
- `trackers/returns.rs:193`, `ofi.rs:562`, `jumps.rs:215`, `volatility.rs:182`, `noise.rs:213`, `trades.rs:247`, `spread.rs:65`, `vpin.rs:316`, `cross_scale_ofi.rs:275`, `quality.rs:65`.

**Production overrides** (8 in opra-statistical-profiler):
- `trackers/greeks.rs:76`, `put_call.rs:67`, `premium_decay.rs:60`, `volume.rs:70`, `effective_spread.rs:96`, `spread.rs:48`, `quality.rs:60`, `zero_dte.rs:81`.

**Orchestrator call sites**:
- `mbo-statistical-profiler/src/profiler.rs:71`: `tracker.begin_day(day_index, utc_offset, day_epoch);`
- `opra-statistical-profiler/src/profiler.rs:96`: `tracker.begin_day(&day_ctx);`

**Regression-guard tests**:
- `mbo-statistical-profiler/src/trackers/spread.rs:210` — `test_intraday_curve_uses_begin_day_utc_offset` calls `tracker.begin_day(0, -4, 0);` and asserts the cached `utc_offset` flows through to `intraday_curve.add()`.
- `mbo-statistical-profiler/src/trackers/trades.rs:472` — analogous `test_intraday_trade_rate_uses_begin_day_utc_offset`.
- ~24 `t.begin_day(&make_day_context())` calls across opra-statistical-profiler tracker tests.

**Why the discovery agent got this wrong**:

The discovery-pass agent likely greppped only `MBO-LOB-reconstructor/` and `feature-extractor-MBO-LOB/`, missing the entire `mbo-statistical-profiler/` and `opra-statistical-profiler/` crates. This is a structural search error. Future audits must grep across ALL standalone repo paths, not just the two named in the audit's primary scope.

**The legitimate adjacent observation**:

The `begin_day` hook EXISTS in the profiler crates, but `feature-extractor-MBO-LOB` does NOT have a parallel hook. The C4 re-derivation pattern in feature-extractor (see §5) is real — the fix would be to ADD a similar lifecycle hook in feature-extractor, not to claim that begin_day "doesn't exist anywhere."

---

## §5 — C4 Re-Derivation Pattern Catalog

The C4 anti-pattern: a parameter that should be set once at lifecycle boundary (begin-of-day) is instead re-derived per-call inside multiple consumers, leading to (a) duplicated computation, (b) potential drift between sites, and (c) silent shadowing of any future centralized parameter.

The discovery pass identified 6 sites. Validation confirmed all 6 and added 3 more.

### C4-1 — `TimeBasedSampler.utc_offset_hours` (locked + re-derived day_epoch_ns)

`feature-extractor-MBO-LOB/crates/hft-sequence-engine/src/sampling.rs:609-628`

`utc_offset_hours` is set ONCE in the constructor. `compute_market_open_ns` re-derives `day_epoch_ns = timestamp_ns - (timestamp_ns % NS_PER_DAY)` per call. The combination is wrong for DST (see F-001). This site is ALSO the most actively-firing C4 bug.

### C4-2 — `SeasonalityComputer.last_timestamp_ns` (re-derived offset)

`feature-extractor-MBO-LOB/crates/hft-feature-core/src/experimental/seasonality.rs:78-86`

`extract` re-derives `(year, month, day)` from `last_timestamp_ns` via `days_to_ymd`, then calls `hft_statistics::time::regime::utc_offset_for_date(year, month, day)`. **DST-aware ✓** but redundant — TimeBasedSampler already has the offset (statically wrong, per C4-1).

### C4-3 — `SignalComputer::extract → compute_time_regime` (re-derived offset)

`feature-extractor-MBO-LOB/crates/hft-feature-core/src/signals/mod.rs:215, 260-269`

`compute_time_regime(self.last_sample_ts)` re-derives `(year, month, day)` from the timestamp, then calls `utc_offset_for_date` to produce the regime label. **DST-aware ✓** but yet another independent re-derivation site.

### C4-4 — `DayBoundaryConfig.timezone_offset_hours` (stored unused)

`MBO-LOB-reconstructor/src/lob/day_boundary.rs:72`

The config field is declared but never read by any consumer logic. `crosses_midnight` (lines 439-443) only uses `i64`-based UTC arithmetic and ignores `timezone_offset_hours`. This is the textbook "stored but unused parameter" — most insidious because it gives the illusion of configurability that isn't real.

### C4-5 — `DayBoundaryStats.first_ts/last_ts` vs `DayBoundary.previous_day_end_ts/new_day_start_ts` (two sources)

`MBO-LOB-reconstructor/src/lob/day_boundary.rs:240, 243` (per-day rolling), `:193, 196` (boundary event)

Two independent timestamp tracks for "when did the day start/end." They happen to agree under normal conditions, but if a consumer chooses one over the other inconsistently (e.g., per-day analytics uses one, cross-day analytics uses the other), drift is possible.

### C4-6 — `MboMessage.timestamp` vs `LobStats.last_timestamp` vs `LobState.timestamp` (three sources)

`MBO-LOB-reconstructor/src/types.rs:138, 312` and `lob/reconstructor.rs:208`

Three "current timestamp" fields populated by different code paths. `LobStats.last_timestamp` is updated only on the non-system-message branch (`reconstructor.rs:937`). `LobState.timestamp = msg.timestamp.or(self.stats.last_timestamp)` (line 801) — silent fallback to the stale stats value if the message has no timestamp. These three can disagree.

### C4-7 (NEW) — `LobConfig.levels` vs `LobState.levels` vs `FeatureLayout.lob_levels()`

Three sources of "how many levels." `LobConfig.levels` is the reconstructor's config; `LobState.levels` is set per-snapshot in `fill_lob_state_with_temporal` (line 803); `FeatureLayout.lob_levels()` is consumed by `LobComputer` (`feature-extractor-MBO-LOB/crates/hft-feature-core/src/lob.rs:67`). The BookSnapshot adapter reports `state.levels` via `book.levels()`, but LobComputer reads `lob_levels` from the layout. Mismatch is silent. F-005 (LB-3 today-active in export) is a special case of this drift.

(Validation correction: an earlier draft cited `crates/hft-feature-core/src/computers/lob.rs:67` — the `computers/` subdirectory does not exist. The actual path is `crates/hft-feature-core/src/lob.rs:67`. Verified by VV1.)

### C4-8 (NEW) — `state.sequence` vs DBN exchange `sequence` vs the Parquet column documented as `sequence`

See F-009. Producer writes the local message counter; the DBN exchange `sequence` is dropped at the bridge; the analyzer's documentation describes the Parquet column as exchange-provided. Three sources, three different semantics, one shared name.

### C4-9 (NEW) — `LobReconstructor.stats.last_timestamp` updates only on the non-system-message branch

`reconstructor.rs:937-938` sets `self.stats.last_timestamp = msg.timestamp;` only inside the non-system-message branch. System messages (filtered at line 887-897) do NOT update this stat. Combined with the consumer's pre-filter (which also bypasses the reconstructor entirely for system messages), the LobStats.last_timestamp can lag behind the actual stream timestamp. If a synthetic MboMessage with `timestamp = None` ever flowed in after a system message had been consumer-filtered, the .or() fallback at line 801 would use a STALE last_timestamp from before the system message gap.

### Pattern summary

The C4 pattern is not just "re-derivation" — re-derivation is cheap and arguably defensive. The actual problems are:

1. **Inconsistent re-derivation** — different sites compute the same quantity differently (e.g., C4-1 is DST-blind; C4-2 and C4-3 are DST-aware).
2. **Stored-but-unused parameters** — give the illusion of configurability (C4-4).
3. **Multiple sources of truth** for the same conceptual quantity (C4-5, C4-6, C4-7, C4-8, C4-9).
4. **Lifecycle hook absence** — `begin_day` exists in profiler crates but not in feature-extractor; this absence is what allows each site to independently re-derive.

### Surface area for redesign (cross-cutting)

- Add a `begin_day(day_index, utc_offset, day_epoch_ns)` lifecycle hook to feature-extractor's `FeatureEngine`/`Computer` traits (parallel to the profiler crates' existing hook).
- Pipeline driver calls `begin_day` once per day, computing the canonical offset via `hft_statistics::time::regime::utc_offset_for_date`.
- Each computer caches the offset/day_epoch and uses the cached value in subsequent extracts.
- Remove all per-call re-derivation in `TimeBasedSampler::compute_market_open_ns`, `SeasonalityComputer::extract`, `SignalComputer::extract`, `compute_time_regime`.
- Resolve the three-way timestamp confusion in `MboMessage`/`LobStats`/`LobState` by adopting a single canonical timestamp source.

---

## §6 — Dead Code Architectural Pathology

The MBO-LOB-reconstructor publicly re-exports a substantial body of functionality that has zero consumers in the production feature-extraction pipeline. This is not corruption — the dead modules don't run, so they don't produce wrong outputs — but it actively misleads consumers and future audits.

### Confirmed dead modules (validated by exhaustive cross-repo grep)

| Module | LOC | Tests | Production Consumers | Notes |
|--------|-----|-------|---------------------|-------|
| `OrderLifecycleTracker` (`src/lob/order_lifecycle.rs`) | 1,637 | 18 unit | 0 | Replaced by `feature-extractor-MBO-LOB/crates/hft-feature-core/src/mbo/order_tracker.rs::OrderTracker` (different semantics) |
| `TradeAggregator` (`src/lob/trade_aggregator.rs`) | 943 | 27 unit | 0 | Production trade flow uses `MboWindow` counters in `feature-extractor-MBO-LOB` |
| `QueuePositionTracker` (`src/lob/queue_position.rs`) | 1,683 | 23 unit + 5 integration | 0 | Features 68/69 hardcoded to 0.0 in production (see F-006) |
| `DayBoundaryDetector` (`src/lob/day_boundary.rs`) | 614 | 6 unit | 0 | Day partitioning is filename-based per file; this detector never invoked |
| `NormalizationParams` (`src/statistics.rs:457+`) | ~125 | tests in `integration_test.rs` only | 0 | Schema incompatible with `feature-extractor-MBO-LOB/crates/hft-normalization`; bridged via `normalization_compat.rs` |
| `MultiSymbolLob` (`src/lob/multi_symbol.rs`) | 385 | 11 unit | 0 | Only used in `examples/multi_symbol.rs` |

**Subtotal**: ~5,387 LOC

### Newly-confirmed dead modules (missed by discovery pass)

| Module | LOC | Tests | Notes |
|--------|-----|-------|-------|
| `analytics.rs` (`DepthStats`, `LiquidityMetrics`, `MarketImpact`) | 566 | tests in `integration_test.rs` + `examples/advanced_analytics.rs` | Zero downstream consumers |
| `warnings.rs` (`WarningTracker`, `WarningTrackerConfig`, `WarningCategory`) | 717 | self-references only | Zero consumers — including `feature-extractor-MBO-LOB`, profiler crates, all backtester/analyzer/trainer crates |
| `statistics.rs::DayStats` / `RunningStats` (the part beyond `NormalizationParams`) | ~413 | `tests/integration_test.rs:1434` + `examples/advanced_analytics.rs` | Zero downstream consumers |

**Subtotal**: ~1,696 LOC

(Note: `NormalizationParams` itself is ~135 LOC at `src/statistics.rs:449-582`, separately confirmed by VV3.)

### Total dead code surface

**~7,093 LOC** of publicly re-exported but production-unused code. The discovery pass estimated ~5,700 LOC; the validation revised this upward. (Validation correction: an earlier draft cited "~7,083" — the small drift is in the NormalizationParams line count, ~135 vs the earlier estimate of ~125.)

### Why this matters (long-term)

1. **Audit confusion**: Multiple discovery-pass agents fell into the trap of auditing dead code in detail before realizing it had no production consumers. The hours spent on `OrderLifecycleTracker`, `TradeAggregator`, and `QueuePositionTracker` audits in the discovery pass were not wasted (the bugs found are real if anyone wires the modules in), but they distorted the severity ranking — bugs in dead code were treated as if they affected production.

2. **Documentation drift**: `MBO-LOB-reconstructor/CLAUDE.md`, `README.md`, `ARCHITECTURE.md`, `CODEBASE.md` describe these modules as the canonical pipeline components. Future readers see an architecturally rich library and assume they should consume it. They actually need to consume `feature-extractor-MBO-LOB`'s parallel implementations.

3. **Parallel implementations diverging**: `MBO-LOB-reconstructor::OrderLifecycleTracker` and `feature-extractor-MBO-LOB::OrderTracker` implement overlapping functionality with subtly different semantics:
   - This file: AHashMap, no eviction, retains completed orders by count.
   - Feature-extractor: BTreeMap (deterministic), age+size eviction (1h, 50K), retains by count too.
   - This file: tracks `OrderModification` history with old/new prices.
   - Feature-extractor: tracks `modifications: u8` count and `price_changes: u8` count separately.
   - This file: `total_filled: u64`, no per-fill list.
   - Feature-extractor: `fills: Vec<(u64, u32)>` (full history with timestamps).
   
   A bug fix in one will not propagate to the other.

4. **Misleading `lib.rs` example**: `MBO-LOB-reconstructor/src/lib.rs:56-74` shows code using `NormalizationParams::from_day_stats(&day_stats, 10)` and `.save_json(...)` as the canonical ML normalization path. Anyone integrating this code will discover too late that the schema is incompatible with `feature-extractor-MBO-LOB/crates/hft-normalization` (which itself is functionally deprecated per T15). The example actively misleads.

### Library-vs-pipeline framing (nuance)

`MBO-LOB-reconstructor` is published as a reusable component (it has its own `Cargo.toml`, README, and was extracted as a standalone crate). README.md, ARCHITECTURE.md, CHANGELOG.md, and CODEBASE.md treat the trackers as advertised public surface. The `examples/` directory contains `multi_symbol.rs` and `advanced_analytics.rs` which exercise these types as examples for external (potential) library consumers.

**Two valid framings**:

1. **The library exposes a richer feature set than the in-house pipeline uses**. If a third party were to pick up `mbo-lob-reconstructor` as a library, the public surface works and is well-tested. From this view, the dead-code modules are "extra batteries included" rather than rot.

2. **The in-house pipeline has parallel reimplementations that compete with and diverge from the library's offerings**. This is the architectural pathology. If the in-house team wants to consume the library's tracker code, they would need to either replace the parallel `feature-extractor-MBO-LOB` implementation OR accept that the two copies will drift.

**The honest position**: the dead-code surface is real and substantial, but it is not "data corruption" — it is "API surface debt." The framing should drive the redesign: either consolidate to one implementation (deprecate the other) or formalize the divergence.

### Surface area for redesign

- Decision needed: is `MBO-LOB-reconstructor` (a) a standalone library for general use, or (b) the upstream half of an in-house pipeline?
  - If (a): leave the rich API surface, add `#[deprecated]` markers to the lib.rs example code that uses incompatible NormalizationParams, document which APIs feature-extractor consumers should NOT use.
  - If (b): move the unused modules to a separate `mbo-lob-reconstructor-extras` crate (or under a feature flag), so the core remains lean and consumers know exactly what's wired.
- For features 68/69: implement queue tracking in production OR remove from the contract.
- For NormalizationParams: either fix the per-level statistics issue and align schemas, OR remove from public re-exports.

---

## §7 — Documentation Drift

### F-019 — `LobState` actual size differs from doc claim [NEW-8 part 1]

**Doc claim** (`MBO-LOB-reconstructor/src/types.rs:268` and CLAUDE.md): ~560 bytes, stack-allocated.

**Discovery-pass claim**: ~580 bytes.

**Validated computation**:

| Field | Type | Bytes |
|-------|------|-------|
| `bid_prices` | `[i64; 20]` | 160 |
| `bid_sizes` | `[u32; 20]` | 80 |
| `ask_prices` | `[i64; 20]` | 160 |
| `ask_sizes` | `[u32; 20]` | 80 |
| `best_bid` | `Option<i64>` | 16 (no niche) |
| `best_ask` | `Option<i64>` | 16 |
| `levels` | `usize` | 8 |
| `timestamp` | `Option<i64>` | 16 |
| `sequence` | `u64` | 8 |
| `previous_timestamp` | `Option<i64>` | 16 |
| `delta_ns` | `u64` | 8 |
| `triggering_action` | `Option<Action>` | 2 |
| `triggering_side` | `Option<Side>` | 2 |
| **Naive sum** | | **572** |
| **+ tail padding to 8-byte align** | | **576** |

**Truth**: ~576 bytes. The doc's 560 is too low (likely assumed niche-optimized 8-byte `Option<i64>`, which doesn't apply to plain i64). The discovery pass's 580 is close but not exact.

**Test that should catch drift but doesn't**: `MBO-LOB-reconstructor/src/types.rs:1374-1375` asserts only `500 < size < 700` — too loose to detect a 16-byte drift.

**Why this matters long-term**: The "stack-allocated 560B" framing in CLAUDE.md is a soft commitment that downstream consumers may rely on. If a future change adds a field and pushes the size above some threshold, no automated test catches it.

**Surface area for redesign**:
- Tighten the test to `assert_eq!(size_of::<LobState>(), EXPECTED_SIZE)` where `EXPECTED_SIZE` is the exact value.
- Update CLAUDE.md to reflect the actual size.
- Consider `#[repr(C)]` if a stable ABI is needed.

### F-020 — `MboMessage` actual size differs from doc claim [NEW-8 part 2]

**Doc claim**: 32 bytes.

**Discovery-pass claim**: ~48 bytes.

**Validated computation**:

| Field | Type | Bytes |
|-------|------|-------|
| `order_id` | `u64` | 8 |
| `action` | `Action` (`#[repr(u8)]`) | 1 |
| `side` | `Side` (`#[repr(u8)]`) | 1 |
| `price` | `i64` | 8 |
| `size` | `u32` | 4 |
| `timestamp` | `Option<i64>` | 16 |

With Rust auto-reorder (default `#[repr(Rust)]`):
- order_id (8) at 0..8
- price (8) at 8..16
- timestamp (16) at 16..32
- size (4) at 32..36
- action (1) at 36..37
- side (1) at 37..38
- 2 bytes padding to 8-byte alignment
- **Total: 40 bytes**

Without reorder (source order preserved): 48 bytes.

**Truth**: 40-48 bytes depending on Rust version's reorder behavior. The doc's 32 is too low (ignored `Option<i64>` cost of 16 bytes, not 8 — `Option<i64>` cannot use a niche). The discovery pass's 48 matches the worst-case observed value.

**Why this matters**: The "32B MboMessage" claim in CLAUDE.md is wrong. Performance reasoning that depended on this number (cache-line fits, copy cost) is also wrong by 25-50%.

---

## §8 — Methodological Lessons (For Future Audits)

The discovery pass downgraded by at least one tier on **8 of 9 smoking guns** (only SG-7 stood at the original severity). The validation pass corrected the record. Here is what we learned about HOW to audit this kind of repository.

(Validation correction: an earlier draft cited "6 of 9 overstated" — that count cannot be reconciled by any consistent counting rule against the §0 severity-change table. The strict count is: SG-1 refuted, SG-2 CRITICAL→LOW-MEDIUM, SG-3 HIGH→LOW empirically, SG-4 CRITICAL→LOW for SNAPSHOT, SG-5 CRITICAL→HIGH, SG-6 HIGH→MEDIUM, SG-7 unchanged, SG-8 HIGH→LOW, SG-9 HIGH→LOW. Eight of nine were downgraded; one stood.)

### Lesson 1 — Distinguish "code path exists" from "code path executes in production"

Several discovery-pass smoking guns were verified at the code level but empirically refuted: SG-1 (Action::Clear handling), SG-3 (Allow policy producing crossed quotes), SG-4 (F_SNAPSHOT replay double-add), SG-8 (phantom cancels biasing named features). In each case, the discovery agent traced a plausible mechanism in code without verifying that the production data exercises the path.

**Rule for future audits**: For every "smoking gun" CRITICAL claim, require:
- Code-path verification (file:line trace).
- Production-config verification (does the production caller actually hit this path?).
- Empirical verification (does the data shape that triggers this actually occur?).

If any of the three fails, downgrade severity. Three different bug classes:
- Code-path correct, production caller correct, data triggers it → ACTIVE.
- Code-path correct, production caller correct, data does not trigger it → LATENT (would activate on data shape change).
- Code-path correct, production caller doesn't hit it → DEAD or BENIGN.

### Lesson 2 — Multi-link causal chains are fragile; verify each link independently

SG-1's chain had 5 links. The discovery pass verified each link locally and concluded the chain held. The validation pass independently re-walked each link and found that link 3 was broken (Clear is filtered before reaching the reconstructor). The discovery agent missed this because the chain "sounded plausible" end-to-end and they didn't have time to independently verify each step.

**Rule for future audits**: For multi-link mechanisms, require explicit per-link verification with explicit "VERIFIED / REFUTED" status per link. The synthesis must call out which link is most fragile.

### Lesson 3 — Grep across ALL standalone repo paths, not just the audit's primary scope

The "begin_day doesn't exist" claim was wrong because the discovery agent greppped only `MBO-LOB-reconstructor/` and `feature-extractor-MBO-LOB/`. The hook is in `mbo-statistical-profiler/` and `opra-statistical-profiler/`. The repo's CLAUDE.md explicitly lists 17 standalone repos under the monorepo umbrella.

**Rule for future audits**: When greping for cross-repo claims (especially "X doesn't exist"), enumerate ALL the repos in `CLAUDE.md`'s "Inter-Repo Topology" section and grep each.

### Lesson 4 — "Dead code" is a structural finding (high confidence by grep)

The dead-code claims are the highest-quality findings in the audit. They were verified by exhaustive grep, are not overstated, and even the validation pass added MORE dead code that the discovery pass missed.

**Rule for future audits**: Lead with structural findings (grep-checkable claims). They have high signal and are easy to verify. Save causal claims (does X cause Y?) for after the structural pass.

### Lesson 5 — Frame severity by *measured* impact, not *theoretical* worst-case

The discovery pass tended toward worst-case framing: "double filter producing inconsistent stats" (in reality, the second filter never executes), "stale data leaking" (in reality, the consumers respect the bound), "ignores sizes" (in reality, the formula is consistent, just under a policy that isn't used in production).

**Rule for future audits**: A finding's severity should be ranked by what the data actually shows, not by what the worst-possible-interpretation of the code would show. If empirical data is unavailable, mark as LATENT and explicitly flag that production impact is unverified.

**Critical caveat (added in post-validation revision)**: This lesson is not "ignore latent bugs." Latent bugs whose triggering would cause **silent data corruption, partial-day exports, training-on-garbage outcomes, or any failure mode without operator-visible signal** must retain their HIGH/MEDIUM severity classification with explicit "latent-but-catastrophic" tagging. The absence of empirical impact is **not** evidence of low risk; it is evidence of an **untested invariant**. Concrete examples in this document where the caveat applies: F-002 / NEW-1 (silent stream termination — listed HIGH despite empirical column "Pending"), F-022 (signed-to-unsigned wraparound — fires only with non-monotonic timestamps but would silently overwrite the downsampler's threshold), F-026 (hotstore race window — fires only under concurrent decompression but would silently serve corrupt cached files). Lesson 5 downgrades **theoretical-worst-case framings**; it does **not** downgrade catastrophic-but-unfired mechanisms. The discovery pass's overclaim was its certainty about catastrophe; the correct calibration is "this WOULD be catastrophic IF triggered" with explicit triggering conditions, not "this is low-priority because it hasn't fired."

### Lesson 6 — Tests are NOT evidence of correctness, but they ARE evidence of intent

The user's instruction "do not depend on tests to catch issues" was correctly interpreted: tests cannot prove correctness because they may test the wrong invariant or mock the bug. But tests DO encode what the original author considered worth testing — which is itself useful information about intent.

For example, `test_intraday_curve_uses_begin_day_utc_offset` (`mbo-statistical-profiler/src/trackers/spread.rs:210`) is positive evidence that the `begin_day` lifecycle is intended to flow through to `intraday_curve.add()`. This test confirms the hook EXISTS, even though it doesn't prove the hook is used correctly everywhere.

**Rule for future audits**: Use tests to discover what the author intended to support; don't use them to conclude correctness.

### Lesson 7 — Honest refutation is as valuable as discovery

The validation refuted SG-1, SG-8, and the begin_day claim. These refutations are not failures of the audit — they are the audit working correctly. Without them, the team would have spent engineering time on phantom mechanisms.

**Rule for future audits**: Validation passes should be treated as first-class deliverables, not as confirmations of the discovery pass. A validation that refutes a finding is doing its job.

---

## §9 — Recommended Remediation Order

This section is presented for the engineering team's convenience but does NOT prescribe specific implementations — those decisions are out of scope for this audit document.

### Priority 0 — Verify before acting (1-2 hours)

P0.1. Confirm whether NVDA DBN files contain non-MBO records mid-stream (relevant to F-002 / NEW-1). Test by running with `skip_invalid=true` and checking `messages_skipped` in stats.

P0.2. Inspect `_reconstruction_stats.json` `errors` field across the 234 days; confirm it's all zeros (relevant to F-007 / NEW-2).

P0.3. Inspect a sample EDT day's time-based feature output (e.g., `session_progress`, `minutes_since_open`) and verify the 1-hour shift relative to actual market open (relevant to F-001 / SG-7).

### Priority 1 — Active production bugs (1-3 days)

1. **F-001 / SG-7 (TimeBasedSampler DST)** — Make `TimeBasedSampler` consume `hft_statistics::time::regime::utc_offset_for_date` per-day. Add a regression test using a known EDT date.

2. **F-002 + F-003 + F-024 (silent stream termination cluster)** — Address together. Make `MessageIterator::Item = Result<MboMessage, _>` so EOF-vs-error is unambiguous. Wire `messages_skipped` into a callable post-iteration API. F-024 (DbnBridge::convert silent EOF) is the same root cause as F-002 at a different layer; the `Result<MboMessage, _>` change addresses both.

3. **F-029 (volatility 10% returns filter)** — Silent dropping of circuit-breaker events. Replace fixed 10% threshold with sample-interval-dependent rule (e.g., `10 × rolling_std`). Add a `filtered_returns_count` stat. Decide whether dropped returns advance the buffer or skip the sample.

4. **F-007 + F-008 / NEW-2 + NEW-3** — Either remove the dead fields (`errors`, `bytes_read`) or wire them up. Hidden zero counters are worse than no counters.

5. **F-006 (features 68/69 hardcoded 0.0)** — Either implement queue tracking (or wire `QueuePositionTracker`) or formally deprecate the features in `pipeline_contract.toml`.

6. **F-021 (silent reconstructor-error swallow at `export_to_parquet.rs:411`)** — Replace `.is_ok()` with explicit error handling. Add per-error-kind counter to `ParquetExportStats`.

### Priority 2 — Architectural cleanup (1-2 weeks, but high long-term value)

7. **§6 Dead code** — Decide library-vs-pipeline framing; either consolidate to one implementation or formalize the divergence.

8. **§5 C4 re-derivation pattern** — Add a `begin_day` lifecycle hook to feature-extractor's `FeatureEngine`/`Computer` traits (parallel to profiler crates); centralize offset/day_epoch derivation; remove per-call re-derivation.

9. **F-009 / NEW-4 (state.sequence cross-repo drift)** — Rename `state.sequence` to `events_processed`, OR thread the actual exchange sequence through `MboMessage`, OR fix the analyzer documentation.

10. **F-005 / LB-3 today-active in export path** — Change `LobBatch::push` to iterate `0..state.levels`, OR clear all `MAX_LOB_LEVELS` slots in `fill_lob_state_with_temporal`.

11. **F-026 (hot-store decompress race + integrity)** — Add per-process unique suffix to `temp_path`, use `OpenOptions::create_new(true)` for exclusive lock, optionally checksum the existing decompressed file.

12. **F-030 (BatchProcessor `FailFast` runs all files)** — Either rename to `RunAllReportFirst` and add a true `FailFast` variant, OR add `OnErrorAction` callback for sibling-task cancellation.

### Priority 3 — Definitional / research / observability

13. **F-004 / SG-6 (executed_pressure event-count)** — Decide if features 86, 88, 89 should be volume-weighted or count-based. If volume, implement and A/B test IC change. **(Post-amendment scope expansion**: F-004 also covers features 48-59 per N-17 verification — see updated F-004 narrative.)

14. **F-012 / NEW-7 (MboWindow.first_ts post-eviction)** — Update `first_ts` after eviction. Affects all rate-based features. Compounds with F-004.

15. **F-010 / NEW-5 (`system_messages_skipped` always 0 in production)** — Either move the counter to the consumer side, document the asymmetry explicitly, or remove the consumer's pre-filter. MEDIUM observability failure that should not languish in P4.

16. **F-027 (`MultiLevelOfiTracker` per-level false spikes)** — Distinguish "true level appearance" from "displacement". Non-trivial: requires comparing the entire price ladder across transitions. Affects features 116-147 if the experimental groups are enabled.

17. **F-028 (hardcoded `SQRT_ANNUAL_TRADING_SECONDS`)** — Make the annualization factor a config field. Allow per-dataset override.

18. Document SG-3 (F-017) and SG-4 (F-018) as "policy choices" with explicit conditions under which they would become bugs (multi-venue / live streaming).

19. **F-019 / F-020 (documentation drift on struct sizes)** — Tighten the `test_lob_state_size_unchanged` assertion to exact-byte match; update CLAUDE.md and the doc-comment at `types.rs:268` to reflect the actual ~576 B (LobState) and 40-48 B (MboMessage) values. One-line edit each, but prevents future drift.

### Priority 4 — Latent bugs (defer; address opportunistically)

20. **Latent-but-catastrophic** (per §8 Lesson 5 caveat — DO NOT defer indefinitely; track triggering conditions): **F-022** (`MinIntervalNs` wraparound on non-monotonic timestamps), **F-026** (hotstore race — already in P2 #11 because the integrity-check side fires under non-concurrent conditions too), **F-002 + F-024** (silent stream termination — already in P1 #2). Tag these with explicit "if X, Y is broken" notes.

21. **Latent operational annoyances**: F-022 (when `MinIntervalNs` configured), F-031 (empty file silent success), F-032 (`.zst` extension filter), F-033 (`extract_date` first-match), F-034 (`EveryN(0)` semantic). Address when convenient.

22. **Latent (won't fire under current code paths/data shapes)**: F-023 (timestamp=0 fallback under DBN), F-025 (state.levels as u8 for MAX_LOB_LEVELS=20), F-011, F-013, F-014, F-015 (which subsumes NEW-9 per §A), F-016, LB-1, LB-2, LB-4, LB-5, LB-6, LB-7 — file as tech-debt.

### Don't work on (refuted)

- R-001 / SG-1 (OFI Clear poisoning) — phantom mechanism.
- R-002 / SG-8 (phantom cancels for features 51/55/89) — refuted; those features don't consume the affected field.
- R-003 / "begin_day creation" — already exists in profiler crates.

---

## §A — Appendix: Discovery-Pass Findings to Validated-Pass Findings Cross-Reference

| Discovery ID | Validated ID | Status |
|--------------|--------------|--------|
| SG-1 | R-001 | REFUTED |
| SG-2 | F-013 | VERIFIED but downgraded LOW–MEDIUM |
| SG-3 | F-017 | VERIFIED in code, REFUTED empirically |
| SG-4 | F-018 | VERIFIED in code, REFUTED empirically (for SNAPSHOT specifically) |
| SG-5 | F-003 (subsumed by F-002) | VERIFIED HIGH |
| SG-6 | F-004 | VERIFIED MEDIUM |
| SG-7 | F-001 | VERIFIED HIGH |
| SG-8 | R-002 | REFUTED for production features; LOW–MEDIUM for profiler |
| SG-9 | F-016 | VERIFIED LOW (blast radius zero) |
| LB-1 | (latent, see §1 table) | VERIFIED, latent-primed |
| LB-2 | (latent, see §1 table) | VERIFIED, today-masked |
| LB-3 | F-005 | VERIFIED + escalated to TODAY-ACTIVE |
| LB-4 | (latent, see §1 table) | VERIFIED as trait-contract violation |
| LB-5 | F-015 (with NEW-9) | VERIFIED, direction corrected |
| LB-6 | (latent, see §1 table) | VERIFIED, cosmetic |
| LB-7 | F-014 | VERIFIED + clarified |
| (begin_day claim) | R-003 | REFUTED |
| (LobState 580B) | F-019 | VERIFIED but imprecise (real value ~576B) |
| (MboMessage 48B) | F-020 | VERIFIED for one layout (real value 40-48B) |
| NEW-1 | F-002 | NEW HIGH |
| NEW-2 | F-007 | NEW MEDIUM |
| NEW-3 | F-008 | NEW MEDIUM |
| NEW-4 | F-009 | NEW MEDIUM |
| NEW-5 | F-010 | NEW MEDIUM |
| NEW-6 | F-011 | NEW LOW |
| NEW-7 | F-012 | NEW MEDIUM (folded into F-004 narrative) |
| NEW-8 | F-019 + F-020 | NEW DOCUMENTATION |
| NEW-9 | F-015 | NEW LOW (folded into F-015) |

### Phase 2 cross-references (post-validation additions)

The validation pass (VV5) surfaced 17 candidate findings in less-deeply-audited modules. Phase-2 verifiers (VVN1 + VVN2) re-confirmed each. Final disposition:

| Phase-2 ID | Validated ID | Status | Severity |
|------------|--------------|--------|----------|
| N-1 | F-021 | VERIFIED | MEDIUM |
| N-2 | F-022 | VERIFIED | MEDIUM (latent-but-catastrophic per Lesson 5 caveat) |
| N-3 | F-023 | VERIFIED-WITH-NUANCE | LOW (defensive gap; doesn't fire under DBN) |
| N-4 | F-024 | VERIFIED | HIGH (library) / N/A (export bin) |
| N-5 | (REFUTED) | REFUTED — Arrow `RecordBatch` built positionally; column-name "drift" is internal-only | — |
| N-6 | F-025 | VERIFIED-WITH-NUANCE | LOW (latent; would fire if `MAX_LOB_LEVELS > 255`) |
| N-7 | (REFUTED) | REFUTED — MBO writer has no downsampling; `rows_seen == rows_written` is honest equality | — |
| N-8 | F-026 | VERIFIED | MEDIUM |
| N-9 | F-027 | VERIFIED | MEDIUM |
| N-10 | F-028 | VERIFIED | MEDIUM (mis-scales features 106, 107, 111 if non-equity dataset) |
| N-11 | F-029 | VERIFIED | HIGH |
| N-12 | F-030 | VERIFIED | MEDIUM |
| N-13 | F-031 | VERIFIED | LOW |
| N-14 | F-032 | VERIFIED | LOW-MEDIUM |
| N-15 | F-033 | VERIFIED | LOW |
| N-16 | F-034 | VERIFIED-WITH-NUANCE | LOW |
| N-17 | (folded into F-004) | VERIFIED-WITH-NUANCE — V5's enumeration had two indexing errors (feature 66 is `size_skewness`, not `large_order_imbalance`; feature 75 IS `large_order_imbalance` and IS volume-weighted). Corrected enumeration: 12 features (48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59) share the F-004 drift; 65 and 75 do NOT. | MEDIUM (extends F-004 scope) |

**Net result**: 14 new F-NNN entries (F-021 through F-034) + F-004 enumeration update + 2 REFUTED-on-re-verification (N-5, N-7) appropriately not added.

---

## §B — Appendix: Modules Audited vs Not Audited

### Audited deeply (both passes)

- `MBO-LOB-reconstructor/src/lob/reconstructor.rs` (2,557 LOC)
- `MBO-LOB-reconstructor/src/lob/order_lifecycle.rs` (1,637 LOC) — confirmed dead in production
- `MBO-LOB-reconstructor/src/lob/queue_position.rs` (1,683 LOC) — confirmed dead in production
- `MBO-LOB-reconstructor/src/lob/trade_aggregator.rs` (943 LOC) — confirmed dead in production
- `MBO-LOB-reconstructor/src/lob/day_boundary.rs` (614 LOC) — confirmed dead
- `MBO-LOB-reconstructor/src/lob/multi_symbol.rs` (385 LOC) — confirmed dead
- `MBO-LOB-reconstructor/src/lob/price_level.rs` (430 LOC)
- `MBO-LOB-reconstructor/src/dbn_bridge.rs` (287 LOC)
- `MBO-LOB-reconstructor/src/loader.rs` (574 LOC)
- `MBO-LOB-reconstructor/src/source.rs` (598 LOC)
- `MBO-LOB-reconstructor/src/hotstore.rs` (857 LOC)
- `MBO-LOB-reconstructor/src/types.rs` (1,377 LOC)
- `MBO-LOB-reconstructor/src/statistics.rs` (826 LOC) — partly dead
- `MBO-LOB-reconstructor/src/analytics.rs` (566 LOC) — confirmed dead
- `MBO-LOB-reconstructor/src/warnings.rs` (717 LOC) — confirmed dead
- `MBO-LOB-reconstructor/src/lib.rs` (242 LOC)
- `MBO-LOB-reconstructor/src/constants.rs` (104 LOC)
- `MBO-LOB-reconstructor/src/error.rs` (103 LOC)
- `feature-extractor-MBO-LOB/crates/hft-extractor/src/pipeline.rs`
- `feature-extractor-MBO-LOB/crates/hft-extractor/src/adapters.rs`
- `feature-extractor-MBO-LOB/crates/hft-feature-core/src/signals/ofi.rs`
- `feature-extractor-MBO-LOB/crates/hft-feature-core/src/signals/mod.rs` (lines 159-188, 260-289)
- `feature-extractor-MBO-LOB/crates/hft-feature-core/src/derived.rs`
- `feature-extractor-MBO-LOB/crates/hft-feature-core/src/mbo/mod.rs` (lines 124-125, 327-389, 648-674)
- `feature-extractor-MBO-LOB/crates/hft-feature-core/src/mbo/window.rs`
- `feature-extractor-MBO-LOB/crates/hft-sequence-engine/src/sampling.rs` (lines 567-660)
- `feature-extractor-MBO-LOB/crates/hft-feature-core/src/experimental/seasonality.rs` (lines 78-130)
- `mbo-statistical-profiler/src/lib.rs` (trait definition)
- `mbo-statistical-profiler/src/profiler.rs` (lines 71-120)
- `mbo-statistical-profiler/src/trackers/ofi.rs` (lines 562, 607-613)
- `opra-statistical-profiler/src/lib.rs` (trait definition)
- `opra-statistical-profiler/src/profiler.rs` (lines 96-110)

### Audited with less depth (worth a follow-up pass)

- `MBO-LOB-reconstructor/src/export/*.rs` (**5 files, ~1,315 LOC**: `batch.rs` 327, `lob_writer.rs` 260, `mbo_writer.rs` 198, `mod.rs` 183, `schema.rs` 347) — Parquet writer surface. **Note**: post-validation, `batch.rs` and `schema.rs` produced active findings (F-005, F-009) and the directory now hosts F-021, F-022, F-025, F-027, F-034 (post-amendment); functionally promoted to "audited" but the original validation scope is preserved here for historical accuracy.
- `MBO-LOB-reconstructor/src/bin/decompress_to_hot_store.rs` (379 LOC) — post-amendment hosts F-032
- `MBO-LOB-reconstructor/src/bin/export_to_parquet.rs` (680 LOC) — post-amendment hosts F-021 (silent error swallow) and F-033 (`extract_date` first-match)
- `MBO-LOB-reconstructor/examples/*.rs` (4 files) — examples; touched by validation but not audited as production code paths.
- `MBO-LOB-reconstructor/benches/reconstruction.rs` — benchmarks; not audited.
- `feature-extractor-MBO-LOB/crates/hft-extractor/src/batch.rs` (BatchProcessor) — post-amendment hosts F-030, F-031
- `feature-extractor-MBO-LOB/crates/hft-feature-core/src/experimental/volatility.rs` — post-amendment hosts F-028, F-029
- `feature-extractor-MBO-LOB/crates/hft-feature-core/src/experimental/institutional_v2.rs`
- `feature-extractor-MBO-LOB/crates/hft-feature-core/src/lob.rs` (the `LobComputer`; **note**: the file is at `src/lob.rs`, NOT `src/computers/lob.rs` — the `computers/` subdirectory does not exist)
- `feature-extractor-MBO-LOB/crates/hft-feature-core/src/ofi/multi_level.rs` — flagged as having latent false-spike bug pattern; post-amendment hosts F-027
- `feature-extractor-MBO-LOB/crates/hft-feature-core/src/derived.rs` (the `extract_derived` paths beyond what was checked; **note**: the file is at `src/derived.rs`, NOT `src/computers/derived.rs`)
- `feature-extractor-MBO-LOB/crates/hft-sequence-engine/src/builder.rs` (SequenceBuilder)
- `feature-extractor-MBO-LOB/crates/hft-export-pipeline/src/normalization_compat.rs`
- All other tracker crates in `mbo-statistical-profiler/` and `opra-statistical-profiler/`

### Not audited (out of scope)

- `databento-ingest/`
- `MBO-LOB-analyzer/` (Python parquet analyzer)
- `lob-dataset-analyzer/`
- `lob-model-trainer/`
- `lob-models/`
- `lob-backtester/`
- `hft-feature-evaluator/`
- `hft-ops/`
- `hft-statistics/` (touched only at `time/regime.rs`)
- `hft-metrics/`

---

## §C — Appendix: Cross-References to Prior Audit Documents

This document does not supersede prior audit work; it complements and corrects it where validation found drift.

- `feature-extractor-MBO-LOB/BACKBONE_FINDINGS_2026_04.md` — earlier audit of feature-extractor-MBO-LOB. Compatible with this document; different scope.
- `feature-extractor-MBO-LOB/BACKBONE_FINDINGS_PHASE2_2026_04.md` — Phase 2 audit. Documents NEW-AUDIT-A3 (the Action::Clear filtering pattern). Validator V1 referenced this document.
- `MBO-LOB-reconstructor/CLAUDE.md` — module orientation. The `begin_day` claim CLAUDE.md makes is CORRECT (R-003). The size claims for LobState (560B) and MboMessage (32B) are slightly off (F-019, F-020).
- `MBO-LOB-reconstructor/CODEBASE.md`, `README.md`, `ARCHITECTURE.md`, `CHANGELOG.md` — describe the dead modules as advertised public surface (see §6).
- Root `CLAUDE.md` — production pipeline orientation. Contains correct C1-C4 audit history. The "C4 latent-bug pattern" framing in root CLAUDE.md is consistent with §5 of this document.
- Root `PIPELINE_ARCHITECTURE.md` — authoritative technical reference. Contains detailed Producer→Consumer matrix. May need updating to reflect F-009 (state.sequence semantics).
- `MBO-LOB-analyzer/CODEBASE.md:179` — documents `sequence` column as "Sequence number from exchange." Inconsistent with producer behavior (see F-009).

---

## §D — Appendix: Verification Methodology Per Finding

Each finding in this document was verified by at least one of the validation agents. The methodology:

1. **Code-trace verification**: open the cited file at the cited line; read ±30 lines for context; confirm the claim matches the code verbatim.
2. **Grep verification**: for "X is not used" / "X is dead" claims, exhaustive grep across all standalone repo paths in CLAUDE.md's Inter-Repo Topology section.
3. **Empirical verification**: for "this fires in production" claims, inspect actual `_reconstruction_stats.json` files (234 NVDA days), raw DBN files (4 sample days, 61M records), and TOML configuration files.
4. **Manual computation**: for size and arithmetic claims (F-019, F-020, LB-2 numeric example), compute manually using Rust struct alignment rules and saturating-arithmetic semantics.
5. **Cross-reference**: for definitional claims (F-004), cross-reference with cited research papers (Cont 2014, Bouchaud 2018, etc.).
6. **Counter-test**: for "this bug doesn't fire" claims, attempt to construct a scenario in which it would fire; confirm whether that scenario can occur with the production data shape.

Each finding's severity uses these labels:
- **HIGH**: Active production bug corrupting features today, OR latent bug primed to detonate on a routine future change.
- **MEDIUM**: Active production bug with limited blast radius, OR architectural debt that will compound, OR observability failure that masks other issues.
- **LOW**: Active bug with minor impact, OR latent bug with low trigger probability, OR cosmetic issue.
- **REFUTED**: Discovery-pass claim that does not hold up under validation. Documented in §4 to prevent re-discovery.

---

## §E — Appendix: Recheck Triggers and Half-Life

This document is a snapshot. Empirical claims, code citations, and severity rankings have a half-life. Operating without an explicit re-validation schedule converts the document from a useful operational record into a slow-drift trust hazard. This appendix makes the half-life explicit.

### What decays and on what timescale

| Decay vector | Half-life | Validation method |
|--------------|-----------|-------------------|
| Empirical day count, message count (e.g., "234 days / 2.77B messages") | 1-3 months (as new data arrives) | Re-run the aggregation script on `data/exports/raw_lob_full/*_reconstruction_stats.json` |
| Empirical EDT/EST partition (~165/234 days) | 6 months (DST schedule changes once per session; new data shifts the ratio) | Re-derive from dataset date range |
| File:line citations | Days-to-weeks under active development; months under stable release | `grep` for the cited symbol; verify line number is approximately correct (off-by-few is OK) |
| dbn library version (currently `dbn 0.20.0` git-pinned) | Indefinite while pinned; immediate on `cargo update` | Check `Cargo.lock` for resolved version vs cited 0.20.0 |
| Severity rankings | Decay only as fixes land — these are remediation-state-dependent | Update §1 verification matrix as fixes ship |
| Production-config citations ("15 production TOML configs all hardcode `-5`") | 1-3 months as configs evolve | Re-grep `feature-extractor-MBO-LOB/configs/*.toml` for `utc_offset_hours` |
| dead-code claims | Stable until a deletion or wire-up event | Re-grep across all 17 standalone repos |
| Methodological lessons (§8) | Indefinite — these are institutional knowledge | No re-validation needed |
| Structural findings (§6 dead code list) | Stable until a wire-up event | Same as dead-code claims |

### Re-validation triggers (re-run a focused validation pass IF any of the following occur)

1. **Data-shape change**:
   - Multi-venue / NBBO consolidated data is ingested → re-validate F-017 (CrossedQuotePolicy) and F-018 (DBN flags) AS THEY WILL ACTIVATE.
   - Live-streaming Databento subscription replaces batch-historical files → re-validate F-018 SNAPSHOT specifically (live streams DO contain `SNAPSHOT` records).
   - Non-equity asset class added (futures, FX, crypto) → re-validate F-001 (DST math), F-028 (`SQRT_ANNUAL_TRADING_SECONDS`), all 12 F-004 enumerated count-vs-volume features.
   - New symbol added with materially different microstructure → re-validate F-013 (modify-of-missing rate), F-027 (multi-level OFI false-spike rate).

2. **Library version change**:
   - `dbn` library upgraded past 0.20 → re-validate F-002, F-024 (silent EOF mode), and all DBN flag-handling paths (the constants and `silence_eof_error` semantics may change).
   - `arrow`/`parquet` major version bump → re-validate F-005 (LobBatch::push iteration), F-009 (state.sequence schema), AND re-confirm the **N-5 refutation rationale** (positional Arrow column construction → schema-name vs Rust-field-name divergence is internal-only) still holds; if Arrow changes its `RecordBatch::try_new` semantics to be field-name-driven instead of positional, N-5 becomes a real finding to add as F-NNN.
   - `rayon` major version bump → re-validate F-030 (BatchProcessor FailFast semantics).

3. **Code-structure change**:
   - `MAX_LOB_LEVELS` raised above 20 → F-025 (state.levels as u8 truncation) becomes ACTIVE.
   - `LobReconstructor` hoisted from local-in-`process_messages` to `Pipeline` field → LB-1 (Pipeline::reset omits LOB) becomes ACTIVE.
   - `update_order_size(.., 0)` is wired into reconstructor (currently only used by dead `QueuePositionTracker`) → F-015 (microprice degenerate collapse) becomes ACTIVE.
   - Anyone wires `MultiSymbolLob` into production → F-016 (asymmetric reset) becomes MEDIUM (currently LOW because zero downstream consumers).

4. **Time-elapsed**:
   - **6 months** (next: 2026-10) → run a focused recheck of all "VERIFIED-WITH-NUANCE" findings, all empirical claims, and the high-stakes citation set from VV1's verification (the 17 specifically-scrutinized citations in §1's "Active production bugs" table; see methodology step 2 below).
   - **12 months** → full re-validation pass (same scope as the original Phase-2 validation).

5. **Remediation-state changes**:
   - **3+ findings remediated** (P1 or P2 items) → re-rank P3/P4 priorities, update §1 verification matrix to mark remediated items, re-derive whether any findings now block on others.
   - **F-002 + F-003 + F-024 fix lands** (silent stream termination cluster) → re-measure the "BBO 99.17%" metric per §3.5; the silent-error-handling fix should surface the actual error rate and allow precise contributor allocation.

### Re-validation methodology

For a focused recheck (not a full re-validation):

1. Re-run the empirical-claim verification script (E1-E12 from VV3): aggregate `_reconstruction_stats.json`, sample DBN flag distribution, count config files with `utc_offset_hours=-5`, etc.
2. Spot-check the high-stakes citation list from VV1 (F-001, F-002, F-006, F-009, R-001, R-003 specifically).
3. Re-derive the §0 severity table — has any finding's severity changed because (a) it was remediated, (b) data shape changed, or (c) new evidence emerged?
4. Update §1 verification matrix marks: VERIFIED → REMEDIATED, or VERIFIED → REFUTED, or new evidence → severity change.
5. Update this §E with the recheck date and notable changes since last recheck.

### Recheck log

| Date | Scope | Notable changes | Validator |
|------|-------|-----------------|-----------|
| 2026-04 (initial) | Full 7-agent discovery + 7-agent validation + 6-agent meta-validation + 2-agent Phase-2 verification | Document created. Refutes SG-1, SG-8, begin_day claim. Adds 14 Phase-2 findings (F-021 through F-034). | Phase-2 verifiers VVN1 + VVN2 |
| **2026-04-26 (post-audit freshness check)** | 5-agent validation across all 34 F-NNN findings + git/CHANGELOG/file-mtime audit of consumer repos + upstream hft-statistics activity check | **ZERO findings remediated. ZERO ground-shift.** All 34 F-NNN findings remain STILL-ACTIVE. MBO-LOB-reconstructor: zero commits since 2026-04-06. feature-extractor-MBO-LOB main: zero commits since 2026-04-10 (one commit on unmerged branch `deps/hft-statistics-v0.2.1` only bumps Cargo.toml pin). Every audit citation matches current source byte-for-byte. **Upstream hft-statistics PR-08 (2026-04-26) shipped the fix infrastructure for F-001 (`RthSession::UsNyseNasdaq` + `chrono-tz` + DST-aware `utc_offset_for_date` under default-on `time-session` feature flag), but the `hft-sequence-engine` crate has NOT added `hft-statistics` as a dependency** — its "Zero external dependencies beyond std" policy is preserved. The F-001 fix is now mechanically possible (upstream is ready) but requires an architectural decision about that policy. **Upstream PR-13 (BoundedFifo / StackBoundedFifo) and PR-14 (`days_to_ymd` consolidation) similarly available but not adopted.** Refinements to existing audit: F-024's caller count is 3 active (not 4 — the 4th was archived monolith code). F-006 has a bonus finding: dangling `include_queue_tracking` TOML config field at `crates/hft-export-pipeline/src/config/features.rs:41` that no consumer reads + contract/code naming skew (`queue_size_ahead` vs `queue_volume_ahead`). | 5 parallel adversarial validators (VS1-VS5) |
| **NEXT: 2026-10 or earlier on data-shape change** | Recheck all VERIFIED-WITH-NUANCE findings + empirical claims + high-stakes citations | TBD | TBD |

### New informational findings from 2026-04-26 recheck (not added as F-NNN)

These are non-blocking observations that do not affect any active F-NNN finding's severity or remediation plan, but are worth tracking:

| ID | Finding | Severity | Status |
|----|---------|----------|--------|
| N5-1 | `hft-feature-core/Cargo.toml:14` pins `hft-statistics = { ..., branch = "main" }` — violates upstream's `VERSIONING.md §3.1` "never `branch = main`" rule. The fix exists on unmerged branch `deps/hft-statistics-v0.2.1` (`tag = "v0.2.1"`); merging it would resolve the floating-pin issue. | LOW-MEDIUM | OPEN |
| N5-2 | `feature-extractor-MBO-LOB` working tree has 4 modified + 2 untracked files (`crates/hft-normalization/src/lib.rs`, `ARCHITECTURE.md`, `CLAUDE.md`, `docs/TEST_INVENTORY.md` modified; `BACKBONE_FINDINGS_2026_04.md`, `BACKBONE_FINDINGS_PHASE2_2026_04.md` untracked). Auditors verifying against `git show HEAD:...` see different content from on-disk files. Doc-only changes; no behavior impact. | LOW | OPEN |
| N5-3 | Upstream `hft-statistics` PR-08 (2026-04-26) shipped `RthSession::UsNyseNasdaq` + DST-aware `utc_offset_for_date` under default-on `time-session` feature flag. **This is the fix infrastructure F-001 needs**, but `hft-sequence-engine/Cargo.toml` has not added `hft-statistics` as a dep. F-001 is now blocked on an architectural decision (relax "zero external deps" policy) rather than upstream availability. | INFORMATIONAL | UPSTREAM-READY |
| N5-4 | Upstream `hft-statistics` PR-13 (`BoundedFifo` + `StackBoundedFifo`) available; could replace hand-rolled bounded-window patterns in `crates/hft-feature-core/src/mbo/window.rs` and the volatility/MBO event windows. Debt-reduction opportunity, not a defect. | INFORMATIONAL | UPSTREAM-READY |
| N5-5 | `crates/hft-feature-contract/src/generated.rs` mtime is 2026-04-25 (codegen rebuild). Content is deterministic from `contracts/pipeline_contract.toml`; mtime can mislead auditors who check file freshness without checking content. Not a defect; a process note. | INFORMATIONAL | OPEN |

---

**END OF AUDIT DOCUMENT**

This document is the complete, validated record of findings as of the 2026-04 audit. All discovery-pass claims have been validated; refuted claims are explicitly marked. Phase-2 verification re-confirmed 14 of 17 newly-discovered findings (2 refuted: N-5 column-name drift, N-7 fake metric; 1 corrected enumeration: N-17). The document is intended to be consumed by the engineering team when designing fixes and by future audit sessions to prevent re-discovery of phantom bugs. **Re-validate per §E triggers.**
