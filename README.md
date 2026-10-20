# Codex Meter

Codex Meter is an independent, unofficial, local-first project for measuring Codex usage. The
native Rust `codex-meter sync` command discovers supported local Codex rollout files and saves
privacy-filtered token and quota evidence in a per-user SQLite database. The `tokens`, `history`,
`inspect`, and `quota` commands report that evidence through read-only native Rust queries. The
existing `codex-meter estimate` command continues to read a supplied Codex Meter SQLite dataset
and report empirical current-capacity estimates through the Python analytics pipeline.

The initial distribution candidate is version `0.1.0`. It pairs a native Rust executable with the
`codex-meter-analytics` wheel and a Python 3.11–3.13 environment; it is not a standalone binary.
Candidate artifacts are unpublished and require manual release review. See the
[installation guide](docs/INSTALLATION.md), [release notes](docs/RELEASE_NOTES_0.1.0.md), and
[release readiness report](docs/RELEASE_READINESS.md).

The product loop is **Track → Measure → Estimate → Compare**, while keeping raw tokens,
weighted/effective usage, and plan quota as distinct measurement domains.

P031's `sync` is an on-demand, one-shot operation. It does not run a background tracker or acquire
authenticated quota data. Initial rollout parsing is verified only for the audited Codex CLI
`0.157.1` format; compressed `.jsonl.zst` files are reported as unsupported. See the
[v0.2 real telemetry ingestion guide](docs/V02_REAL_TELEMETRY_INGESTION.md) for source coverage,
privacy behavior, storage, and validation status.

Run from a development checkout with `cargo run --manifest-path rust/Cargo.toml -- sync`. P031 sync
and P032 reporting are not part of the published v0.1.0 assets, installers, or release behavior.

After syncing, inspect local usage without providing a database path or request JSON:

```console
cargo run --manifest-path rust/Cargo.toml -- tokens
cargo run --manifest-path rust/Cargo.toml -- history --limit 20
cargo run --manifest-path rust/Cargo.toml -- quota
```

Reporting reads the same per-user database as `sync` in SQLite read-only mode. Its token and quota
coverage is limited to supported local rollout evidence. See the
[usage history CLI guide](docs/V02_USAGE_HISTORY_CLI.md) and
[P032 implementation manifest](docs/PROMPT032_MANIFEST.md) for accounting semantics and validation
status.

## Quick install (v0.1.0 technical preview)

User-local scripts for Linux/macOS and Windows are in [script installation](docs/SCRIPT_INSTALLATION.md). They download pinned v0.1.0 assets, verify SHA-256 hashes, and install the native CLI plus matching Python wheel. No root/Administrator permissions or provider credentials are needed. An existing compatible SQLite database remains necessary for estimates.

## Development

The repository contains a Rust `codex-meter` CLI, an internal Python analytics package, versioned
JSON Schemas, and contract fixtures. After installing Rust, Python 3.11+, `uv`, and `just`, run the
complete local gate with:

```console
just check
```

See the [development guide](docs/DEVELOPMENT.md) for setup and direct commands.
See the [CLI analytics guide](docs/CLI_ANALYTICS_INTEGRATION.md) for current-capacity usage,
request examples, and integration limits.
See the [P031 implementation manifest](docs/PROMPT031_MANIFEST.md) for the branch, gates, and
outstanding device-validation status.
See the [P032 implementation manifest](docs/PROMPT032_MANIFEST.md) for native reporting, quality
gates, and the outstanding Arch-device integration gate.
See the [cross-platform runtime guide](docs/CROSS_PLATFORM_RUNTIME_COMPATIBILITY.md) for local
wheel installation, installed-worker selection, and runtime compatibility coverage.

## Project documents

- [Product contract](docs/PRODUCT_CONTRACT.md)
- [Technical architecture](docs/ARCHITECTURE.md)
- [Versioned data contracts](docs/DATA_CONTRACTS.md)
- [Development guide](docs/DEVELOPMENT.md)
- [Installation guide](docs/INSTALLATION.md)
- [Release readiness](docs/RELEASE_READINESS.md)
- [Roadmap completion audit](docs/ROADMAP_COMPLETION_AUDIT.md)
