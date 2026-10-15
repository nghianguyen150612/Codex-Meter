# Prompt 021 Manifest

- Starting verified main SHA: `55c94a2cd44804ff43fac4ebb93d42cf6492eb92`.
  Fetched `origin/main`, verified the supplied baseline and clean worktree, then
  created `feat/p021-historical-capacity-history` from that exact commit.
- Git workflow: follows `docs/DEVELOPMENT.md`; no implementation commits or
  pushes on main. Required implementation message:
  `feat: analyze historical capacity trends`. PR title:
  `Add historical capacity estimates and trend comparisons`, targeting main.
  Merge must use `merge_method=merge`, preserve original feature commits,
  have exactly two parents, and respect required checks/reviews without bypass.
- Public historical API: `codex_meter.analysis.analyze_historical_capacity`,
  `compare_historical_snapshots`, immutable `HistoricalAnalysisPolicy`,
  `HistoricalCapacityAnalysis`, `HistoricalCapacitySeries`,
  `HistoricalCapacitySnapshot`, `HistoricalCapacityChange`, typed errors,
  `TrendDirection`, and `ComparisonUnavailableReason`.
- Windows: one snapshot per anchored terminal row per requested meter. At index
  i only rows through i are eligible for the recent-N window. All anchored
  terminal rows count, including excluded/incomplete/invalid evidence. Eviction
  triggers full recomputation. Both limits reject bool/non-int/out-of-range
  values; hard ceiling 500 total snapshots across requested meters. Dataset
  rows (including unanchored) or total requested snapshots exceeding
  maximum_snapshots fail with P017 bounded-selection guidance, never truncate.
- Timestamps: P017 canonical_time_key validates every provided timestamp,
  preserving nanoseconds. Anchor fallback is finalized_at, ended_at, started_at.
  Canonical key ascending then Observation ID ascending normalizes unordered
  inputs without mutation. Equal-time ID order implies no causal order.
  Duplicate IDs, nonterminal input, invalid timestamps/types/meters fail closed.
- Missing time: all-three-absent rows are counted as unanchored and excluded
  from point-in-time windows; no anchor or clock is fabricated. Nonhistorical
  P017/P018 behavior is unchanged.
- Configuration: all five dimensions compare exactly, including unknown vs
  known Codex versions. Selected configuration must match all rows; empty
  data retains only an explicitly known configuration. No cross-group pooling.
- Meters: five-hour and weekly pipelines and comparisons remain independent,
  including reset handling, quality, candidates, outliers, and confidence.
- P018/P019 reuse: every window constructs an immutable ObservationDataset,
  derives candidates, and robustly aggregates with the existing implementation.
  Full capacity is weighted median; P50 remains unweighted retained median.
- P020 reuse: public pure `assess_capacity_evidence` returns immutable
  `CapacityEvidenceAssessment` (confidence, reasons, relative IQR, outlier rate).
  Both current and historical builders use it. Inclusive HIGH/MEDIUM/LOW policy,
  method version 1.0.0, weights, MAD, and percentiles remain unchanged.
- Snapshots: preserve Observation identity, canonical UTC time, meter,
  configuration, method, confidence, full capacity, P25/P50/P75, sample
  accounting, MAD, relative IQR, outlier rate, and deterministic reasons.
  Insufficient evidence stays explicit with no numerical full capacity or
  percentiles. No current quota percentage or used/remaining estimate fields.
- Trends: only adjacent ordered snapshots with matching meter/configuration/
  method and sufficient estimates get deltas. Absolute = current - previous;
  relative percent = 100 * absolute / previous. Exact numerical direction has
  no significance threshold. Insufficient endpoints and nonfinite arithmetic
  produce explicit unavailable comparisons; gaps are never bridged.
- Tests cover empty/small/large bounded datasets, all directions, ordering and
  fractional precision, prefix invariance, rolling eviction/accounting, both
  reset directions, quality/confidence, exclusions, outliers, missing time,
  configuration identity, invalid input/policy/targets, nonfinite arithmetic,
  deterministic replay, immutable output, no storage access, and P020 behavior.
  Local validation: all 310 Python tests pass (196 existing plus 114 added),
  Ruff lint and format checks pass, and git diff --check passes. Rust format,
  workspace Clippy (all targets/features, warnings denied), all 158 Rust tests,
  and workspace build pass. All 6 schemas, 16 positive fixtures, and 5 negative
  fixtures validate. A detached checkout of the starting commit produced 37
  representative current-builder JSON strings (empty/one/multiple samples,
  A/C quality, confidence levels, outliers, 0/24/100-percent positions, and
  weighted center different from P50); feature output matched byte-for-byte.
- Dependencies: no runtime or development dependency additions; lockfiles,
  schemas/v1, fixtures, migrations, Rust, CLI, and process protocol unchanged.
- Privacy: outputs contain only permitted typed projections and compact
  diagnostics. No raw events, conversational content, paths, account data,
  credentials, network/time/randomness, or SQLite write path is added.
- Documentation: `HISTORICAL_CAPACITY_ANALYSIS.md` records API, chronological and
  statistical semantics, numerical limitations, privacy, complexity, and exact
  P022 assumptions. Complexity is approximately O(m n w log w), n/w <= 500.
- Deferrals: P022 remaining-workload prediction and P023 metering/quota-change
  detection are explicitly not implemented. No historical estimate claims an
  official provider allowance or confirms a regime change.
- PR/merge results and final commit SHA handling: actual PR URL, latest-feature
  CI results, diff review, merge SHA/parents, original-commit reachability, and
  final main/worktree verification belong in the completion report and PR.
  This manifest does not self-embed its own commit SHA or predict a merge SHA.
