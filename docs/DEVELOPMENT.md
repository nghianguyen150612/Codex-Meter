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

## Contract validation

`scripts/validate_contracts.py` parses and meta-validates all six Draft 2020-12 schemas, builds an
in-memory registry for repository-local references, enables format checking, and validates every
explicitly inventoried fixture. It performs no network retrieval. Normal fixtures must pass;
fixtures below `fixtures/contracts/v1/invalid/` encode contradictions and must produce a validation
error. An unknown or missing fixture fails validation rather than being silently skipped.

## Current scope

This is bootstrap infrastructure only. The identity-only Rust executable and Python package do not
track usage or produce estimates. Telemetry discovery and runtime implementation begin in later
roadmap prompts; no parser, quota acquisition, database, reconciliation, estimator, benchmark, or
user interface is present yet.
