# Quota Acquisition

## Scope

P010 normalizes the rate-limit snapshot already persisted in a Codex rollout:

```text
event_msg → token_count → rate_limits
```

The implementation is evidence-backed only for Codex CLI `0.157.1` and upstream
revision `8f7a0f7a878199c6886600370e5be6bd37ca38a3`. It observes plan-quota
percentages; it does not reinterpret raw tokens, weighted usage, context-window
usage, or quota percentages as tokens.

## Source filtering

The privacy-filtered source model retains only `limit_id`, `primary`,
`secondary`, `plan_type`, and the safe fields of each window:
`used_percent`, `window_minutes`, and `resets_at`. Credits, spend-control
amounts, model aliases, arbitrary JSON, credentials, and account identifiers are
discarded.

The main Codex bucket is a missing `limit_id` or a value equal to `codex`,
matched case-insensitively. An explicit other value, such as
`codex_some_model`, is classified as non-main and cannot produce main quota
samples. P010 does not carry sparse fields forward from earlier snapshots.

## Window classification

`primary` and `secondary` are source slots, not meter names. Each present
window is classified independently by its observed duration, using inclusive
five-percent tolerance:

| Meter | Reference | Accepted duration |
| --- | ---: | ---: |
| `five_hour` | 300 minutes | 285 through 315 minutes |
| `weekly` | 10,080 minutes | 9,576 through 10,584 minutes |

Missing, daily, monthly, yearly, and other durations remain unsupported. If two
windows classify to the same supported meter, P010 reports an ambiguity instead
of choosing a source slot.

## Normalized values

For a supported window, `used_percent` is copied as an observed 0–100 value.
It must be finite and within the inclusive range; malformed values are rejected
at the source-model boundary rather than clamped. `remaining_percent` is
explicitly derived as `100 - used_percent` and is labeled `derived`.

An observed `resets_at` Unix timestamp is converted to an RFC 3339 UTC value
ending in `Z` using the maintained Rust `time` dependency. An absent reset is
represented as unavailable; no reset is inferred from the sample timestamp,
duration, or local clock. Supported samples still succeed without reset data.

Recognized provider plan enum values are preserved exactly, including distinct
workspace and education SKUs. Missing and unknown values are unavailable.
Model, reasoning level, speed mode, and Codex version remain unavailable
because this account-wide evidence does not prove task configuration identity.

All samples use `source_kind: "local_meter"` and
`acquisition_status: "succeeded"` when the supported percentage is valid.

## Identity and replay

Quota sample IDs use the existing length-delimited SHA-256 identity machinery
with the domain `codex-meter/quota-sample/v1`. The digest includes source
rollout identity, source generation, record byte positions, optional ordinal,
source slot, and normalized meter type. IDs are opaque, deterministic, and
independent of wall-clock time. One source record can therefore produce two
distinct samples.

## Assembly and privacy

Ordered assembly preserves the existing token-count snapshot diagnostic output
and additionally emits zero, one, or two `QuotaSample` outputs. Quota samples
do not receive task ownership or task IDs. Unsupported and ambiguous source
classification remains typed normalization evidence, not a fabricated failed
quota sample.

## Limitations

P010 does not calculate quota deltas, detect reset crossings, infer local
windows, reconcile quota evidence with tasks, stabilize meters, finalize
observations, persist SQLite state, or estimate token-equivalent capacity.
Missing resets and sparse snapshots remain unknown by design. Local rollout
evidence is not a complete account-wide usage source.

## Inputs for P011

P011 may assume:

- 5-hour and weekly quota snapshots are normalized independently;
- meter classification is based on duration, not primary/secondary slot;
- `used_percent` is provider-observed;
- `remaining_percent` is derived;
- observed reset timestamps are retained when present;
- missing reset timestamps remain unknown;
- replay sample IDs are deterministic;
- model-specific/non-main limit buckets do not contaminate main quota samples;
- no quota deltas have yet been calculated.

P011 will implement:

**reset/window tracking and reset-safe quota delta semantics.**
