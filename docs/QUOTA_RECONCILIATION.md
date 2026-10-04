# Task–Quota Reconciliation

P012 associates one completed local task with independent five-hour and weekly
quota evidence. It consumes normalized quota samples and P011 tracking
outcomes; it does not know provider field names, perform acquisition, sleep,
persist state, calculate token estimates, or create an `Observation`.

## Measurement interval

`TaskReconciliationTarget` contains the privacy-safe normalized task ID, an
optional session ID, and explicit `started_at` and `ended_at` timestamps. A
known end is required. A missing start is retained as `MissingTaskStart` and
prevents a task-specific baseline claim. `ended_at < started_at` is a
structural error; timestamps are never swapped. `TaskEnded` remains distinct
from observation finalization.

## Before selection

Each meter selects independently from samples satisfying:

```text
sample.meter_type == meter
sample.sampled_at <= task.started_at
```

The latest eligible timestamp wins, with `sample_id` as the deterministic tie
breaker. Equality with `started_at` is explicitly eligible. A caller-supplied
`max_before_sample_age` rejects an otherwise latest sample as `TooOld`; P012
does not silently use stale evidence. No baseline is selected from a sample
inside the task interval or after task start.

The accepted baseline is initialized into a private P011 tracker and remains
immutable for the life of the reconciliation. Samples during the task are
still sent through P011 so reset crossings, meter instability, and plan
discontinuity cannot be hidden by choosing a later pair.

## Policy and retry actions

`ReconciliationPolicy` is explicit caller/test input. It contains:

- `max_before_sample_age`;
- strictly increasing, non-negative `post_task_sample_offsets`;
- `stabilization_not_before`;
- positive `required_stable_confirmations`;
- a finite, non-negative `deadline` relative to task end.

Validation rejects an empty schedule, negative durations, duplicate or
decreasing offsets, a deadline before the last schedule offset, zero
confirmations, or a stabilization threshold after the deadline. P012 has no
product retry constants and never sleeps or starts timers.

The pure API evaluates at an explicit timestamp. It emits `AwaitSample`,
`RequestAnotherSample`, `AwaitDeadline`, and `ReconciliationComplete` actions.
The future runtime decides how to execute those intentions. A missing scheduled
opportunity remains missing evidence and does not fabricate a value.

## Delayed updates and stabilization

Only samples with `sampled_at >= ended_at` can become an after sample. An
unchanged immediate read therefore cannot finalize a zero delta by itself.
Eligible post-task samples at or after `stabilization_not_before` form an
explicit `StableCandidate`:

1. the first eligible observation is confirmation one;
2. an exact normalized percentage and compatible P011 window identity increase
   the confirmation count;
3. a value or window change replaces the candidate and resets its count;
4. reaching the configured count accepts the latest confirming sample.

The accepted `after_sample` is the latest sample required to satisfy the
confirmation rule, and `stabilized_at` is its timestamp. Exact normalized
values are compared; P012 introduces no rounding or epsilon. The same logic
accepts a genuine stable zero delta after the configured evidence requirement.

The final delta is accumulated only from P011 `SameWindowDelta` outcomes. P012
never subtracts percentages directly and never constructs a negative delta.

## Reset, instability, and plan behavior

P011 is authoritative for window identity and reset-safe comparison. A P011
`ResetDetected` or `InferredWindowBoundary` becomes terminal `ResetCrossed` for
that meter and retains the baseline, boundary sample, and both window records.
Later post-reset samples cannot rehabilitate that task. A reset during the task
and a reset during post-task stabilization therefore produce no accepted delta.

P011 `MeterUnstable` becomes terminal `MeterUnstable`. A same-reset decrease or
contradictory reset evidence cannot be hidden by waiting for a convenient later
value. A known observed plan change becomes terminal `PlanDiscontinuity`; later
same-plan samples do not repair a cross-plan interval. Missing plan metadata is
not itself contradictory and does not invalidate a pair.

Five-hour and weekly state are independent. A reset, instability, plan change,
acquisition failure, or timeout in one meter does not erase evidence in the
other meter.

## Acquisition failures and deadlines

`QuotaAcquisitionAttempt` is either a normalized sample or a stable,
privacy-safe failure reason. Failure is missing evidence, never `0%` and never
zero usage. Temporary failures remain non-terminal while policy opportunities
remain. Recovery can still stabilize the meter, and the failure remains visible
through the attempt count and reconciliation outputs.

At or after the explicit deadline, unresolved meters become `TimedOut`. When
there were attempts but no successful sample, the typed result is
`AcquisitionFailed`. An insufficient candidate is never promoted merely because
the deadline arrived.

## Attribution risk

The target can carry known locally overlapping task IDs. P012 records
`KnownLocalOverlap` and never divides or proportionally allocates a quota delta.
An empty local overlap set is `NoKnownLocalOverlap`, not proof that Work, web,
another process, or any other external account activity was absent. Final
validity, reason codes, and quality grades belong to P013.

## Deterministic replay and privacy

Inputs are sorted chronologically per meter, with `sample_id` or a stable
failure key breaking equal timestamps. Timestamp arithmetic uses explicit
`time` values and checked overflow. The system clock, randomness, provider
payloads, raw IDs, prompts, responses, account identifiers, credentials, and
backend error bodies do not enter the reconciliation domain. Duplicate sample
and attempt identities do not mutate state. Replaying the same target, policy,
and evidence selects the same baseline, candidate, after sample, stabilization
time, delta, actions, and terminal state.

## Inputs for P013

P013 may assume:

- task start/end measurement interval is explicit;
- baseline samples are selected before task start and age-validated;
- after samples are post-task only;
- samples during the task still participate in reset/continuity detection;
- delayed meter updates cannot be prematurely finalized;
- stable zero deltas are supported;
- retry/stabilization policy is configurable rather than hard-coded;
- reset crossings terminally invalidate only the affected meter reconciliation;
- meter instability and plan discontinuity do not produce deltas;
- acquisition failures remain missing evidence, not zero;
- five-hour and weekly reconciliation are independent;
- stable reconciled evidence contains before/after/delta/window data;
- unresolved reconciliation retains explicit failure/timeout evidence.

P013 will own:

**Observation assembly, per-domain validity/reason codes, quality grades, finalization/incomplete/invalid states, and v1 observation serialization.**
