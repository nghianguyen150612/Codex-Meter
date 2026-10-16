# Prompt 023 Manifest

- Starting verified `main` SHA: `d8ded331dda3665027e76a48a1e20532a7af469d`.
- Verification: fetched `origin/main`, confirmed both `main` and
  `origin/main` matched the supplied starting SHA, and confirmed a clean
  worktree before implementation.
- Feature branch: `feat/p023-metering-regime-detection`, created from the
  verified starting `main`.
- Required implementation commit message:
  `feat: detect historical metering regime signals`.
- Required PR title: `Detect historical metering-regime change signals`,
  targeting `main`.
- Git workflow: follows `docs/DEVELOPMENT.md`; no implementation commit or
  push on `main`. The PR must pass the complete local gate and available
  GitHub Actions checks, receive a complete diff review, and merge with
  `merge_method=merge`. Required checks and reviews must not be bypassed.
  Verified PR, feature commit, merge parents, and final `main` identifiers will
  be recorded after those remote events occur; no future SHA is asserted here.

## Detection method

- `REGIME_SIGNAL_METHOD_VERSION`: `1.0.0`; independent of P019/P020 estimator
  version `1.0.0` and P022 workload version `1.0.0`.
- Default cohort size: 20 chronologically ordered anchored terminal
  Observations, in consecutive non-overlapping groups.
- Default minimum P019 retained/used samples: 8 in each compared cohort.
- Default effect threshold: inclusive `abs(relative_change) >= 0.25`.
- P019 weighted median is the primary capacity center. P019 retained unweighted
  P25/P50/P75, MAD, outlier filtering, quality weights, and accounting remain
  unchanged.
- Both compared complete cohorts require P020 `medium` or `high` confidence.
- IQR intervals must be strictly separated in the direction of change. Touching
  or overlapping intervals produce `ambiguous_variation` after a material
  effect. The IQRs are descriptive distributions, not confidence intervals.
- Status precedence: `insufficient_evidence`, `no_detected_shift`,
  `ambiguous_variation`, then `shift_signal` when all conditions pass.
- Comparisons are only between adjacent complete cohorts. Shared cohort
  evidence is not treated as independent confirmation; no p-values or provider
  change probabilities are reported.

## Data, safety, and limits

- Chronology reuses P021 validation and P017 canonical timestamps. Time anchor
  order is `finalized_at`, `ended_at`, `started_at`; sort order is canonical
  time ascending then Observation ID ascending. Equal-time ID order is only a
  deterministic tie-breaker.
- Unanchored terminal rows are counted separately and never assigned to a
  cohort. A final partial cohort retains IDs, count, and time boundaries, with
  no fabricated statistics and no comparison.
- Exact configuration isolation covers plan, model, reasoning level, speed
  mode, and Codex version, including unknown-versus-known distinctions and
  selected-configuration validation.
- Five-hour and weekly pipelines, candidates, confidence, reset effects, and
  comparisons are independent. A reset invalidates only the affected meter's
  candidate; it is not evidence of an allowance change.
- Default maximum input size: 500; hard policy bound: 100,000, matching the
  P017 reader safety ceiling. Oversized manually constructed datasets fail
  without truncation.
- Numerical capacity/IQR/delta values must be finite and positive where
  required. Invalid P018 projections raise a typed numerical error;
  non-finite comparison arithmetic returns an explicit unavailable comparison.
  No NaN, infinity, clamping, or substitute constants are emitted.
- Workload composition, cache behavior, concurrent use, telemetry changes,
  deployment differences, meter timing, and measurement noise can confound a
  signal. Adjacent-cohort v1 can miss gradual drift and is not a
  multiple-testing-controlled hypothesis test.
- Output is observational and unofficial. It does not verify an undocumented
  provider quota-policy change or claim a precise provider change timestamp.

## Implementation and verification

- Public API: `codex_meter.analysis.detect_metering_regime_signals`, with
  immutable `RegimeDetectionPolicy`, `RegimeCohortSummary`,
  `RegimeChangeComparison`, `MeterRegimeAnalysis`, and
  `MeteringRegimeAnalysis` models, typed statuses, reasons, and exceptions.
- P021's chronology/configuration validation is reused. Complete cohorts are
  recomputed through P018, P019, and P020 per target meter.
- Runtime dependencies remain `dependencies = []`; Python standard library
  only.
- No schemas, fixtures, migrations, SQLite writes, CLI, dashboard,
  Rust-to-Python subprocess contract, current-capacity transport, or P024/Phase
  G implementation is included.
- New regression tests cover input integrity, policy bounds, time ordering,
  disjoint membership, partial tails, prefix stability, P018–P020 reuse,
  confidence/sample gates, thresholds, IQR separation/touching, quality,
  resets, outliers, numerical overflow, meter independence, deterministic
  replay, immutability, and absence of SQLite access.
- Local full quality gate: Ruff lint and format checks passed; all 438 Python
  tests passed (67 P023 regressions); Rust formatting and workspace Clippy
  with all targets/features and warnings denied passed; all 158 Rust tests
  passed; the workspace build passed; contract validation passed for 6
  schemas, 16 positive fixtures, and 5 negative fixtures; `git diff --check`
  passed.
- Diff review: the seven changed files are limited to P023 analysis models and
  detection, a small P021 chronology-helper parameter for reader-compatible
  input bounds, regression tests, and the P023 documentation/manifest. No
  public schema, fixture, migration, Python dependency, Rust source, SQLite
  write path, CLI, or Phase G implementation changed.
- Documentation review confirms an explicit observational uncertainty
  statement and the P024 assumptions. PR/Actions status, merge method and
  parents, feature SHA reachability, and final clean `main == origin/main`
  verification will be filled from actual GitHub results before completion.

## Phase G handoff

P024 may assume:

- P017 loads read-only historical Observations.
- P018 derives meter-specific capacity candidates.
- P019 performs robust aggregation.
- P020 supplies confidence-aware empirical capacity.
- P021 produces chronological historical trends.
- P022 predicts remaining tasks from raw task-size quantiles.
- P023 identifies conservative possible metering-regime shift signals from
  non-overlapping historical cohorts.
- Regime signals remain observational and unofficial.
- Benchmark experiments are needed to improve calibration and distinguish
  measurement variability from actual environmental changes.

P024 begins **Phase G — Controlled Benchmark Framework**. No P024 work is
authorized or started by this manifest.
