# Prompt 020A Manifest

- Starting verified main: `a3e60b2f361f8775ca78bc7334e1f1c6b8c5f9b6`.
- Baseline: cloned the repository, verified `main == origin/main` at the supplied
  P020 SHA, and confirmed a clean worktree before creating the feature branch.
- Feature branch: `fix/p020a-final-estimate-invariants`.
- Implementation commit message: `fix: enforce final estimate model invariants`.
- PR title: `Fix final estimate model invariants`; base: `main`.
- Workflow: implementation commits belong only to the feature branch. The PR
  must pass the full quality gate and available GitHub Actions checks, receive
  a diff review, and merge using `merge_method=merge`. Required checks or reviews
  must never be bypassed. Verify two merge parents, preservation of original
  feature SHAs in main history, `main == origin/main`, and a clean worktree.
- Permanent policy: added `Git Workflow Policy` to `docs/DEVELOPMENT.md` while
  preserving existing development instructions. Previous prompt manifests and
  completed Git history remain unchanged.
- `QuotaCapacityEstimate`: insufficient confidence requires fewer than two
  retained samples, no numerical capacity/percentile values, and at least one
  typed reason. Sufficient confidence requires at least two retained samples,
  positive finite full capacity and percentiles, non-negative finite used and
  remaining capacity, an ordered percentile triple, and a used/remaining sum
  matching full capacity with relative tolerance `1e-12` and zero absolute
  tolerance. Boolean numbers are rejected; valid negative zero is normalized.
  Unrepresentably large integers also fail with `EstimatorInvariantError`.
- Center independence: the quality-weighted full-capacity center may differ
  from P50 and lie outside P25–P75. Confidence labels are not recalculated by
  direct constructors.
- `AnalyticsResultV1`: supplied warnings must exactly match the per-meter reason
  union, with no missing, extra, or duplicate warning. Empty results require
  exactly `unknown_reason`. Existing common-schema reason ordering, unique
  meter identities, and result-status rules remain enforced. Failed results
  are not fabricated.
- Regression coverage: direct valid and invalid construction, sample-count
  boundaries, all numerical fields, percentile order/positivity, invalid reasons,
  zero/full usage, negative zero, sum tolerance, center independence, warning
  completeness/deduplication/order, empty results, meter identity/status rules,
  and unchanged builder JSON serialization. Existing P017–P020 tests remain.
- Scope: changed only the final result model, its tests, estimator documentation,
  development workflow documentation, and this manifest. The builder algorithm,
  estimator method `1.0.0`, confidence thresholds, A/B/C weights, MAD and weighted
  median policies, schemas, fixtures, dependencies, Python read-only history,
  Rust code, and process protocol are unchanged.
- Local validation: `ruff check .` and `ruff format --check .` passed; all 196
  Python tests passed (123 result tests). Rust formatting, workspace Clippy
  with all targets/features and warnings denied, all 158 Rust tests, and the
  workspace build passed using `rust/Cargo.toml`. Contract validation passed
  for 6 schemas, 16 positive fixtures, and 5 negative fixtures. `git diff --check`
  passed. All commands used the committed Python lockfile and existing gate.
- Builder compatibility: compared 32 serialized results against the starting
  P020 commit, including zero/one/multiple samples, A/C quality, 0/24/100-percent
  usage, a weighted center different from P50, and an empty result. All outputs
  matched byte-for-byte.
- PR/merge identifiers, available GitHub Actions results, diff-review outcome,
  and final history/worktree verification are recorded in the completion report;
  this manifest does not self-embed its commit SHA.
- Remaining boundaries: current quota usage remains explicit caller input;
  AnalyticsRequest v1 current-position transport and process integration remain
  deferred. P021 is not authorized and was not started.
