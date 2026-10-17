# Release readiness — Codex Meter 0.1.0

## Status

`READY_FOR_MANUAL_RELEASE_REVIEW` means a candidate passed the recorded build, installation,
compatibility, integrity, security, privacy, and regression checks. It does not authorize
publication. Any missing required platform or failed gate changes the status to `BLOCKED`.

This document describes the P030 candidate procedure. The aggregate CI artifact manifest records
the specific commit, workflow run, byte sizes, and SHA-256 hashes for each run; those values are
not copied into source before the post-merge run exists.

## Version and source identity

- Product and component version: `0.1.0`.
- Verified P030 source baseline: `a7de09f97456a6f5305d0fdf0f85d5134a06a91e` (`origin/main` before
  implementation).
- Authoritative policy: root [`VERSION`](../VERSION). Cargo metadata, Python project metadata,
  Python `__version__`, wheel filename, CLI version, archive names, and build manifests must match
  this file. `scripts/p030_release_candidate.py check-version` enforces the check.
- AnalyticsRequest schema v1, AnalyticsResult schema v1, analytics subprocess protocol `1.0.0`,
  estimator method `1.0.0`, workload method `1.0.0`, regime signal method `1.0.0`, benchmark
  framework/runtime methods `1.0.0`, and benchmark analysis method `1.0.0` remain independent of
  product version `0.1.0`.
- Candidate provenance records the exact checked-out commit SHA, repository, Git ref and event,
  workflow run ID and URL, Rust/Cargo/Python versions, target triple, component hashes, and build
  command. PR candidates are built from the feature-branch head SHA. The post-merge workflow builds
  from the resulting `main` merge commit SHA.

## Supported distribution targets

| Platform | Architecture | Rust target | Archive |
| --- | --- | --- | --- |
| Linux | x86_64 | `x86_64-unknown-linux-gnu` | `.tar.gz` |
| Windows | x86_64 | `x86_64-pc-windows-msvc` | `.zip` |
| macOS Intel | x86_64 | `x86_64-apple-darwin` | `.tar.gz` |
| macOS Apple Silicon | arm64 | `aarch64-apple-darwin` | `.tar.gz` |

Each target is built and smoke-tested on a native runner. A target is not supported if its job is
unavailable or fails. The separately maintained runtime matrix tests Python 3.11, 3.12, and 3.13
on Linux, macOS, and Windows. The analytics wheel declares `Requires-Python: >=3.11`, but the
verified support claim remains limited to those three tested versions.

## Distribution layout and installation contract

Each platform archive contains exactly the native executable, README, installation guide, license,
one copy of the shared analytics wheel, and `build-manifest.json`. One `py3-none-any` wheel is built
twice and the exact same validated bytes are reused for every bundle and the separately listed
wheel. The final candidate artifact also contains `artifact-manifest.json` and
`SHA256SUMS.txt`.

Installation requires the Rust executable plus `codex-meter-analytics` installed into a supported
Python 3.11–3.13 environment. Users explicitly select `--worker-mode installed` and `--python
PATH`. Virtual environments are not assumed to be relocatable. Installation and removal use
user-local paths and do not require administrator privileges. See
[`INSTALLATION.md`](INSTALLATION.md).

## Build and integrity procedure

The focused workflow runs on pull requests, `workflow_dispatch`, and pushes to `main`. It has
`contents: read` permission, builds with locked Cargo dependencies, validates the wheel's filename,
metadata, package allowlist, Python requirement, empty runtime dependency set, and RECORD hashes,
then creates native bundles from an explicit allowlist. It validates archive paths and file types,
extracts each archive to a fresh temporary directory outside the checkout, installs the wheel into a
fresh virtual environment without dependency downloads, and runs the relocated CLI against
synthetic SQLite data.

Wheel builds with identical source and `SOURCE_DATE_EPOCH` are byte-compared. Archive entries use
sorted file names and normalized timestamps; identical package inputs are archived twice and
byte-compared. Rust compiler and Cargo versions are recorded, but bit-for-bit native binary
reproducibility across builds, hosts, or toolchains is not claimed. The source SHA identifies code;
it does not imply identical binary bytes.

`SHA256SUMS.txt` covers every downloadable candidate archive, the shared wheel, and the aggregate
artifact manifest. It excludes its own checksum file. A checksum file delivered alongside an
artifact provides an integrity comparison and does not cryptographically authenticate the
publisher.

## Installation smoke and product behavior

For each native target, the extracted executable reports version `0.1.0`, shows estimate help,
loads the installed wheel from a fresh virtual environment, runs from an unrelated directory, and
returns sufficient and insufficient AnalyticsResult v1 output for synthetic history. The smoke
checks deterministic JSON, manual percentage arithmetic, Rust/Python version compatibility,
source-checkout independence, and unchanged SQLite bytes. Existing P029 CI continues to cover the
15 original jobs, including all nine OS/Python runtime cells. Rust end-to-end regression preserves
the P029 representative 1,159-byte result and SHA-256
`13ba5044594d01cff0313d14a6fd514883d4512b6278b5cd8db74f34be5a296a`.

## Security and privacy

- Release builds use the existing isolated Python `-I -S` launcher and strict source/installed
  worker resolution. No release-only shell or lower-security worker launch path is introduced.
- The release build omits the developer checkout root from the binary. Bundled use selects an
  installed wheel; a missing, ambiguous, or version-mismatched worker fails closed.
- Request/result limits, 60-second timeout, bounded diagnostics, process cleanup, exact completion
  marker, closed AnalyticsRequest validation, strict AnalyticsResult validation, and sanitized
  errors remain active.
- The worker reads SQLite in read-only/query-only mode, verifies published migration checksums,
  performs no automatic migrations, creates no database, and leaves historical rows unchanged.
- Archives are assembled from a fixed allowlist and contain no tests, credentials, environment
  files, telemetry, databases, build caches, or source history. CI uses synthetic fixtures and does
  not call provider APIs, read user authentication, run workloads, execute benchmarks, or consume
  quota.

## Known limits and residual risks

- Automatic tracker startup, live quota acquisition, background monitoring, additional CLI
  analyses, production benchmarks, and provider-attested calibration are not available.
- Product estimates are empirical and unofficial. Manual current-meter values are unverified.
- Apple signing/notarization and Windows Authenticode are not configured. These unsigned candidate
  executables may receive operating-system warnings. Installation documentation does not advise
  disabling security protections.
- No verified GitHub artifact attestation or other publisher signature is included. Checksums do
  not authenticate the publisher.
- A virtual environment depends on its original Python installation and may not be relocated.
- Native binary bit-for-bit reproducibility is not established.
- Cross-platform support claims rely on the exact native build and smoke jobs and the existing nine
  runtime matrix jobs. A failed or unavailable required job blocks the candidate classification.

## Signing and public-release judgment

Apple notarization and Windows Authenticode are not configured. GitHub-native artifact attestations
are technically available to public-repository workflows, but this candidate workflow neither
requests `id-token: write` / `attestations: write` nor emits and verifies an attestation. No
attestation is claimed. No unmanaged signing keys are introduced. The workflow does not create tags,
GitHub Releases, PyPI or crates.io packages, installers, or registry publications.

Final public-release judgment is `READY_FOR_MANUAL_RELEASE_REVIEW` only after all required PR CI,
the four native build and extraction smoke jobs, aggregate checksum validation, and the exact
post-merge `main` candidate run pass. Otherwise the judgment is `BLOCKED` and the failed or
unavailable gate must be named in the completion report.
