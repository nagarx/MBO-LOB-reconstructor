# MBO-LOB-reconstructor: audit evidence and coverage

Date: 2026-08-03
Audit mode: report-only; no production, configuration, contract, dataset, or wiki edits
Primary audited commit: `c1d1f147d9b1cae544d676083efe08feb1c96d93` (`main`, `v0.3.0`)
Auditor conclusion: **not admissible as a fresh scientific-data producer until the critical event-semantics and publication defects are corrected and independently revalidated**

## 1. Scope and claim discipline

This report records the repository-local evidence behind the cross-pipeline synthesis. It does not certify every branch or every unexecuted failure path. It distinguishes:

- **RV — runtime-verified:** independently exercised in this audit, not merely covered by an existing test.
- **SR — statically reviewed:** implementation and contracts inspected, but the path was not independently executed for this audit.
- **AV — artifact-verified:** an emitted artifact was independently decoded/reconciled.
- **TC — test-covered but not independently verified:** an existing test/build exercised the surface, but no independent oracle was applied.
- **DO — documentation-only:** only prose evidence exists.
- **BL — blocked:** the intended validation could not be executed; the reason is stated.
- **OOS — intentionally out of scope:** a precise scope reason is stated.

“Not found” below means a bounded search returned no match. It is not proof that the behavior is absent from every external orchestrator. A passing suite is not treated as proof that the behavior is correct; several findings below are preserved by passing tests.

## 2. Audited states

| Audit record | Commit/tree | Evidence | Result |
|---|---|---|---|
| Current main / `v0.3.0` | `c1d1f147d9b1cae544d676083efe08feb1c96d93` | RV + SR + AV | Primary audited runtime state. Documentation files were already dirty and were treated as a separate overlay. |
| Completed-event branch | `f347ed56c2d8cb335da04a99ad127066fc5eb070` | SR | Contains proposed semantics work, but the accepted F064 program is blocked before market compute. It is not current-main behavior and supplies no empirical recertification. |
| `v0.2.0` | `cb4a7d250736e105fd7b0ba4cf089f0f26c7b2df` | SR/diff | T/F merge remains present. |
| `v0.2.1` | `28a9a2272aacc007e15353fe03adb9d24aca74e8` | SR/diff | T/F merge remains present. |
| Dirty-main documentation overlay | current worktree over `c1d1f14` | DO + SR | Existing user/agent documentation changes were read but not attributed to the audited commit and not modified. |

Tag refs were peeled with `git rev-parse <tag>^{commit}`. The annotated-tag object hashes shown by `show-ref` are not commit identities.

## 3. Audit inventory and denominator

### 3.1 Production modules

All 27 files under `src/**/*.rs` were inventoried. Primary evidence status: **10 RV, 16 SR, 1 TC, 0 DO, 0 BL, 0 OOS = 27/27 inventoried**. This is inventory coverage, not 27/27 branch or invariant coverage.

The `pub` count is a lexical count of lines matching `^\s*pub(\([^)]*\))?\s+`; it includes re-exports, fields, methods, and `pub(crate)` items and therefore is not a count of unique semantic APIs.

