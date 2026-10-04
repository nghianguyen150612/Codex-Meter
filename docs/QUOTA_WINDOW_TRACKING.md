# Quota Window Tracking

P011 tracks normalized quota samples after P010 and before task/quota
reconciliation. It answers whether two samples can be compared as one meter
window before it exposes any percentage-point delta. It never converts raw
tokens, weighted/effective usage, or plan allowance into another quantity.

## Architecture

`QuotaTrackingState` contains two independent `MeterTrackingState` values:
`five_hour` and `weekly`. Each meter retains only normalized quota evidence,
its current `TrackedQuotaWindow`, the latest normalized sample, and deterministic
sample IDs already seen for replay detection. There is no persistence, SQLite,
timer, polling loop, task association, or estimator logic.

The batch API clones the supplied state and returns outcomes plus `next_state`.
An invalid structural input returns an error without exposing a partially
advanced state. The per-meter transition API is shared by both meters, so reset
rules cannot drift between the five-hour and weekly paths.

## Window identity

`TrackedQuotaWindow` explicitly stores the meter type, identity, confidence, and
the first and last observed sample timestamps. Identity is one of:

- `ObservedReset(observed_reset_at)`: provider reset evidence, with `high`
  confidence;
- `LocallyInferred(local_window_id)`: deterministic local evidence, with
  `medium` confidence.

Provider reset timestamps are the strongest current identity evidence. A local
identity uses the domain `codex-meter/quota-window/v1`, the meter type, and the
anchor sample ID in the existing length-delimited SHA-256 identity machinery:

```text
window:<sha256(codex-meter/quota-window/v1, meter type, anchor sample ID)>
```

The identity is deterministic and replay-stable; it is not a provider-issued
identifier. When a locally inferred window later gains compatible observed
reset evidence, the tracker upgrades the identity to observed, keeps its
evidence range, and uses high confidence only after that observed identity is
available. No reset timestamp is invented for local inference.

The typed `QuotaWindowIdentity` support includes optional
`local_window_id`, `evidence_first_sample_at`, and `evidence_last_sample_at`
fields already supported by the common schema. Existing P010 sample output is
unchanged: source samples still contain only their original observed-reset
fields or an unavailable reset identity.

## Same-window rules

Only `SameWindowDelta` contains `delta_percentage_points`, constructed through
a checked non-negative 0–100 type. Deltas use percentage-point semantics: 20 to
27.5 is 7.5 percentage points. Zero is valid. A negative value is never
constructed or emitted.

- The first sample establishes a baseline and emits `BaselineEstablished`.
- Matching observed reset timestamps permit a delta when usage is
  non-decreasing and the known reset boundary has not been reached.
- A locally inferred window permits a delta only while samples are chronological,
  usage is non-decreasing, and the gap is less than the meter's nominal duration.
- Missing reset evidence does not weaken a known prior reset boundary: before
  that boundary, a non-decreasing sample can still produce a delta; at or after
  it, the pair is a reset transition.
- A locally inferred window that later receives an observed reset can continue
  when timing and usage remain compatible, with the identity upgraded.

Nominal durations are fixed from P010's meter classification: five hours is
300 minutes and weekly is 10,080 minutes. P011 does not re-run P010's ±5%
duration classification. A gap at least as long as the nominal duration starts
a new local baseline when there is no stronger observed-reset identity.

## Reset and discontinuity rules

`ResetDetected` contains the before and after samples, the previous and new
window identities, and whether the boundary evidence was observed or locally
inferred. It never contains a delta.

- If the previous observed reset is at or before the current sample timestamp,
  the known boundary wins even when current usage is higher. A `97 → 4` pair
  therefore never becomes `-93`, `4`, `7`, or `103` percentage points.
- A changed observed reset after the old boundary is a reset transition; the
  current observed reset becomes the new identity.
- A changed observed reset before the old boundary is
  `MeterUnstable`, not a confirmed reset. The current sample becomes the next
  baseline so future tracking can recover.
- With missing reset evidence, a usage decrease is an
  `InferredWindowBoundary`; the current sample anchors a new deterministic
  local identity.
- A same-observed-reset usage decrease is `MeterUnstable`, not an ordinary
  reset, because provider identity says both samples are the same window.

The five-hour and weekly states are updated independently. A five-hour reset
cannot invalidate a weekly delta, and vice versa.

## Plan discontinuity

When both adjacent plan values are observed and differ, the outcome is
`PlanDiscontinuity` and no delta is emitted. The current sample becomes the next
baseline. Missing plan evidence is not a plan change: unavailable-to-observed
and observed-to-unavailable transitions may continue through the normal window
rules.

## Replay and ordering

Sample IDs are treated as deterministic replay identities. A repeated ID emits
`DuplicateSample` and leaves state unchanged. A different sample with an older
timestamp emits `OutOfOrderSample` and leaves state unchanged. Equal timestamps
are allowed for distinct samples; they do not create elapsed-time evidence for
a boundary. Ordered replay from the same state produces the same outcomes,
state, local IDs, and deltas without wall-clock or random input.

## Privacy and limitations

Tracker state contains only sample IDs, meter type, timestamps, percentages,
window identity, configuration plan evidence, and the normalized sample fields
allowed by P010. It does not retain raw rollout JSON, account identifiers,
paths, credentials, cookies, authorization material, or monetary balances.

P011 is conservative. A sparse missing-reset sequence may start a new local
window even when a provider reset did not occur, and an unstable meter does not
produce consumption evidence. P011 does not claim provider certainty for local
identities and does not attribute a delta to a task.

## Inputs for P012

P012 may assume:

- five-hour and weekly samples are tracked independently;
- same-window deltas are always non-negative;
- reset-crossing pairs never produce deltas;
- observed reset timestamps are the strongest window identity;
- missing-reset samples can form explicitly locally inferred windows;
- gaps beyond nominal duration do not produce same-window deltas without stronger evidence;
- percentage decreases never produce negative consumption;
- same-observed-window decreases are meter instability;
- cross-plan known changes do not produce deltas;
- deterministic local window IDs and replay behavior exist.

P012 will own:

**task ↔ quota reconciliation, delayed meter updates, retry/stabilization semantics, and before/after evidence selection.**
