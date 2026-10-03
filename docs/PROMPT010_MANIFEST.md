# Prompt 010 Manifest

## Repository state

- Starting SHA: `bc02f65402e3e6ed58246802e80831d68be7660b`
- Verified P009 baseline: `origin/main` and expected merge commit matched the starting SHA on October 3, 2026.
- Feature branch: `codex/p010-local-quota-acquisition`
- Required commit: `feat: normalize Codex quota meter snapshots`
- Final SHA handling: reported after the single P010 commit; the manifest does not embed a self-referential hash because changing the file would change that hash.
- Commit count: one logical P010 commit.

## Pinned evidence

- Codex CLI: `0.157.1`
- Official `openai/codex` revision: `8f7a0f7a878199c6886600370e5be6bd37ca38a3`
- Source path: `event_msg` → `token_count` → `rate_limits`
- Safe source structures: `limit_id`, `primary`, `secondary`, `plan_type`; window `used_percent`, `window_minutes`, `resets_at`.

## Implementation

- Added narrow privacy-filtered rate-limit source structures; monetary credits and spend-control values are not retained.
- Added `telemetry::quota_normalization::normalize_quota_item` with typed outcomes for no evidence, non-main limits, unsupported windows, ambiguity, and samples.
- Main-limit rule: missing `limit_id` or case-insensitive `codex`; explicit other IDs are excluded.
- Window rule: inclusive ±5% duration tolerance, `285..=315` minutes for `five_hour` and `9576..=10584` for `weekly`.
- Plan mapping preserves the pinned provider enum vocabulary; missing/unknown plans are unavailable.
- Added maintained `time` dependency `0.3` for checked Unix-seconds to RFC 3339 UTC conversion.
- Added deterministic SHA-256 sample IDs under `codex-meter/quota-sample/v1`.
- Assembly emits quota samples in source order without changing token snapshot behavior.

## Fixtures and validation

- Source fixtures: normal, reversed slots, missing reset, missing plan, non-main ID, unsupported duration, missing duration, 0%, 100%, invalid below 0, invalid above 100, and duplicate-meter ambiguity.
- Contract fixtures: `quota-sample-codex-five-hour.json` and `quota-sample-codex-weekly.json`.
- Contract inventory target: 13 positive fixtures and 5 negative fixtures.
- Tests cover boundary inclusivity, reversed slots, missing reset, exact reset conversion, invalid percentages, non-main filtering, unknown plans, duplicate meters, replay identity, and ordered assembly.

## Privacy and deferrals

- No raw rate-limit JSON, credentials, account IDs, emails, cookies, credit balances, monetary strings, or arbitrary backend payloads are persisted.
- Quota samples do not attach model, reasoning, speed, Codex version, or task IDs.
- Deferred: quota deltas, reset crossing detection, local-window inference, stabilization/retries, task reconciliation, observation finalization, SQLite, estimator, benchmarks, live CLI, and final status.

## Handoff

- Push result and PR metadata are recorded in the completion report after publication.
- Main remains unchanged; P010 is not merged by this workflow.
- Unrelated-work confirmation: changes are limited to P010 quota acquisition, fixtures, contract inventory, docs, and the time dependency lockfile update.