| Module | Primary status | Lexical public lines | Audit focus |
|---|---:|---:|---|
| `src/analytics.rs` | SR | 51 | Derived book analytics, finite/empty guards, units |
| `src/bin/decompress_to_hot_store.rs` | TC | 0 | CLI target compiled/help inspected; decompression CLI not independently run |
| `src/bin/export_to_parquet.rs` | RV | 0 | Real DBN export, malformed input, empty input, error/finalize/summary paths |
| `src/constants.rs` | SR | 10 | Fixed-point constants and limits |
| `src/dbn_bridge.rs` | RV | 5 | DBN action/side/time conversion and field loss |
| `src/error.rs` | SR | 3 | Error taxonomy and generic conversions |
| `src/export/batch.rs` | RV | 38 | LOB/MBO column materialization, `None -> 0`, release assertions |
| `src/export/lob_writer.rs` | RV | 17 | Direct final-file creation, flush/close semantics |
| `src/export/mbo_writer.rs` | RV | 14 | Direct final-file creation, flush/close semantics |
| `src/export/mod.rs` | RV | 26 | Export configuration, downsampling and schema version |
| `src/export/schema.rs` | RV | 6 | Physical schema, units, metadata, six-field MBO projection |
| `src/hotstore.rs` | SR | 22 | Basename identity, existence-only reuse, atomic decompression limits |
| `src/lib.rs` | SR | 30 | Public re-export surface and default feature exposure |
| `src/loader/error.rs` | SR | 1 | Typed boundary errors |
| `src/loader/mod.rs` | RV | 33 | Typed/legacy iterators, error recovery, termination/finalize statistics |
| `src/lob/day_boundary.rs` | SR | 45 | Reset/day-boundary helpers |
| `src/lob/mod.rs` | SR | 13 | LOB public composition |
| `src/lob/multi_symbol.rs` | SR | 17 | Symbol-scoped reconstructor ownership |
| `src/lob/order_lifecycle.rs` | SR | 106 | Order state/lifetime transitions and Fill mutation |
| `src/lob/price_level.rs` | SR | 17 | Queue maps, total-size accounting, saturating arithmetic |
| `src/lob/queue_position.rs` | SR | 48 | Queue position/lifetime metrics |
| `src/lob/reconstructor.rs` | RV | 59 | Book transitions, resets, repair policies, statistics and state persistence |
| `src/lob/trade_aggregator.rs` | SR | 38 | Trade grouping and reset behavior |
| `src/source.rs` | SR | 24 | Infallible `Iterator<Item=MboMessage>` source contract |
| `src/statistics.rs` | SR | 53 | Online distribution and aggregation primitives |
| `src/types.rs` | RV | 71 | Action/side/message/state representation and validation |
| `src/warnings.rs` | SR | 51 | Warning aggregation and thresholds |

Lexical public-line denominator: **798** across the 27 modules. Every line was assigned to a module in the inventory; this audit did not prove every public method’s behavioral contract independently.

The exact path/line inventory is [`AUDIT_PUBLIC_SURFACE_INVENTORY_2026_08_03.md`](AUDIT_PUBLIC_SURFACE_INVENTORY_2026_08_03.md); each declaration is conservatively SR, with behavioral status inherited from this module ledger.

### 3.2 Executable, test, example, and benchmark surfaces

| Surface | Count | Status | Evidence |
|---|---:|---|---|
| Library target `mbo_lob_reconstructor` | 1 | RV | All-feature and no-default-feature suites; real DBN consumer path |
| CLI `export_to_parquet` | 1 | RV | Help, empty directory, malformed file, and real CRSP day |
| CLI `decompress_to_hot_store` | 1 | TC | Compiled/help surfaced; no independent source-to-hotstore run |
| Integration-test targets | 5 | TC (executed) | All five executed; behavioral claims remain test-covered unless independently oracled |
| Examples | 4 | TC | Compiled by Cargo; not independently executed |
| Criterion benchmark | 1 | TC | Source/build surface inspected; no performance run |
| Ordinary Rust tests | 397 | TC (executed) | 397 passed, 0 failed, 0 ignored |
| Doctests | 49 | TC (executed) | 8 passed, 41 ignored |

Exact file surfaces: integration tests (TC executed) are `tests/export_test.rs`, `tests/integration_test.rs`, `tests/loader_typed_iterator.rs`, `tests/lob_stats_counters.rs`, and `tests/queue_position_nvidia_test.rs`; examples (TC compiled) are `examples/advanced_analytics.rs`, `examples/basic_usage.rs`, `examples/multi_symbol.rs`, and `examples/process_nvda_single_day.rs`; the benchmark (TC compiled/not run) is `benches/reconstruction.rs`.

No daemon, service, scheduler, async background worker, or build script was found in Cargo target metadata or `src/` searches. Status: **not found in this repository**, not proven absent from external orchestration.

### 3.3 Input, configuration, persistence, and generated-artifact surfaces

