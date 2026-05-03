# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added — Phase O Cycle 1 (2026-05-03, 2 commits + this docs sweep)

**Producer-side correctness cycle** — closes the Action::Clear silent-
passthrough class (NEW-AUDIT-A3 / FIND-NEW-6 follow-up) that was
DEFERRED from Phase K K.5 and previously documented in the sibling
`feature-extractor-MBO-LOB` repo's CLAUDE.md / EXPORT_INDEX.md /
diagnostics.rs as "remains OPEN — out of K.5 scope" at the (phantom)
`adapters.rs:123` location. Plan + per-commit ship-ledger at
`/Users/knight/.claude/plans/spicy-tickling-mist.md` §"Phase O".

- **B.1** `29a3c96` — fix(export): single source of truth for
  `LobBatch::push` `level_count` column. Pre-B.1 the metadata-data
  twin tuple at adjacent sites in `src/export/batch.rs` read
  `level_count` from BOTH `state.levels.min(MAX_LOB_LEVELS)` (capped)
  AND `self.levels` (per-file invariant); on heterogeneous-`LobState`
  pushes the twin pair could silently desync. Post-fix: single read of
  `self.levels` at both sites + `debug_assert_eq!(state.levels,
  self.levels)` defense-in-depth in test/dev builds. Schema docstring
  at `src/export/schema.rs` updated documenting the per-file invariant.
  +3 regression tests (`b1_level_count_uniform_under_homogeneous_pushes`,
  `b1_debug_assert_fires_on_heterogeneous_state_levels` (test only),
  `b1_metadata_data_pair_consistent_post_fix`).

