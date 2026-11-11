# P043 — Observation evidence integrity and lifecycle completion

## Scope and starting point

P043 audits and hardens the existing P031 `codex-meter sync` recorder. It does
not introduce a second recorder, poll a provider, or change the P017 Observation
storage contract. The branch starts at verified P042 HEAD
`9eb79dd3d032f3828105894816aa8a088f728ff1` and is stacked on
`feat/p042-forecast-readiness`.

The path remains local and transactional:

```text
supported rollout records → P009 assembly → P012 reconciliation →
P013 Observation → P016 SQLite commit with evidence and source checkpoint
```

## Evidence invariants

- Task and session IDs are normalized opaque IDs derived from observed rollout
  identifiers. A missing session remains unavailable; a task hash does not
  establish that the session is known. The Observation source instance comes
  from the current source generation even when token evidence is absent.
- A start time exists only when a validated task-start event was read. The
  first observed timestamp is checkpointed; repeated starts do not replace it.
  No clock-derived start or end is created. A start after the terminal event is
  omitted from Observation timing.
- A task terminal marker establishes only that a terminal event was observed.
  Completion, failure, and abort remain distinct in the typed P009 checkpoint
  state. Duplicate same-outcome and conflicting terminal evidence are
  anomalies. A generic EOF is not terminal evidence.
- The pinned rollout format has no validated guarantee that every response
  event for a task has been flushed, read, and attributed. Therefore the
  recorder marks all newly generated token evidence incomplete, including
  completed tasks with a full set of currently visible raw totals. Observed
  raw totals remain available where present; they are never reconstructed or
  filled with zero. Snapshots are not additive consumption.
- The first observed canonical per-response row for each event ID contributes
  at most once through the existing storage identity/checksum path. Conflicting
  duplicate evidence fails the batch. Out-of-order source records and
  post-terminal token events do not upgrade completeness.
- Effective model, reasoning level, and Codex version come only from observed
  rollout provenance. Plan and speed mode remain unavailable unless directly
  evidenced by their existing validated source paths. `service_tier` is not
  treated as speed mode. Conflicts do not select one configuration or combine
  cohorts.
- External usage is unknown for this local source. Quality cannot earn A or B
  from local-task uniqueness. Token, five-hour, and weekly statuses remain
  independent; summary quality remains the worst domain grade.
- P031 quota snapshots are account meter samples, not task consumption. The
  recorder currently passes no samples into P012 reconciliation; resulting
  meter evidence is unavailable and no quota delta is generated. No nearby
  sample, inferred reset, provider poll, or causal estimate is substituted.
- Each newly observed terminal task also records the existing sanitized
  `data_coverage_incomplete` and `quota_evidence_incomplete` diagnostics. The
  `sync` summary reports both gaps when it processes terminal task evidence;
  readiness includes the persisted diagnostics in its coverage report.
- Observation finalization and capacity eligibility are separate. P016
  terminal rows are immutable, and P022 accepts only token-valid rows with its
  existing quality and positive-raw-total requirements. P041/P042 readiness
  therefore does not improve merely because a terminal Observation exists.

## Lifecycle and recovery

Start and first-terminal timestamps are stored in runtime checkpoint format 2.
The format-2 reader accepts format-1 payloads and defaults these fields to
unavailable. The source cursor, token/quota evidence, sanitized diagnostics,
Observation, and updated checkpoint continue to commit through
`commit_usage_batch()` in one bounded SQLite transaction. A rollback leaves the
prior cursor available for deterministic replay. Source replacement,
truncation, ambiguous generations, and checkpoint conflicts retain their
existing fail-closed behavior.

The first terminal Observation remains immutable. Later records cannot silently
upgrade its token completeness or change its quality. A corrected historical
interpretation would require a separately versioned, dry-run corrective policy;
P043 does not rewrite old rows. P017 v1 does not encode whether its task ended
successfully, failed, or aborted, or persist the full typed lifecycle anomaly
set. P043 retains those details in checkpoint state and emits sanitized
coverage diagnostics, but changing the Observation schema is deferred.

Diagnostics use fixed codes and hashed source/event identities. They do not
include rollout JSON, paths, prompts, responses, source content, account data,
or credentials.

## Supported source evidence and limits

The implementation remains restricted to the already audited plain rollout
format and pinned Codex compatibility policy. Canonical per-response
`payload.usage` is additive token evidence. Turn/thread cumulative snapshots
and token-count snapshots remain separate. Supported terminal task events are
observed lifecycle markers, not a guarantee of token stream closure. Unsupported
records, partial JSONL tails, source boundaries, and future appends remain
coverage limits; they cannot establish completeness. No unaudited runtime
version is accepted.

The empty-sample P012 call was audited: it cannot select a baseline or an
after-sample and yields unavailable meter states. Reusing historical P031
snapshots would require a verified same-window pair, valid ordering and reset
boundary, plan continuity, stability evidence, and acceptable overlap risk.
The current recorder cannot prove those task-specific conditions, so P043 does
not attach the snapshots.

## Forecasting and readiness

New recorder rows retain directly observed tokens and real lifecycle timestamps
but have incomplete token validity and unavailable quota evidence. P022
excludes them under the existing token-validity requirement. P041's method and
eligibility are unchanged. P042 readiness continues to report insufficient
history or missing capacity inputs as appropriate; it does not treat row
existence as readiness. Existing P039 task totals are unchanged.

## Validation and qualification

The deterministic lifecycle fixtures cover a completion without a start, a
normal start timestamp, duplicate terminal evidence, and start-after-terminal
ordering. The CLI sync fixture asserts that a completion with a real start
preserves both timestamps, preserves the directly observed raw total, records
token validity as incomplete, and leaves both quota meters unavailable. It also
checks fixed coverage diagnostics, readiness reporting, and idempotence across
a repeated terminal event. Exact sync replay adds no second Observation. Local
validation passed:

- `cargo fmt --all -- --check` and workspace/all-target/all-feature Clippy with
  warnings denied.
- Rust workspace/all-target tests: 535 passed; all-feature mode: 526 passed.
- Python tests: 677 passed; Ruff lint and format: passed.
- Contract validation: 8 schemas, 19 positive fixtures, 7 negative fixtures.
- Installed-worker runtime smoke: passed, including source and installed modes,
  estimate/workload/readiness, SQLite read-only behavior, and replay checks.
- Linux interactive controlling-terminal PTY resize smoke: passed.

Final-head cross-platform CI and release-candidate validation are not asserted
until the pushed commit's GitHub workflows finish.

Local compilation and synthetic tests establish only the pinned fixture
behavior. They do not establish current Codex rollout semantics or physical
device restart behavior. P038 G4/G5 remain UNVERIFIED; no live Codex task,
provider API, or physical device qualification was performed for P043.

## Next evidence gap

P044 should focus on a trustworthy source-coverage signal for per-task canonical
response streams and a task-specific quota evidence workflow that can satisfy
P012's existing baseline, same-window, reset, ordering, plan-continuity,
stability, and overlap checks. It should also decide whether a new versioned
Observation contract is needed to persist terminal outcome and lifecycle
anomaly details. No token completeness, quota delta, or isolation grade should
be promoted until those evidence guarantees are demonstrated.
