# Codex Meter 0.1.0 — distribution candidate

This is an unpublished candidate for manual release review. It does not create a Git tag, public
release, installer, or registry package.

## Available today

- A Rust `codex-meter estimate` command for empirical current-capacity estimates.
- Python analytics that read an existing Codex Meter SQLite database in read-only mode.
- Empirical five-hour and weekly capacity estimation with confidence, sample accounting, and
  exclusion information.
- Optional manually supplied current-meter percentages.
- Human-readable and validated AnalyticsResult v1 JSON output.
- An installed Python worker selected with `--worker-mode installed` and an explicit `--python`
  interpreter.
- Native candidate executables for validated Linux x86_64, Windows x86_64, macOS x86_64, and
  macOS arm64 runners, paired with one shared `py3-none-any` analytics wheel.

The executable and wheel are separate components. A supported Python 3.11–3.13 environment is
required. This is not a standalone binary.

## Implemented internally, not fully integrated into the CLI

- Historical capacity trends.
- Remaining-workload prediction.
- Regime-change signal analysis.
- Controlled benchmark framework and statistical calibration.
- Authorized benchmark runtime and orchestration components.

These modules are internal capabilities and are not additional public CLI commands in this
candidate.

## Not supported as automatic user-facing functionality

- Automatic tracking startup or background monitoring.
- Live provider quota percentage acquisition.
- Production benchmark execution or provider-attested benchmark calibration.
- Additional analytics CLI commands.
- Automatic installation or update of software on a user's machine.

Manual meter inputs are unverified. All release validation uses synthetic local data and makes no
provider API calls.

**Codex Meter is independent and unofficial. Its capacity estimates are empirical and are not
documented provider quota allowances.**

## Platform and signing notes

The four target artifacts are only considered supported after the native build and extracted
installed-worker smoke job pass for each target. Windows Authenticode and Apple code signing and
notarization are not configured. SHA-256 manifests support integrity comparison; they do not
authenticate the publisher. No GitHub artifact attestation or other verifiable publisher signature
is included.