| Boundary/artifact | Status | Invariants checked or missing |
|---|---|---|
| Compressed and uncompressed DBN input | RV | Decode count/action census; typed iteration; schema-specific field conversion |
| `MboMessage` in-memory contract | RV | Field/action/side/time mapping; system predicate; provenance loss |
| `LobState` in-memory/state JSON envelope | SR + TC | Shape/reset/stats tests; not independently resumed across process failure |
| MBO Parquet | RV + AV | Row/action counts, Arrow readability, metadata and projection loss |
| LOB snapshot Parquet | RV + AV | Row counts, levels/timestamps/state metadata and invalid-state counters |
| `_export_summary.json` | RV + AV | Presence and counts; write failure is warning-only |
| Hot-store DBN | TC | Unit tests; source identity, digest and zero-byte reuse not independently fault-injected |
| CLI/default export configuration | RV | Help/default path, empty and malformed inputs, real export |
| Optional TOML exporter configuration | SR | Parser and validation source inspected; no exhaustive config permutation run |

There is no repository-owned, versioned completion receipt binding all files, row counts, source digest, code/dependency identity, and clean terminal state into one consumable generation.

## 4. Runtime evidence ledger

### R-MBO-01 — current suite and static checks

- **Commands:**
  - `cargo test --all-features -- --test-threads=1`
  - `cargo test --no-default-features --lib -- --test-threads=1`
  - `cargo clippy --lib --bins --all-features -- -D warnings`
  - `cargo fmt --all -- --check`
- **Observed:** 397 ordinary tests passed; 8 doctests passed and 41 were ignored. The no-default-feature library suite passed 242 tests. Library/binary Clippy and formatting passed.
- **Qualification:** `cargo clippy --all-targets --all-features -- -D warnings` failed on test-target style/deprecated legacy-iterator uses. This is not a production compile failure, but it means the all-target lint gate is not clean.
- **Confidence:** high.
- **Alternative explanation:** none for the command outcomes; passing tests still preserve the T/F defect.

### R-MBO-02 — raw DBN versus exported MBO action census

- **Fixture:** `/Volumes/WD_Black/HFT-data/XNAS_ITCH/CRSP/mbo_2025-07-01_to_2026-01-09/xnas-itch-20251224.mbo.CRSP.dbn.zst`; release-receipt SHA-256 `a2fff0228a10ec989899714ab3939869ada8792c7e9c66f777e9c56f0c719282` in `/Volumes/WD_Black/HFT-data/audits/databento/releases/dbc-20260802-v1/VALIDATION_RECEIPT.md`.
- **Reproduction:** `dbn -C <fixture>` plus `export_to_parquet` and an independent PyArrow group/count over the emitted MBO Parquet.
- **Raw observed:** 76,364 rows: A 35,287; C 35,769; T 3,224; F 2,083; R 1. All 3,224 T rows have `order_id=0`; all 2,083 F rows have nonzero order IDs. T side counts were A 1,189 / B 894 / N 1,141; F side counts were A 894 / B 1,189.
- **Export observed:** A 35,287; C 35,769; T 5,307; R 1; F absent. The 2,083 F rows became T. Total row count remained 76,364.
- **Expected:** retain raw event class and side semantics, or emit a separately versioned derived execution event; never merge two opposite side conventions into the same action code.
- **Confidence:** very high; exact count conservation identifies the merged population.
- **Unresolved alternative:** none that explains exact `T_export = T_raw + F_raw` while F disappears; this is also the literal implementation at `src/dbn_bridge.rs:155-165`.

### R-MBO-03 — real LOB export

- **Reproduction:** run `export_to_parquet` on the R-MBO-02 fixture, then inspect Parquet metadata and row counts with PyArrow.
- **Observed:** 76,364 MBO rows and 76,353 LOB snapshots. Summary: 73,140 messages processed, 3,224 system messages skipped, 1 clear, 1,770 cancel-order-not-found, 141 trade-order-not-found, and 11 invalid states following successful processing.
- **Expected:** once T/F semantics are corrected, MBP-10 book-affecting agreement and all anomaly budgets must be remeasured; current counts cannot certify book correctness.
- **Confidence:** high for counts; no claim is made that this one day is representative of all instruments/regimes.

### R-MBO-04 — malformed input publishes invalid final artifacts

