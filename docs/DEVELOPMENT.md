# Codex Meter development

## Prerequisites

- A stable Rust toolchain with `rustfmt` and Clippy.
- Python 3.11 or newer.
- [`uv`](https://docs.astral.sh/uv/) for locked Python environments.
- [`just`](https://just.systems/) for the unified commands (optional when using the direct
  equivalents below).

From the repository root, `uv sync --project python --frozen` creates the project environment from
the committed `python/uv.lock` without changing dependency resolution. The Python distribution is
an internal development package at version `0.0.0`; it is not a separately released product.

## Developer commands

| Command | Purpose |
| --- | --- |
| `just build` | Build the Rust workspace and synchronize the locked Python environment. |
| `just test` | Run Rust and Python tests. |
| `just lint` | Check Rust formatting/Clippy and Python Ruff lint/format. |
| `just contracts` | Validate all schemas and positive/negative fixtures. |
| `just check` | Run the complete local quality gate. |

The direct commands are recorded in the root `justfile`. In particular, Cargo commands use
`--manifest-path rust/Cargo.toml`, and Python commands use `uv run --project python --frozen`.

Published SQLite migration SQL is checked out with LF endings on every platform. The migration
registry hashes the exact SQL bytes, so CRLF conversion on Windows would appear as migration drift
and invalidate the published checksum regression tests.

## Git Workflow Policy

Effective from P020A onward, this policy supersedes all earlier direct-to-main workflow
instructions. Preserve the development instructions above and the history of completed prompts.

1. Never commit or push implementation changes directly to `main`.
2. Fetch and verify the latest `origin/main`, then create a dedicated feature or fix branch
   from it. Branch names must describe the prompt's actual function; do not use agent-generated
   prefixes such as `codex/`, `agent/`, or `jules/`.
3. Open a pull request targeting `main`. The title must describe the actual function, without
   agent branding or generic labels such as `[Codex]`, `[Agent]`, or `Agent:`.
4. Review the changed files for scope, run the full project quality gate (Python lint, format,
   and tests; Rust format, Clippy, tests, and build; contract/fixture validation), and run
   `git diff --check`. Do not weaken tests or CI to obtain green results.
5. Check all available GitHub Actions results for the PR and wait for required CI checks.
   Review the PR diff before merging. If required checks or reviews block merging, report
   the blocker and leave the PR open; never bypass repository protection.
6. Merge using **Create a merge commit**, equivalent to `--no-ff` or GitHub API
   `merge_method=merge`. Never squash, rebase, or fast-forward merge.
7. Preserve every original feature-branch commit and its SHA in `main` history. Do not
   rewrite existing history. Verify that the merge commit has exactly two parents and
   that all original feature commits are ancestors of `main`.
8. After merging, fetch and update local `main`, verify `main == origin/main`, and confirm
   a clean local worktree. Report the feature commit SHAs, PR, merge SHA and parents,
   quality-gate outcomes, and final verification.

## Contract validation

`scripts/validate_contracts.py` parses and meta-validates all six Draft 2020-12 schemas, builds an
in-memory registry for repository-local references, enables format checking, and validates every
explicitly inventoried fixture. It performs no network retrieval. Normal fixtures must pass;
fixtures below `fixtures/contracts/v1/invalid/` encode contradictions and must produce a validation
error. An unknown or missing fixture fails validation rather than being silently skipped.

## Current scope

The Rust executable exposes `codex-meter estimate` for empirical current-capacity estimates. It
validates AnalyticsRequest v1, invokes the locked Python analytics package through a bounded local
subprocess, reads the existing SQLite dataset strictly read-only, and validates AnalyticsResult v1
before rendering output. See [CLI analytics integration](CLI_ANALYTICS_INTEGRATION.md) for setup,
examples, supported options, and current limitations. Automatic quota acquisition and other
analysis types are not exposed by this command.
