# Prompt 041 Manifest — Workload Forecast CLI

## Baseline and stack

- Repository: `nghianguyen150612/Codex-Meter`.
- Starting verified P040 SHA: `f7385427fa0296efe40381e27352ea7815e71386`.
- Feature branch: `feat/p041-workload-forecast-cli`, based directly on
  `feat/p040-task-usage-tui`.
- Required PR base: `feat/p040-task-usage-tui`; do not target `main` while the
  dependency chain remains unmerged.
- Baseline audit: PRs #33, #34, and #35 were open Draft PRs with bases `main`,
  `feat/p038-windows-execution-reliability`, and
  `feat/p039-task-token-attribution`, respectively. Each branch was an
  ancestor of the next, and the supplied P040 SHA matched the remote head.
- Inherited P038 G4 (Windows installed Codex) and G5 (physical Arch reference
  device) remain unverified and deferred by the user.

## Integration contract

- Rust accepts `codex-meter workload` using the existing closed
  AnalyticsRequest v1 file and unchanged explicit generated timestamp and
  optional manual meter positions.
- The subprocess selects the explicit `workload` operation. The default
  `estimate` operation and AnalyticsRequest v1 / AnalyticsResult v1 contracts
  remain unchanged.
- Python reads one bounded, checksummed, terminal P017 ObservationDataset
  through the existing read-only SQLite reader. It passes that same selected
  dataset to the existing P020 capacity candidate and result constructors and
  to `predict_remaining_workload` from P022.
- No P020 capacity or P022 workload statistics are reimplemented. Five-hour
  and weekly estimates remain independent.
- P039 per-task usage evidence is not converted into P022 samples. A P039-only
  database may correctly return `insufficient_evidence`.

## Result and evidence policy

- WorkloadResult v1 is defined by `schemas/v1/workload-result.schema.json`.
  Rust validates the versioned outer result and the embedded AnalyticsResult v1
  against the original request before JSON or human output is returned.
- Overall result status is `succeeded` only when at least one meter forecast is
  available; otherwise it is `insufficient_evidence`. Technical and integrity
  failures remain nonzero categorized CLI failures.
- P022's minimum of three eligible terminal Observations, A/B/C token quality,
  valid token evidence, positive integer raw totals, exact five-dimension
  configuration matching, Type-7 P25/P50/P75, floor counts, safe integer bound,
  and typed insufficient states are preserved.
- Unavailable quantiles, capacities, and counts are null. Human output shows
  sample accounting, exclusion reasons, configuration and provenance, meter
  confidence, insufficiency state, and an empirical unofficial warning.
- No freshness threshold is introduced. The result reports supplied generation
  time and P020 provenance but does not claim that history is contemporaneous.

## Privacy and reliability

The CLI does not launch Codex App Server, create or resume turns, contact OpenAI,
write SQLite, or persist derived forecasts. The bounded worker keeps existing
interpreter isolation, process cleanup, output limits, fixed success marker,
installed-worker integrity checks, and sanitized diagnostics. Output contains
only validated aggregate data, typed reason codes, and opaque Observation IDs.
Prompts, responses, reasoning, source code, repository paths, credentials,
account details, and raw rollout payloads are not emitted.

## Local validation

Exact local results are recorded in the P041 delivery report and current-head
CI checks after the implementation commit. Required commands include Rust
formatting, all-feature Clippy with warnings denied, both workspace test modes,
Python tests/Ruff, contract fixtures, installed-worker mode, and release
candidate validation. Cross-platform jobs must qualify Linux, Windows, macOS
Intel, and macOS ARM64 on the final PR head.

## Deferred release gates and merge state

P041 engineering qualification does not satisfy P038 G4/G5. Keep PRs #33–#35
and P041 unmerged in the stacked dependency chain until dependencies and
release conditions are met. The final implementation SHA, P041 PR URL, CI run
URLs, release-candidate status, and worktree state are recorded in the final
P041 report after push; this manifest does not claim device qualification.
