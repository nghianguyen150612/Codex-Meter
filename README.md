# Codex Meter

Codex Meter is an independent, local-first project for empirically measuring Codex usage. The
`codex-meter estimate` CLI reads an existing Codex Meter SQLite dataset and reports empirical
current-capacity estimates through the Python analytics pipeline. Automatic tracker startup and
live provider quota acquisition are not connected to this command.

The product loop is **Track → Measure → Estimate → Compare**, while keeping raw tokens,
weighted/effective usage, and plan quota as distinct measurement domains.

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

## Project documents

- [Product contract](docs/PRODUCT_CONTRACT.md)
- [Technical architecture](docs/ARCHITECTURE.md)
- [Versioned data contracts](docs/DATA_CONTRACTS.md)
- [Development guide](docs/DEVELOPMENT.md)