- **B.2a** `90b0966` — fix(lob): exempt `Action::Clear` from the inner
  `is_system_message()` filter at `src/lob/reconstructor.rs:1179` AND
  from `validate_message` at `:1206`. Pre-B.2a default-config callers
  (`LobConfig::skip_system_messages = true` — DEFAULT) silently swallowed
  every real-data `Action::Clear` message because Clear messages match
  the `is_system_message()` predicate (zero `order_id`/`size`/`price`
  by structural definition). The Clear handler at the
  `Action::Clear` arm of `process_message_into`'s match dispatch was
  thus UNREACHABLE under default config; book never reset on
  session-boundary Clear messages → `LobStats::book_clears` permanently
  shadowed at zero → stale orders persisted across session breaks →
  consumer features silently corrupt for the post-Clear segment of
  every Clear-bearing trading day. Fix exempts Clear specifically
  (other zero-field messages still filtered as heartbeats per
  documented `WARNINGS.md` § BOOK_CLEARED behavior; `Action::None`
  intentionally NOT exempted per `types.rs:48` "no-op action [that]
  may carry flags or other information"). +1 regression test
  (`b2a_action_clear_works_under_default_config`) verifying default-
  config Clear pass-through; pre-existing `test_clear_resets_book`
  using `with_skip_system_messages(false)` workaround retained as
  audit-pointer regression guard.

  Companion fix in extractor `feature-extractor-MBO-LOB` commit
  `ab52176` (B.2b) exempts Clear from the OUTER filter at
  `crates/hft-extractor/src/pipeline.rs:273`. Together: `Action::Clear`
  now flows through both filter layers → reconstructor's Clear handler
  fires → `self.reset()` clears the book → downstream features reflect
  the post-Clear empty book correctly.

  **Mid-cycle 6-agent adversarial validation**
  (`/Users/knight/code_local/HFT-pipeline-v2/PHASE_O_VALIDATION_FINDINGS_2026_05_03.md`)
  surfaced 9 BLOCKING items + 9 follow-up items. This commit (B-1
  reconstructor docs sweep) is one of the Stage E-pre BLOCKING items
  the validation surfaced.

### Changed — User-visible behavior shift (default-config callers)

**Default `LobConfig` semantic restoration** (B.2a): pre-B.2a callers
using `LobConfig::default()` (which sets `skip_system_messages = true`)
experienced silent book-state corruption on any trading day containing
real-data `Action::Clear` messages (mid-session halts, session
transitions, exchange system resets, end-of-day clears). The book was
never reset on these messages because the inner `is_system_message()`
filter at `src/lob/reconstructor.rs:1177-1180` swallowed them BEFORE the
`Action::Clear` arm of the match dispatch (lines 1216-1227 post-B.2a)
could run.

Post-B.2a: under default `LobConfig`, `Action::Clear` messages now reach
the Clear handler and trigger `self.reset()` as the docstring at
`src/lob/reconstructor.rs::process_message_into` and `WARNINGS.md`
§ BOOK_CLEARED have always documented. This is a CORRECTNESS RESTORATION
— pre-existing tests using `LobConfig::with_skip_system_messages(false)`
(non-default workaround) were the only ones that exercised the Clear
handler path on real data; default-config callers had a silent regression
that this commit closes.

**Operator-impact note**: any pre-B.2a NPY exports (produced by the
sibling `feature-extractor-MBO-LOB` consumer) of trading days containing
`Action::Clear` messages have silently corrupt book state for the
post-Clear segment. A two-class corpus inventory + remediation plan is
scheduled separately (Phase O follow-up **F-9** per
`PHASE_O_VALIDATION_FINDINGS_2026_05_03.md` §"FOLLOW-UP Items").

### Notes

- `LobStats::book_clears` field was UNCHANGED by B.2a — the counter's
  pre-B.2a permanent zero was a downstream symptom of the silent-filter
  bug, not a counter-side issue. Post-B.2a the counter increments
  correctly per Clear message that reaches the handler.
- Bit-exact preservation re-verified by sibling extractor's golden
  hashes `GOLDEN_HASH_SEQUENCES_NPY = 0x8bebad9b09b564cd` +
  `GOLDEN_HASH_LABELS_NPY = 0x5dcf907068fcadcc` UNCHANGED across B.1 +
  B.2a (synthetic-fixture argument: zero-Clear-message fixtures make
  B.2a exemption a structural no-op).
- Cycle 1 close push will tag this as `v0.2.1` and the extractor will
  bump its `Cargo.toml:35` pin from `tag = "v0.2.0"` to
  `tag = "v0.2.1"` per the standard 5-repo atomic-coordinated cross-repo
  pattern. Pre-push extractor CI on its own HEAD currently tests
  against OLD `v0.2.0` (which does NOT contain B.1 or B.2a); end-to-end
  Clear-handling correctness verified locally but the cross-repo CI
  Green Badge does NOT prove it until the cycle-close push completes
  (B-4 in `PHASE_O_VALIDATION_FINDINGS`). Stage E-pre B-2 cross-repo
  E2E integration test (separate Stage E-pre commit) closes this gap.

### Fixed — Phase O Cycle 1: B.3 MinIntervalNs wraparound + DownsampleStats observability (2026-05-03, 1 commit)

Closes the silent-semantic-inversion bug at
`src/export/lob_writer.rs::should_write` (renamed `write_decision`
post-B.3) under the `DownsampleStrategy::MinIntervalNs(min_ns)` arm.
The pre-B.3 expression `(ts - last) as u64 >= *min_ns` cast a negative
`i64` (when `ts < last`, i.e., out-of-order timestamps) to a near-
`u64::MAX` value, which is ALWAYS `>= min_ns` for any finite `min_ns`,
causing out-of-order snapshots to be ALWAYS WRITTEN instead of SKIPPED.
This is the same defect class as the EveryN(0) silent reinterpretation
(closed F-034 in Phase M M.A.4) — a downsample-time predicate that
silently degenerates under out-of-spec input.

Closes the C-4 latent companion bug (surfaced by post-design adversarial
review): `last_written_ts = state.timestamp` overwrote the throttle
anchor with `None` when a `None`-timestamp snapshot was written in the
middle of a stream, silently resetting the throttle so the NEXT
snapshot was always written regardless of its delta. Post-B.3 the
anchor only advances when `state.timestamp.is_some()` — a real
timestamp must back the throttle.

- **B.3** — fix(export): close MinIntervalNs i64→u64 wraparound +
  surface out-of-order skip counter via DownsampleStats. File scope
  (5 source + 1 test + this CHANGELOG):
  - `src/export/mod.rs` — new `DownsampleStats { out_of_order_skipped: u64 }`
    `#[non_exhaustive]` struct + `ParquetExportStats` extended with
    `downsample: Option<DownsampleStats>` field + `#[non_exhaustive]`
    attribute (sibling-policy alignment with the four peer Phase M
    REV 3 boundary-discipline-aligned types: `LoaderStats`, `LobStats`,
    `LobStatsExportEnvelope`, `BoundaryError`, plus `TlobError`).
  - `src/export/lob_writer.rs` — private `WriteDecision` enum
    (`Write` / `SkippedDownsample` / `SkippedOutOfOrder`) returned from
    new `write_decision(&self, state) -> WriteDecision` (refactor of
    `should_write`); `out_of_order_skipped: u64` counter on
    `LobSnapshotWriter`; `write_snapshot` matches on `WriteDecision`
    and increments the counter + emits `log::warn!` on
    `SkippedOutOfOrder`; `last_written_ts` only advances when
    `state.timestamp.is_some()` (C-4 fix); `finish()` constructs
    `Some(DownsampleStats { .. })` when `config.downsample.is_some()`,
    `None` otherwise.
  - `src/export/mbo_writer.rs` — `finish()` always returns
    `downsample: None` (MBO events are written 1:1 with no downsample
    strategy; preventing the "fake metric" anti-pattern where a writer
    reports counters it doesn't compute).
  - `src/bin/export_to_parquet.rs` — `DayResult.rows_skipped_out_of_order`
    field reads `lob_export_stats.downsample.as_ref().map(|d|
    d.out_of_order_skipped).unwrap_or(0)` per day; aggregator loop sums
    into `total_rows_skipped_out_of_order: u64`; `_export_summary.json`
    `rows_skipped` block gains `downsample_out_of_order` key for
    operator-facing observability per HFT-rules §8 (no silent drops
    without recording diagnostics).
  - `tests/export_test.rs` — 9 new regression tests in the Phase O
    Cycle 1 / B.3 section (after the existing downsample tests):
    `b_3_downsample_min_interval_out_of_order_skipped_and_counted`
    (main fix), `b_3_downsample_min_interval_zero_acts_as_non_decreasing_filter`
    (documents new well-defined `MinIntervalNs(0)` semantics — see
    DEVIATION below), `b_3_downsample_min_interval_zero_writes_equal_ts_snapshots`
    (post-validator F-3 sub-boundary: pin equal-ts-write semantic
    distinct from `MinIntervalNs(1)`),
    `b_3_downsample_min_interval_equal_ts_skips_when_min_ns_positive`
    (boundary preservation invariant), `b_3_downsample_min_interval_none_timestamp_does_not_overwrite_anchor`
    (C-4 latent fix), `b_3_downsample_min_interval_extreme_underflow_does_not_panic`
    (`i64::MIN` `checked_sub` overflow defensive arm),
    `b_3_parquet_export_stats_downsample_is_none_when_no_strategy`
    (Option discriminant), `b_3_parquet_export_stats_downsample_is_some_when_strategy_configured`
    (Option discriminant inverse), `b_3_mbo_event_writer_downsample_field_is_always_none`
    (fake-metric prevention).

  **DEVIATION from pre-impl planning agent's recommendation**: the
  agent recommended rejecting `MinIntervalNs(0)` at
  `ExportConfig::validate()` for symmetry with `EveryN(0)` (F-034
  closure). I rejected this recommendation per HFT-rules §0 ("only
  what the task requires") because the two cases have distinct
  semantics: `EveryN(0)` is undefined (modulo by zero); `MinIntervalNs(0)`
  has well-defined post-B.3 semantics (`delta >= 0` arm: writes all
  NON-DECREASING ts including equal-ts; skips only strictly-
  decreasing-ts — a legitimate "non-decreasing-only filter" use case
  unlocked by B.3). The new tests
  `b_3_downsample_min_interval_zero_acts_as_non_decreasing_filter`
  + `b_3_downsample_min_interval_zero_writes_equal_ts_snapshots`
  document the use case so future operators can rely on the
  semantics. Pre-B.3, `MinIntervalNs(0)` silently degenerated to
  "write all" because `(negative) as u64 >= 0` was always true; that
  was a bug, not a feature.

  **Naming precision (per B.3 post-impl validator F-3)**: "non-
  decreasing" is the mathematically-correct label (allows equal-ts
  duplicates; only strictly-decreasing-ts is rejected). "Monotonic"
  alone is ambiguous — it can mean either non-decreasing OR strictly-
  increasing depending on context. CHANGELOG, test names, and test
  docstrings all use "non-decreasing" for `MinIntervalNs(0)` and
  document the equal-ts vs strictly-decreasing distinction.

  **Bit-exact preservation invariant**: GOLDEN_HASH_SEQUENCES_NPY
  (`0x8bebad9b09b564cd`) and GOLDEN_HASH_LABELS_NPY
  (`0x5dcf907068fcadcc`) UNCHANGED across B.3. The hashes are golden
  fixtures of the sibling `feature-extractor-MBO-LOB`'s NPY export
  pipeline (1D event-loop tests with monotonic synthetic timestamps);
  B.3 changes Parquet-export downsample behavior under MinIntervalNs
  on out-of-order timestamps, which the golden fixtures never exercise
  (synthetic-fixture argument: monotonic-only fixtures make the
  out-of-order branch a structural no-op).

  **`PRODUCER_DIAGNOSTICS_SCHEMA_VERSION` UNCHANGED at 2.9.0**: B.3
  introduces new producer-side diagnostic counters (`DownsampleStats`
  + `_export_summary.json` `downsample_out_of_order` key) but these
  are reconstructor-internal Parquet-export observability, NOT part
  of the sibling extractor's `_diagnostics.json` schema (the schema
  version that gates cross-repo wire format). Phase L set the
  precedent: wire-format-invariant changes to producer-internal types
  do not bump the diagnostics schema version (Phase L stayed at 2.6.0;
  B.3 stays at 2.9.0).

  **`#[non_exhaustive]` on `ParquetExportStats`**: prophylactic — at
  the time of writing, no out-of-crate `ParquetExportStats { ... }`
  struct-literal constructors exist (verified by grep across all 7
  sibling repos). Adding `#[non_exhaustive]` ensures future field
  additions remain non-breaking by construction; matches the Phase M
  REV 3 sibling-policy alignment.

  **Test count delta**: reconstructor 391 → **400** (+9, all
  passing — 8 initial B.3 tests + 1 post-validator F-3 sub-boundary
  test for `MinIntervalNs(0)` equal-ts semantic). Lib tests +
  integration tests unchanged at 307 + 92.

  **Adversarial validation cycle**:
  - Pre-implementation: parallel adversarial Plan agent (B.3 design
    challenge) → 15-angle audit → SHIP-WITH-MODIFICATIONS verdict
    with 10 mandatory items: WriteDecision enum (A-1, applied),
    `#[non_exhaustive]` on ParquetExportStats (C-2, applied), drop
    `debug_assert!(false)` (D-1, applied — using `log::warn!`
    instead), Option<DownsampleStats> sub-struct (D-5, applied),
    C-4 latent fix (applied), `_export_summary.json` wiring (X-1,
    applied), `MinIntervalNs(0)` rejection (D-7, REJECTED — see
    DEVIATION rationale above), 9 specific tests (8 applied, 1
    deferred per DEVIATION).
  - Post-implementation: standard quality-gate cascade.

  **Phase O follow-ups deferred to Phase P** (per pre-impl agent's
  scope-discipline triage):
  - `OnOutOfOrderTimestamp::{Skip, Write, Error}` policy enum
    (operator opt-in for legacy "write everything including out-of-
    order" behavior) — separate cycle; current Skip-default is
    appropriate per HFT-rules §3 monotonic-timestamp invariant.
  - `decompressed_path_for` basename-collision class (silent data-
    substitution bug between different compressed sources mapping to
    same basename) — Phase P task.
  - At-startup orphan `.tmp*` sweep — Phase P operational task.

## [0.2.0] — 2026-04-30

Phase M REV 3 — Boundary Discipline Cycle. Closes 12 findings sharing
the root cause "silent failure at boundary" (per the
`BACKBONE_AUDIT_VALIDATED_2026_04.md` validated cluster F-002/F-003/
F-007/F-008/F-010/F-013/F-021/F-023/F-024/F-031/F-034 + DESIGN-1).

### Added

- **`BoundaryError` peer enum** (`src/loader/error.rs`) — typed error
  domain for the loader yield path. Variants `Decode(String)` +
  `Convert(TlobError)`. `#[derive(Error, Debug, Clone)]` +
  `#[non_exhaustive]`. (M.A.1)

- **`CountingReader<R>`** (`src/loader/mod.rs`) — `Read + BufRead`
  wrapper that tracks bytes consumed via `Arc<AtomicU64>`. Closes
  F-008: pre-M.A.2, `LoaderStats::bytes_read` was always 0 and
  `progress()` returned 0.0 in production logs. Single-thread-per-
  iterator scope (Decision 17 — never crosses Rayon boundary). (M.A.2)

- **`TypedMessageIterator`** + `iter_messages_typed()` API
  (`src/loader/mod.rs`) — `Iterator<Item = Result<MboMessage,
  BoundaryError>>` with compile-time error handling. Closes F-002 +
  F-003 + F-024 (silent decode/convert error swallow + clean-EOF vs
  torn-EOF disambiguation). Adds `LoaderStats::mid_record_eof: u64`
  counter (Decision 5b) + `is_clean_eof()` helper. Adds `finalize(self)
  -> LoaderStats` (Decision 5c — caller-decides abort/warn policy).
  (M.A.3)

- **`legacy-iterator-api` cargo feature** (default-on) — gates the
  legacy `iter_messages()` API with `#[deprecated(...)]` for
  transition. Removable in next MAJOR (calendar 2026-10-29). (M.A.3)

- **`LobStats::modify_order_not_found` + `add_order_id_collision`
  counters** (`src/lob/reconstructor.rs`) — closes F-013: silent
  modify-of-missing fall-through to add(msg) and silent
  add-of-existing fall-through to modify(msg) now have observability
  counters incremented BEFORE the recovery semantic. Recovery
  preserved bit-for-bit; only the silence is closed. (M.A.4)

- **`LobStatsExportEnvelope`** (`src/lob/reconstructor.rs`) — on-disk
  envelope wrapping `LobStats` with a `schema_version` field.
  `pub const LOB_STATS_SCHEMA_VERSION: &str = "2.0.0"`. Exposed at
  crate root via `pub use lob::reconstructor::{
  LobStatsExportEnvelope, LOB_STATS_SCHEMA_VERSION }`. (M.A.5)

- **`TlobError::InvalidTimestamp(i64)` variant** (`src/error.rs`) —
  closes F-023: pre-M.A.6 `DbnBridge::convert` silently coerced
  `ts_event == 0` (Databento sentinel) and u64 → i64 overflow into
  `Some(<wrong-value>)`. Now fail-loud per hft-rules §2 + §8.
  Wrapped by typed iterator as `BoundaryError::Convert(TlobError::
  InvalidTimestamp(_))`. (M.A.6)

- **5 anomaly counters in parquet binary**
  (`src/bin/export_to_parquet.rs::DayResult`) — closes F-021: pre-
  M.A.6 `is_ok() && is_valid()` silent dual-drop. Per-day counters
  `rows_skipped_crossed`, `rows_skipped_invalid_price`,
  `rows_skipped_other`, `rows_skipped_invalid_state`,
  `rows_skipped_decode_or_convert` aggregated into
  `_export_summary.json::rows_skipped` block. Default error policy
  `WarnAndContinue` (Decision 6a). (M.A.6)

- **`LoaderStats::system_messages_seen: u64` field** (`src/loader/
  mod.rs`) — closes F-010: producer-side counter incremented at the
  loader on every yielded message matching `is_system_message()`.
  Pre-M.A.7, `LobStats::system_messages_skipped` was permanently
  shadowed at zero by consumer-side pre-filtering. Loader now yields
  the message regardless; downstream consumer policy preserved. (M.A.7)

- **`#[non_exhaustive]` on `LobStats`** — additive-only future
  evolution. External crates can no longer construct via struct
  literal; in-crate construction exempt. Documented at struct + in
  cycle commit messages. (M.A.4 Decision 18)

### Changed

- **Atomic write for `_reconstruction_stats.json`** — replaced plain
  `BufWriter + serde_json::to_writer_pretty` with `tempfile::
  NamedTempFile::new_in(parent_dir)` → `to_writer_pretty` →
  `sync_all` → `persist`. POSIX-atomic. Fallback to direct write +
  fsync + WARN log on tempfile-creation/persist failure (NFS
  EROFS/EXDEV edge cases). SSoT-deferred consolidation onto
  `hft_statistics::io::atomic_write_json` documented inline. (M.A.5)

- **Dual-format read for `LobStats::load_from_file`** — accepts BOTH
  envelope shape (post-M.A.5) AND legacy flat shape (pre-M.A.5).
  Explicit `serde_json::Value`-peek dispatch (post-validation
  hardening — replaces an earlier `#[serde(untagged)]` enum that had
  silent-acceptance failure modes for malformed envelopes). Legacy
  branch logs WARN per call with calendar 2026-10-29 removal note.
  (M.A.5 + post-validation hardening)

- **`process_day` migration to `iter_messages_typed()`** — closes
  F-021 by surfacing decode/convert errors per-message into the
  binary's structured-match path. The `.skip_invalid(true)` builder-
  call removed at the `process_day` callsite (the
  `LoaderConfig::skip_invalid(bool)` builder method + struct field
  remain available for other callers); errors now surface to the
  structured match path so the new anomaly counters actually populate
  (M.A.6 post-validation hardening). (M.A.6 + M.A.7)

- **`DbnBridge::convert` 3-case timestamp dispatch** — closes the
  M.A.6↔M.A.7 cross-cascade (Agent A1 H-1 from post-validation round):
  pre-M.A.9, M.A.6 F-023 rejected ALL `ts_event=0` as
  `InvalidTimestamp`, which silently shadowed the M.A.7 F-010
  `system_messages_seen` counter at the typed iterator (Databento
  heartbeat / metadata system messages with both `order_id=0` AND
  `ts_event=0` flowed through the `BoundaryError::Convert` arm and
  inflated `rows_skipped_decode_or_convert` instead of counting as
  expected heartbeats). Post-M.A.9 dispatch: (1) overflow
  (`ts_signed < 0`) always corrupt → `Err`; (2) `ts_event == 0` AND
  is-system-message → `Ok(timestamp = None)` so message reaches the
  iterator's F-010 counter; (3) `ts_event == 0` AND non-system →
  `Err(InvalidTimestamp(0))` (genuine corruption). (M.A.9)

- **`DownsampleStrategy::EveryN(0)` rejected at config-validation
  time** — closes F-034: pre-M.A.4 `EveryN(0)` was silently
  reinterpreted as "no downsample" via a latent fall-through at
  `LobSnapshotWriter::should_write`. Now rejected with
  `TlobError::InvalidConfig`. Use `DownsampleStrategy::None` for
  no-downsampling explicitly. (M.A.4)

### Removed

- **`LobStats::errors: u64` field** — closes F-007: declared but
  NEVER incremented (verified zero genuine increment sites pre-
  implementation gate). Per Decision 10b: REMOVE the dead field
  rather than wire it to an arbitrary path. Specific anomaly
  counters now expose the silent fall-through behavior:
  `modify_order_not_found`, `add_order_id_collision`,
  `cancel_order_not_found` (existing), `cancel_price_level_missing`
  (existing), etc. (M.A.4)

### Deferred

- **F-031** (`BatchProcessor` empty-input silent Ok) — pre-
  implementation gate verified `BatchProcessor` does NOT exist
  anywhere in the reconstructor crate. Plan-cited site
  `src/lib.rs::BatchProcessor` is a phantom reference. Closure
  deferred to a follow-up commit pending clarification of the actual
  target site. Documented in M.A.4 commit message.

- **`benches/typed_iterator_overhead.rs` criterion bench** — the plan
  §M.A.8 budgeted a criterion bench harness comparing typed vs legacy
  iterator overhead. Deferred to a focused performance-tracking cycle
  since (a) the existing `benches/reconstruction.rs` already covers
  the LobReconstructor hot path; (b) Decision 17 (single-thread-per-
  iterator `Arc<AtomicU64>`) is analytically <0.3% regression per
  Agent V5 (verified by reading dbn 0.20.0 source); (c) the
  boundary-discipline correctness ship is the load-bearing piece for
  unblocking Stage B. Tracked for post-Stage-B hardening cycle.

- **End-to-end DBN-fixture tests for typed iterator** (BoundedTruncatingReader
  + synthetic-DBN-byte feeds) — plan §M.A.8 budgeted these but they
  require crate-internal `DecodeRecord` mock infrastructure. F-002 +
  F-003 + F-024 wires currently have STRUCTURAL coverage only
  (LoaderStats field-existence locks; no live counter increment from
  iterator drive). Per Agent V3 + Agent V5: closing this gap requires
  synthesizing `dbn::MboMsg` via `dbn::encode::DbnEncoder` writing to
  `Vec<u8>`. Tracked for post-Stage-B test-hardening cycle.

- **`tests/parquet_export_counters.rs`** — plan §M.A.8 budgeted a
  2-test integration file for the parquet binary's 5-arm match. The
  binary itself has zero `#[test]`. Tracked alongside the DBN-fixture
  work above.

### Migration notes

- **Schema version 1.0 → 2.0** for `_reconstruction_stats.json`. The
  envelope wrapper `{schema_version, stats}` is a structural
  breaking change. `load_from_file` accepts BOTH shapes for
  back-compat; legacy-shape reads emit a one-time-per-call WARN
  log. Calendar removal: 2026-10-29 (aligned with
  `legacy-iterator-api` deprecation).
- **Crate version 0.1.0 → 0.2.0** (M.A.3). MINOR bump per
  Cargo.toml; `iter_messages_typed()` is the new preferred API
  (yields `Result<MboMessage, BoundaryError>`); legacy
  `iter_messages()` retained behind default-on
  `legacy-iterator-api` feature with `#[deprecated]` annotation.
- **`#[non_exhaustive]` on `LobStats`** — external crates that
  constructed `LobStats { errors: 3, ... }` via struct literal will
  break at compile time. Migration: use `LobStats::default()` +
  struct-update syntax `..Default::default()`.
- **`#[non_exhaustive]` on `TlobError` + `LoaderStats`** (M.A.10
  polish) — same discipline as `LobStats`. The M.A.6 `InvalidTimestamp`
  variant addition was technically a Rust SemVer break for any
  external exhaustive match; post-M.A.10 future variant additions are
  non-breaking. Verified zero live exhaustive matches in
  feature-extractor / mbo-statistical-profiler / opra-statistical-profiler
  by Agent V1 cumulative ground-truth audit. External crates
  pattern-matching on `TlobError` MUST include a wildcard arm.
- **`ErrorMode::FailFast` semantic change** for downstream
  consumers — pre-Phase-M, torn DBN silently completed. Post-Phase-M
  under default `FailFast`, a single torn-DBN day surfaces at the
  iterator's `finalize().mid_record_eof` counter. The reconstructor
  itself takes WARN-and-continue policy (analytical batch tool); the
  feature-extractor M.B cycle will land contract-change docs +
  operator-communication recommending `processing.error_mode =
  "collect_errors"` for resilient batch processing.

### Test count

- **Library tests: 271 → 285** (+14 net across Phase M REV 3
  cumulative). Per-commit deltas: M.A.4 +1, M.A.5 +2, M.A.5
  hardening +1, M.A.6 +3, M.A.7 +1, M.A.9 +2, M.A.11 +5 (M.A.10
  + M.A.12 unchanged at lib level — pure attribute / feature-gate
  changes). Plus 3 doctests + ~31 ignored doctests via
  `--include-ignored`. **240 lib** under `--no-default-features`.
  **282 lib** under the new combo `--features "databento"
  --no-default-features` (M.A.12 enabled this combo; was
  uncompilable pre-M.A.12 with 24+ E0599 errors).
- **Integration tests: 41 total** under `--features
  legacy-iterator-api` = `tests/integration_test.rs` (21) +
  `tests/loader_typed_iterator.rs` (7, NEW M.A.8) +
  `tests/lob_stats_counters.rs` (8, NEW M.A.8) +
  `tests/queue_position_nvidia_test.rs` (5). Plus 35 in
  `tests/export_test.rs` when `--features "databento export"` is
  enabled, bringing total to 76 with full-feature build.
- Caveat per hft-rules §11 ("Numeric facts that vary with code
  state... MUST NOT be hand-typed in cross-pipeline documentation"):
  these counts are HAND-TYPED here; the authoritative source is
  `cargo test 2>&1 | grep "test result"`. Cited counts will drift
  with future commits.

### Added

- **LobState Temporal Fields** (`src/types.rs`)
  - `previous_timestamp` - Previous LOB update timestamp for Δt calculation
  - `delta_ns` - Time delta since last update in nanoseconds
  - `triggering_action` - Action that caused this state change (Add/Modify/Cancel/Trade/Fill)
  - `triggering_side` - Side affected by the triggering action (Bid/Ask)
  - New temporal helper methods:
    - `delta_seconds()` - Get time delta in seconds
    - `event_intensity()` - Get events per second (1/Δt)
    - `was_triggered_by(action)` - Check if triggered by specific action
    - `was_triggered_on_bid()` / `was_triggered_on_ask()` - Check affected side
    - `is_trade_event()` / `is_add_event()` / `is_cancel_event()` - Event type checks
  - Enables FI-2010 time-sensitive features (u6-u9): dP/dt, dV/dt, inter-arrival times

- **LobReconstructor Temporal Population** (`src/lob/reconstructor.rs`)
  - `fill_lob_state_with_temporal()` - Enhanced fill with temporal context
  - `process_message_into()` now populates all temporal fields automatically
  - 100% delta tracking accuracy verified with real NVIDIA data

- **Queue Position Tracker** (`src/lob/queue_position.rs`)
  - `QueuePositionTracker` - FIFO queue position tracking using `IndexMap`
  - `QueueLevel` - Per-level order tracking with insertion order preserved
  - `QueuePositionConfig` - Configuration for tracking behavior
  - Methods: `queue_position()`, `volume_ahead()`, `best_level_imbalance()`, `multi_level_imbalance()`
  - Verified with 5 integration tests on real NVIDIA data

- **Order Lifecycle Tracker** (`src/lob/order_lifecycle.rs`)
  - `OrderLifecycleTracker` - Track orders from creation to terminal state
  - `OrderLifecycle` - Complete order lifecycle with modifications history
  - Handles pre-existing orders gracefully (inferred from first observation)
  - Memory management with configurable max tracked orders

- **Day Boundary Detection** (`src/lob/day_boundary.rs`)
  - `DayBoundaryDetector` - Automatic trading day boundary detection
  - `DayBoundaryConfig` - Configurable trading hours and gap thresholds
  - `DayBoundary` - Day transition event with statistics

- **Trade Aggregator** (`src/lob/trade_aggregator.rs`)
  - `TradeAggregator` - Aggregate individual fills into trades
  - `Trade` - Complete trade with aggressor side detection
  - `Fill` - Individual fill event representation

- **Hot Store Infrastructure** (`src/hotstore.rs`)
  - `HotStoreConfig` - Configuration for hot store directory and preferences
  - `HotStoreManager` - Manages decompressed data cache
    - `resolve()` - Auto-prefer decompressed files when available
    - `decompress()` - Decompress single file to hot store
    - `list_hot_files()` - Enumerate cached files
    - `hot_store_size()` - Calculate total cache size
    - `clear()` - Remove all cached files
  - Enables ~30% faster processing by skipping zstd decompression

- **MarketDataSource Abstraction** (`src/source.rs`)
  - `MarketDataSource` trait - Provider-agnostic data source interface
  - `SourceMetadata` - Metadata about the data source (symbol, date, etc.)
  - `VecSource` - In-memory source for testing
  - `DbnSource` - DBN file source with hot store integration
    - `with_hot_store()` - Enable hot store path resolution

- **Auto-Detect DBN Compression**
  - `DbnLoader` now uses `DynDecoder` to auto-detect file format
  - Supports both compressed (`.dbn.zst`) and uncompressed (`.dbn`) files
  - No configuration needed - just provide the file path

- **CLI: decompress_to_hot_store** (`src/bin/decompress_to_hot_store.rs`)
  - Standalone tool to populate hot store directory
  - Parallel decompression using Rayon
  - Supports single file, directory, or glob patterns
  - `--dry-run` mode to preview operations
  - `--force` to re-decompress existing files

- **PriceLevel with Cached Size** (`src/lob/price_level.rs`)
  - `PriceLevel` struct with O(1) `total_size()` queries (was O(n))
  - Encapsulated mutation methods: `add_order()`, `remove_order()`, `reduce_order()`
  - Debug assertions verify cache consistency
  - 16 comprehensive unit tests

- **Zero-Allocation API**
  - `LobReconstructor::process_message_into()` - Fill pre-allocated `LobState`
  - Eliminates heap allocation per message in hot loop
  - `fill_lob_state()` helper for reusable state

- **Stack-Allocated LobState**
  - `LobState` fields changed from `Vec` to `[T; MAX_LOB_LEVELS]`
  - `MAX_LOB_LEVELS = 20` constant for fixed-size arrays
  - ~560 bytes per snapshot (fits in cache)

### Changed

- `DbnLoader` now accepts both compressed and uncompressed DBN files
- `DbnLoader` I/O buffer increased from 8KB to 1MB (`IO_BUFFER_SIZE`)
- `LobReconstructor` now uses `BTreeMap<i64, PriceLevel>` instead of `BTreeMap<i64, AHashMap<u64, u32>>`
- Size aggregation in `fill_lob_state()` now O(1) per level (was O(n))

### Performance

- **10.2 million messages/sec** throughput (release mode)
- **0.10 µs** per-message latency
- **~30% faster** with pre-decompressed files via hot store
- Validated against 37M+ real NVIDIA messages with 0 mismatches

## [0.1.1] - 2025-12-04

### Added

- **System Message Filtering**
  - `LobConfig::skip_system_messages` - Skip system messages (order_id=0, size=0, price<=0) by default
  - `LobStats::system_messages_skipped` - Track count of skipped system messages
  - `LobConfig::with_skip_system_messages(bool)` - Configure system message handling
  
- **Full Reset Method**
  - `LobReconstructor::full_reset()` - Completely reset reconstructor including statistics
  - Distinction: `reset()` preserves stats (for Action::Clear), `full_reset()` clears everything

### Fixed

- Collapsed nested if statement for system message check (clippy)

### Changed

- `reset()` now explicitly documents that it preserves statistics (for monitoring across Action::Clear)
- System messages are now filtered at the `LobReconstructor` level, not at the loader level

## [0.1.0] - 2025-12-01

### Added

#### Core LOB Reconstruction
- `LobReconstructor` - High-performance single-symbol LOB reconstruction
- `MultiSymbolLob` - Multi-symbol LOB management
- `LobConfig` - Configurable LOB behavior
- `CrossedQuotePolicy` - Four policies for handling crossed quotes (Allow, UseLastValid, Error, SkipUpdate)

#### Core Types
- `MboMessage` - Market-By-Order message representation
- `LobState` - LOB snapshot with enriched analytics
- `Action` - Order actions (Add, Modify, Cancel, Trade, Fill, Clear, None)
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
- ~1 microsecond latency per message
- 100% data quality on real NVIDIA MBO data (17.8M messages)

### Testing
- 89+ unit tests
- 19+ integration tests with real market data
- 8+ edge case tests for robustness

[Unreleased]: https://github.com/nagarx/MBO-LOB-reconstructor/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/nagarx/MBO-LOB-reconstructor/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/nagarx/MBO-LOB-reconstructor/releases/tag/v0.1.0
