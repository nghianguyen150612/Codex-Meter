# PROMPT029 implementation manifest

## Repository and Git workflow

- Starting `origin/main` SHA: `422b40e5316e91f1a53f8f51cdd0f9798095279a`.
- Feature branch: `feat/p029-cross-platform-runtime-compatibility`.
- Starting worktree: clean; branch created from the required starting SHA.
- Required implementation commit: `532661cad985a16fa04f889702ed07a17b0b4d2a`, message
  `feat: improve installed analytics runtime compatibility`.
- Follow-up commits before this manifest update:
  `52ada600351ce7cdc460aeac0cba22ac9d7c8eca` (runtime matrix setup),
  `8273d31011b8323f63d2e5497a645d646c6f1d64` (Windows-path diagnostic),
  `a6cbc8344f33cb46a76b9057b96486afce9a6599` (Windows SQLite paths), and
  `5efe8aa5e990a990324eaf2ea343c7cadb319087` (bounded cleanup test timing).
- The feature branch was pushed; the pre-manifest head listed above passed CI run
  `39013527538` (#105).
- PR #22: `Improve cross-platform analytics runtime compatibility`, targeting `main`:
  https://github.com/nghianguyen150612/Codex-Meter/pull/22 . The PR was open at this manifest
  update.
- Complete diff review found no outstanding findings. GitHub review submissions and inline review
  threads were empty at verification time.
- Merge commit, parents, feature-commit reachability, final `main` SHA, local/origin equality, and
  clean-worktree verification will be recorded in the completion report after merge.

## Package build design

- Backend: Hatchling through the standard PEP 517 build-system metadata.
- Runtime distribution: `codex-meter-analytics` version `0.0.0`, import package `codex_meter`.
- Python minimum: `>=3.11`; runtime dependencies remain empty.
- Wheel includes the Python package only, with explicit exclusions for cache, credential, database,
  environment, and telemetry files. The smoke test checks package modules, metadata, forbidden
  paths, and byte-for-byte reproducibility across two local builds.
- Build-only requirement: Hatchling in `build-system.requires`; no runtime dependency was added.
- `python/uv.lock`: unchanged; the locked contributor environment and runtime dependencies did not
  change.

## Installed-worker resolution

- Worker selection is available with `--worker-mode auto|source|installed`; the backward-compatible
  default is `auto`.
- Auto mode chooses the validated compile-time checkout path when present and the selected
  interpreter's installed package only when the checkout path is absent. Explicit source mode uses
  only the checkout path; explicit installed mode ignores it.
- Installed mode requires one compatible `codex-meter-analytics` distribution, metadata declaring
  Python `>=3.11`, and the worker modules listed in its installed file inventory. Missing or
  ambiguous candidates fail with a sanitized resolution error.
- Both strategies use the existing isolated `-I -S` bootstrap and P028 subprocess supervisor.
- Installed path validation establishes location and containment, not cryptographic authenticity.

## Runtime compatibility and tests

- Supported OS targets: Linux, macOS, Windows.
- Package Python minimum: `>=3.11`; CI matrix: 3.11, 3.12, 3.13 on all three OS targets.
- Local host: Linux x86_64, Python 3.12.14. The installed-worker smoke test passed, including a
  copied CLI binary, an independent caller working directory, and source-directory absence.
- Local macOS/Windows runs and Python 3.11/3.13 runs: not available on this host; CI results are
  pending and will be recorded from the final feature-head workflow runs.
- Smoke test covers wheel contents and reproducibility, a fresh venv, no-dependency installation,
  isolated worker loading, source checkout present, copied CLI, unrelated and home working
  directories, Unicode/space paths, shadow packages, missing and ambiguous installations,
  current-capacity and insufficient-evidence results, deterministic replay, concurrent read-only
  requests, and unchanged SQLite bytes.
- Existing Rust tests retain P028 process limits, exact marker bytes, cleanup, and failure coverage.
- The representative CLI integration output from the verified starting SHA and P029 is byte-
  identical (1,159 bytes; SHA-256
  `13ba5044594d01cff0313d14a6fd514883d4512b6278b5cd8db74f34be5a296a`), now asserted by the
  Rust end-to-end regression test. Direct reproduction showed that the P028 manifest's recorded
  1,173-byte digest does not match this fixture at the starting SHA; this check uses the verified
  starting-commit output.
- Contract/schema versions and SQLite migrations are unchanged.
- Local quality gate: Ruff lint and format checks passed; pytest passed (623 tests); Rust fmt and
  Clippy passed; Rust unit tests passed (296) and CLI integration tests passed (4); Cargo workspace
  build passed; contract validation passed (6 schemas, 16 positive fixtures, 5 negative fixtures).
- Local installed-runtime smoke passed with `python scripts/p029_runtime_smoke.py`; it built two
  byte-identical wheels, installed one without dependencies into a fresh venv, relocated the CLI,
  and passed the synthetic current-capacity, insufficient-evidence, path, isolation, deterministic,
  concurrent, and read-only SQLite checks.
- `uv lock --check --project python`, frozen sync, and `git diff --check` passed. `python/uv.lock`
  is unchanged.

## Security and operational compatibility

- Request/result schemas and protocol `1.0.0` remain unchanged.
- Request, output, diagnostic, and timeout limits remain 64 KiB, 2 MiB, 8 KiB, and 60 seconds.
- Environment clearing, no `PYTHONPATH` fallback, strict worker-root containment, process groups,
  Windows Job Objects, broken-pipe handling, exact completion acknowledgement, sanitized errors,
  Rust result validation, and manual-meter arithmetic remain enforced.
- Rust preflight validates the request before worker launch, checks the selected interpreter path,
  and opens the existing database read-only without creating it.
- Windows extended drive and UNC prefixes are normalized only for SQLite file-URI construction;
  path containment and read-only access remain enforced. Live UNC shares are unavailable in the
  hosted test environment.
- Provider API calls, automatic quota acquisition, benchmark execution, and release publication
  remain disabled.

## Dependency changes and known limitations

- Runtime dependencies: no changes.
- Build backend: Hatchling, used only to create the local wheel.
- Lockfile: unchanged.
- UNC/network-share behavior is not verified without a hosted share. Windows long-path handling is
  limited to what the CI runner enables.
- Virtual environments are not guaranteed to be relocatable. An installed package directory
  writable by an attacker is not trusted; package contents are not cryptographically signed.

## P030 deferrals

- Public releases, official installers, registry publishing, artifact signing, and release
  integrity are deferred.
- Provider quota acquisition, live benchmarks, daemons, and recurring execution remain unsupported.
- P030 has not started.

## PR and merge verification

- Required PR title: `Improve cross-platform analytics runtime compatibility`.
- PR URL: https://github.com/nghianguyen150612/Codex-Meter/pull/22 .
- CI run `39013527538` (#105), on feature commit `5efe8aa5e990a990324eaf2ea343c7cadb319087`, passed
  all 15 jobs: the six original OS/quality jobs and the nine installed-runtime OS/Python cells.
- Merge method and post-merge synchronization are pending; the completion report will record the
  merge SHA, both parents, preserved commits, final main SHA, `main == origin/main`, and clean tree.
