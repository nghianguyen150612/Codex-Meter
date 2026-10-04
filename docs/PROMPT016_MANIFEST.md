# Prompt 016 Manifest

- Starting SHA: `764277b5b973b1dc7f164fae7f63f8ec1c6c5b71`.
- Verified P015 baseline: fetched `origin/main`, checked out `main`, confirmed
  `main == origin/main`, confirmed the P015 atomic runtime checkpoint commit,
  and confirmed a clean worktree before editing.
- Workflow: direct-to-main only; no feature branch and no pull request.
- Migration: added version `3`, `0003_observations`; migrations 0001 and 0002
  were not modified.
- Migration 0001 checksum: `1ffa336dcdc5abc63fdf74276c354c82a7b8f157af9412625723a7d8fe20c5aa`.
- Migration 0002 checksum: `ae4a25ab39f865e7d1d5f02c9d03cc95a2786b72a48733025db1a0abe01d7199`.
- Migration 0003 checksum: `1ed907c9f124697b7f5620799672e49128dde7110860d513b0efaa9f57cd305c`.
- Table schema: strict `observations`, keyed by P013 `observation_id`, with
  canonical JSON, lowercase SHA-256, positive revision, identity/lifecycle/
  timing/configuration/token/quota projections, and no wall-clock storage
  metadata.
- Projection inventory: schema and identity IDs; lifecycle/timing; summary
  quality; available-only configuration; token validity/quality/raw total; and
  independent five-hour and weekly validity/quality/delta/reset fields.
- Payload/checksum: deterministic `serde_json` bytes are authoritative; the
  checksum is exact-byte SHA-256 and is verified before closed typed decode.
- Revision/CAS: insert is revision 1 with expected `None`; non-identical
  updates require exact CAS and checked increment; exact replay is a no-op even
  for stale caller revision.
- Lifecycle: detected → active → task_ended → awaiting_meter → reconciling;
  terminal branches are finalized, incomplete, and invalid. Provisional
  same-state updates are allowed, regressions are rejected, and terminal rows
  are immutable except for exact replay.
- Queries/indexes: bounded exact filters, terminal/provisional separation,
  deliberate time/lifecycle/configuration/quality/validity indexes, stable
  keyset pagination by fallback order time and observation ID.
- Composite handoff: `commit_observation_and_checkpoint` performs both CAS
  writes in one `BEGIN IMMEDIATE` transaction and rolls both back on error.
- Corruption: checksum, closed decode, typed invariant, meter identity, reset
  semantics, and projection consistency failures are fail-closed and never
  repaired automatically.
- Tests: migration checksum/upgrade/reopen tests; canonical payload checksum;
  replay/revision/lifecycle/terminal tests; reset/zero/incomplete fixture
  round trips; corruption/projection tests; query filters/pagination; and
  composite handoff/rollback tests.
- Dependencies: no new dependency; existing serde, serde_json, sha2, time, and
  bundled rusqlite are sufficient.
- Privacy: no prompts, responses, reasoning text, tool content, paths,
  credentials, account data, raw rollout archive, or generic records table.
- Phase E deferrals: no estimator, weighting, capacity, benchmark, analytics
  result table, or Python write path was added.
- Final SHA handling: the final commit SHA, push result, equality check, and
  clean-worktree result are recorded in the completion report after the single
  logical P016 commit is created.
- Unrelated-work confirmation: no legitimate newer work was present; P013
  measurement semantics and P008–P015 behavior were preserved.