- **Fixture:** repository `README.md` passed as an input file to `export_to_parquet` with a fresh temporary output directory.
- **Observed:** process exited 1 after decoder initialization failed, but final-path files already existed: `README.md_lob_snapshots.parquet` and `README.md_mbo_events.parquet`, each 4 bytes with SHA-256 `fbc62d...`, plus `_export_summary.json`. PyArrow rejected both files as shorter than the minimum Parquet footer.
- **Code evidence:** `src/export/lob_writer.rs:108`, `src/export/mbo_writer.rs:67` create final destinations before successful decode; summary writing at `src/bin/export_to_parquet.rs:608-641` ignores serialization failure and only warns on create failure.
- **Expected:** no reusable final artifact until decode, write, close, reconciliation, and completion receipt all succeed. Failed output may exist only in an explicitly non-consumable quarantine/staging area.
- **Confidence:** very high.
- **Unresolved alternative:** a consumer could happen to reject the four-byte files, but filename/presence-based consumers can mistake them for completed outputs. The producer supplies no validity marker.

### R-MBO-05 — empty input is a successful no-op

- **Fixture:** a fresh empty directory.
- **Observed:** `export_to_parquet` exited 0 and reported “No files found”.
- **Expected:** for an explicitly requested export, zero discovered partitions is a fatal population/coverage error unless the caller explicitly selected an allow-empty mode.
- **Confidence:** high.

### R-MBO-06 — truncation probe

- **Status:** BL.
- **Reason:** the workspace’s protected-artifact hook blocked creation of an intentionally truncated DBN fixture. The hook was respected; no bypass was attempted.
- **Static evidence:** `src/loader/mod.rs:236-253` documents that dbn’s EOF silencing can make `mid_record_eof` unreachable; `src/bin/export_to_parquet.rs:513-527` makes suspicious termination warning-only; the typed iterator requires an optional caller `finalize()` check.
- **Conclusion:** tail-truncation behavior is **not exercised** here. It is neither proven safe nor proven corrupt by this audit’s runtime evidence.

## 5. Material findings

### MBO-01 — CRITICAL — DBN Trade and Fill are collapsed and then treated as book mutations

- **Locations:** `src/dbn_bridge.rs:119-165`; `src/lob/reconstructor.rs:794-921`; dispatch at `src/lob/reconstructor.rs:1212-1218`; `src/lob/order_lifecycle.rs:566`.
- **Evidence type:** implementation + vendor decoder semantics + independent raw/export census R-MBO-02 + current wiki correction cluster.
- **Observed:** `b'T' | b'F' => Action::Trade`; `Trade | Fill` calls `process_trade`, reducing/removing the resting order. True T is a non-book event with aggressor-side semantics; F is a non-book event with resting-side semantics. The downstream extractor subsequently filters true T (`order_id=0`) and retains F, so the actual feature stream is not “all flow annihilated”; it is a narrower, still-invalid F-as-Trade stream.
- **Expected:** raw T and F remain distinct; both are book no-ops. A separately owned completed-execution semantic layer may group/sign them, with explicit confidence and causal timestamp rules.
- **Affected states:** current main/v0.3.0, v0.2.0, v0.2.1, and the inspected F064 branch all retain the decode mapping. Branch documentation or proposed code is not a landed fix.
- **Affected consumers:** in-process feature extractor, MBO Parquet consumers, profiler/analyzer paths, `xsec_equity_discovery/extractor`, and any user of `Action::Trade` or order-lifecycle fills.
- **Failure class:** fatal before any fresh scientific artifact is promoted.
- **Scientific disposition:** exact execution-coverage, completed-event, trade-volume, exact-book and event-mechanism claims are **implementation-contaminated** until bounded re-extraction/revalidation. This finding does not by itself invalidate every historical direction null: on the measured NVDA/XNAS feature path, the surviving F-only resting-side direction is sign-preserving but incomplete and accidental under FINDING-122. Other instruments/consumers remain unresolved until their exact filters and populations are measured.
- **Confidence:** very high.
- **Alternative explanations:** no alternative reconciles the code and exact raw/export action counts. The magnitude/blast radius across historical exports remains artifact-specific and must be measured, not guessed.

### MBO-02 — CRITICAL — exporter publishes final artifacts before transaction completion

- **Locations:** `src/export/lob_writer.rs:108`; `src/export/mbo_writer.rs:67`; `src/bin/export_to_parquet.rs:435-458, 513-527, 608-641`.
- **Evidence type:** independent malformed-input runtime R-MBO-04 + static lifecycle review.
- **Observed:** final filenames are created before decoder success; fatal initialization leaves invalid files and a summary. Decode/convert and torn-stream paths can be warn/continue; summary write errors are non-fatal.
- **Expected:** generation-scoped staging, content/row reconciliation, close/fsync, and a completion receipt written last; failure leaves only quarantined diagnostics.
- **Failure class:** fatal. Warning-only is scientifically inadmissible because truncation or skipped records can change datasets and conclusions.
- **Confidence:** very high for initialization failure; high for the statically reachable mid-run paths. Tail truncation remains BL rather than runtime-proven.

