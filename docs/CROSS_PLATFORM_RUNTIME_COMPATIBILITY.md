# Cross-platform analytics runtime compatibility

P029 validates `codex-meter estimate` with both a development checkout and a separately installed
`codex-meter-analytics` wheel. No package is published by these steps. The Rust executable keeps
the name `codex-meter` (`codex-meter.exe` on Windows).

## Supported platforms and Python versions

The supported operating systems are Linux, macOS, and Windows. CI exercises the installed-wheel
CLI path on all three. The package declares a Python minimum of `>=3.11`; P029's tested versions
are Python 3.11, 3.12, and 3.13 on each operating system. Later versions have not been verified by
this matrix. The wheel is pure Python and declares no mandatory runtime dependencies. The worker
uses the standard library, including SQLite.

The matrix tests the local virtual-environment layouts used by the supported runners:

- Linux and macOS: `venv/bin/python` and `venv/lib/pythonX.Y/site-packages/`.
- Windows: `venv\Scripts\python.exe` and `venv\Lib\site-packages\`.

The runtime does not assume a system Python path, Python manager, or Homebrew location. It does not
assume virtual environments can be moved.

## Development source mode

From the repository root, synchronize the locked contributor environment:

```bash
uv sync --project python --frozen
cargo build --manifest-path rust/Cargo.toml --workspace
```

`--worker-mode auto` is the CLI default for P028 compatibility. It uses the verified `python/` tree
relative to the crate manifest recorded at build time when present, then uses the selected
interpreter's installed distribution only when that tree is absent. Both candidates are validated;
an invalid present source tree does not trigger fallback. Auto mode does not search the current
directory or `PYTHONPATH`. `--worker-mode source` explicitly requires the checkout tree and never
searches installed distributions. Source mode is intended for contributors and tests.

## Build and install a local wheel

Build from the analytics project directory without naming the wheel file:

```bash
uv build --project python --wheel --out-dir /tmp/codex-meter-wheelhouse
```

Create a fresh environment and install the resulting wheel without fetching runtime packages:

```bash
python -m venv /tmp/codex-meter-venv
python -m pip install --no-index --no-deps /tmp/codex-meter-wheelhouse/*.whl
```

On Windows, use equivalent paths under `%TEMP%`, and invoke the interpreter at
`venv\Scripts\python.exe`. The smoke command below builds the wheel twice, compares the artifacts,
checks package contents, installs one into a fresh environment, and runs the copied CLI. It uses
only synthetic SQLite fixtures:

```bash
cargo build --manifest-path rust/Cargo.toml --workspace
python scripts/p029_runtime_smoke.py
```

Run the script with the system Python executable, outside `python/.venv`. It temporarily renames
the source package directory to verify the installed runtime when the checkout tree is absent.

The script creates Unicode and space-containing installation, database, request, executable, and
working-directory paths. It also checks repeated results, concurrent read-only requests, missing
and ambiguous worker installations, source mode, and database immutability.

## Installed package mode

Get the interpreter path from the environment where the wheel is installed. Then run the CLI with
both explicit selections:

```text
codex-meter estimate --python <installed-interpreter> --worker-mode installed \
  --database <existing-database> --request <analytics-request.json> \
  --generated-at 2026-10-16T07:00:00Z --five-hour-used-percent 24 --json
```

The Python bootstrap still runs under `-I -S`. It reads only standard-library configuration and
the selected executable's `sysconfig`, `sys.prefix`, `sys.base_prefix`, and virtual-environment
layout. It resolves `purelib` and `platlib`, checks the venv's `pyvenv.cfg` path for Python 3.11
and 3.12 layouts where `-S` does not apply venv prefixes, and uses `site.getsitepackages()` only as
an additional candidate within the same interpreter environment. It does not rely on
`PYTHONPATH`, user-site packages, or the caller's current directory.

Installed mode validates the `codex-meter-analytics` distribution metadata and required worker
files within those interpreter package roots. It requires exactly one compatible distribution.
It does not fall back to the compiled source path or to another Python executable. Source mode does
not search installed distributions. Select `--worker-mode installed` when a source checkout is
present and the installed package is the intended runtime. This explicit selection prevents a
checkout from replacing the intended installed worker.

## Binary relocation and working directories

The runtime smoke test copies the built `codex-meter` executable to a separate temporary directory
and runs it from another directory against the installed virtual environment. The source checkout
may still be present, but `--worker-mode installed` ignores its path. No worker or wheel path is
inferred from the caller's current directory.

Absolute database and request paths keep their caller-supplied meaning. Relative database and
request paths are resolved from the CLI process's current working directory. The smoke test covers
absolute and relative paths with spaces and Unicode, and local symlink paths where the host permits
creating symlinks. Windows `\\?\` drive paths are normalized for SQLite's `file:` URI after path
resolution; the filesystem path remains the same. Windows CI checks the extended namespace and
attempts a path longer than `MAX_PATH` when the runner supports creating one. A network UNC share is
not available in hosted CI, so UNC traversal is not claimed as a tested configuration. The URI
conversion handles the standard `\\?\UNC\server\share` form, but a live UNC share remains
untested. Linux/macOS process cleanup uses a process group; Windows cleanup retains the P028 Job
Object.

## Environment and process isolation

Both worker modes use the same P028 invocation path:

- The interpreter receives `-I -S`; no flags are removed for wheel imports.
- The child environment contains only the documented runtime variables; inherited credentials,
  `PYTHONPATH`, and `PYTHONHOME` are cleared.
- The bootstrap imports only from the explicitly selected source root or installed package root
  plus standard-library paths. It rejects shadow files and ambiguous installations.
- Request and result limits remain 64 KiB and 2 MiB. Retained diagnostics remain 8 KiB, the
  timeout remains 60 seconds, and the worker must emit the exact byte-level completion marker.
- Broken pipes, process groups, Windows Job Objects, timeout cleanup, bounded pipe drains, and
  strict Rust AnalyticsResult v1 validation remain enabled.

Path checks establish containment and package-root provenance, not cryptographic package
authenticity. The wheel is not signed. An installation directory writable by an attacker cannot be
treated as trustworthy. Signing and release-distribution integrity remain for P030 or a later
security milestone.

## SQLite and privacy

The smoke suite uses synthetic fixtures only. The analytics worker opens existing SQLite files in
read-only URI mode, runs no migrations, and stores no result. Repeated and concurrent analyses
preserve the database bytes. A missing database is rejected before Python starts and is never
created. If a separate writer holds a transaction, SQLite's existing busy timeout applies; callers
may receive a sanitized read failure until the writer finishes. P029 does not change journal mode
or add locks.

No provider APIs, credentials, benchmarks, tracker, or quota acquisition are used. Current used
percentages remain optional manual inputs and are not verified by the application.

## Safe errors and troubleshooting

| Error code | Meaning | Corrective step |
| --- | --- | --- |
| `python_worker_resolution_failed` | The selected mode found no single valid worker distribution. | Install the local wheel in the selected interpreter and pass `--worker-mode installed`, or run source mode from a checkout with its Python tree present. |
| `python_unavailable` | The selected executable could not be started. | Check the interpreter path and its execute permissions. No alternate interpreter is tried. |
| `database_missing` | The supplied database is missing or is not a file. | Check the database path relative to the caller's working directory. The CLI will not create it. |
| `database_open_failed` | The database cannot be opened for a read-only analysis. | Check file permissions and whether another process is holding an incompatible lock. |
| `invalid_request` | The request could not be read or does not match AnalyticsRequest v1. | Check UTF-8 JSON, required fields, and the 64 KiB limit. |
| `process_timeout` | The worker exceeded its bounded lifetime. | Investigate the local interpreter or database condition; the CLI does not retry automatically. |

Diagnostics contain fixed safe messages and do not include local paths, database contents,
tracebacks, credentials, prompts, responses, or raw telemetry.

## Unsupported configurations and remaining limits

- Python versions older than 3.11 are rejected. P029 has tested versions 3.11–3.13; later versions are not yet verified.
- A venv is tied to its original Python installation and is not guaranteed to be relocatable.
- Installed mode does not authenticate wheel contents cryptographically.
- UNC shares and Windows long paths beyond the hosted runner's enabled settings are not verified.
- Windows volume-GUID device namespace paths are not supported; use a drive-letter or UNC path.
- No official installer, published package, or release artifact is produced.
- No provider quota acquisition, automatic position inference, daemon, or live benchmark is enabled.

## Inputs for P030

P030 may assume:

- The Rust CLI works outside the source checkout.
- A local Python analytics wheel can be built and installed.
- Installed-worker resolution is deterministic and fail-closed.
- The hardened P028 subprocess launcher remains intact.
- Linux/macOS/Windows runtime smoke tests pass.
- Supported Python versions are documented and verified.
- Binary relocation is tested.
- Existing SQLite read-only and privacy guarantees remain unchanged.
- Provider automatic quota acquisition remains unsupported.
- Production benchmark execution remains disabled.
- No public release has been published.

P030 will focus on:

**Release readiness, distributable artifacts, installation documentation, versioning, reproducibility, and final project verification.**
