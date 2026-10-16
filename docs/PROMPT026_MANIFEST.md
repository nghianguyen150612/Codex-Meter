# PROMPT026 manifest

## Repository and workflow

- Verified starting `main` and fetched `origin/main`:
  `75e9660f11b355829d507914285310c2eff6fdbc`.
- Initial worktree: clean.
- Feature branch: `feat/p026-benchmark-calibration-analysis`.
- Required implementation commit subject:
  `feat: analyze benchmark capacity calibration`.
- Required PR title: `Add benchmark calibration and statistical analysis`.
- Implementation is confined to the feature branch. No direct implementation
  commit/push to main, squash, rebase, force push, or fast-forward merge is used.
- PR review, latest-head six-job CI, merge-commit parents, original feature
  reachability, and final main synchronization are checked before completion.
  GitHub creates the merge SHA only at merge time; actual post-merge facts are
  recorded in the completion report without predicting SHAs or requiring a
  separate documentation closeout PR.

## Method, modules, and public APIs

- `BENCHMARK_ANALYSIS_METHOD_VERSION = "1.0.0"`.
- Added `python/codex_meter/analysis/benchmark.py` (pure analysis policy) and
  `benchmark_models.py` (typed immutable evidence and reports).
- Stable exports through `codex_meter.analysis`:
  `analyze_benchmark_calibration`, `BenchmarkAnalysisPolicy`,
  `BenchmarkEvidenceDataset`, `BenchmarkExperimentEvidence`,
  `BenchmarkRunEvidence`, `BenchmarkRegimeBoundary`,
  `BenchmarkCalibrationAnalysis`, `BenchmarkCapacitySummary`,
  `BenchmarkCalibrationComparison`, `BenchmarkEvidenceAccounting`,
  typed provenance/status/cohort/warning/recommendation enums, and privacy-safe
  `BenchmarkAnalysisError`, `BenchmarkDatasetError`, `BenchmarkPolicyError`,
  `BenchmarkNumericalError`.
- Existing estimator, workload, regime, framework, and runtime method versions
  remain unchanged at `1.0.0`.

## Evidence and integrity

- Accepted provenance: typed `synthetic_fixture` and
  `caller_supplied_unverified` only. Provenance and the missing trusted-export
  warning survive every statistical projection. No accepted verified flag,
  runtime-attested enum, or arbitrary metadata field exists.
- No production benchmark results are authenticated by P026 until a trusted
  Rust-originated export/integration boundary is implemented.
- Isolation values are supplied claims, never an authentication boundary or
  authority to assign Grade A. Unknown/concurrent isolation remains explicit.
- P024-compatible opaque benchmark ID shapes and Rust Observation ID shape;
  bounded repetition indices and explicit experiment/workload/configuration
  scope; unique run IDs, experiment/repetition pairs, and Observation links.
- Duplicate rows, nonterminal rows, malformed enums, invalid numerical evidence,
  mixed or mismatched configuration, and overlap with the reference are rejected.
- Unresolved links, incomplete/invalid run metadata, and extra unlinked rows
  remain separately accounted for; no fabricated observations or measurements.
- Available UTC timing is validated and canonicalized; absent timing stays
  absent. Stored terminal lifecycle/summary quality never substitutes for
  P018's independent per-meter token/quota eligibility.

## Grouping, statistics, and comparisons

- Exact five-dimensional configuration isolation: plan/model/reasoning/speed/
  Codex version. Unknown differs from known, with no guessed defaults.
- Separate configuration/workload/experiment/meter summaries; distinct
  experiments are never silently pooled, even with matching workload keys.
  Experiment and run counts remain separate.
- Five-hour and weekly candidates/statistics/confidence/comparisons are
  independent. A reset or unavailable target meter does not remove valid
  evidence from the other meter.
- All statistical derivation reuses P018 `derive_capacity_candidates`, P019
  `aggregate_capacity_candidates`, and P020 `assess_capacity_evidence`.
- P019 weights, weighted median, unweighted Type-7 percentiles, pre-filter MAD,
  zero-MAD/outlier policy, and accounting remain authoritative and unchanged.
- P020 HIGH/MEDIUM/LOW/INSUFFICIENT confidence and thresholds are unchanged.
- Comparison requires at least four retained candidates on each side and
  independent P020 MEDIUM/HIGH confidence. Policy can raise this sample minimum.
- Ordinary references have `observational_reference` cohort kind and no
  benchmark experiment/workload identity. Reference statistics are independent
  of benchmark derivation; all benchmark dataset IDs and claimed IDs must be
  disjoint from the reference.
- B=benchmark weighted median, R=reference weighted median:
  absolute difference B−R; relative difference (B−R)/R; relative percentage
  100×(B−R)/R; ratio B/R.
