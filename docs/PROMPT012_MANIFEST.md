# Prompt 012 Manifest

## Repository state

- Starting SHA: `773d29d822add1a19cd2a24fad69d87bd3acff5d`.
- Verified P011 baseline: local `main` and `origin/main` matched the expected
  P011 commit before editing; the worktree was clean.
- Workflow: direct-to-`main`; no feature branch and no pull request.
- Required commit: `feat: reconcile tasks with delayed quota meters`.
- Final SHA handling: reported after the single P012 commit; it is not embedded
  here to avoid a self-referential commit loop.

## Implementation

- Added `telemetry::quota_reconciliation` for task targets, policy validation,
  baseline selection, transactional evidence application, stabilization, and
  independent meter state.
- Added `docs/QUOTA_RECONCILIATION.md` and the synthetic scenario inventory at
  `fixtures/quota-reconciliation/v1/scenarios.json`.
- Reused P011 `QuotaTrackingOutcome` for all reset, continuity, instability,
  plan, and percentage-point semantics.
- No dependency was added. Existing `time`, `serde`, `serde_json`, and `sha2`
  remain sufficient.

## Policy and state

- `ReconciliationPolicy` requires a validated baseline-age limit, explicit
  sample offsets, stabilization threshold, confirmation count, and deadline.
- `TaskReconciliationTarget` carries only normalized task/session identity and
  lifecycle timestamps; raw upstream turn IDs are not introduced.
- Five-hour and weekly `MeterReconciliation` instances are fully independent.
- Retry actions model scheduling intent only; P012 performs no I/O, sleeping, or
  timers.
- Stable evidence contains immutable before/after samples, P011 window identity,
  reset-safe accumulated percentage-point delta, stabilization timestamp, and
  attempt count.

## Rules and tests

- Before selection uses the latest `sampled_at <= started_at` candidate and
  rejects samples older than `max_before_sample_age`.
- Delayed unchanged reads cannot finalize zero; stable zero requires the same
  configured confirmations as any other candidate.
- Reset crossings, P011 instability, and known plan discontinuities are
  terminal for only the affected meter and never produce a delta.
- Acquisition failures remain missing evidence and can recover within policy.
- Known local overlap is retained as attribution risk without guessed split
  allocation; no overlap is not proof of globally isolated usage.
- Rust tests cover delayed positive updates, stable zero, changing values,
  resets during task/stabilization, independent meters, instability, plan
  discontinuity, failure recovery/timeout, stale baselines, ordering, missing
  lifecycle timestamps, policy validation, replay, and overlap evidence.

## Privacy and deferrals

- No raw provider payloads, provider error bodies, prompts, responses, account
  IDs, credentials, tokens, or arbitrary metadata enter P012 state.
- P012 does not create or finalize `Observation`, modify observation schemas,
  persist SQLite, acquire quota, watch files, sleep, or estimate tokens/capacity.
- External Work/web usage remains an explicit blind spot for P013 quality policy.

## Final SHA handling

The final SHA, push result, `main == origin/main` verification, and worktree
status are recorded in the completion report after publication. The manifest is
not changed after the required commit.

## Unrelated-work confirmation

Changes are limited to task/quota reconciliation, its documentation, synthetic
fixtures, and exports/tests required to expose the P012 internal domain.
