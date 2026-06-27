# Foundation Integrity — Validated Audit Findings + Design Plan (2026-05-29)

**Status:** DESIGN COMPLETE — no code written. User APPROVED (2026-05-29): implement **P0+P1** (Export
Artifact Integrity & Provenance); **quarantine (not delete)** the polluted export dir; dead-API
**deferred** to backlog. **Compaction-prep snapshot — next session: read "## START HERE" below first.**
**Origin:** Comprehensive read-only validation of `MBO-LOB-reconstructor` (foundation Rust crate) →
re-validation against REAL DATA + cross-repo ground truth → adversarial validation of the design.
**Method:** 15 parallel Opus deep-dive agents (9 audit + 3 re-validation + 3 adversarial) + direct
ground-truth verification. **Every finding cited to file:line / git / measured data.** This file is the
durable home so NO discovered issue is lost. Local coordination doc (monorepo root is not under git);
can be committed into a repo later.

> **Authority rule (followed throughout):** ground-truth CODE + measured DATA over documentation.
> Docs were repeatedly found stale/misleading and are themselves a finding.

---

## START HERE (next-session resume — read first)

> ✅ **2026-05-30 — STATUS: COMMITTED + PUSHED. CYCLE COMPLETE.** The entire P0+P1+CYCLE-2 Foundation-Integrity
> cluster is now committed AND pushed to `origin/main` across 4 repos (hft-contracts `448929f`, hft-ops `0612926`,
> feature-extractor `e7d0409`, lob-model-trainer `366b8ab`; all clean fast-forwards). **Every "UNCOMMITTED" note below
> is now HISTORICAL.** R-21 (the user's intended next experiment) was found to be SISTER-OWNED (uncommitted
> `lob-model-trainer/configs/experiments/r21_tlob_mean_return_h300_v3p0.yaml` + an in-flight `EXPERIMENT_INDEX.md`
> entry) → left to the sister, NOT started, per the don't-interfere-with-sisters mandate. Only the monorepo-root
> `contracts/pipeline_contract.toml` (C2 source) + root `CLAUDE.md` (C4) remain on-disk-only — expected (the root is
> not git-tracked). Open follow-ups are all backlog/opportunistic: #12b CLI subcommand, #8 (DEFER), #9, A.8.2 (**CHANGELOG `### Added` + CODEBASE.md DONE @`786b47e` 2026-05-30** — only the eventual 2.9.0 version STAMP remains, which is the sister's coordinated release event, NOT ours and NOT a blocker). **hft-contracts is now FULLY COMPLETE** (code + docs): a 4-agent fresh-eye sweep + empirical run found 928 green / 0 bugs / no hidden incomplete phase / consumers back-compat-safe / deprecations all future — READY to move to other repos.

> 🟣 **2026-05-30 — CYCLE-3: post-commit adversarial RE-VALIDATION + 2 test-locks COMMITTED + PUSHED.** 6 fresh-eye
> Opus agents (3 code-reviewers + hft-architect + REFUTE + test-adequacy) + a direct empirical run re-validated the WHOLE
> shipped cluster from scratch against ground-truth code + real data → **VERDICT SOUND**: zero bugs, all 8 REFUTE challenges
> refuted-by-code, ARCH-CLEAN 7/7 (no coupling, no goal-drift), every key decision (CF-1 / off-exchange-defer / fail-open /
> dedup-exclusion / days_emitted-drop / C3-quarantine / #8-defer) validated, and the **C4 doc claim confirmed bit-for-bit**
> (polluted `e5_timebased_60s` is exactly {schema 3.0×230, 2.2×3} + {commit b5e746d×230, c5e9d64×3} → the validator emits
> precisely the 8 documented violations; `e5_timebased_60s_v3p0` → 0 errors; `basic_nvda_60s` → 1 clear off-exchange pointer).
> Only open items were test-coverage regression-locks (NOT bugs). Closed the 2 highest-value (test-only, mutation-proven),
> both COMMITTED + PUSHED (ff): **hft-contracts `ea8842d`** `test_manifest_schema_differs_from_uniform_day_schema_raises`
> (locks validation.py:781-787 — uniform day-files but manifest header lies about schema) + **hft-ops `b9ed7bd`**
> `test_extraction_cache_hit_populates_producer_commits` (locks the cache-HIT capture extraction.py:141, structurally
> distinct from the already-covered completed-subprocess site :200). **New verify counts (SUPERSEDE the CYCLE-2 block
> below): hft-contracts 928 / `test_validate_export_dir` 28 ; hft-ops 1081 / `test_resolve_build_provenance` 19.** Remaining
> gaps stay in #9 (C2 Rust Σ-accum [A.8.1] — already wire-format-locked + consumer-cross-checked on real data; 2 minor C1
> branches: int-skipped WARN, skipped-as-list). **Current cluster HEADs: hft-contracts `ea8842d`, hft-ops `b9ed7bd`,
> feature-extractor `e7d0409`, lob-model-trainer `366b8ab`.** Foundation-Integrity work stays COMPLETE — this cycle only
> hardened regression coverage. R-21 / experiments remain SISTER-OWNED (untouched).