- Material threshold defaults to 0.20; equality is material. It must be chosen
  before analysis. IQR endpoints touching count as overlap; IQRs are not CIs.
- Precedence: integrity errors reject; no reference is unavailable; sample/
  confidence insufficiency precedes temporal unavailability; otherwise below
  threshold is descriptively consistent, material with overlapping IQRs is
  `material_difference_overlapping`, strict separation is
  `material_difference_separated`.
- Supplied suspected/detected boundaries block calibration when either cohort
  or their combined time range crosses a boundary. Missing timestamps with a
  relevant supplied boundary fail conservatively. No P023 absence-of-signal
  result is treated as proof of common provider policy.
- Ordered recommendations suggest collecting evidence, investigating meter/
  workload/temporal differences, or reviewing calibration; no cause or official
  provider change is claimed.

## Numerical safety, privacy, dependencies, and bounds

- Finite positive capacities/percentiles; ordered percentiles; finite
  nonnegative dispersion; P018 nonfinite candidate exclusions preserved;
  impossible aggregation is a typed numerical error; comparison overflow is
  typed unavailable with omitted metrics. No NaN, Infinity, clamping, rounding,
  fabricated zero, or automatic recalibration.
- Frozen/slotted models retain counts, detailed exclusions, outliers, C-quality,
  missing meters, resets, time ranges, provenance, and uncertainty diagnostics.
- Standard library only; runtime `dependencies = []`. No Rust dependencies.
- Default bound: 500 rows per dataset/typed evidence tuple, maximum 100,000;
  experiments have at most 100 repetitions. Oversized input is rejected without
  truncation; P017's bounded reader remains caller-owned.
- No SQLite access/write, provider invocation, network, credentials, current
  clock, randomness, synthetic data generation, logging, or persistence.
- No prompts, responses, reasoning, tool outputs, source/repository contents,
  remotes, account IDs, emails, keys, rollout JSONL, or generic payload fields.
- `schemas/v1/*`, public AnalyticsResult v1, published migration SQL/checksums,
  Rust runtime/execution behavior, and P017–P025 implementation are unchanged.

## Validation and review

- Python: **610 passed**, including the **438** existing baseline tests and
  **172** focused P026 regressions.
- Rust: **254 passed**, preserving the baseline and migration checksum tests.
- Ruff lint and formatting: passed.
- Rust formatting, all-target/all-feature Clippy with warnings denied, full
  workspace tests, and workspace build: passed.
- Contracts: **6 schemas, 16 positive fixtures, 5 negative fixtures**, passed.
- `git diff --check`: passed.
- `just check`: unavailable (`just` is not installed); every direct equivalent
  in the required full quality gate was run.
- Local validation: Python 3.12 on Linux; GitHub CI additionally validates Python
  3.11 and Rust on Ubuntu/macOS/Windows.
- Full diff review checks numerical policy reuse, privacy, scope, reference
  disjointness, workload/experiment separation, provenance warnings, temporal
  guards, and unchanged schemas/migrations/dependencies/Rust code.

## Phase G completion and Phase H deferrals

P026 supplies the internal statistical/reporting layer and completes Phase G.
No official provider quota/policy inference, automatic estimator updates,
provider execution, CLI rendering, dashboard, subprocess protocol, SQLite
migration/write, or P027 work is included.

Exact P027 assumptions are recorded under `Inputs for P027` in
[BENCHMARK_CALIBRATION_ANALYSIS.md](BENCHMARK_CALIBRATION_ANALYSIS.md).
Trusted Rust-to-Python benchmark provenance and transport remain outstanding;
production benchmark execution remains disabled; statistical results are never
automatically applied to the estimator. P027 begins Phase H — CLI and
Runtime/Analytics Integration, and is not authorized here.

## Pull request verification record

- Implementation commit: `857702cf37add219461d0ffbc0e9f2dab798b6dc`
  (`feat: analyze benchmark capacity calibration`).
- PR [#19](https://github.com/nghianguyen150612/Codex-Meter/pull/19):
  `Add benchmark calibration and statistical analysis`, targeting `main`.
- Reviewed the complete six-file GitHub PR diff against the local diff; all
  patch content matched (GitHub omits an optional hunk context label). No
  correctness, scope, privacy, dependency, schema, migration, or execution
  blocker was found.
- GitHub initially reported the head mergeable with no outstanding review
  decision. The branch-protection details endpoint returned HTTP 403 for this
  integration; GitHub's merge API must enforce any required reviews and checks.
  No rule bypass or administrator override is permitted.
- All six CI jobs must pass on the final feature head before the authorized
  `merge_method=merge` attempt. Actual final-head CI conclusions, merge SHA,
  both parents, feature-commit reachability, reviewed-tree identity, and clean
  synchronized `main` are reported in the completion response after GitHub
  produces those facts. This manifest does not invent future merge facts.
