# Codex Meter v0.1.0 — Technical Preview

This is the first public **technical preview** of Codex Meter, an independent, local-first, privacy-first empirical usage estimator.

## Available in v0.1.0

- `codex-meter estimate` reads an **existing, compatible** Codex Meter SQLite database in read-only mode.
- Empirical 5-hour and weekly **raw-token-equivalent capacity estimates** with sample accounting, uncertainty/confidence, and insufficient-evidence reporting.
- Optional **manually supplied, unverified** current used percentages for used/remaining estimates.
- Human-readable output and strictly validated AnalyticsResult v1 JSON.
- Native CLI bundles for Linux x86_64, Windows x86_64, macOS Intel, and macOS Apple Silicon.
- Shared `codex_meter_analytics-0.1.0-py3-none-any.whl` for Python 3.11–3.13.

## Installation

This is a **two-component installation**, not a standalone executable. Install the matching Python wheel into a Python 3.11–3.13 environment, and invoke the CLI with `--worker-mode installed --python <interpreter-path>`.

Consult the [platform installation guide](https://github.com/nghianguyen150612/Codex-Meter/blob/main/docs/INSTALLATION.md) and [CLI usage guide](https://github.com/nghianguyen150612/Codex-Meter/blob/main/docs/CLI_ANALYTICS_INTEGRATION.md).

The release assets include four platform bundles, the shared wheel, `SHA256SUMS.txt`, and `artifact-manifest.json`. Verify checksums before installation. The build manifest identifies the exact source commit **`171b54ee4f80c007ab6e0b4af3cd1bb22613e755`**.

## Important limitations

- No automatic session tracking, database population, live quota acquisition, daemon, or background monitoring.
- History/workload/regime and benchmark analyses are internal modules, not additional CLI commands.
- Production benchmark execution is disabled.
- Requires an existing compatible SQLite dataset and may require manual quota percentages.
- Binaries are **unsigned** and are not Apple-notarized or Windows Authenticode-signed. Checksums confirm byte integrity against the published list, **not publisher authenticity**.
- This release has been validated with synthetic data and cross-platform CI, **not with a production provider-backed dataset**.

**Codex Meter is independent and unofficial. Its estimates are empirical; they are not documented OpenAI plan allowances.**

## Source and verification

- Source: [P030 verified merge commit](https://github.com/nghianguyen150612/Codex-Meter/commit/171b54ee4f80c007ab6e0b4af3cd1bb22613e755)
- Release candidate validation: [GitHub Actions run 39022653731](https://github.com/nghianguyen150612/Codex-Meter/actions/runs/39022653731)
- Roadmap audit: [P001–P030 completion report](https://github.com/nghianguyen150612/Codex-Meter/blob/main/docs/ROADMAP_COMPLETION_AUDIT.md)
