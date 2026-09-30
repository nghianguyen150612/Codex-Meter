# Prompt 001 Manifest

## Identification

- **Prompt:** P001 — Product Contract and Measurement Semantics
- **Starting commit:** `ef5fcd42019cf33a1c304c7cc864f1ab6122bef6`
- **Branch:** `work`
- **Final commit:** Not embedded. A Git commit cannot contain its own final SHA without changing that SHA; the completion report records the resulting commit.

## Repository state discovered

At the starting commit, the repository contained only `README.md` and `LICENSE`; it had no implementation, documentation directory, automated documentation checks, or configured Git remote. The worktree was clean before P001 changes.

## Files created

- `docs/PRODUCT_CONTRACT.md`
- `docs/ROADMAP.md`
- `docs/PROMPT001_MANIFEST.md`

## Files modified

None. P001 leaves the existing `README.md` and `LICENSE` unchanged.

## Key decisions

- Codex Meter is an independent, unofficial, local-first and privacy-first CLI product, initially focused on ChatGPT Plus.
- The product loop is **Track → Measure → Estimate → Compare**.
- Raw tokens, Codex Meter-derived weighted/effective usage, and observed plan quota are distinct domains and MUST NOT be silently interchanged.
- Observations carry conceptual task, environment, token, quota, reconciliation, and quality evidence without prescribing a storage schema.
- Quality grades `A`, `B`, `C`, `D`, and `X` have stable evidence semantics; numerical estimator weights remain evolvable implementation policy.
- Delayed-meter reconciliation and per-window reset detection are correctness requirements.
- Estimates require multiple compatible samples, robust statistics, outlier handling, dispersion/stability reporting, and confidence information.
- Data isolation includes plan, model, reasoning level, speed mode, and relevant time period; Codex version may become a segmentation dimension.
- Normal telemetry excludes content, identity, remote, and credential data. Any future sanitized numerical community contribution is explicit opt-in only.
- Rust owns the intended runtime/measurement layer; Python owns the intended analytics/research layer; SQLite and versioned JSON schemas form the initial boundary.
- The documented components are product concepts, not final module or crate boundaries.

## Explicit non-goals

P001 does not implement runtime behavior or bring later roadmap work forward. It introduces no file watcher, JSONL parser, quota API/RPC client, SQLite schema, Rust workspace decomposition, Python estimator, benchmark executor, TUI, web dashboard, community upload service, PyO3/FFI bridge, TypeScript/React UI, or C/C++ component.

Codex Meter v1 is not a general API billing platform, content logger, repository analyzer, AI observability SaaS, OpenAI credential manager, web dashboard, or general-purpose AI benchmark platform. Docker is not a runtime requirement.

## Validation performed

- Reviewed the three P001 documents for internal contradictions and unsupported statements about undocumented OpenAI internals.
- Checked repository, binary, and product naming consistency.
- Checked that the product uses empirical capacity terminology rather than claiming a fixed official token allowance.
- Checked explicit separation of raw tokens, weighted/effective usage, and plan quota.
- Checked that reconciliation and reset handling are correctness requirements.
- Checked all required privacy exclusions.
- Checked Rust/Python responsibilities and the SQLite plus versioned JSON schema boundary.
- Reviewed the scoped Git diff and whitespace diagnostics for unrelated or malformed changes.
- Confirmed there were no repository-provided automated documentation checks to run at the starting commit.

## Known limitations

- Actual Codex telemetry formats, completeness, locations, and stability have not yet been discovered or validated.
- Quota source access, meter latency, stabilization behavior, reset metadata, and metering rules remain unknown and subject to later empirical validation.
- Suggested observation weights and reconciliation timings are examples, not implemented or fixed policy.
- No storage schema, estimator, command, or runtime component exists yet.
- The starting repository has no configured Git remote, so P001 cannot be pushed until one is configured.

## Unrelated work confirmation

No pre-existing user changes were present, and no unrelated files were modified or removed. Only the three files listed under **Files created** belong to P001.
