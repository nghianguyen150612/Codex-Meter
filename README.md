# Codex Meter

Codex Meter is an independent, local-first project for empirically measuring Codex usage. The
repository is currently bootstrapped for development; telemetry collection and runtime behavior
are intentionally not implemented yet.

The product loop is **Track → Measure → Estimate → Compare**, while keeping raw tokens,
weighted/effective usage, and plan quota as distinct measurement domains.

## Development

The repository contains a minimal Rust `codex-meter` workspace, an internal Python analytics
package, versioned JSON Schemas and contract fixtures. After installing Rust, Python 3.11+, `uv`,
and `just`, run the complete local gate with:

```console
just check
```

See the [development guide](docs/DEVELOPMENT.md) for setup and direct commands.

## Project documents

- [Product contract](docs/PRODUCT_CONTRACT.md)
- [Technical architecture](docs/ARCHITECTURE.md)
- [Versioned data contracts](docs/DATA_CONTRACTS.md)
- [Development guide](docs/DEVELOPMENT.md)
