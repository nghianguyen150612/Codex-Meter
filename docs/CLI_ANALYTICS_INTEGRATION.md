# Current-capacity estimate CLI

P027 connects the native `codex-meter` executable to the existing Python current-capacity estimator. It supports one analysis type: `current_capacity_estimate`. The command reads the existing Rust-owned SQLite database through P017's strict read-only API and runs P018 candidate derivation, P019 robust aggregation, and P020 finalization.

## Prerequisites and Python environment

- Stable Rust with Cargo, rustfmt, and Clippy.
- Python 3.11 or later.
- `uv` for the repository's locked development environment.

From the repository root, create the locked Python environment with `uv sync --project python --frozen`. The worker has no Python runtime dependencies. During source-tree development, Rust resolves the package from the source root relative to its compiled crate manifest. To select the interpreter from that environment explicitly, obtain its path with:

```bash
uv run --project python --frozen python -c "import sys; print(sys.executable)"
```

Pass that path with `--python`. The default executable name is `python`. Rust never uses the working directory or `PYTHONPATH` to identify the worker. When the compiled source root is absent, the selected interpreter must have exactly one worker package in its standard or virtual-environment site-packages locations. The source tree is not bundled into the Rust binary.

## Command

```text
codex-meter estimate --database PATH --request FILE --generated-at UTC_TIMESTAMP [OPTIONS]
```

Required arguments:

- `--database`: existing Codex Meter SQLite database. Missing paths are rejected and never created.
- `--request`: one AnalyticsRequest v1 JSON document.
- `--generated-at`: explicit UTC RFC 3339 timestamp ending in `Z`. There is no clock-derived default.

Optional arguments:

- `--five-hour-used-percent N`: manually supplied current five-hour used percentage points, from 0 through 100.
- `--weekly-used-percent N`: manually supplied current weekly used percentage points, from 0 through 100.
- `--python PATH`: trusted Python interpreter executable. Each process argument is passed directly; no shell is involved, and there is no fallback if this executable fails.
- `--json`: print only the validated AnalyticsResult v1 JSON document.

The `--help` options show CLI and estimate usage. `--version` prints the executable version. Unknown commands, unknown arguments, repeated flags, and invalid values fail with a categorized diagnostic.

## AnalyticsRequest v1 example

The example uses synthetic values and demonstrates that `speed_mode` remains unavailable. `availability` and `provenance` are preserved independently; an unavailable value is not inferred from an available value elsewhere.

```json
{
  "schema_version": "1.0.0",
  "request_id": "demo-capacity-001",
  "analysis_type": "current_capacity_estimate",
  "dataset_selector": {
    "configuration": {
      "plan": {"availability": "available", "value": "Plus", "provenance": "observed"},
      "model": {"availability": "available", "value": "gpt-demo", "provenance": "derived"},
      "reasoning_level": {"availability": "available", "value": "high", "provenance": "observed"},
      "speed_mode": {"availability": "unavailable", "provenance": "unavailable"},
      "codex_version": {"availability": "unavailable", "provenance": "unavailable"}
    },
    "observed_from": "2026-10-01T00:00:00Z",
    "observed_through": "2026-10-15T23:59:59Z"
  },
  "target_quota_windows": ["five_hour"],
  "options": {"maximum_observations": 100}
}
```

Save the document to a local file, then request a five-hour estimate:

```bash
cargo run --manifest-path rust/Cargo.toml -- estimate \
  --database /path/to/codex-meter.db \
  --request /path/to/analytics-request.json \
  --five-hour-used-percent 24 \
  --generated-at 2026-10-16T07:00:00Z
```

For both meters, include both targets in the request and supply both positions:

```bash
codex-meter estimate --database /path/to/codex-meter.db \
  --request /path/to/both-meters.json \
  --five-hour-used-percent 24 --weekly-used-percent 42 \
  --generated-at 2026-10-16T07:00:00Z
```

For JSON output, add `--json`. Stdout then contains one validated AnalyticsResult v1 document with no banner or presentation-only provenance fields:

```bash
codex-meter estimate --database /path/to/codex-meter.db \
  --request /path/to/analytics-request.json \
  --five-hour-used-percent 24 --generated-at 2026-10-16T07:00:00Z --json
```

If the selected history has no eligible observations, P020 returns `insufficient_evidence`. In that case the corresponding current position may be omitted, and the output reports insufficient evidence, counts, and reason codes without zero-valued capacity estimates.

## Process architecture and context boundary

The public AnalyticsRequest v1 remains exactly the existing closed schema. Rust deserializes it into strict typed structures, rejects unknown fields, validates identity/provenance/history/meter semantics, and rejects unsupported estimator options before starting Python.

The local process context is separate and versioned as `ANALYTICS_PROCESS_PROTOCOL_VERSION = 1.0.0`. Rust passes these arguments individually to a fixed, application-owned Python bootstrap. The bootstrap validates a trusted source-tree or installed-package root before running `codex_meter.integration.worker`:

