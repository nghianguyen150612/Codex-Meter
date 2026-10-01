# Prompt 004 Manifest

## Identification and baseline

- **Prompt:** P004 — Repository Bootstrap, Tooling, and CI
- **Starting SHA:** `d8bd4822bda108d6b912c2f35912d45817abf681`
- **Branch:** `work`
- **Verified P003/P003A baseline:** `HEAD` exactly matched the expected merge commit, including the
  P003 schemas/fixtures and P003A invariant corrections. The starting worktree was clean.
- **Final SHA:** Not embedded because a commit cannot contain its own SHA; the completion report
  records it.

The authoritative product, architecture, contract, ADR, roadmap, manifest, schema, and fixture
inputs were inspected and preserved. No unrelated work was present or modified.

## Files created

- `.github/workflows/ci.yml`, `.gitignore`, and `justfile`
- `rust/Cargo.toml` and the single `rust/crates/codex-meter/` package
- `python/pyproject.toml`, `python/uv.lock`, `python/codex_meter/__init__.py`, and its bootstrap test
- `scripts/validate_contracts.py`
- `docs/DEVELOPMENT.md` and this manifest

## Files modified

- `README.md` was expanded only with current bootstrap status, entry points, and document links.

## Workspace and package decisions

Rust uses one dependency-free, unpublished Edition 2021 package with both the `codex-meter` binary
and `codex_meter` library. It contains product identity and honest bootstrap output only. Python uses
one unpublished-intent, version `0.0.0`, Python 3.11+ distribution with the import package
`codex_meter`. It is an internal non-building uv project. The development group constrains
pytest 8.3–9.x, Ruff 0.11–0.x, jsonschema 4.23–4.x, and directly declared
referencing 0.36–0.x; exact resolutions are committed in `uv.lock`.

## JSON Schema validation

The validator explicitly maps all fixture paths to their intended schema and rejects unclassified
or missing fixtures. It parses and checks all six schemas with `Draft202012Validator.check_schema`,
uses `pathlib`, assigns process-local file URIs, supplies all schemas through a `referencing`
in-memory registry, and enables `FormatChecker`. It never resolves references over the network.
Eight positive fixtures must pass and five negative fixtures must produce errors.

## CI and validation

CI has a Linux Rust formatting/Clippy job, a Rust build/test matrix across Ubuntu, macOS, and
Windows, a Python 3.11 frozen-environment Ruff/pytest job, and an independent frozen contract job.
Workflow permissions are read-only. The local `just check` gate runs the same fundamental build,
test, lint, formatting, and contract checks. The final pre-commit run completed all required Cargo,
Ruff, pytest, contract, and Git hygiene checks; exact commands and results are reported in the
completion response. Contract inventory was six schemas, eight positive fixtures, and five negative
fixtures. The execution environment could not reach PyPI, so frozen synchronization was verified as
far as the lock consistency check and Python checks ran from matching preinstalled package versions;
normal CI performs the full frozen installation.

## Explicit deferrals and limitations

No telemetry/session discovery, JSONL parsing, token extraction, watchers, quota acquisition or UI
scraping, authentication, SQLite, task tracking, reconciliation, reset runtime, estimator algorithm,
scientific dependency, clustering, benchmark, TUI/web UI, community upload, or release packaging is
implemented. The executable only identifies the development bootstrap. P005 begins telemetry work.
The schemas retain their existing repository-relative identifiers; the validator supplies local base
URIs at runtime rather than changing accepted contract semantics.

## Unrelated work confirmation

The initial worktree was clean and exactly at the expected baseline. Only P004 bootstrap files and
the narrowly scoped README update were staged; accepted P001–P003A documents, schemas, and fixtures
were not changed.
