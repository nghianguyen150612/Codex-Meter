# Prompt 020 Manifest

- Starting SHA: `886ca5793d8ebcb6077346f1b7a93aed7513975f`.
- Verified P019 baseline: fetched `origin/main`, checked out `main`, confirmed
  `main == origin/main`, confirmed the expected P019 SHA, and confirmed a
  clean worktree before editing.
- Workflow: direct-to-main only; no feature branch and no pull request.
- Estimator method: reused P019 `ESTIMATOR_METHOD_VERSION = "1.0.0"`.
- Final center: sufficient results use P019's weighted median; P25/P50/P75
  remain P019's unweighted retained percentiles, with P50 intentionally not
  replaced by the weighted center.
- Confidence: high requires used >= 8, relative IQR <= 0.20, no C quality,
  and outlier rate <= 0.20; medium requires used >= 4, relative IQR <= 0.50,
  at least half A/B, and outlier rate <= 0.35; other sufficient results are
  low. Threshold boundaries are inclusive.
- Current position: `CurrentQuotaPosition` is explicit runtime evidence with
  a finite 0–100 percentage-point `used_percent`; historical observations are
  never used as current meter state. Missing current position raises a typed
  error only for sufficient evidence.
- Arithmetic: used is `full * used_percent / 100`; remaining is `full - used`;
  values are finite, unrounded Python floats.
- Configuration: typed result configuration values preserve observed/derived/
  unavailable provenance, omit unavailable `value`, and must match every
  candidate-set `ConfigurationKey` exactly.
- Result status: at least one sufficient meter is `succeeded`; all requested
  meters insufficient or no targets is `insufficient_evidence`; pure P020
  construction raises typed errors rather than manufacturing `failed`.
- Reason mapping: fixed common-schema order, duplicate-free per-meter codes
  and global warning union; reset, unavailable meter, unstable meter,
  incomplete telemetry, C-quality concurrency, and conservative unknown
  mappings are documented in `docs/FINAL_CAPACITY_ESTIMATOR.md`.
- Serialization: immutable result models emit schema version `1.0.0`, only
  `current_capacity_estimate`, constant empirical metric metadata, omitted
  insufficient numerical fields, and deterministic compact JSON.
- Contract/schema tests: generated success, mixed, and all-insufficient result
  objects validate against Draft 2020-12 `analytics-result.schema.json` with
  the repository common-schema registry. Existing result fixtures and schemas
  remain unchanged.
- Request boundary: AnalyticsRequest v1 lacks current meter position; P020
  accepts it as explicit runtime context and defers Rust↔Python subprocess
  versioning. Request `minimum_quality` execution and SQLite reloads remain
  deferred.
- Dependencies: runtime dependencies remain `[]`; `jsonschema` and
  `referencing` remain development/test dependencies only.
- Privacy: result errors expose only safe request, meter, and configuration
  dimension context; no prompts, responses, observations, telemetry payloads,
  credentials, or observation IDs are serialized in AnalyticsResult v1.
- Phase F deferrals: no historical regime splitting, quota-change detection,
  workload estimation, benchmarking, persistence, CLI presentation, or
  Rust↔Python process bridge was added.
- Final SHA handling: final commit SHA, push result, equality with
  `origin/main`, and clean-worktree status are recorded in the completion
  report after the single logical P020 commit; this manifest does not
  self-embed its own commit hash.
- Unrelated-work confirmation: no legitimate newer work was present; P017
  read-only behavior, P018 candidate semantics, P019 robust behavior, Rust
  storage, schemas, and existing contract fixtures were preserved.
