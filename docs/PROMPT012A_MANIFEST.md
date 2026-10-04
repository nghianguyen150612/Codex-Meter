# Prompt 012A Manifest

## Repository state

- Starting SHA: `e5f662102bf8c18fac5d7bd09c37f2508d18d4df`.
- Verified `main` includes the accepted P012 commit and the worktree was clean
  before editing.
- Workflow: direct-to-`main`; no feature branch and no pull request.
- Required corrective commit: `fix: finalize no-baseline reconciliation states`.
- Final SHA handling: reported after the single corrective commit; it is not
  embedded here to avoid a self-referential commit loop.

## Review finding and correction

P012 treated `NoBaseline` as evidence but omitted it from
`MeterReconciliation::is_terminal()`. Because post-task samples cannot create a
valid pre-task baseline, that state is resolved for the current attempt and
must not wait for a deadline. P012A includes `NoBaseline` in the resolved
terminal-state predicate without changing its reason or evidence.

Resolved meters are now:

```text
NoBaseline | Stable | ResetCrossed | MeterUnstable |
PlanDiscontinuity | TimedOut | AcquisitionFailed
```

`AwaitingAfterSample` and `Reconciling` remain unresolved. Actions skip
`NoBaseline`, and `ReconciliationComplete` is emitted when both independent
meters are resolved.

## Tests and files

- Modified `rust/crates/codex-meter/src/telemetry/quota_reconciliation.rs` with
  the terminal-state fix and regressions for both missing baselines,
  no-baseline plus stable, no-baseline plus reconciling, stale baselines, and
  missing task starts.
- Modified `docs/QUOTA_RECONCILIATION.md` with the resolved-state and completion
  semantics.
- Created this manifest.
- No dependency, schema, fixture, SQLite, observation, or P013 implementation
  was added.

## Validation and publication

- Full Rust, Python, contract, formatting, lint, build, test, and diff checks
  are run before publication.
- P013 was not started.
- Push result, final SHA, `main == origin/main`, and clean worktree status are
  recorded in the completion report.

## Unrelated-work confirmation

The correction is limited to P012 resolved-state semantics, regression tests,
and the minimal reconciliation documentation update.