### MBO-03 — HIGH — stream APIs do not enforce a terminal integrity decision

- **Locations:** legacy iterator `src/loader/mod.rs:677-755`; typed iterator `:780-947`; infallible source trait `src/source.rs:195-242`; default feature/deprecation surface in `Cargo.toml` and `src/lib.rs`.
- **Evidence type:** SR + test execution + consumer call-site scan.
- **Observed:** legacy `Iterator<Item=MboMessage>` turns decode/convert errors into warning/EOF when skipping is off, indistinguishable to callers. Typed iteration returns item errors but requires a separate, optional `finalize`; dbn v0.64 can silence an EOF error. The default feature still exposes the legacy API even though the package is already v0.3.0 and the deprecation says removal in “0.3.0 / calendar 2026-10-29”.
- **Expected:** one fallible stream contract with mandatory terminal receipt; consuming code cannot obtain a “complete” state/artifact without checking terminal integrity.
- **Consumer evidence:** profiler retains a legacy call; inspected xsec consumers use typed iteration but do not all call `finalize` (specialist cross-check). Each call site needs explicit disposition.
- **Failure class:** fatal for producers; warning-only may be allowed only for interactive analysis whose output is marked non-consumable.
- **Confidence:** high. The exact real-world reachability of dbn’s truncated-tail behavior was not runtime-tested in this audit.

### MBO-04 — HIGH — the so-called raw MBO Parquet is lossy and unit metadata is overgeneralized

- **Locations:** conversion `src/dbn_bridge.rs:47-111`; schema `src/export/schema.rs:125-166`; materialization `src/export/batch.rs:269-330`.
- **Evidence type:** SR + AV.
- **Observed:** output preserves only timestamp, order ID, merged action, side, price, and size. It drops `ts_recv`, publisher/instrument identity, flags, channel, and sequence; it cannot reconstruct the original DBN envelope. Metadata labels all prices “nanodollars” and all sizes “shares”, although the current raw-catalog contract says price fixed-point is scaled by 1e9 after sentinel handling while currency/quantity units remain instrument- and schema-contextual.
- **Expected:** either name/version the projection as lossy derived data, or emit the complete canonical envelope with explicit unit descriptors and source identity. “Raw” must be byte/semantic reversible or explicitly qualified.
- **Failure class:** fatal when a consumer requires causal availability, multi-symbol identity, or source replay; otherwise a versioned reduced projection may be admissible.
- **Confidence:** high.

### MBO-05 — HIGH — repair and crossed-book policies mutate state before deciding whether to reject or reuse

- **Locations:** duplicate Add and missing Modify recovery `src/lob/reconstructor.rs:713-782`; missing reduction recovery `:803-921`; consistency policy applied only after dispatch `:1212-1261`; policy returns last state/errors at `:1264-1323`.
- **Evidence type:** SR + existing tests.
- **Observed:** duplicate Add silently becomes Modify; missing Modify silently becomes Add; missing Cancel/Trade returns `Ok` after counters; price-level anomalies delete tracking; crossed/locked policy is evaluated after the event mutates internal book state. `Error`, `UseLastValid`, and `SkipUpdate` therefore describe emitted state, not transactional rejection of the mutation.
- **Expected:** explicit strict/quarantine/repair modes; precondition validation; apply-to-candidate then commit; anomaly budget/receipt; a rejected update cannot remain in authoritative state.
- **Failure class:** fatal in strict scientific production when anomaly thresholds or state agreement fail; repair mode may be quarantined and separately analyzed.
- **Confidence:** high for behavior; whether every heuristic is wrong depends on declared warm-start policy. The defect is implicit ownership/semantics, not the mere existence of a repair mode.

### MBO-06 — HIGH — hot-store identity is basename/existence-only

