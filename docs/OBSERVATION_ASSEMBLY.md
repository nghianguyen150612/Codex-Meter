# Observation Assembly

P013 assembles provider-independent `Observation` values from P009 attributed
token events and P012 task quota reconciliation. It does not parse rollout
records, acquire quota, poll meters, persist state, or estimate capacity.

## Inputs

`ObservationInput` contains one `TaskQuotaReconciliation`, canonical
`AttributedTokenEvent` values, explicit `TokenTelemetryCompleteness`, an
`IsolationEvidence` quality context, and an optional caller-supplied
`evaluated_at` timestamp. The builder never reads global state or the system
clock. The task ID and optional session ID come directly from the P012 target.

## Identity and timing

The observation ID is a SHA-256, length-delimited identity in the
`codex-meter/observation/v1` domain. It uses the normalized task ID, optional
session ID, and optional task end timestamp. It excludes finalization time,
quota values, prompt content, and random data, so lifecycle progression does
not change the logical observation ID. Timing duration is checked UTC time
arithmetic and is omitted when the task start is unavailable.

## Tokens

Only attributed canonical per-response events are accepted. Events for another
task are rejected. Replay duplicates are keyed by `event_id`; equal duplicate
payloads count once and conflicting payloads return a structural error. Each
metric is summed independently with checked arithmetic. A metric is unavailable
when any contributing event lacks it, and no-event tasks remain unavailable.
`raw_total` is the checked sum of canonical per-response raw totals; it is not
reconstructed from component counters. Overflow is an assembly error.

Complete raw-total evidence is valid only when token telemetry is explicitly
complete. Incomplete streams receive `incomplete / D` and
`telemetry_incomplete`; interrupted streams additionally receive
`process_interrupted`. Optional source capability missingness is not treated as
stream truncation.

## Configuration and plans

One consistent effective task configuration is preserved from attributed token
events. Mixed configurations do not select first, last, or majority evidence;
the serialized identity is conservatively unavailable and estimator-relevant
token evidence is `incomplete / D`. Plan values are merged from consistent
quota sample evidence when task configuration does not prove a plan. Conflicting
observed plans remain unavailable and degrade compatible estimator evidence.
Speed mode is never inferred from quota proximity or `service_tier`.

## Isolation and quality

Quality grades are ordered `A < B < C < D < X`. `ControlledBenchmark` is the
only route to A, and `IsolatedNormalTask` is the only route to B. Possible or
unknown external usage is C and emits `concurrent_usage_possible`. P012
`KnownLocalOverlap` also caps usable evidence at C and emits the same reason;
quota is never divided between tasks. D represents incomplete or delayed
evidence and X represents invalid evidence.

Each domain has independent validity and quality: token evidence, five-hour
quota, and weekly quota do not contaminate one another. Summary quality is the
worst grade across those three statuses; it is never averaged.

## Quota mapping

Observation stores only the reduced quota sample snapshot, not the full
`NormalizedQuotaSample`.

| P012 meter state | Validity | Quality | Reset | Delta | Main reason |
| --- | --- | --- | --- | --- | --- |
| Stable | valid | A/B/C | not_detected | yes | optional concurrent |
| NoBaseline | unavailable | D | unavailable | no | meter_unavailable |
| ResetCrossed | invalid | X | detected | no | quota_reset_crossed |
| MeterUnstable | incomplete | D | unavailable | no | meter_unstable |
| PlanDiscontinuity | invalid | X | unavailable | no | unknown_reason |
| TimedOut | incomplete | D | unavailable | no | meter_unavailable or meter_unstable |
| AcquisitionFailed | incomplete | D | unavailable | no | source_acquisition_failure |
| AwaitingAfterSample | incomplete | D | unavailable | no | meter_unavailable |
| Reconciling | incomplete | D | unavailable | no | meter_unavailable |

Reset-crossed branches preserve safe boundary samples, set
`window_identity` unavailable, and cannot serialize a delta. Stable zero deltas
remain valid and serialize `0`. Acquisition failures never become zero.

## Lifecycle

`Reconciling` has precedence over `AwaitingAfterSample`, which maps to
`awaiting_meter`. Once both meters are terminal, valid complete evidence maps
to `finalized`; missing, unstable, timed-out, failed, or incomplete evidence
maps to `incomplete`. `invalid` is reserved for a globally unusable terminal
observation, currently one with no valid token evidence and both quota domains
invalid. A single reset therefore does not invalidate the whole observation.
Terminal states use the explicit caller timestamp as `timing.finalized_at`;
provisional states never fabricate one.

## Privacy and limitations

Serialized observations contain normalized IDs and measurement evidence only.
They never contain prompts, responses, reasoning text, source code, repository
paths, remotes, account identifiers, credentials, tokens, rollout payloads, or
provider error bodies. P013 does not calculate weighted/effective tokens,
remaining tokens, capacity, or any observation weighting constants.

## Inputs for P014

P014 may assume:

- Observation IDs are deterministic and lifecycle-stable;
- canonical token events are replay-deduplicated;
- task token counters use checked aggregation and raw totals come only from canonical per-response totals;
- mixed configuration cannot silently enter one estimator regime;
- token, five-hour, and weekly validity are independent;
- quality A/B/C/D/X is deterministic from explicit evidence;
- resets invalidate only the affected quota branch;
- incomplete and unavailable meter states retain safe evidence without fabricated deltas;
- concurrency risk is explicit;
- provisional and terminal lifecycle states are serialized deterministically;
- Codex-specific Observation fixtures validate against v1.

P014 begins **Phase D — SQLite storage, migrations, and durable runtime state**.
P014 owns database design and migration bootstrap. P013 does not begin that work.
