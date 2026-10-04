# Prompt 011 Manifest

## Repository state

- Starting SHA: `6f5dd1a3a65021285786f496165b2caf40814fdf`.
- Verified P010 baseline: local `main` was fast-forwarded to `origin/main` at
  the expected P010 merge baseline before editing; the worktree was clean.
- Workflow: direct-to-`main`; no feature branch and no pull request.
- Required commit: `feat: track quota windows and reset-safe deltas`.
- Final SHA handling: the final commit SHA is reported in the completion
  response rather than embedded here, avoiding a self-referential commit loop.

## Implementation

- Created `telemetry::quota_tracking` with independent five-hour and weekly
  tracker state, transactional batch advancement, and per-meter transitions.
- Added explicit `TrackedQuotaWindow` and `TrackedWindowIdentity` models.
- Added typed `NonNegativePercentagePoints`; only `SameWindowDelta` contains a
  delta.
- Extended typed quota window identity support for local IDs and evidence ranges
  already permitted by the common schema; schema version remains `1.0.0`.
- Added the deterministic identity domain `codex-meter/quota-window/v1`.
- Reused the existing `sha2` identity implementation and `time` dependency;
  enabled its existing parsing feature. No new dependency was added.

## Rules

- Observed reset identity uses meter type plus provider reset timestamp and has
  high confidence.
- Locally inferred identity is anchored by meter type plus sample ID and has
  medium confidence; no synthetic reset timestamp is created.
- Known reset boundaries override apparent monotonic usage.
- Same-reset decreases are meter instability; changed resets before the old
  boundary are identity instability; missing-reset decreases and long gaps are
  locally inferred boundaries.
- Known observed plan changes suppress deltas; missing plan evidence does not
  prove a plan change.
- Zero deltas are valid; negative deltas are structurally unrepresentable.
- Duplicate IDs and out-of-order samples do not mutate state.

## Synthetic sequences and tests

- Added `fixtures/quota-tracking/v1/sequences.json` covering same observed
  windows, reset crossing, `97 → 4`, same-reset decrease, reset correction,
  missing-reset monotonic/decrease/long-gap sequences, independent meters, and
  known plan change.
- Rust tests cover positive and zero deltas, canonical reset regression, known
  boundary precedence, local inference and fixed local-ID regression, plan
  discontinuity, replay, ordering, transactionality, and meter independence.
- Existing P008/P009 identity tests and P010 sample serialization tests remain
  passing.

## Privacy and explicit deferrals

- State retains normalized quota evidence only; no raw rollout payload,
  credentials, account data, paths, cookies, authorization data, or monetary
  balances are retained.
- Deferred: task/quota attribution, before/after selection, delayed polling,
  stabilization timing, concurrent-usage classification, observation lifecycle,
  SQLite, migrations, estimator, benchmarks, and CLI/live UI.

## Validation and publication

- Tests and the complete quality gate are run before publication; exact results
  are recorded in the completion response.
- Push result and final `main == origin/main` verification are recorded in the
  completion response after the single logical commit.
- Unrelated-work confirmation: changes are limited to P011 tracker code,
  normalized typed support, synthetic quota fixtures, documentation, and the
  existing dependency feature flag needed for timestamp parsing.