- **Locations:** `src/hotstore.rs:248-335, 363-405`; extractor duplicate resolution at `../feature-extractor-MBO-LOB/crates/hft-extractor/src/config.rs:261-284`.
- **Evidence type:** SR + repository path census.
- **Observed:** distinct source paths with the same basename resolve to the same flat hot-store destination; an existing file is accepted without size/hash/source-manifest validation. A stale, wrong-source, or zero-byte file can be preferred. No realized basename collision was found in the current enumerated MBO source set; the risk is latent, not claimed as a current corrupted file.
- **Expected:** content-addressed or source-manifest-qualified cache keys; source size/hash/catalog identity; atomic completion marker; verification on every reuse.
- **Failure class:** fatal on identity mismatch or unverifiable cache entry.
- **Confidence:** high for design risk, moderate for current blast radius because no collision was found.

### MBO-07 — HIGH — timestamp/shape coercions can create plausible rows

- **Locations:** `src/export/batch.rs:122-155`, especially `state.timestamp.unwrap_or(0)`; fixed-list length checks `:350-385` use debug assertions; schema timestamp is non-null `src/export/schema.rs:77-105`.
- **Evidence type:** SR.
- **Observed:** an absent LOB-state timestamp is serialized as epoch zero rather than rejected. Several invariants are development-only assertions. The normal producer path currently supplies timestamps for accepted records, so this is a latent external-API/configuration fault, not observed on the real CRSP run.
- **Expected:** non-null timestamp contract enforced in release; shape/level mismatch returns a typed error before staging.
- **Failure class:** fatal for persisted output.
- **Confidence:** high for reachability through public types, moderate for normal-path likelihood.

### MBO-08 — HIGH — zero-work and warning-only outcomes are indistinguishable from intended success

- **Locations:** CLI discovery and exit handling in `src/bin/export_to_parquet.rs:350-390, 740-855`; summary writer `:608-641`.
- **Evidence type:** R-MBO-05 + SR.
- **Observed:** empty input exits 0. Day-level errors produce nonzero overall exit, but files for failed days may already exist; summary serialization errors are ignored.
- **Expected:** explicit requested/discovered/attempted/succeeded/failed/skipped counts and a completion status. Zero discovered is fatal by default.
- **Failure class:** fatal unless the caller explicitly requests allow-empty.
- **Confidence:** high.

### MBO-09 — MEDIUM — versioned contract/documentation surfaces are internally inconsistent

- **Evidence:** current `../contracts/pipeline_contract.toml`, target docs, generated module contract, wiki interfaces, live code, and Git tags.
- **Observed:** root contract prose still describes a persisted `LobState` producer-consumer boundary and stale numeric certification; the extractor actually opens DBN and constructs the reconstructor in-process. The generated contract fixture is materially shorter than the canonical contract. Documentation already contains uncommitted corrections without their cited validation artifact. The legacy iterator says it will be removed in 0.3.0 while 0.3.0 ships it default-on.
- **Expected:** behavior-qualified interfaces governed by `../contracts/pipeline_contract.toml`; Python bindings generated in `../hft-contracts`; producer-local Rust types/constants checked against the same authority unless a Rust generator is separately approved; code and artifacts identify the exact contract version; historical measurements remain attached to immutable receipts.
- **Failure class:** fatal only when a consumer uses the stale contract to accept incompatible data; otherwise a release blocker.
- **Confidence:** high.

### 5.10 Material-claim evidence closure

Locations, observations, expectations, failure semantics and confidence are stated in each finding above. This table supplies the remaining evidence/reproduction/alternative fields explicitly.

| Finding | Evidence type | Reproduction command or fixture | Unresolved alternative explanations / limit |
|---|---|---|---|
| MBO-01 | RV + AV + SR | R-MBO-02, exact release-hashed CRSP fixture | No alternative for T/F merge; historical blast radius is consumer/result-specific |
| MBO-02 | RV + AV + SR | R-MBO-04 malformed `README.md` recipe | Mid-run/power-loss variants are statically reachable but not fault-injected |
| MBO-03 | SR + TC | R-MBO-06 plus profiler/xsec call-site scan | Tail truncation runtime path is BL; intentional early stop needs its own terminal state |
| MBO-04 | AV + SR | R-MBO-02/R-MBO-03 Parquet artifacts | Reduced projection can be valid if explicitly named/versioned; it is not canonical raw data |
| MBO-05 | SR + TC | Existing policy tests plus cited transition paths | A declared diagnostic repair policy may be useful; implicit production repair is the defect |
| MBO-06 | SR | Cited HotStore paths and bounded basename search | No realized collision artifact was found; exact search output was not retained |
| MBO-07 | SR | Public `LobState`/batch path; no mutation fixture | Normal producer currently supplies timestamps, so normal-path incidence is unresolved |
| MBO-08 | RV + SR | R-MBO-05 fresh empty directory | An explicit allow-empty invocation would be valid; no such mode was selected |
| MBO-09 | SR + DO | Canonical root contract, live code, tags and dirty-doc overlay | Documentation can be corrected without changing behavior; stale-contract consumer blast radius is not fully measured |

