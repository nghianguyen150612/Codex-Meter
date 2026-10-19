# Codex Meter

Codex Meter is an independent, local-first project for empirically measuring Codex usage. The
`codex-meter estimate` CLI reads an existing Codex Meter SQLite dataset and reports empirical
current-capacity estimates through the Python analytics pipeline. Automatic tracker startup and
live provider quota acquisition are not connected to this command.

The initial distribution candidate is version `0.1.0`. It pairs a native Rust executable with the
`codex-meter-analytics` wheel and a Python 3.11–3.13 environment; it is not a standalone binary.
Candidate artifacts are unpublished and require manual release review. See the
[installation guide](docs/INSTALLATION.md), [release notes](docs/RELEASE_NOTES_0.1.0.md), and
[release readiness report](docs/RELEASE_READINESS.md).

The product loop is **Track → Measure → Estimate → Compare**, while keeping raw tokens,
weighted/effective usage, and plan quota as distinct measurement domains.

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
