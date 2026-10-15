# Prompt 022 Manifest

- Starting main SHA: `dc37e9e40610e78dd01707c2f0284d6a5d20f117`.
- Expected P021 merge baseline: verified fetched `origin/main` at the starting
  SHA, with a clean worktree before implementation.
- Feature branch: `feat/p022-remaining-workload-prediction`.
- Required implementation commit: `feat: predict remaining workload from task history`.
- Required PR title: `Estimate remaining workload from historical task sizes`,
  targeting `main`.
- Git workflow: follows `docs/DEVELOPMENT.md`; implementation commits remain on
  the feature branch. The PR must pass the complete local gate and required
  GitHub checks, receive a diff review, and merge using Create a merge commit
  (`merge_method=merge`). Post-merge verification checks both parents, feature
  commit reachability, `main == origin/main`, and a clean worktree. Final PR,
  CI, and merge verification outcomes are filled in after those events; no
  future merge SHA is guessed.
- Task-size policy: terminal rows only; eligibility uses valid token telemetry,
  A/B/C token quality, and a positive integer raw total. Quota-meter validity,
  reset crossings, quota deltas, and summary quality do not gate task-size
  samples. D/X quality, invalid token validity, unavailable totals, and zero
  totals retain distinct, potentially multiple per-row reasons.
- Sample accounting: each terminal Observation receives one deterministic
  decision; candidate rows equal eligible samples plus excluded rows. The
  P017 reader safety limit (100,000 observations) bounds manually supplied data.
- Distribution: at least three eligible tasks are required. P25/P50/P75 use
  unweighted Type-7 interpolation over raw integer task sizes. A/B/C are
  categorical; no P019 weights or P019 capacity-outlier removal is applied.
- Scenarios: light=P25, normal=P50, heavy=P75, relative task-size labels only;
  the current Observation model provides no semantic task-type classification.
- Capacity inputs: only the matching P020 `AnalyticsResultV1` per-meter
  `estimated_remaining_raw_tokens`; no quota or remaining capacity is inferred
  from P017/P021 history. P020 confidence, reason codes, provider authority,
  request identity, and timestamp are preserved.
- Formula: `floor(estimated_remaining_raw_tokens / scenario_raw_tokens)`;
  zero remaining produces three zero counts, unavailable evidence uses `None`,
  and task counts must be monotone from light to heavy.
- Configuration: exact five-dimension match for plan, model, reasoning level,
  speed mode, and Codex version, including unknown vs known values.
- Insufficiency: task history (<3 samples) and each meter's P020 capacity are
  independent; both-insufficient is distinguishable. Capacity confidence is
  never upgraded by task history, and one meter cannot substitute for another.
- Numerical safeguards: finite positive quantiles, finite non-negative
  remaining capacity, explicit flooring, and a maximum safe count of
  `9_007_199_254_740_991`; out-of-range math fails with a typed error.
- Runtime dependencies: `dependencies = []`; Python standard library only.
- Local quality gate: all 371 Python tests pass, including 61 new workload
  regressions; Python Ruff lint and format pass; all 158 Rust tests pass; Rust
  formatting and workspace Clippy with warnings denied pass; the workspace
  build passes; contract validation passes for 6 schemas, 16 positive fixtures,
  and 5 negative fixtures; `git diff --check` passes.
- Scope review: no schema, fixture, migration, Python runtime dependency,
  SQLite write path, current-quota inference, CLI integration, or P023 change.
- Privacy: internal typed projections only; no prompts, responses, reasoning,
  source code, repository paths, account data, credentials, rollout payloads,
  or derived-data persistence.
- Documentation: `docs/REMAINING_WORKLOAD_PREDICTION.md` records method,
  eligibility, quantiles, scenarios, limitations, privacy, and P023 assumptions.
- P023 deferrals: no metering-regime or quota-change detection was started.
- Python/Rust test counts, contract counts, CI results, PR URL, merge commit
  parents, and post-merge verification are recorded in the completion report
  after their results are available.