## 6. Positive findings, with limits

- The current all-feature and no-default-feature suites pass, and library/binary Clippy plus formatting are clean. This verifies build/test health, not semantic correctness.
- Typed per-item decode/convert errors exist and are used by the feature extractor; this is a sound direction, but the terminal check remains optional.
- Current-main processing of `Action::Clear` reaches the reset handler; the extractor also exempts Clear from its outer system filter. This does not repair T/F semantics.
- Real XNAS CRSP row counts reconcile between raw DBN and the MBO projection; the action census exposes, rather than hides, the exact T/F merge.
- No current hot-store basename collision was found in the enumerated MBO dataset paths. This is “not found,” not proof that the cache design is safe.

## 7. Required invariants and failure semantics

| Boundary | Required invariant | Current disposition | Required disposition |
|---|---|---|---|
| DBN record identity | raw action, two clocks, instrument/publisher, flags/channel/sequence preserved | fields dropped; T/F merged | Fatal before canonical event emission |
| Fixed-point fields | sentinel handled; unit instrument/schema-qualified | price scale assumed; size labeled shares | Fatal on unknown/incompatible unit |
| Stream ordering/termination | chosen clock nondecreasing under declared policy; terminal receipt clean | no mandatory receipt | Fatal; quarantine partial diagnostics |
| Book transition | only A/M/C/R affect book under qualified rules | T/F mutate; repair implicit | Fatal in strict mode; explicit repair artifact otherwise |
| Reset/lifecycle | Clear/day/symbol reset explicit and observable | Clear handled; day/multi-symbol paths separate | Fatal on missing reset; receipt counts resets |
| Parquet publication | complete generation atomic and reconciled | direct final writes | Staging + receipt last; quarantine failures |
| Cache reuse | source identity/hash/size match | basename + existence | Fatal on unverifiable or stale cache |
| Counts | requested=success+failed+skipped with emitted row reconciliation | partial summary; empty succeeds | Fatal on mismatch or unrequested zero-work |

## 8. Response decision matrix

| Finding | A leave | B harden validation/observability | C local refactor | D move to existing owner | E new shared primitive | F boundary redesign | Recommended response |
|---|---|---|---|---|---|---|---|
| MBO-01 T/F semantics | Reject | Necessary, insufficient | Necessary | Completed-event logic should not live in book mutator | Canonical event envelope | Yes | E + F, with local C implementation |
| MBO-02 publication | Reject | Necessary | Yes | Reuse `hft-statistics` atomic I/O where appropriate | Shared generation receipt | Yes for cross-producer parity | C + E, then F rollout |
| MBO-03 stream terminal state | Reject | Necessary | Yes | No suitable complete owner today | Fallible stream + terminal receipt | Yes | E + F |
| MBO-04 projection/provenance | Reject for canonical use | Necessary | Possible | Schema authority belongs in `../contracts/pipeline_contract.toml`; Python binding in `../hft-contracts`, Rust type local | CanonicalMboEventV2 | Yes | D + E + F |
| MBO-05 repair modes | Reject implicit mode | Necessary | Yes | Reconstructor remains owner | Shared anomaly receipt shape | Internal boundary redesign | B + C + E |
| MBO-06 hot store | Reject | Necessary | Yes | Could be shared artifact-cache owner later | Content/source identity primitive | Maybe | B + C now; evaluate E |
| MBO-07 coercion | Reject | Yes | Yes | Contract fields from shared schema | Validity bitmap/error types | No broad rewrite needed | B + C |
| MBO-08 empty/warning success | Reject | Yes | Yes | Outcome ledger shared | Partition outcome schema | Yes across exporters | B + C + E |