- `--database`: absolute path to an existing database. Relative paths are resolved from the CLI's working directory. It is not placed in AnalyticsRequest JSON.
- `--runtime-context-version`: currently `1.0.0`; unknown versions fail closed.
- `--generated-at`: explicit deterministic UTC result timestamp.
- `--five-hour-used-percent` and `--weekly-used-percent`: optional manually supplied percentage points.

Standard input carries only the bounded AnalyticsRequest v1 JSON. Standard output carries only one AnalyticsResult v1 JSON document. Standard error carries categorized, sanitized worker errors. The context contains no prompts, source code, credentials, or arbitrary provider payloads.

`python/codex_meter/integration/worker.py` validates its inputs, selects history through `ObservationDatabase.open()` and `load_dataset()`, calls `derive_capacity_candidates()` for each target, then calls `build_current_capacity_result()`. It does not contain capacity formulas or aggregation policy of its own.

## Request options and current meter evidence

P027 supports `options.maximum_observations` from 1 through 100,000, matching P017's reader safety limit. It rejects every `minimum_quality` value as `unsupported_estimator_option`: P018 already owns its A/B/C eligibility and exclusion accounting, and P027 does not add another filter.

Manual current positions are finite percentage points in `[0, 100]`; `24` means 24%, not 0.24%. A position for a meter absent from `target_quota_windows` is rejected. Missing positions remain absent; P020 raises its current-position-required error only when that meter has sufficient historical evidence.

Human output labels a supplied value `Current meter position: ... (manually supplied, unverified)`. This is caller-provided input, not an authenticated provider reading. P027 does not acquire live quotas or infer current position from the newest Observation.

## Database read-only behavior

The analytics process never opens the database through Rust's writable `SqliteStore`. P017 opens the existing database using SQLite read-only URI mode, enables `PRAGMA query_only`, verifies supported schema and migration checksums, checks stored Observation payload checksums and projections, and runs no migrations. It cannot create a missing file. Analysis results are returned to Rust and are not persisted. No provider operation or benchmark workload is launched.

## Output and errors

Human output names the five-hour and weekly meters, raw-token units, empirical full capacity, estimated used/remaining amounts, confidence, P25/P50/P75, sample accounting, and reason codes. Missing configuration values stay unavailable. The footer states that the estimate is not an official provider allowance. A caller-supplied position is labeled manual and unverified.

JSON output is reserialized by Rust only after strict result deserialization and semantic checks for schema/method versions, request ID, configuration and exact requested meter scope/order, authority/method, confidence/status, finite numbers, capacity arithmetic, percentile order, sample accounting, reason codes, warning consistency, and manual-position arithmetic. Every sufficient meter result requires its explicitly supplied position; Rust independently checks used and remaining amounts using P020's `1e-12` relative tolerance.

Errors distinguish invalid requests, unsupported analysis/options, manual meter errors, missing/incompatible databases, migration drift, corrupt Observations, unavailable Python, worker failure, timeout, invalid result JSON, and result/request mismatch. Raw Python tracebacks and unrestricted stderr are not printed.

The worker protocol limits request input to 64 KiB, result output to 2 MiB, retained diagnostics to 8 KiB, and the complete process lifecycle to 60 seconds. The isolated interpreter runs with `-I -S`; the child environment is cleared except for `PATH`, Windows startup variables, and temporary-directory variables. A fixed stderr success marker acknowledges the worker read the request through EOF. Rust reads stdout and stderr concurrently, kills the process group or Windows Job Object on timeout/size overflow, bounds cleanup, and reaps the direct child. The hardened behavior and process-escape limitations are documented in [CLI security hardening](CLI_SECURITY_HARDENING.md).

## Privacy and limitations

Only the closed request and result structures cross the process boundary. The CLI does not log database rows, prompts, responses, repository data, account identifiers, credentials, tokens, or raw rollout content. Diagnostics use fixed categories and messages rather than displaying worker stderr or tracebacks. Successful human output states that results are Codex Meter empirical estimates, not official provider quota allowances. The CLI does not call provider services, run live benchmarks, start the tracking agent, or expose the other Python analysis types. Those APIs remain internal.

`history_summary`, `quota_change_analysis`, `workload_estimate`, and `benchmark_analysis` are rejected as unsupported analysis types. Automatic quota acquisition, release packaging, and production benchmark execution remain out of scope.

## P028 security hardening

P028 preserves the supported command and AnalyticsRequest/Result v1 contracts while hardening
worker resolution, interpreter environment, process cleanup, error privacy, result scope, and manual
meter arithmetic. See [CLI security hardening](CLI_SECURITY_HARDENING.md) for the threat model,
enforced guarantees, known limits, and P029 handoff.