> ⚠️ **2026-05-30 CURRENT STATE — supersedes the numbered "First implementation moves" below.** Full
> validated designs: **§8 (C2/C4)** + **§9 (P1)**. **P0 EXPORT-INTEGRITY CLUSTER COMPLETE (UNCOMMITTED):**
> **C1** (`validate_export_dir` SSoT + hft-ops `validate_manifest` preflight enforcement; 24
> `test_validate_export_dir` tests) + **C2** (honest `total_sequences_emitted` manifest field — Rust
> `save_manifest` writer + wire-format test [55 bin tests] + additive `[export.manifest]` TOML + regen
> `_generated.py`, NO schema bump; `days_emitted` DROPPED as redundant per the 5-agent wave) + **C4**
> (doc-reconcile: root CLAUDE.md "uniformly 2.2" lie → POLLUTED + pre/post-align footnote + manifest-field
> row; `nvda_e5_60s.yaml`; EXPORT_INDEX e5-row). **C3** = operator `QUARANTINE_README` playbook (§8, NOT run
> — empirical `data/` mutation, low-value since C1 fail-loud + C4 docs already prevent training).
> **P1 (provenance, #7) IN PROGRESS** — 5-agent wave validated minimal = **P1a GRACEFUL** (§9; **P1b + P1c
> DEFERRED**). **✅ P1a STAGE 1 SHIPPED** (hft-contracts `Provenance.producer_commits` record-level OBSERVATION
> [NOT in the dedup fingerprint] + `build_provenance`/`record_from_artifacts` passthrough + 7 tests; full
> hft-contracts suite **889 pass**). **✅ P1a STAGE 2 SHIPPED 2026-05-30 (UNCOMMITTED; 4-agent pre-impl
> validation [GO] + 2-agent post-impl review [APPROVE-WITH-FIXES, both fixes applied]):** hft-ops
> `resolve_build_provenance` (FAIL-OPEN, never raises) + shared `resolve_patched_crate_dir` `.cargo` git-URL
> patch parser (hft-rules §0; fixes the dead `_resolve_hft_statistics_sha` tier-2 crates-io key-bug —
> behavior-preserving in the std layout) + `_git_status_porcelain_dirty`. Captured at EXTRACTION TIME into
> `captured_metrics["producer_commits"]` on the cache-hit + subprocess-completed paths ONLY (skip/dry/failed →
> `{}` = honest not-applicable); harvested in `cli._record_experiment` (mirrors `cache_info`) → passed to
> `record_from_artifacts`; `producer_commits` added to `dedup.py` fingerprint `exclude_keys` (defense-in-depth,
> mirrors rng_state). Locked vocabulary {extractor_git_sha, reconstructor_git_sha, hft_statistics_git_sha,
> reconstructor_source=`path-override@<sha>+<clean|dirty|unknown>`|`git-pin`, completeness=`full`|`partial`}.
> 18 resolver + 1 dedup-exclusion + 2 wiring + 1 ExtractionRunner-integration tests; hft-contracts 7
> provenance-field + 25 validate_export_dir tests. Post-impl-review fix: `isinstance(str)` guard on TOML
> `path` (closed a fail-open hole on a non-string path) + regression test. **P1b + P1c remain DEFERRED** (§9).
>
> **🔵 2026-05-30 ADVERSARIAL RE-VALIDATION + HARDENING (UNCOMMITTED).** Full ground-truth + REAL-DATA
> re-validation of the WHOLE cluster (Opus agents: P1a-deep + C1-real-data + arch/completeness + test-adequacy
> + a 3-change review; first 2 rate-limited → re-run + my own empirical checks). **ALL CONFIRMED-SOUND:** C1
> passes all 4 clean v3p0 dirs (0 errors) + rejects polluted `e5_timebased_60s` (8 errors, right reasons) +
> avoids the CF-1 PRE-vs-POST false-positive (real manifests verified — `split`/`partial_failure`-dict-or-null/
> `partition_key`/`days_processed` all match the code's assumptions); C2 cross-language aligns BY CONSTRUCTION
> (per-day metadata `n_sequences` AND manifest `total_sequences_emitted` both = `normalized_sequences.len()`,
> pipeline.rs:431/633) + `EXPORT_MANIFEST_REQUIRED_FIELDS` has no hard-enforcement consumer (the add is
> non-breaking); P1a §0 refactor behavior-preserving (real `.cargo` uses git-URL keys) + crash-risk REFUTED
> (`config.paths.{extractor,reconstructor}_dir` are pure `pipeline_root / "…"` joins, paths.py:43-48, already
> called in `validate()` before `run()`). **ONE real defect found + FIXED: git-pin sha contradiction** — the
> resolver emitted the local checkout HEAD as `reconstructor_git_sha` EVEN in git-pin mode, contradicting the
> `Provenance` docstring ("git-pin carries NO sha"); the earlier "fix (2)" was DOCSTRING-ONLY / one-sided. NOW
> the resolver emits `reconstructor_git_sha = "unresolved"` (+ `completeness = "partial"`) in git-pin mode →
> 3-way coherent (resolver + test + provenance.py docstring); **production no-op** (path-override is always
> live in the real pipeline; git-pin is unreachable). Closed 2 test gaps: **ExtractionRunner.run() integration
> test** (mutation-proven lock on the real `extraction.py` capture site — the prior wiring tests stopped at a
> hand-built StageResult) + **`partial_failure: null` C1 fixture** (real on-disk shape the synthetic dict
> fixture never exercised). **hft-ops 1080 / hft-contracts 920 — all green; 0 regressions.** Backlog (#9) +=
> (a) Rust integration test for the `total_sequences_emitted` accumulation loop (LOW-RISK: by-construction +
> independently cross-checked by validate_export_dir's per-split file-count reconciliation; needs full
> extraction harness) + (b) hft-contracts **2.9.0 CHANGELOG/SemVer** entry for the new public surfaces
> (`validate_export_dir` + `Provenance.producer_commits`) — CHANGELOG.md is sister-F1-owned + active,
> `__version__`/pyproject correctly stay 2.8.1; coordinate at commit. **P1a (= validated-minimal P1) COMPLETE +
> RE-VALIDATED.** NEXT: see the **2026-05-30 CYCLE-2 block** immediately below (supersedes this NEXT line).
> **All edits UNCOMMITTED** (commit only when asked).

> 🟢 **2026-05-30 CYCLE-2 — NEXT-PHASE PREP (5 read-only Opus agents) + off-exchange fail-clear SHIPPED (UNCOMMITTED).**
> Re-validated the whole cluster + scoped the candidate next-phases from scratch (5 agents: state-verify + #12-scope +
> #8-scope + adversarial-priority + incomplete-phase sweep; all read-only, zero sister interference). **Cluster re-confirmed
> GREEN** (hft-ops 1080 / hft-contracts now **927**: +2 new off-exchange tests; the prior +5 over 920 was sister test-file
> growth, NOT a regression). **ONE genuine incomplete-phase item found + CLOSED this cycle (the only must-fix):**
> `validate_export_dir` shipped a FALSE docstring + error claiming off-exchange exports "carry no dataset_manifest.json"
> — disk disproves it (`data/exports/basic_nvda_60s/dataset_manifest.json` EXISTS, 6314 B, `contract_version=
> "off_exchange_1.0"`) AND it emitted 233 spurious per-day schema errors on an off-exchange dir. FIX (hft-contracts
> `validation.py`, UNCOMMITTED): corrected docstring + missing-manifest message + added an **off-exchange fail-clear
> precondition guard** (`str(manifest.get("contract_version") or "").startswith("off_exchange")` → ONE clear
> `ContractError` pointing to `validate_off_exchange_export_contract`, mirroring the existing missing/unparseable
> preconditions; this is NOT the full off-exchange dir routing — that stays deferred #12). +2 tests
> (`test_validate_export_dir.py` 25→**27**). Real-data verified: `basic_nvda_60s` now raises ONE 275-char clear error
> (no 233-wall); v3p0 still PASSES (`contract_version=None` → falls through). C1 is now behaviorally honest (hft-rules
> §11) — **the Foundation-Integrity cycle is COMPLETE; no incomplete phase remains.**
>
> **NEXT-PHASE VERDICT (5-agent convergent — SUPERSEDES the old "#12→#8→#9" order):**
> 1. **COMMIT the validated cluster** — highest value (uncommitted across 4 repos, machine-migration-fragile, racy tree).
>    Stage by EXACT name; coordinate the hft-contracts **2.9.0 CHANGELOG** with the live sister-F1 session (A.8.2;
>    `__version__` stays 2.8.1). Requires the user's explicit ask (standing rule).
> 2. **RESUME EXPERIMENTS** (R-21 128-feat TLOB regression per `DECISION-008`) — the actual program goal; the harness is
>    ready and is NOT the bottleneck (R-20/E17 ran without friction). Beats more integrity scaffolding.
> 3. **#12(a) trainer direct-path gate → DROPPED (phantom):** the per-day gate (`dataset.py:890`) + idx-97 check already
>    run on that deprecation-warned path; the orchestrated path is already C1-gated. **#12(b) standalone `hft-ops
>    validate-export-dir <dir>` CLI (it's `click`, NOT argparse) → opportunistic post-commit only** (clean, not urgent;
>    inherits the new off-exchange guard).
> 4. **#8 release/pin hygiene → DEFER INDEFINITELY (preventive, no live victim). CORRECTED FACTS vs §3/§6/§7:**
>    it is **2 branch-pins, not 3** (the extractor already pins `tag="v0.2.1"` at `crates/hft-extractor/Cargo.toml:35` —
>    pin (c) "stale comment" is WRONG); the "**busts the 14 GB cache**" rationale is **MOOT** (`data/exports/_cache/`
>    does not exist — the real decouple reason is provenance/dedup desync); **BQP pin (b) is SISTER-ACTIVE TODAY**
>    (basic-quote-processor: 5 commits 2026-05-30 incl. `3ee3595` "#35 dep-pin migration" + dirty lock — DO NOT TOUCH);
>    profiler pin (a) dormant. Only the reconstructor `v0.2.2` cut is self-contained/safe, but zero-payoff + unblocks nothing.
> 5. **#9 backlog** (off-exchange full routing #12, split/splits drift, dead `validate_outputs` hooks, missing `_v3p0`
>    EXPORT_INDEX row, A.8.1 Rust accum test, A.8.2 CHANGELOG, dead-API ~7-8k LOC, UNDEF guard) — ALL re-confirmed
>    safe-to-defer (none makes a shipped phase incomplete; the P1a end-to-end thread is verified intact).

**Uncommitted surface (this session — everything else dirty in these repos is PRE-EXISTING sister work, do NOT touch):**
- hft-contracts: `M src/hft_contracts/validation.py` (C1 + C2 cleanup + 2026-05-30 CYCLE-2 off-exchange fail-clear guard + docstring/missing-manifest-msg fix) · `M src/hft_contracts/__init__.py` (C1) · `M src/hft_contracts/_generated.py` (C2 regen) · `M src/hft_contracts/provenance.py` (P1a Stage 1: producer_commits; 2026-05-30: git-pin docstring coherence, matches resolver fix) · `M src/hft_contracts/experiment_recorder.py` (P1a Stage 1: passthrough) · `?? tests/test_validate_export_dir.py` (C1; +partial_failure:null + off-exchange ×2 → **27 tests**) · `M tests/test_provenance.py` (P1a Stage 1: +7 tests)
- hft-ops: `M src/hft_ops/manifest/validator.py` (C1) · `?? tests/test_validator_export_dir.py` (C1) · `M src/hft_ops/scheduler/extraction_cache.py` (P1a Stage 2: resolve_build_provenance + resolve_patched_crate_dir + _git_status_porcelain_dirty + _resolve_hft_statistics_sha git-URL bug-fix; 2026-05-30: git-pin sha coherence fix) · `M src/hft_ops/stages/extraction.py` (P1a Stage 2: capture producer_commits on cache-hit + completed) · `M src/hft_ops/cli.py` (P1a Stage 2: harvest + pass producer_commits) · `M src/hft_ops/ledger/dedup.py` (P1a Stage 2: exclude_keys += producer_commits) · `?? tests/test_resolve_build_provenance.py` (P1a Stage 2, 18 tests incl. ExtractionRunner integration) · `M tests/test_dedup.py` (P1a Stage 2: +1 exclusion test)
- feature-extractor-MBO-LOB: `M crates/hft-extractor/src/bin/export_dataset.rs` (C2) · `M EXPORT_INDEX.md` (C4)
- lob-model-trainer: `M configs/bases/datasets/nvda_e5_60s.yaml` (C4 doc-reconcile)
- monorepo-root (NOT git-tracked): `CLAUDE.md` (C4 doc-reconcile) · `contracts/pipeline_contract.toml` (C2 additive field) · `FOUNDATION_INTEGRITY_PLAN_2026_05.md` mirror
- MBO-LOB-reconstructor: `?? FOUNDATION_INTEGRITY_PLAN_2026_05.md` (this file; identical root-mirror at monorepo root)
- **⚠️ RACY TREE — stage by EXACT name, NEVER `git add -A` / `.`:** the "Uncommitted surface" list above IS
  the authoritative set of THIS work's files — stage ONLY those. EVERYTHING ELSE dirty in these repos is
  concurrent SIBLING-session work and the exact set CHANGES BETWEEN CHECKS (verified 2026-05-30: hft-contracts
  `CHANGELOG.md` + `pairwise_compare_artifact.py` were dirty then got committed + NEW `test_feature_sets.py`/
  `test_label_factory.py`/`test_gate_report.py` appeared — all within one session). So do NOT trust any frozen
  sister list; treat ALL non-"Uncommitted surface" dirty paths as sister-owned. Known sibling streams (examples,
  not exhaustive): hft-contracts F1 (pairwise_compare_artifact + CHANGELOG/2.9.0 [coordinate per A.8.2] + parity
  fixtures + assorted `test_*`); hft-ops R-NN sweeps (`experiments/sweeps/cycle*_r*.yaml`, `ledger/r20_*`,
  `scripts/analyze_r20_*`); lob-model-trainer (`EXPERIMENT_INDEX`/`CONSOLIDATED_FINDINGS`/r21/r20/E17). MEMORY.md
  is shared+racy too (agent-local; multiple sessions write it).

**Verify state on resume (~1 min, all read-only):**
```bash
python -m pytest hft-ops/tests/ -q                                   # expect 1080 passed / 0 fail (P1a Stage 2 + 2026-05-30 hardening: +18 resolver +1 dedup +2 wiring +1 ExtractionRunner-integration)
python -m pytest hft-ops/tests/test_resolve_build_provenance.py -q    # expect 18 passed (P1a Stage 2 resolver + ExtractionRunner integration; git-pin test asserts unresolved)
python -m pytest hft-contracts/tests/ -q                              # expect ~927 passed (920 prior + 2 CYCLE-2 off-exchange tests; +sister test growth)
python -m pytest hft-contracts/tests/test_validate_export_dir.py -q   # expect 27 passed (25 + 2 CYCLE-2 off-exchange)
python -m pytest hft-ops/tests/test_validator_export_dir.py -q        # expect 6 passed (C1 enforcement)
python -c "from hft_contracts.validation import validate_export_dir as v; v('data/exports/e5_timebased_60s_v3p0'); print('v3p0 PASS')"
# e5_timebased_60s (polluted) → raises ContractError with 8 violations
# basic_nvda_60s (off-exchange) → raises ONE clear ContractError ('off-exchange export … use validate_off_exchange_export_contract'), NOT a 233-wall (CYCLE-2 guard)
# C2 Rust wire-format (verified this session; Rust source unchanged since):
#   cargo test -p hft-extractor --features "parallel,databento" --bin export_dataset   → 55 passed
```

**State:** Design + validation COMPLETE for the `MBO-LOB-reconstructor` audit → pivoted to the
**"Export Artifact Integrity & Provenance"** cluster. **No code written.** 15 agents (9 audit + 3
re-validation + 3 adversarial) + direct disk verification. User approved (2026-05-29): implement
**P0+P1**; **quarantine (not delete)** the polluted export dir; dead-API **deferred** (#9).

**Durable record:** this file — canonical git-trackable copy at
`MBO-LOB-reconstructor/FOUNDATION_INTEGRITY_PLAN_2026_05.md` + mirror at monorepo root. Memory:
`~/.claude/projects/-Users-knight-code-local-HFT-pipeline-v2/memory/project_2026_05_29_reconstructor_audit_design.md`
(+ MEMORY.md pointer). TODOs #6 (P0) / #7 (P1) / #8 (P2 preventive) / #9 (deferred backlog).

**First implementation moves (in order — full detail §6):**
1. **CF-1 + CF-2** (~30 min, ground-truth read): `feature-extractor-MBO-LOB/crates/hft-extractor/src/batch.rs:316`
   `output.total_sequences()` semantics + dry-run current `save_manifest` → decide whether the ~2×
   `total_sequences` (136,902 vs disk 66,182) + `days_processed` 233-vs-230 are stale-artifact vs
   writer-bug. Locks C1 invariant wording + C2 scope.
2. **C1** — new `hft_contracts.validation.validate_export_dir(...)` (directory-level; reuses per-day
   `validate_export_contract` at `validation.py:394`); fail-loud on manifest↔disk mismatch + mixed
   schema/commit. Tests + wire at 4 call-sites (hft-ops extraction stage post-export; hft-ops
   `manifest/validator.py`; trainer `dataset.py` pre-train gate; CLI).
3. **C2** honest manifest → **C3** quarantine `e5_timebased_60s` (operator) → **C4** doc reconcile.
4. **P1** — promote `extraction_cache.py` resolver → `ExperimentRecord.provenance`; commit leaf locks;
   `price==UNDEF_PRICE` guard. Then **P2** preventive.

**ANTI-DRIFT (the validation already broke the wrong-direction loop — do not re-enter it):**
- Reconstructor CORE is CORRECT (Phase M REV3 + Phase O closed the prior HIGH cluster). Do NOT "fix" it.
- Boundary anomalies (UNDEF/flags/torn-EOF/out-of-order/Modify) are PHANTOM on real feeds (verified on
  230-day diagnostics + 36.5M records). Do NOT design fixes for them. ONLY the 1-line
  `price==UNDEF_PRICE` guard survives (multi-venue insurance).
- Do NOT build a `build.rs` Cargo.lock parser — REUSE the Python resolver (adversarial verdict / §0).
- Do NOT force-move tag `v0.2.1` (breaks v3p0 reproducibility) — cut `v0.2.2`.
- 3 branch-pins exist (profiler→reconstructor, BQP→hft-statistics, extractor stale comment).
- Decouple the extractor reconstructor-pin migration from the release (avoids a 14 GB cache bust).

---

## 0. Headline (what changed after validation)

1. **The reconstructor core is mature and CORRECT.** Phase M REV 3 ("Boundary Discipline") + Phase O
   Cycle 1 closed the entire prior HIGH-severity cluster. The Add/Modify/Cancel/Trade/Clear dispatch,
   `full_reset`, B.2a Clear wiring (`book_clears` exactly-once), determinism (no AHashMap-order leak),
   zero-alloc hot path, atomic stats envelope, 3-case `ts_event` fail-loud, and `RunningStats` Welford
   (bit-identical to the `hft-statistics` SSoT) all verified correct. **There is no active
   data-corruption fire in the reconstructor itself.**

2. **The audit's "critical" boundary findings are PHANTOM on the real feeds** (validated on 36.5M
   records / 4 days AND the full 230-day XNAS `_diagnostics.json` corpus): 0 mid-session Clears
   (book_clears always exactly 1/day = pre-market), 0 crossed, 0 locked, 0 anomalies, 0 mid_record_eof.
   XNAS emits 0 Modify records. torn-EOF is already mitigated (databento-ingest SHA-256 verify is
   mandatory + inline). → Boundary hardening demoted to defensive backlog (one cheap exception, below).

3. **The REAL, live, goal-threatening defect is EXPORT ARTIFACT INTEGRITY & PROVENANCE** — surfaced by
   adversarial validation, then verified directly from disk. Export directories are not validated for
   internal consistency or producer provenance; re-exports pollute directories; manifests are stale/
   templated (not recomputed from disk); docs mislabel directories. A researcher cannot trust on-disk
   data to be what its name/manifest/docs claim, nor trace it to the exact producer code. This directly
   attacks the program goal ("empirically precise, traceable, trackable experiments").

---

## 1. The realized victim (VERIFIED from disk — keystone)

Measured `data/exports/e5_timebased_60s*`:

| | `e5_timebased_60s` (docs: "schema 2.2 / pre-Phase-O / FAILS strict") | `e5_timebased_60s_v3p0` (docs: canonical) |
|---|---|---|
| manifest schema/days/total | 3.0 / **233** / **136,902** / commit `b5e746d` | 3.0 / **233** / **136,902** / commit `c62a1c0` |
| on-disk day-files | 163+35+35 = **233** | 162+35+33 = **230** |
| Σ per-day `n_sequences` (disk) | **66,500** | **66,182** |
| per-day `schema_version` | **MIXED `{2.2, 3.0}`** | clean `{3.0}` |
| per-day producer `git_commit` | **MIXED `{b5e746d, c5e9d64}`** | clean `{c62a1c0}` |

Three unambiguous defects:
- **D-1 Polluted directory:** `e5_timebased_60s` co-mingles two producer commits AND two schema
  versions (a partial re-export layered over an old one without clearing). Docs call it "uniformly
  2.2" — false. Nothing prevents training on it.
- **D-2 Lying manifest:** `days_processed=233` but 230 files exist (v3p0); `total_sequences=136,902` is
  identical in both dirs despite different disk contents (66,500 vs 66,182) → the manifest header is
  templated/stale, NOT recomputed from emitted days. Root CLAUDE.md repeats 136,902 (and is itself
  internally inconsistent: table says 230 days emitted but cites 136,902).
- **D-3 No validator** asserts manifest↔disk count parity or uniform schema/commit within a dir, and no
  gate fails-loud on it.

---

## 2. Validated finding register (status after re-validation + adversarial)

Severity is **production-calibrated** (does it affect live experiment data) with a `latent` tag where a
bug lives in unwired code. `PHANTOM` = verified not to occur on real data.

### REAL — live, goal-threatening (→ design target)
| ID | Finding | Evidence | Status |
|---|---|---|---|
| **A-VICTIM** | Export dirs not internally consistent / honestly self-described (D-1/D-2/D-3) | measured disk, §1 | REAL, live |
| **A-PROV** | NPY metadata captures extractor commit but NOT resolved reconstructor commit; `git_dirty:false` blind to gitignored `.cargo` path-override; ledger `Provenance.git` = `NOT_GIT_TRACKED_SENTINEL` (pipeline_root not under git); `data_export_fp` = filename/size hash → does NOT change with producer code | extractor `build.rs:6-22`, `config.rs:508-520`, `input.rs:133-148`; hft-contracts `provenance.py` | REAL |
| **A-PIN** | profiler `mbo-lob-reconstructor = {branch="main"}` (unpinned, 25 commits / 0.1.0↔0.2.1 behind, B.2a absent in locked build); basic-quote-processor `hft-statistics = {branch="main"}`; inconsistent pin discipline (tag/branch/rev + committed-vs-gitignored locks) | profiler `Cargo.toml:22`+`Cargo.lock`; BQP `Cargo.toml:19` | REAL (determinism §7); profiler correctness HISTORICAL-only (dormant; outputs last regenerated 2026-03-12) |
| **A-REL** | v0.2.1 tag → commit `28a9a22` whose Cargo.toml says `0.2.0` (bump `4cddcd3` is post-tag); no `[0.2.1]` CHANGELOG section | `git show v0.2.1:Cargo.toml`; CHANGELOG `[Unreleased]` | REAL (release hygiene) |
| **B-UNDEF (P1)** | `dbn_bridge` does not guard `price==UNDEF_PRICE` (`i64::MAX`); existing `price<=0` cannot catch it (positive sentinel); un-instrumented; only caught incidentally on the Clear (order_id/size=0) | `dbn_bridge.rs:89,108`; `types.rs:178` | REAL-but-latent on XNAS/NVDA; multi-venue/future blast radius → cheap P1 hardening |

### PHANTOM / benign on real feeds (→ defensive backlog, NOT critical)
| ID | Finding | Why phantom | Evidence |
|---|---|---|---|
| C-2 | UNDEF_PRICE garbage-level injection | every occurrence is the session Clear (already handled); 0 non-system price≤0 or UNDEF in 36.5M | real-data scan |
| C-4 | dropped `flags`/F_BAD | only flags {0,8,128,130}; F_MAYBE_BAD_BOOK=0, F_SNAPSHOT=0; F_BAD_TS_RECV only on the (handled) Clear | real-data scan |
| C-1(correctness) | profiler pre-B.2a Clear-skip corrupts books | all Clears pre-market before any order; 0 mid-session Clears across 230-day diagnostics; profiler dormant | diagnostics corpus + git |
| C-3 | torn-EOF undetectable | databento-ingest SHA-256 verify mandatory+inline (size+hash before atomic rename) | `downloader.py:372-496` |
| L-6 | Modify size-change loses queue priority | XNAS 0 Modifies; ARCX size-up modifies 0.0034%, only touch unwired QueuePositionTracker; LobState has no queue position | real-data scan |
| F-misc | out_of_order / Side::None uncounted; u32 total_size overflow | out-of-order ~0 (XNAS 0 inversions/33.1M); Side::None only on trades (normal); overflow needs >4.29B shares/level | real-data scan + code |

### DEAD / unwired public API (~7-8k LOC) — maintainability debt, GATED on roadmap (Q1)
Zero production consumers across both consumer repos (verified by symbol grep): `TradeAggregator`
(L-1 aggressor inversion, L-2 volume double-count — both latent), `QueuePositionTracker` (L-5),
`OrderLifecycleTracker` (L-3/L-4), `DayBoundaryDetector` (hardcoded -5 EST, but `crosses_midnight`
ignores the offset → dead), `analytics.rs` (MarketImpact/DepthStats/LiquidityMetrics), `statistics.rs`
(DayStats/NormalizationParams/RunningStats — T15-deprecated), `MarketDataSource`/`DbnSource`/`VecSource`
(YAGNI, inferior error contract), `MultiSymbolLob`, `WarningTracker`. **Decision (Q1):** delete /
feature-gate / wire+fix.

### DOC DRIFT (→ backlog, but A-VICTIM's doc piece is promoted into the design)
ARCHITECTURE.md stale (reconstructor.rs 2557 vs 3293; LobStats "17 fields" omits 2 F-013 counters;
"12 variants" vs 13; "v0.2.0"); PA §17.3 phantom `reconstructor/src/state.rs` + `reconstructor::read_mbo`;
PA test count "340" (real default=376, all-features=446, no-default=275); RULE.md stale duplicate of
root hft-rules.md; constants.rs:43 cites archived monolith path. **Promoted to design:** the
e5_timebased_60s mislabel (docs say "2.2/fails-strict"; disk is mixed {2.2,3.0}) actively corrupts
data-selection → fix in the design, not deferred.

---

## 3. Root cause + design: "Export Artifact Integrity & Provenance"

**Root cause:** the export artifact (NPY days + `dataset_manifest.json`) has no enforced
internal-consistency or producer-provenance contract, and re-exports can silently pollute a directory.
So names/manifests/docs diverge from disk, and an experiment's data cannot be trusted to be what it
claims or traced to the code that produced it.

**Design principle:** *an experiment is trustworthy only if its inputs are (a) internally consistent,
(b) honestly self-described, and (c) traceable to the exact producer code — and the pipeline must
FAIL-LOUD the instant any is violated, as close to the source as possible.*

This reuses existing machinery (no new patterns): `hft-contracts.validate_export_contract` (the
existing export-validator SSoT), the `hft-ops/.../extraction_cache.py` build-env resolver (already
computes `reconstructor_git_sha` + `hft_statistics_git_sha` + cargo_lock + binary hash, path-override
aware — codified in `pipeline_contract.toml:664-683`), and the `hft_contracts.provenance.Provenance`
dataclass. It does NOT touch the verified-correct reconstructor core (zero regression risk).

### Components (priority order, anchored to the realized victim)

**P0 — Export Integrity (DO TOGETHER; the realized victim):**
- **C1 Validator** (extend `hft-contracts.validate_export_contract`, SSoT): given an export dir, assert
  manifest.days_processed == #day-files (per split + total); manifest.total_sequences == Σ per-day
  n_sequences; uniform `schema_version` across day-files == manifest; single producer `git_commit` (or
  explicitly recorded set) across day-files; split day-sets disjoint + match manifest. FAIL-LOUD.
- **C2 Honest manifest:** fix extractor `save_manifest` (`export_dataset.rs`) to recompute
  days_processed / total_sequences / per-split day-lists from ACTUALLY-EMITTED days (post fail-loud
  drop), never templated.
- **C3 Two enforcement points** ("catch as fast as possible"): post-export self-check in the extractor;
  pre-train gate in hft-ops (before consuming an export).
- **C4 Remediate:** regenerate stale v3p0 manifests; quarantine/relabel the polluted `e5_timebased_60s`
  (DECISION required — deleting empirical data is high-stakes; default = quarantine + honest manifest,
  not delete).
- **C5 Doc reconciliation:** fix root CLAUDE.md + `nvda_e5_60s.yaml` claims about e5_timebased_60s
  (mixed, not "uniformly 2.2").

**P1 — Producer provenance + cheap hardening:**
- **Promote** the existing `extraction_cache.py` build-env resolver from cache-gated to a first-class
  UNCONDITIONAL provenance producer (reconstructor + hft_statistics commit + toolchain + binary hash;
  path-override via `cargo metadata` `source==null`). Write into `ExperimentRecord.provenance` (new
  Optional `producer_commits` dict mirroring `config_hashes`; bump `PROVENANCE_SCHEMA_VERSION`) and
  stamp the export manifest. **NO build.rs Cargo.lock parser** (rejected: blind in clean git-pin
  builds, untestable, duplicates the Python resolver → §0 violation). Fix the `[patch.crates-io]`
  vs git-URL key bug in the resolver.
- Commit leaf/binary `Cargo.lock`s (git-pin resolution only) + CI check rejecting path-source entries
  for foundation crates.
- One-line `dbn_bridge` guard rejecting `price==dbn::UNDEF_PRICE` (B-UNDEF).

**P2 — Release/pin hygiene (preventive, no current victim):**
- Reconstructor clean release: CHANGELOG `[Unreleased]`→`[0.2.2]` (incl. rust-toolchain pin —
  reproducibility input), Cargo.toml=0.2.2, tag `v0.2.2` at HEAD. NEVER force-move `v0.2.1`.
- Remove all 3 branch-pins (profiler→reconstructor, BQP→hft-statistics, extractor stale comment) →
  immutable tag/rev. **DECOUPLE** the extractor reconstructor-pin migration from the release (migrating
  busts the 14GB extraction cache for zero correctness benefit — only migrate when re-extracting).
- CI gates: tag==Cargo.toml version==CHANGELOG-section on tag push; no-branch-pin policy.

### Resolved design contradictions (escalated, not papered over)
- **Rust build.rs vs Python resolver:** REJECT build.rs (ADV-2): it runs from the extractor crate dir
  → its `git rev-parse` sees the extractor repo, not the reconstructor; in a clean git-pin/CI build
  there is no local reconstructor checkout; untestable; duplicates the existing Python resolver (§0).
  REUSE/promote the Python resolver instead.
- **Metadata.json vs ExperimentRecord:** prefer `ExperimentRecord.provenance` (additive Optional field;
  avoids the Class A export-contract ripple that adding a `metadata.json` provenance field triggers per
  the Change-Coordination Checklist). Manifest stamp is free-form (low cost).

### Remaining traceability holes to scope explicitly (do not silently drop — "no incomplete phase")
- `data` symlink → external APFS (`/Volumes/WD_Black/HFT-data`): input-data identity only captured when
  caching on; capture the databento manifest hash unconditionally.
- `hft-statistics` is a co-equal path-overridden foundation crate → capture its commit too (not
  reconstructor alone), or the "exact producer code" goal is half-met.
- Downstream hops (trainer/backtester/ledger) currently DROP source-export provenance — thread
  `producer_commits` through to the ledger.

---

## 4. Open decisions (user)
- **Q-SCOPE:** confirm the pivot from "reconstructor boundary fixes" → "export artifact integrity &
  provenance" (spans hft-contracts + feature-extractor + hft-ops + reconstructor + docs).
- **Q-REMEDIATE:** polluted `e5_timebased_60s` — quarantine+relabel (recommended) vs delete vs leave
  +document. (Touches empirical data; needs explicit direction.)
- **Q1 (dead API):** delete / feature-gate / wire+fix the ~7-8k LOC unwired surface.
- **Q2 (Parquet):** is the `export` Parquet path + MBO-LOB-analyzer live? (gates Cluster D).

---

## 5. Provenance of this analysis
- 9 audit agents (per-module), 3 re-validation agents (real-data 36.5M records + cross-repo deps +
  Phase M/O history), 3 adversarial agents (cluster challenge + design attack + reuse map). Keystone
  (§1) verified directly from disk by the lead. Boundary-phantom verdict corroborated against the full
  230-day `_diagnostics.json` corpus (incl. 2025-04-09 tariff-crash day).
- Exact ground-truth numbers: reconstructor tests **376 default / 446 all-features / 275 no-default**
  (toolchain 1.94.0). reconstructor.rs=3293 LOC; TlobError=13 variants; LobStats=18 fields.

---

## 6. Detailed Implementation Blueprint — APPROVED cluster P0+P1 (still no impl)

User decisions (2026-05-29): cluster = **Export Integrity & Provenance (P0+P1)**; remediation =
**quarantine+relabel** the polluted dir (no deletion); dead-API = **deferred to backlog (#9)**.

Ground-truth anchors confirmed for the blueprint:
- `validate_export_contract(metadata: dict, *, strict_completeness=False) -> list[str]` is **PER-DAY**
  (`hft-contracts/.../validation.py:394`); raises `ContractError` on hard violation. Wired at:
  `lob-model-trainer/.../data/dataset.py:48,894` (pre-train, per-day; comment :894 acknowledges the
  "metadata-only gap"), `hft-ops/.../manifest/validator.py:422`, `lob-dataset-analyzer/.../session.py:454`.
- Extractor manifest writer: `crates/hft-extractor/src/bin/export_dataset.rs::save_manifest` (:1109
  writes `days_processed`/`total_sequences`), accumulates `total_sequences += output.total_sequences()`
  (`batch.rs:316`); has `split_days_emitted` + `skipped_days` + `partial_failure` blocks. Prior
  `day_files.len()` overcount fix at :916. NPY per-day provenance = `ProvenanceInfo`
  (`crates/hft-export-pipeline/src/input.rs:133-148`), written at `pipeline.rs:444`.
- Provenance resolver to REUSE: `hft-ops/.../scheduler/extraction_cache.py:1055-1129`
  (`reconstructor_git_sha` :181/:1069; `_resolve_hft_statistics_sha` :1151-1193 path-override-aware),
  **gated on `config.cache_extraction`** (the gap). Contract slot `pipeline_contract.toml:664-683`.
- Python provenance plane: `hft_contracts.provenance.Provenance` (:235-296), `PROVENANCE_SCHEMA_VERSION`
  (:233), `build_provenance` (:307-405), embedded in `ExperimentRecord` via `experiment_recorder.py`.

### P0 — Export Artifact Integrity (DO TOGETHER)

**C1 — Directory-level Export Integrity Validator (NEW, the root fix).**
- *Problem:* per-day `validate_export_contract` cannot see manifest↔disk divergence or cross-day
  schema/commit mixing (its known "metadata-only gap").
- *Target:* new SSoT `hft_contracts.validation.validate_export_dir(export_dir, *, strict=True) ->
  list[str]` that, scanning the directory + per-day metadata, asserts (fail-loud `ContractError`):
  (1) per-split on-disk `*_sequences.npy` count == manifest emitted-day accounting
  (`days_processed - len(skipped_days) - len(partial_failure.failed_partitions)`);
  (2) `Σ per-day n_sequences == manifest.total_sequences` (after the §6 total_sequences-semantics
  confirmation — if `output.total_sequences()` is a legitimately different metric, assert the correct
  identity and add a `total_sequences_emitted` field);
  (3) **uniform `schema_version` across all day-files == manifest.schema_version** (catches the
  e5_timebased_60s {2.2,3.0} pollution);
  (4) **single producer `git_commit` across all day-files** (or an explicitly recorded set) — catches
  the {b5e746d,c5e9d64} mix;
  (5) split day-sets disjoint; (6) every `*_sequences.npy` has a matching `*_metadata.json`.
  Internally calls the per-day `validate_export_contract` for each day (reuse, no dup).
- *Wiring (catch-as-fast-as-possible, no Rust dup):* (a) hft-ops extraction stage — invoke immediately
  post-export (production-time fail-loud); (b) extend `hft-ops/.../manifest/validator.py` to call it;
  (c) trainer `dataset.py` pre-train gate — it already globs `*_sequences.npy`, add the dir check
  beside the per-day loop; (d) a thin `hft-ops` CLI subcommand + CI usage for ad-hoc/audit.
- *Tests (hft-contracts):* golden fixtures — clean dir passes; mixed-schema dir raises; mixed-commit
  dir raises; manifest day/seq mismatch raises; missing-metadata raises. + a fixture mirroring the
  real e5_timebased_60s pollution.
- *Contract-coordination:* if a `total_sequences_emitted` / `days_emitted` field is added to the
  manifest, it goes in `[export.manifest].required_fields` (additive, no schema bump — B4.3a/B-9
  precedent) + regen `_generated.py` + `[[changelog]]`.

**C2 — Honest manifest writer.**
- *Confirm-then-fix:* read `batch.rs:316`/`builder.rs:647` `total_sequences()` semantics + dry-run the
  current HEAD writer on a sample to learn whether it already matches disk. If mismatch persists, emit
  unambiguous `days_emitted` (= on-disk file count) + `total_sequences_emitted` (= Σ per-day
  n_sequences) alongside the existing attempted-counts; keep `days_processed`=attempted (documented).
- *Tests:* wire-format test asserting `manifest.days_emitted == #emitted files` and
  `total_sequences_emitted == Σ n_sequences` on a synthetic 2-day + 1-fail-loud-day fixture.

**C3 — Remediate existing exports (quarantine, per user decision).**
- Move polluted `data/exports/e5_timebased_60s` → a quarantine namespace (e.g.
  `data/exports/_quarantine/e5_timebased_60s_MIXED_2026-05-29/`); rewrite its manifest to reflect TRUE
  mixed contents (per-day schema/commit sets) OR drop a `QUARANTINE_README` documenting the pollution.
  Regenerate the v3p0 manifests via the fixed C2 writer so they pass C1. **No data deleted.**
  (Operator step — touches `data/`; the `block_data_edits` hook guards `data/` so this is run as an
  explicit, logged operator action, not an automated edit.)

**C4 — Doc/name reconciliation.**
- Fix root `CLAUDE.md` (e5_timebased_60s described as "uniformly schema 2.2 / fails strict" — FALSE;
  it's quarantined-mixed) + `lob-model-trainer/configs/.../nvda_e5_60s.yaml` comment. Reconcile the
  136,902/230-day numbers (cite measured disk truth). Same-session per hft-rules §11.

### P1 — Producer Provenance + cheap hardening

**P1a — Promote the build-env resolver to first-class provenance (REUSE, not build.rs).**
- *Refactor:* extract the `build_env` block of `extraction_cache.py:1055-1129` into a standalone
  `resolve_build_provenance(extractor_dir, reconstructor_dir, hft_statistics_dir) -> dict`
  ({reconstructor_git_sha, hft_statistics_git_sha, extractor_cargo_lock_sha256, compiled_binary_sha256,
  toolchain (rust-toolchain.toml/rustc), platform_target, `reconstructor_source`:
  `git-tag#sha | path-override@<localHEAD>+<dirty>` via `cargo metadata` `source==null`}). Call it
  **UNCONDITIONALLY** in the hft-ops extraction stage (not gated on `cache_extraction`). Fix the
  `[patch.crates-io]` → git-URL `[patch."https://github.com/nagarx/MBO-LOB-reconstructor.git"]` key bug
  in the resolver while there.
- *Persist:* add `producer_commits: Dict[str,str] = field(default_factory=dict)` (+ `build_env`) to
  `hft_contracts.provenance.Provenance` (mirrors `config_hashes`; Optional → back-compat via
  `from_dict`); bump `PROVENANCE_SCHEMA_VERSION`; populate in `experiment_recorder.record_from_artifacts`.
  Optionally stamp the (free-form) manifest provenance block too. **No `metadata.json` contract field**
  (avoids the Class A export-contract ripple) — record-level is sufficient for traceability.
- *Rejected:* a `build.rs` Cargo.lock parser (blind in clean git-pin/CI builds — build.rs sees the
  extractor repo, not the reconstructor; untestable; duplicates the Python resolver → §0).
- *Tests:* resolver unit tests (git-pin case → sha from Cargo.lock; path-override case →
  localHEAD+dirty); Provenance round-trip + back-compat-absent-field; schema-version bump golden.

**P1b — Commit leaf/binary Cargo.locks (git-pin resolution) + CI guard.**
- Stop gitignoring the extractor/profiler/BQP `Cargo.lock` for the **git-pin (no-override)** resolution
  only; add a CI check rejecting committed locks that contain `source`-absent (path) entries for
  foundation crates (prevents committing a path-polluted lock — the extractor lock currently has TWO
  reconstructor entries).

**P1c — `price == dbn::UNDEF_PRICE` guard (one-liner, the lone boundary survivor).**
- In `dbn_bridge.rs::convert`, reject `msg.price == dbn::UNDEF_PRICE` (i64::MAX) → `BoundaryError::Convert(InvalidPrice)`
  (the existing `price<=0` cannot catch the positive sentinel). Test: a synthetic UNDEF record → Err.
  Multi-venue/future-data insurance; un-instrumented today.

### Sequencing + decision gates
1. **Confirm-2** (implementation start): `output.total_sequences()` semantics + processed-vs-emitted →
   locks C1's identity (2) wording + whether C2 is fix vs regenerate. (~30 min, 2-file read + dry-run.)
2. **C1 validator + tests** (hft-contracts) → **C2 honest manifest + tests** (extractor) → wire C1 at
   the 4 call-sites. Gate: C1 passes on v3p0 (post C2-regenerate) and FAILS on the polluted dir.
3. **C3 quarantine** (operator, explicit) → C4 doc reconciliation (same session).
4. **P1a resolver promotion + Provenance field** → **P1b lockfile/CI** → **P1c UNDEF guard**.
5. Each step independently testable; no consumer breaks silently (additive fields, Optional, back-compat
   from_dict). P2 (release hygiene / branch-pins) follows as preventive; decoupled from extractor pin
   migration to avoid the 14 GB cache bust.

### Two open implementation-start confirmations (not design blockers)
- **CF-1:** `output.total_sequences()` (batch.rs:316) vs per-day metadata `n_sequences` — is the ~2×
  (136,902 vs 66,182) a different valid metric or a count bug? Decides C1 identity (2) + C2 scope.
- **CF-2:** `days_processed` semantics (attempted vs emitted) in the current writer → decides whether
  C2 adds `days_emitted` or fixes `days_processed`.
Both are resolved by a ~30-min ground-truth read at implementation start; the validator design is
correct under either answer.

---

## Appendix A — Complete deferred-finding detail (DO NOT LOSE; backlog #9)

Validated findings NOT in the P0/P1/P2 plan, captured with file:line so a future session can act
without re-running the 9-agent audit. Severity is production-calibrated; most are latent (unwired) or
low-value-on-current-data. Gating decisions: Q1 (dead-API roadmap), Q2 (Parquet/MBO-LOB-analyzer
liveness).

### A.1 Dead/unwired public API (Cluster C — GATED on Q1: delete / feature-gate / wire+fix)
Zero production consumers across feature-extractor + mbo-statistical-profiler (verified by symbol grep).
Production uses ONLY: `LobReconstructor`/`process_message_into`/`LobState`/`MboMessage`/`Action`/`Side`/
`constants` + loader (`iter_messages_typed`/`TlobError`/`Result`) + `dbn_bridge` + `hotstore`.
- `trade_aggregator.rs` (943) — **L-1** aggressor inverted for Trade('T') (`:261-269`; Databento 'T'
  side IS aggressor; correct only for Fill); **L-2** volume double-count (`:251-298`; sums 'T' summary
  + 'F' constituents). Root: `dbn_bridge.rs:125` merges `T|F`→Action::Trade. PHANTOM today (XNAS 0
  Modifies; aggregator unwired).
- `order_lifecycle.rs` (1637) — **L-3** inferred orders pool into observed counters (`:668-725`);
  **L-4** `time_alive_ns() as u64` wraps negative durations → ~1.8e19 (`:860-862`).
- `queue_position.rs` (1683) — **L-5** `average_queue_position` sums per-level FIFO indices across
  levels + AHashMap iter (`:763-785`); **L-6** same-price size-increase keeps priority (`:516-527`;
  PHANTOM: XNAS 0 modifies, ARCX 0.0034%).
- `day_boundary.rs` (614) — hardcoded `timezone_offset_hours=-5` BUT `crosses_midnight` ignores it
  (`:439-443`) → dead/moot; consume `hft_statistics::time` if ever wired.
- `statistics.rs` (826) — RunningStats/DayStats/NormalizationParams; T15-DEPRECATED. RunningStats is a
  3rd Welford (bit-identical to hft-statistics SSoT but lacks overflow guard `:67-80`). **DELETE not
  migrate** (dead).
- `analytics.rs` (566) — MarketImpact/DepthStats/LiquidityMetrics; no consumers (MarketImpact qty=0
  slippage bug `:340-365`).
- `source.rs` (623) — MarketDataSource/DbnSource/VecSource: YAGNI; `Item=MboMessage` can't carry errors
  (inferior to typed loader); DbnSource self-deprecated.
- `multi_symbol.rs` (385); `warnings.rs` (717) — unwired (WarningTracker cap-drop no counter `:363`).

### A.2 Doc drift (Cluster E)
- `ARCHITECTURE.md` systematically stale: reconstructor.rs 2557 vs **3293**; LobStats "17 fields" + a
  `pub errors` field (actual **18**, no `errors`, omits 2 F-013 counters); TlobError "12" vs **13**;
  "v0.2.0" vs 0.2.1; flat `loader.rs` (now `loader/mod.rs`); feature table omits `legacy-iterator-api`;
  MboMessage "32B" (actual ~40B; no `size_of` test).
- `PIPELINE_ARCHITECTURE.md` §17.3: phantom `reconstructor/src/state.rs` (LobState is `types.rs`) +
  `reconstructor::read_mbo` (nonexistent); test count "340" (real **376** default / **446** all /
  **275** no-default); flat `src/loader.rs`.
- `RULE.md` = stale 12-section copy of root `.claude/rules/hft-rules.md` (14 sections) → retire/pointer.
- `constants.rs:43` archived-monolith path cite; `export/schema.rs:153`+`mod.rs:49` cite nonexistent
  "RULE.md Section 1.44/1.45"; CHANGELOG `[Unreleased]` shipped in v0.2.1 (no `[0.2.1]` section).

### A.3 Defensive boundary/robustness (Cluster F — low-value on current data; PHANTOM/benign verified)
- out_of_order counter missing: `reconstructor.rs:1055-1058` + dup `:1299-1302` (`delta_ns` silently 0).
- Side::None directional drop, no counter: `reconstructor.rs:732-735` (`InvalidSide` variant dead).
- u32 `PriceLevel.total_size` overflow saturates silently: `price_level.rs:30,167`.
- `extract_date` filename collision silent overwrite: `export_to_parquet.rs:390-402,435`.

### A.4 Cross-repo consolidation candidates
- Welford 3rd-impl: DELETE reconstructor RunningStats (dead) rather than migrate to hft-statistics SSoT.
- `atomic_write_json` SSoT deferral (Cargo.toml:58-62 documents it; fold into a 0.3.x cycle).
- `DIVISION_GUARD_EPS` cross-repo dup + stale citation (`constants.rs:43`).

### A.5 Trust-column blind spot (relevant to P1 provenance)
`hft_contracts.provenance` `data_export_fp = data_dir_hash` = hash of NPY filenames+sizes
(`hash_directory_manifest`) → does NOT change when producer code changes (same shapes) ⇒ two exports
from different reconstructor commits get the same fingerprint. P1's `producer_commits` is the fix.
Ledger `Provenance.git` = `NOT_GIT_TRACKED_SENTINEL` (pipeline_root not under git).

### A.6 Remaining traceability holes (scope explicitly in P1 — "no incomplete phase")
data symlink → external APFS (input-data identity); hft-statistics co-equal path-overridden foundation
crate (capture its commit too); downstream hops (trainer/backtester/ledger) currently drop
source-export provenance.

### A.7 Parquet export contract (Cluster D — gated Q2: is MBO-LOB-analyzer live?)
EXP-1 consumer never validates `schema_version`; EXP-2 consumer hardcodes `FixedSizeList(int64,10)` →
any `levels!=10` export silently mis-read; C-5 exported `sequence` column = internal counter
(`reconstructor.rs:1074`), consumed by MBO-LOB-analyzer as if exchange sequence.

### A.8 Post-validation backlog (2026-05-30 re-validation cycle — DO NOT LOSE)
Two items surfaced by the 2026-05-30 adversarial re-validation of the SHIPPED P0+P1 cluster. Both are
NON-blocking — the cluster is complete + green without them — but must not be lost:
- **A.8.1 — Rust integration test for the `total_sequences_emitted` accumulation loop.** The Rust wire-format
  test (`export_dataset.rs::c2_save_manifest_emits_total_sequences_emitted_distinct_from_total_sequences`)
  proves the two manifest keys are DISTINCT (synthetic 400 vs 999) but does NOT exercise the real
  `total_sequences_emitted += export.n_sequences` accumulation (`export_dataset.rs:1041`). LOW-RISK deferral:
  the equality `total_sequences_emitted == Σ per-day n_sequences` holds BY CONSTRUCTION (both sides =
  `normalized_sequences.len()`, `pipeline.rs:431` + `:633`) AND is independently cross-checked by
  `validate_export_dir`'s per-split file-count reconciliation; a real producer-side test needs the full
  extraction harness (the accumulation lives in `main()`'s export loop — no cheap unit seam). The Python-side
  reconciliation IS already fixture-locked (`test_validate_export_dir.py::TestValidateExportDirEmittedFields`).
- **A.8.2 — hft-contracts 2.9.0 CHANGELOG + SemVer bump** for the TWO new public surfaces this cluster adds:
  `validate_export_dir` (new public validator, in `__all__`) + `Provenance.producer_commits` (+ the
  `build_provenance`/`record_from_artifacts` passthrough param) — per the root Change-Coordination Checklist
  ("Add a new validator to hft-contracts" + "Add a new upstream shared primitive → CHANGELOG + bump SemVer
  MINOR"). NOT yet written because `hft-contracts/CHANGELOG.md` is ACTIVELY SISTER-F1-OWNED (the F1 session is
  editing it alongside `pairwise_compare_artifact.py` + driving its own 2.9.0 note); `hft_contracts.__version__`
  + pyproject correctly stay **2.8.1** (do NOT race the bump). COMMIT-TIME coordination action: stage by exact
  file name, fold the joint 2.9.0 entry in with F1. (Distinct from the reconstructor's OWN `[0.2.2]`
  release-hygiene CHANGELOG item — see §3/§6/A.2 + task #8.)

### A.9 CYCLE-2 discoveries (2026-05-30 next-phase re-validation — DO NOT LOSE)
Re-usable findings from the 5-agent pass, captured so a future session need not re-investigate:
- **#12b CLI blueprint (Agent 2, ground-truth).** `hft-ops validate-export-dir <dir>` is a clean, in-scope,
  no-experiment-collision win (no such subcommand exists today). hft-ops CLI is **`click`, NOT argparse** — mirror the
  flat `@main.command()` `validate` command at `hft-ops/.../cli.py:659-689` (`click.argument` `Path(exists=True)` +
  `console.print` + `sys.exit(1)`); add `--strict/--no-strict` (default `--no-strict` to surface all issues, matching
  `_validate_existing_exports`' `strict=False`). It inherits the CYCLE-2 off-exchange fail-clear guard automatically
  (the guard lives in `validate_export_dir`). Tests via click `CliRunner` (clean MBO → exit 0; polluted → exit 1;
  off-exchange → 1 clear error; `--strict` raises-path). ~40 LOC + ~4 tests. Per the Change-Coordination Checklist
  "Add a new hft-ops CLI subcommand": update the cli.py module docstring command list + PA §14.6.
- **Off-exchange FULL routing is DEFER-JUSTIFIED, not merely postponed (Agent 4 call-path trace).** NO live caller
  reaches `validate_export_dir` with an off-exchange dir TODAY: the hft-ops orchestrated path runs ONLY the MBO
  `export_dataset` binary (`stages/extraction.py:174`); 0 hft-ops/trainer manifests reference `basic_*`/off-exchange;
  the trainer does not import `validate_export_dir`. So the (now-fixed) 233-error fault was real-but-unreachable → the
  CYCLE-2 **fail-clear guard is sufficient**; the FULL per-day off-exchange routing (delegate to
  `validate_off_exchange_export_contract`; note `validate_any_export_contract` ALREADY auto-routes per-day via
  `cv.startswith("off_exchange")`) stays #12/backlog until off-exchange is actually wired through hft-ops OR a trainer
  dir-gate is added. The trainer's MBO-vs-off-exchange discriminator today is path-prefix-based
  (`compatibility.py::derive_data_source` ~:117-131, `name.startswith("basic_")`); the manifest `contract_version` is
  the more authoritative discriminator for any future dir-level routing.
- **NEW latent finding (Agent 3, out of #8 scope → backlog).** The reconstructor `.github/workflows/ci.yml` `msrv` job
  pins Rust **1.82.0** while `rust-toolchain.toml` pins **1.94.0** — a latent CI/toolchain inconsistency. Harmless today;
  relevant if/when a reconstructor release-hygiene CI gate (#8) is added.

---

## Appendix B — Ground-truth reference (measured, not from docs)
- Reconstructor tests: **376 default / 446 all-features / 275 no-default** (toolchain 1.94.0); CI runs
  all-features + no-default. reconstructor.rs=3293 LOC; TlobError=13 variants; LobStats=18 fields.
- Real-data scan: 36.5M MBO records / 4 days (xnas-itch 20250305/20250428/20250626 + arcx-pillar
  20250825). 230-day XNAS `_diagnostics.json` corpus: book_clears always exactly 1/day (pre-market),
  0 crossed/locked/anomalies/mid_record_eof. XNAS 0 Modify records. flags ∈ {0,8,128,130};
  F_MAYBE_BAD_BOOK=0, F_SNAPSHOT=0. databento-ingest SHA-256 verify mandatory+inline
  (`downloader.py:372-496`).
- Polluted dir verified: `data/exports/e5_timebased_60s` = 233 files, schema {2.2,3.0}, commit
  {b5e746d,c5e9d64}, Σn_seq=66,500; `_v3p0` = 230 files, schema {3.0}, commit {c62a1c0}, Σn_seq=66,182;
  both manifests claim 233 days / 136,902 seqs.

---

## 7. RE-VALIDATION CORRECTIONS + C1 SHIPPED (2026-05-29, post-compaction)

A second from-scratch re-validation (5 parallel Opus agents: ground-truth victim, impl-surface+CF,
interference, + 2 adversarial) ran BEFORE implementation. It CONFIRMED the direction (export integrity +
provenance is the real, live defect; reconstructor core correct; boundary phantom holds) but corrected
three load-bearing premises. **These corrections SUPERSEDE the conflicting claims in §1 (D-2) and §6
(P1a).**

### Corrections (ground-truth, file:line)
1. **D-2 was MISDIAGNOSED.** `save_manifest` is NOT templated/stale — it RECOMPUTES from runtime
   accumulators (`export_dataset.rs:1069-1073`). It reports PRE-alignment / ATTEMPTED metrics:
   `total_sequences` = `output.total_sequences()` = `sequences_generated()` (pre label-alignment drop,
   `input.rs:120`); `days_processed` = `successful_count()` = extraction successes INCLUDING the 3 days
   that fail later at export. So 136,902 (pre-drop) vs ~66,182 (on-disk post-drop) and 233 vs 230 are
   CORRECT-BY-CONSTRUCTION and ALREADY documented in `partial_failure`. ⇒ C1 must NOT assert
   `total_sequences == Σ disk n_sequences` (would false-positive on every healthy export). C1 reconciles
   via `#files[split] == split[s].days − failed[s] − skipped[s]`. **CF-1 + CF-2 RESOLVED.**
2. **C1 home = hft-contracts (SSoT), not a new validator.** Two partial dir-validators already exist
   (hft-ops `manifest/validator.py::_validate_existing_exports` — break-after-one-day; lob-dataset-analyzer
   `ExportContractValidator._validate_manifest` — feature_count only). To avoid a 3rd (§0), the canonical
   `validate_export_dir` lives in hft-contracts (per the `validate_idx_97_reserved` fs-read precedent) and
   the hft-ops scanner DELEGATES to it.
3. **P1a resolver claim was FALSE.** `extraction_cache.py` has ZERO `cargo metadata` calls; it
   `git rev-parse`s the path-override checkout (`reconstructor_dir = pipeline_root/"MBO-LOB-reconstructor"`).
   For our always-path-override extraction it DOES capture the reconstructor commit, but it's fail-closed
   to None. P1a: promote unconditional + FAIL-LOUD/explicit-degrade on None (don't silently write empty);
   correct the mechanism description in §6.
4. **C3 quarantine is low-value + order-sensitive.** Polluted `e5_timebased_60s` is ABANDONED (0 ledger
   consumers; trainer base + all May cycles use `_v3p0`). Physically moving it breaks 5 April-era hft-ops
   manifests → PREFER relabel-in-place (honest manifest + README) + rely on C1 fail-loud. Sequence:
   C2-fix → regenerate → THEN remediate (NEVER quarantine-first with the unfixed writer).
5. **Content-hash manifest** (per-day sha256, an adversarial alternative) = FUTURE enhancement, not now
   (sha256-of-14GB cost; the uniformity + count validator already catches the observed defects).
6. **B-UNDEF** confirmed LATENT-INSURANCE (never fires on the corpus) — stays P1, framed honestly.
7. **Interference: GO** — no sibling session holds any target file dirty (hft-metrics + hft-wiki bounded
   to their own repos; the recent hft-contracts validation commits are this project's own prior cluster).

### C1 SHIPPED + VERIFIED (this session)
- NEW `hft_contracts.validation.validate_export_dir(export_dir, *, strict=True) -> list[str]` + package
  surface in `__init__.py`. Reuses `validate_day_metadata` per day; log-free; fail-loud; reconciles
  per-split + total counts via `partial_failure`/`skipped_days`; cross-day schema + producer-commit
  uniformity; pairing; split disjointness; forward-compat `total_sequences_emitted`/`days_emitted`.
- NEW `hft-contracts/tests/test_validate_export_dir.py` (25 tests incl. the CF-1 pre-align regression).
  **101 passed** (new + existing validation suites; full hft-contracts suite **869 passed**; 0 regressions).
- **Real-data gate PASSED:** `validate_export_dir('e5_timebased_60s_v3p0', strict=True)` → PASS (0
  warnings; pre-align `total_sequences=136,902` correctly NOT asserted). `validate_export_dir(
  'e5_timebased_60s')` → RAISES 8 precise violations (3× stale-2.2 per-day; `train 163≠162` + `test
  35≠33` stale-file counts; mixed schema `{2.2,3.0}`; mixed commit `{b5e746d,c5e9d64}`; total `233≠230`).

### C1 enforcement — LANDED (hft-ops orchestrated path) + sweep census
- Consolidated hft-ops `manifest/validator.py::_validate_existing_exports` → delegates to
  `validate_export_dir` (full-corpus + manifest↔disk + cross-day schema/commit uniformity, replacing the
  sample-one-day-then-`break`). It is ALREADY wired into `validate_manifest` → the `hft-ops validate` +
  run-preflight (the SUPPORTED entry point). 6 delegation tests + 170 hft-ops manifest tests pass.
- **DEAD-HOOK finding (→ backlog #9):** the per-stage `validate_outputs` hooks are defined on all 8 stage
  runners but have ZERO invocation sites — a true post-extraction producer-boundary gate would require
  making the stage protocol live (orchestration change). Today the live enforcement is the preflight.
- **42-dir on-disk sweep** (`validate_export_dir(strict=True)`): **8 PASS** (every current v3p0 +
  `nvda_xnas_148feat_*_v3p0` + `_smoke` + `nvda_v3p0_tb_pt40_sl20_h30`); **34 FAIL** = pre-Phase-O 2.2
  archives (correctly rejected under the current contract, consistent with the existing per-day gate) +
  `e5_timebased_60s` (the 8-violation pollution) + `basic_nvda_60s` (off-exchange schema "1.0").

### Remaining in the cluster (no incomplete phase — explicitly scoped)
- **C1 deferred (task #12):** trainer direct-path `load_split_data` dir-check + a `hft-ops` CLI subcommand.
  DEFERRED with rationale: `validate_export_dir` applies the MBO per-day contract, but the trainer loader
  is POLYMORPHIC (also serves off-exchange — `basic_nvda_60s` has a manifest + schema "1.0", and the sweep
  shows it would false-fail). Needs MBO-vs-off-exchange routing before gating the generic loader. The
  orchestrated path (hft-ops) is the supported entry and is now gated; the direct trainer path retains its
  per-day `_validate_day_metadata` gate (so this is additive defense-in-depth, not a hole).
- **C2** (#10): add honest disk-truth fields `total_sequences_emitted` + `days_emitted` to the extractor
  writer (additive TOML) + manifest-regen tool (no re-extract).
- **C3/C4** (#11): remediate polluted dir (operator, relabel-in-place) + doc reconcile (root CLAUDE.md
  136,902/230 numbers + `nvda_e5_60s.yaml`).
- **P1** (#7): promote resolver → `producer_commits` on `Provenance` + UNDEF guard + leaf locks.

---

## 8. C2 SCOPE REVISED (2026-05-29 post-compaction-2, 5-agent adversarial wave — SUPERSEDES §6 C2 + §7 #C2)

Before implementing C2, a 5-agent from-scratch wave ran (2 ground-truth Explore on the writer + the
manifest contract/consumers; 1 empirical-disk Explore; 2 adversarial general-purpose on necessity +
regen/sequencing safety). It CONFIRMED `total_sequences_emitted` is needed but found C2 **over-scoped**.
Every verdict is file:line-grounded. **This supersedes the C2 field list in §6/§7.**

1. **`days_emitted` → DROP (redundant; 3 independent witnesses).** The plan's claim that a
   `split_days_emitted` *manifest field* exists is **FALSE** — it is an in-process `BTreeMap`
   (`export_dataset.rs:774`, populated `:1037-1040`) used ONLY to build the `diagnostics_files` path list.
   Emitted-day count is already encoded 3 ways: (a) the PRIMARY `validate_export_dir` reconciliation
   `days_processed − failed − skipped == on-disk *_sequences.npy count` (`validation.py:768-777`, against
   on-disk ground truth — this is what catches the pollution: `230 ≠ 233`); (b) `len(diagnostics_files
   filtered by split)` == emitted days EXACTLY (Agent 3 empirical on v3p0: train 162 / val 35 / test 33,
   and the day-SETS match, zero diff); (c) the per-day `*_metadata.json` glob. A 4th `days_emitted` scalar
   adds ZERO detection capability — proven on the real polluted `e5_timebased_60s`.
2. **`total_sequences_emitted` → KEEP (irreducible).** Post-align on-disk sum (66,182) is in NO manifest
   field; `total_sequences` (136,902) is PRE-align (`output.total_sequences()` = `sequences_generated()`,
   `batch.rs:316`/`input.rs:120`); the trim is per-day-variable (`136902 − 230×311 = 65372 ≠ 66182`, off by
   810 from the 3 zero-emitting failed days). It is the only honest manifest-level sequence scalar, completes
   the self-describing-manifest principle, and activates C1's already-shipped+tested dormant hook
   (`validation.py:789`). De-scoped value note (Agent 4): it is a manifest-vs-its-own-per-day-metadata
   consistency check (catches a templated/drifted/hand-edited manifest), NOT an NPY-shape gate.
3. **⚠️ IMPLEMENTATION TRAP (Agent 4).** Do NOT compute it as `Σ split_stats.sequences` — those are
   PRE-align (`:1069` `output.total_sequences()`). Accumulate `Σ export.n_sequences` (POST-align) from the
   `Ok(export)` arm at `export_dataset.rs:~1026-1031` (currently only `log::info!`-ed), parallel to how
   `split_days_emitted` accumulates. The wire-format test MUST include a fail-loud day that contributes 0,
   to catch a pre-vs-post-align mistake. (Verify exact lines by reading the writer at impl start.)
4. **C1 dead-hook removal (hft-contracts, consistency).** `days_emitted` dropped ⇒ C1's dormant
   `days_emitted` reconciliation (`validation.py:~795-800`) + its 2 tests (`test_validate_export_dir.py:
   ~367-374` `test_days_emitted_mismatch_raises` + the days_emitted-present case) + the docstring mention
   (`validation.py:~547-549`) are dead code no producer will feed → REMOVE (no incomplete phase / no dead
   code, §0). C1 is UNCOMMITTED ⇒ ships minimal-correct from the start. KEEP the `total_sequences_emitted`
   hook + its tests. (SHIPPED: removed the 1 dedicated `days_emitted` test + stripped the `days_emitted`
   key from the sibling passing `total_sequences_emitted` test fixture → 25 → 24.)
5. **Regen tool → DEFER (speculative).** v3p0 PASSES C1 today with 0 warnings (forward-compat
   skip-if-absent); no consumer requires the field; retrofitting existing exports buys nothing for
   validation. Build only against a real need. SAFETY if/when built (Agent 5): regenerating a manifest
   shifts `hash_directory_manifest` SIZE → future re-runs of the ~10 v3p0-output manifests would dedup-MISS
   + re-run (bounded/benign, NOT a correctness/replay break — stored fingerprints stay frozen);
   cache-poison is MOOT (`data/exports/_cache/` does not exist); cleanest = `hft-ops export regen-manifest
   <dir> --check` CLI (dry-run default, operator-run).
6. **CORRECTED PREMISE (Agent 5) — the data-edit hook does NOT block `.json`/`.md`/`.toml`.** It blocks only
   `BLOCKED_EXTENSIONS = .npy/.dbn/.dbn.zst/.pt` + `BLOCKED_BASENAMES = _generated.py` (no `data/`
   path-prefix match). So C3-relabel + a regen tool are NOT hook-blocked; the safety guardrail is
   operator-run-only + dry-run discipline, NOT the hook. (Earlier "hook-guarded operator action" framing for
   C3 was WRONG — correct it in memory + this plan.) `_generated.py` IS hook-blocked → regenerate via
   `python contracts/generate_python_contract.py`, never hand-edit.
7. **C3 = relabel-in-place, NOT move (Agent 5 verified the "5 April manifests").** 5 abandoned manifests
   (`e5_60s_huber_cvml_unified.yaml`, `sweeps/{seed_stability,backtest_cost_sensitivity,loss_ablation,
   e5_phase2_sweep}.yaml`; 0 ledger consumers) reference bare `e5_timebased_60s` with `skip_if_exists` →
   physically MOVING the dir triggers a silent ~14 GB re-extraction (not a crash); relabel-in-place avoids
   it → genuinely safer. Resolves the §6 "quarantine-move" vs §7 "relabel-in-place" conflict → relabel-in-place.
8. **C4 doc-honesty can go FIRST (zero-risk .md).** Exact disk truth (Agent 3): `e5_timebased_60s` = 233
   files, MIXED schema {2.2,3.0} + MIXED commit {b5e746d,c5e9d64}, Σn_seq=66,500 (incl. 3 orphan days
   20250703/20251128/20251224 with full data but NO `diagnostics.json`); `_v3p0` = 230 files, clean {3.0} /
   {c62a1c0}, Σn_seq=66,182, `partial_failure` enumerates the 3 dropped days with the `311 prices` reason.

**REVISED MINIMAL C2 (this cycle):** (i) Rust writer emits ONLY `total_sequences_emitted` (post-align) +
(ii) one wire-format test (fail-loud day = 0) + (iii) additive `[export.manifest].required_fields` + typed
sub-section + regen `_generated.py` (no schema bump) + (iv) companion C1 dead-hook removal (drop
`days_emitted` hook/tests/docstring). `days_emitted` DROPPED; regen tool DEFERRED.

### C2 SHIPPED (2026-05-29 post-compaction-2) — code+contract+test, UNCOMMITTED
- **C1 cleanup (hft-contracts):** removed dead `days_emitted` hook + its 1 dedicated test + docstring
  mention; kept `total_sequences_emitted`. `test_validate_export_dir` 25 → **24**; full hft-contracts
  suite **882 pass**; real-data gate UNCHANGED (v3p0 PASS; polluted RAISES the SAME 8 violations WITHOUT
  the days_emitted check → redundancy empirically proven).
- **C2a writer (feature-extractor `export_dataset.rs`):** new `total_sequences_emitted` accumulated as
  `Σ export.n_sequences` (POST-align) in the `Ok(export)` arm (declared near `total_sequences`; threaded
  through the `save_manifest` call + signature; emitted in the json! block). `cargo check` clean.
- **C2c test:** `c2_save_manifest_emits_total_sequences_emitted_distinct_from_total_sequences` (passes
  pre=999/emitted=400 distinct, reads the produced manifest, asserts both keys distinct). **55 bin tests pass.**
- **C2b contract:** `[export.manifest].required_fields` += `total_sequences_emitted` + typed sub-section
  (B-9 precedent); regenerated `_generated.py` (isolated diff — only the new tuple entry + timestamp; NO
  schema bump).
- **DEFERRED to #11 (C4 doc-reconcile, bundled to avoid racing the sister-session root-CLAUDE.md banner):**
  Change-Coordination doc items (f) EXPORT_INDEX.md note + (g) root CLAUDE.md "Cross-Module Data Contracts"
  `dataset_manifest.json` row. EXPORT_INDEX has no clean field-changelog home (per-export-row ledger) → C4 pass.
- **→ backlog #9 (pre-existing drift, Agent 2 finding — NOT introduced by C2):** `required_fields` lists
  `"splits"` (plural) but the writer emits `"split"` (singular); and `total_sequences` is emitted but absent
  from `required_fields`. `validate_export_dir` already tolerates split/splits. Decide separately — do NOT churn in C2.
- **Pre-commit (when user authorizes):** run full `cargo test -p hft-extractor --features "parallel,databento"`
  (workspace-level) before committing the Rust change; the bin-target run (55) + `cargo check` already cover save_manifest.

### C4 SHIPPED + C3 operator playbook (2026-05-29) — doc-reconcile DONE, data-remediation operator-gated
**C4 doc-reconcile SHIPPED (UNCOMMITTED):**
- root CLAUDE.md: (a) Cross-Module Data Contracts `dataset_manifest.json` row += `total_sequences_emitted`;
  (b) v3p0 spec table "Total sequences" relabeled "(PRE-align generated)¹" + footnote clarifying 136,902 =
  pre-align vs ~66,182 on-disk emitted (+ the new field); (c) FALSE "e5_timebased_60s schema_version=2.2"
  archive claim → "POLLUTED: mixed {2.2,3.0} + mixed commit {b5e746d,c5e9d64}; FAILS validate_export_dir (8)".
- `lob-model-trainer/configs/bases/datasets/nvda_e5_60s.yaml`: same false-"2.2" comment → "POLLUTED mixed".
- feature-extractor `EXPORT_INDEX.md`: annotated the `e5_timebased_60s -- CURRENT` row (STALE+POLLUTED; bare
  disk no longer matches the documented 56,660/2.2 — it is 66,500/mixed; canonical = _v3p0; +C2 field).
- Verified: NO residual false `schema_version="2.2"` bare-dir claim; `total_sequences_emitted` documented in 3 surfaces.
- DELIBERATELY UNCHANGED: the §"10-step walkthrough" E5 example commands (describe the HISTORICAL E5 run using
  the bare-dir name — a historical example, not a false schema claim). The missing `_v3p0` EXPORT_INDEX export
  ROW → backlog (update-indexes domain, not C4 doc-reconcile scope).

**C3 operator playbook (NOT run — empirical `data/` mutation; the "no-train-on-polluted" intent is ALREADY met
by C1 `validate_export_dir` fail-loud + the C4 doc flags, so this is a LOW-value human-facing marker):**
Relabel-in-place via a `QUARANTINE_README.md` inside the polluted dir. **Do NOT physically move it** — moving
triggers a ~14 GB silent re-extraction on the 5 abandoned April manifests that resolve to the bare path.
Operator command (run with `!` or manually; the data-edit hook does NOT block `.md`):
```bash
cat > data/exports/e5_timebased_60s/QUARANTINE_README.md <<'EOF'
# ⚠️ QUARANTINED — POLLUTED EXPORT (do NOT train on this directory)
Co-mingles TWO producer commits + TWO schema versions (a partial v3p0-era re-export layered over the
pre-Phase-O original): MIXED schema_version {2.2,3.0} + MIXED git_commit {b5e746d,c5e9d64} across 233
day-files (Σ 66,500 seqs). FAILS hft_contracts.validate_export_dir (8 violations). Verified 2026-05-29.
Canonical replacement: ../e5_timebased_60s_v3p0 (clean {3.0}/{c62a1c0}, 230 days, ~66,182 seqs).
See FOUNDATION_INTEGRITY_PLAN_2026_05.md §8.
EOF
```
A full "honest mixed" manifest rewrite (per-day schema/commit enumeration) is NOT recommended — high-effort +
the validator already fails-loud; the README suffices.

---

## 9. P1 VALIDATED + SCOPED (2026-05-30, 5-agent wave) — minimal = P1a GRACEFUL; P1b + P1c DEFERRED

A 5-agent pre-impl wave (2 ground-truth + 3 adversarial) re-validated P1 from scratch. CONVERGENT verdict:
**MINIMAL CORRECT P1 = P1a ONLY (graceful/fail-open), scoped slightly WIDER; P1b + P1c DEFERRED.**

KEY GROUND-TRUTH (all file:line-cited in the agent reports this session):
- **`producer_commits` is NOT in the dedup fingerprint** (DEFINITIVE): `compute_fingerprint` is over the
  ExperimentManifest CONFIG, never Provenance/ExperimentRecord (`dedup.py:716-909`). → record-level
  OBSERVATION field (the `signal_export_output_dir`/`compatibility_fingerprint` precedent). A-PROV
  traceability is achieved record-level (`ledger show`); producer-change CORRECTNESS is already handled by
  the extraction cache KEY which includes `reconstructor_git_sha` (`extraction_cache.py:169-187`). Putting it
  in the fingerprint would re-create the C2 mass-invalidation catastrophe + only catch committed changes.
- **Today `Provenance.git` = `NOT_GIT_TRACKED_SENTINEL`** because the monorepo root is NOT a git repo
  (`provenance.py:99-102,344`; `capture_git_info(pipeline_root)`). ⇒ records currently capture NEITHER
  extractor NOR reconstructor commit reliably. P1a is MORE justified than the plan stated; capture the
  EXTRACTOR sha into producer_commits too (it survives only in the NPY `ProvenanceInfo`, not the record).
- **The resolver returns `None` TODAY in practice** (EMPIRICAL, Agent 4): of the 8 inputs, only #6
  `raw_input_manifest_hash` fails — `data/hot_store/` has NO `manifest.json` (assembled by copy, not
  databento-ingest; the fallback was deliberately removed `extraction_cache.py:1203-1208`). All git dirs +
  Cargo.lock + binary + git resolve fine. So caching is SILENTLY DISABLED (consistent with `_cache/` absent).
  ⇒ promoting to unconditional + FAIL-LOUD would HARD-FAIL every production extraction (STRICT REGRESSION).
  **MUST be GRACEFUL/PARTIAL/FAIL-OPEN**: capture what resolves, sentinel+WARN per missing field,
  `completeness` marker. SPLIT contracts: cache-key path stays fail-CLOSED (None disables cache); the
  provenance record is fail-OPEN. DO NOT reuse the cache-key None for provenance.
- **3 reconstructor commits in play**: lock-pin `28a9a22` / moved tag v0.2.1=`d091b34` / local HEAD
  `2b74523` (2 ahead of tag + DIRTY working tree). Only `git rev-parse HEAD` on the path-override dir gets
  the TRUE built commit. Tag `reconstructor_source = "path-override@<localHEAD>+<clean|dirty|unknown>"`
  (override present) vs `"git-pin"` (no override) — detect via `.cargo/config.toml` `[patch.*reconstructor*]`
  key presence + `git status --porcelain` for dirty. NOT `cargo metadata` (timed out 90s on cold cache).
  **SHIPPED-FORM CORRECTION (2026-05-30 validation):** the original `"git-pin#<sha>"` was DROPPED — in git-pin
  mode the local HEAD ≠ the Cargo-pinned built commit, so the resolver emits `reconstructor_git_sha="unresolved"`
  + `completeness="partial"` there (see Stage 2 (d) below).
- **hft-statistics key-bug CONFIRMED**: `_resolve_hft_statistics_sha` tier-2 reads
  `patch.crates-io.hft-statistics.path` (`extraction_cache.py:1178`) but the real config uses a git-URL key
  `[patch."https://github.com/nagarx/hft-statistics.git"]` → tier-2 is dead, silently falls to tier-3
  sibling-dir guess. Fix while there (parse `patch.<git-url>.<crate>.path`).
- **SKIP PROVENANCE_SCHEMA_VERSION bump**: it is free-text with ZERO machinery keying off it (only 3 test
  assertions); the field's OWN docstring (`provenance.py:249-251`) says bump-on-BREAKING, and an Optional
  additive field is non-breaking → no bump. (Contrast INDEX_SCHEMA_VERSION which DOES drive auto-rebuild.)

DEFERRED (not in P1 this cycle):
- **P1b (un-gitignore Cargo.locks + CI guard) → DEFER (P2/backlog).** Premise WRONG: the leaf reconstructor
  ALREADY commits its lock; the real gap is the EXTRACTOR's uncommitted+gitignored lock. Redundant with P1a
  for traceability (P1a captures the true built commit regardless of lock state). The proposed CI guard looks
  for `path=` sources, but the actual pollution is SOURCELESS workspace-promoted entries (`Cargo.lock:627`).
  Conflicts with the standing path-override. Defer until path-override removed OR lock regenerated overrides-off.
- **P1c (UNDEF_PRICE guard) → DEFER (backlog #9).** Validated SAFE + REAL latent hole (UNDEF=i64::MAX is
  POSITIVE → escapes `price<=0` → an UNDEF Add reaches `add_order` → phantom price level poisons the book).
  Spec (Agent 4): new `TlobError::UndefPrice(i64)` variant (enum is `#[non_exhaustive]`), exact-equality check
  in `dbn_bridge::convert` (~:84, before constructing MboMessage :108), route through `convert_or_skip` →
  increment a counter per hft-rules §8 (NOT crash). 0/36.5M occurrences → latent-insurance; don't gate P1.

**VALIDATED MINIMAL P1a (implement, staged with adversarial review after):**
- **Stage 1 — hft-contracts (PASSIVE) ✅ SHIPPED 2026-05-30 (UNCOMMITTED; +7 producer_commits tests, full hft-contracts suite 889 pass, 0 regress):** add `producer_commits: Dict[str,str] =
  field(default_factory=dict)` to `Provenance` (+ hand-list in `to_dict`, + `data.get("producer_commits") or
  {}` in `from_dict`); add `producer_commits: Optional[Dict[str,str]] = None` passthrough to
  `build_provenance` + `record_from_artifacts`. NO schema bump. NOT projected into `index_entry` (record-level
  only — the `signal_export_output_dir` path). Tests: default + round-trip + back-compat-absent-key.
- **Stage 2 — hft-ops (the resolver) ✅ SHIPPED 2026-05-30 (UNCOMMITTED):** added
  `resolve_build_provenance(*, extractor_dir, reconstructor_dir, hft_statistics_dir=None) -> Dict[str,str]`
  (FAIL-OPEN/partial; reuse `_git_rev_parse_head`; path-override/dirty detection; FIXED the hft-statistics
  git-URL key-bug; `completeness` + `unresolved` per-field sentinel). Harvested in `cli._record_experiment` +
  passed `producer_commits=` to `record_from_artifacts`. The direct-trainer path passes `None` (graceful).
  Tests: partial-capture + path-override-detection + **FINGERPRINT-STABILITY** (mirroring
  `test_dedup.py::test_artifacts_field_excluded_from_fingerprint`).
  **Refinements applied vs the original plan (per 4-agent pre-impl GO + 2-agent post-impl review):** (a)
  capture at EXTRACTION TIME (extraction stage → `captured_metrics`, harvested by cli — mirrors `cache_info`),
  NOT at record time — more honest (training-only runs → `{}`; no mid-run HEAD drift; cache-hit shas match the
  cache key by construction); set on cache-hit + subprocess-completed ONLY (skip/dry/failed → `{}`). (b) §0:
  factored the shared `resolve_patched_crate_dir` git-URL parser, routed `_resolve_hft_statistics_sha` through
  it (ONE parser; fixes the dead `crates-io` tier-2; behavior-preserving in the std layout — override
  `../hft-statistics` == sibling). (c) emit the Stage-1-LOCKED key vocabulary (`*_git_sha` +
  `reconstructor_source` + `completeness`). (d) `git-pin` carries NO sha — the resolver emits
  `reconstructor_git_sha="unresolved"` + `completeness="partial"` in git-pin mode (local HEAD ≠ Cargo-pinned
  built commit), matching the `provenance.py` docstring + `test_git_pin_when_no_cargo_override`. **(2026-05-30
  re-validation found UPDATE-5's "fix (d)" had reconciled ONLY the docstring while the resolver still emitted
  the local HEAD — one-sided; the resolver was then fixed so code+test+docstring are all coherent. Production
  no-op: path-override is always live, git-pin unreachable in real runs.)** (e) post-review: `isinstance(str)`
  guard on TOML `path` (fail-open hole) + regression test; `path-override@unresolved+unknown` edge locked.
  (f) 2026-05-30 re-validation closed 2 test gaps: ExtractionRunner.run() integration test (locks the real
  `extraction.py` capture site, mutation-proven) + `partial_failure:null` C1 fixture (real on-disk shape).
  Result: hft-ops **1080 pass / 0 fail** (18 resolver incl. ExtractionRunner integration + 1 dedup + 2 wiring);
  hft-contracts **920 pass** (incl. 25 validate_export_dir + 7 producer_commits). **P1a (Stage 1 + Stage 2) =
  validated-minimal P1 COMPLETE + RE-VALIDATED 2026-05-30 — all C1/C2/C4/P1a CONFIRMED-SOUND vs ground-truth +
  REAL DATA; the only defect found (git-pin) was fixed; 2 items → Appendix A.8.**

---

**END OF COMPACTION-PREP SNAPSHOT (updated 2026-05-30 — adversarial re-validation + git-pin coherence fix + 2
gap-closing tests; P0+P1 cluster COMPLETE + RE-VALIDATED; ALL UNCOMMITTED).** Everything discovered / decided /
planned / validated across this work is above + in "## START HERE" (the cold-start handoff) + Appendix A.8 (the
2 new backlog items). Next session: read "## START HERE" first.
