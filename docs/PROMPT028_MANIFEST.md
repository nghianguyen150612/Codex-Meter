# PROMPT028 Manifest — CLI Privacy and Process Hardening

## Starting point and workflow

- Starting `main` SHA: `7de6e79551cb2596b8ee48b662fc26f2ec6905fe`.
- Required feature branch: `fix/p028-cli-boundary-security`.
- Required implementation commit subject: `fix: harden CLI analytics process boundaries`.
- Required PR title: `Harden CLI analytics privacy and subprocess isolation`.
- Implementation commit: `64be318e67f7f041557f5fd5ca2790810a23380f`.
- PR #21: [Harden CLI analytics privacy and subprocess isolation](https://github.com/nghianguyen150612/Codex-Meter/pull/21), targeting `main`.
- The starting revision was fetched and verified on `origin/main`; the worktree was clean before
  the feature branch was created. Implementation remains off `main`.
- Latest-head CI, merge commit SHA and parents, commit reachability, final `main` SHA, and final
  clean-tree verification are reported after the remote workflow. No future identifiers are
  prefilled here.

## Security findings and changes

- Removed worker identity dependence on the current directory and inherited `PYTHONPATH`. The Rust
  launcher selects either the source root relative to the compiled crate manifest or exactly one
  worker installation in the configured interpreter's standard/virtual-environment package roots.
  Both Rust and a fixed Python bootstrap resolve and check the package files before execution.
- The fixed bootstrap runs with `-I -S`; it imports only from the checked application root and
  standard-library locations. It does not evaluate request content or derive an import path from
  request fields. Missing and ambiguous roots fail closed.
- The caller-selected `--python PATH` stays supported with discrete `Command` arguments. There is no
  shell, fallback interpreter, or automatic retry. The child environment is rebuilt with only
  `PATH`, `PATHEXT`, `SystemRoot`, `WINDIR`, `TEMP`, `TMP`, and `TMPDIR` when present.
- Process supervision concurrently writes bounded request input and drains capped stdout/stderr.
  The 60-second deadline includes launch elapsed time, input, output, exit, and cleanup, with up to
  250 ms reserved for termination. Unix process groups and Windows Job Objects terminate the
  worker tree on timeout, overflow, or inherited pipes. Reader joins are made only after their
  threads report completion; cleanup failures never yield a result.
- The existing local protocol version remains `1.0.0`. A fixed stderr acknowledgement is required
  after the worker reads request input through EOF. The acknowledgement is a compatibility check,
  not authentication.
- Rust typed deserialization rejects duplicate/unknown fields and forbidden nulls. Semantic checks
  bind the result to request identity, exact configuration, meter order/coverage, timestamp,
  status, warning/reason union, and accounting. Every sufficient meter requires its explicit manual
  position; Rust checks used and remaining arithmetic with P020's `1e-12` relative tolerance.
- Python worker errors use fixed diagnostic messages. Tracebacks, unrestricted stderr, raw request
  data, Observation payloads, environment values, and credentials are not emitted in CLI errors.
- The Python analytics reader remains P017's SQLite `mode=ro` reader with `PRAGMA query_only = ON`.
  It performs no migrations, writes, repairs, checkpoints, or implicit database creation. Synthetic
  database tests verify missing-file behavior, schema and migration failures, Observation
  corruption/projection failures, safe diagnostics, and unchanged database bytes.
- Successful JSON remains the closed AnalyticsResult v1 structure. A P027 baseline and P028 run
  over the same synthetic request and database produced byte-identical 1,173-byte JSON (SHA-256
  `a7d2c26e1abc02a3b84c9d9ad90f9998f7cba3da28b88b8b8a066110c7bf26bf`): full capacity 362,500,
  used 87,000, and remaining 275,500 raw tokens. Deterministic replay also passes; human output
  states that estimates are not official provider quota allowances.
- The Rust CLI integration test passes a database path containing spaces and Unicode through the
  worker boundary.

## Threat model and limitations

- Request JSON, stored database contents, subprocess output, inherited interpreter configuration,
  working-directory contents, and child timing are treated as untrusted.
- The caller-selected interpreter is trusted executable code with the caller's OS permissions. P028
  does not sandbox arbitrary native executables or cryptographically authenticate the interpreter
  or package files.
- A process with write access to the selected source/install tree can replace worker files. A file
  may also be replaced between path validation and SQLite open. These filesystem races require a
  larger packaging/storage design to eliminate.
- Unix descendants that deliberately leave the worker process group may escape termination;
  Windows Job Object assignment occurs immediately after process creation, leaving a brief race.
  OS process creation itself is synchronous and cannot be interrupted, though its elapsed time is
  counted and checked when it returns. Cleanup uncertainty returns an error, never success.
- Local verification ran on Linux. macOS and Windows execution, including Windows Job Object
  behavior and Windows path handling, remains subject to the PR's platform CI jobs.

## Quality gate and dependencies

- Python Ruff check and format: passed.
- Python tests: 622 passed.
- Rust formatting and Clippy with warnings denied: passed.
- Rust unit tests: 295 passed.
- Rust subprocess CLI integration tests: 4 passed.
- Rust workspace build: passed.
- Contract validation: 6 schemas, 16 positive fixtures, and 5 negative fixtures passed.
- `git diff --check`: passed.
- No Rust or Python dependencies, schemas, SQLite migrations, or lockfiles were changed.
- Latest-head CI across Rust quality, Ubuntu/macOS/Windows Rust platform jobs, Python quality, and
  Contracts is checked after the PR is opened. No provider API, quota request, or live benchmark is
  used by tests.

## P029 handoff

P029 may assume:

- P027's current-capacity CLI remains functional.
- Rust/Python transport uses the hardened worker-resolution policy.
- Environment-dependent module shadowing is prevented within the documented trust model.
- Process invocation has bounded output, diagnostics, and lifecycle handling.
- Request/result validation is strict and cross-language semantics are tested.
- Explicit current-meter percentages are verified against returned arithmetic.
- SQLite analytical access remains read-only.
- Errors do not expose raw telemetry or sensitive payloads.
- No provider execution occurs.
- No automatic quota acquisition is claimed.
- Benchmark production execution remains disabled.

P029 focuses on:

**Cross-platform resilience, installation/runtime compatibility, and operational hardening.**

P030 remains reserved for release readiness. P028 does not begin P029 or P030.
