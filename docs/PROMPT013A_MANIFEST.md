# Prompt 013A Manifest

- Starting SHA: `3d4a6384b82d1f4944d046a3a555139945f88649`.
- Review finding: P013 plan merging inspected quota samples but omitted an available task-effective plan, allowing task `plus` plus quota `pro` to serialize `plus`; detected conflicts also left token evidence fully valid.
- Unified rule: collect every available task-effective and retained five-hour/weekly quota plan into one deterministic set; zero values means unavailable, one value preserves/promotes the plan, and multiple values means unavailable plus conflict.
- Provenance: preserve an available task plan's existing provenance; quota-only plans are provider-observed, never derived.
- Conflict behavior: task-vs-quota conflicts are symmetric and source-order independent; no task, quota, latest, or majority winner is selected.
- Degradation: plan conflict adds `telemetry_incomplete` and degrades valid token and quota evidence to `incomplete / D`; existing invalid `X` states and specific reasons remain stronger.
- Lifecycle: resolved plan-conflicted observations become `incomplete` through the existing evidence-based lifecycle rule; summary quality remains the worst domain grade.
- Tests: added task-plus/quota-pro, task-pro/quota-plus, agreement, task-only, quota-only, missingness, token/quota degradation, reset-preservation, lifecycle, and deterministic reason coverage.
- Files modified: `rust/crates/codex-meter/src/telemetry/observation.rs`; `docs/OBSERVATION_ASSEMBLY.md`.
- Files created: `docs/PROMPT013A_MANIFEST.md`.
- Schema status: no schema or reason-code changes; `1.0.0` contracts remain authoritative.
- Validation: final report records formatting, clippy, Rust tests, build, Ruff, Python tests, contract validation, and diff checks.
- P014 status: not started; no SQLite, persistence, migration, or Phase D work was introduced.
- Final SHA handling: this manifest is created before the single corrective commit and is not edited afterward with a self-referential SHA.
- Workflow: direct-to-main only; no branch or pull request.