## 9. Proposed repository responsibility after redesign

The repository should remain specialized. It should own:

1. fallible decoding into a versioned, lossless canonical MBO envelope;
2. deterministic, symbol/session-scoped book state for book-affecting actions only;
3. strict/quarantine/repair policies with explicit anomaly receipts;
4. book snapshots and optional reduced event projections whose lossiness is named;
5. terminal reconciliation and transactional publication for any persisted output.

It should not own:

- statistical feature formulas or labels;
- train/validation/test policy;
- inferred execution signing/grouping; DECISION-031 authorizes only publisher-specific `XnasCompletedUpdateEnvelopeV1`, while any generic semantic layer and its owner remain unresolved;
- global experiment manifests and compatibility rules that belong in `hft-contracts`;
- generic market-session/calendar logic already owned by `hft-statistics`.

## 10. Acceptance gates before reuse

1. Raw T and F decode to distinct variants; both are no-ops for book mutation.
2. Independent MBP-10 oracle comparison achieves the DECISION-033 named acceptance gate on exact source bytes and commits; no assertion from dirty documentation is accepted as evidence.
3. Zero named cancel/trade `order_not_found` on the accepted 234/234 sample, or a new exact operator decision explains a qualified nonzero population.
4. Typed consumers cannot produce a valid completion receipt without terminal integrity checks.
5. Malformed, truncated, interrupted, duplicate, reordered, NaN/sentinel, stale-cache, and mixed-source inputs yield fatal or quarantined outcomes; no final consumable artifact appears.
6. MBO and LOB artifact schemas carry source digest/size/catalog identity, code/dependency/contract/config/toolchain identities, clock definitions, units, row counts, and per-action reconciliation.
7. Historical affected outputs are inventoried by exact producer commit/config/source identity and either recomputed or permanently marked incompatible; no blanket claim is made from repository version alone.

## 11. Open questions

1. Which clock is the canonical causal-availability/index clock for each feed: `ts_recv`, `ts_event`, or a declared pair with an ordering policy?
2. Should the reconstructor expose only strict mode to production exporters, leaving repair mode exclusively for diagnostics?
3. Is persisted reduced MBO Parquet still required once the extractor consumes canonical events in-process? If yes, which downstream consumers justify it?
4. If separate authority restarts this work, should publisher-specific completed execution grouping/sign inference live in a sibling repository or a versioned subcrate? DECISION-031 does not authorize a generic cross-publisher owner.
5. What exact anomaly budgets, warm-start interval, and reset semantics are admissible per exchange/schema?
6. Which historical exports can be identified immutably enough to recompute, and which must be classified unresolved because source/config/dependency custody is incomplete?

## 12. Reproduction recipes and evidence limits

Primary cwd: `/Users/knight/code_local/HFT-pipeline-v2/MBO-LOB-reconstructor`. Retained exact suite commands are in R-MBO-01. Reusable recipes for the primary probes are:

```bash
dbn -C /Volumes/WD_Black/HFT-data/XNAS_ITCH/CRSP/mbo_2025-07-01_to_2026-01-09/xnas-itch-20251224.mbo.CRSP.dbn.zst

cargo run --release --features 'databento export' --bin export_to_parquet -- \
  --input /Volumes/WD_Black/HFT-data/XNAS_ITCH/CRSP/mbo_2025-07-01_to_2026-01-09/xnas-itch-20251224.mbo.CRSP.dbn.zst \
  --output '<fresh-output-dir>' --symbol CRSP

cargo run --release --features 'databento export' --bin export_to_parquet -- \
  --input README.md --output '<fresh-malformed-output-dir>' --symbol MALFORMED
```

These are reproduction recipes reconstructed from the CLI and fixtures, not a verbatim shell transcript. The audit did not retain complete environment dumps, exit-code/log files or hashes for every invocation, so this section is not a fully reproducible command receipt. Runtime artifacts under `/tmp/hft-backbone-audit-20260803.Ipf8oZ` are ephemeral and unregistered. The workspace protected-data hook prevented the truncation fixture. No result or documentation registration was performed in `hft-wiki` because this was an audit, not a scientific experiment.
