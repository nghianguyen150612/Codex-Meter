# One-command installation scripts (v0.1.0)

The v0.1.0 GitHub prerelease distributes four native executables and a matching Python wheel. These installation helpers are maintained separately on the `main` branch; they install the **unchanged, checksum-pinned v0.1.0 release assets**. They do not modify the v0.1.0 tag or released binaries.

## Linux x86_64 and macOS Intel/Apple Silicon

Review the [installer source](../scripts/install.sh), then run:

```bash
curl -fL https://raw.githubusercontent.com/nghianguyen150612/Codex-Meter/main/scripts/install.sh -o codex-meter-install.sh
bash codex-meter-install.sh
```

The installer detects the OS/architecture, downloads the matching v0.1.0 binary archive and shared Python wheel, checks **each release asset against its pinned SHA-256**, installs a Python 3.11–3.13 wheel into a user-local virtual environment, and creates an executable `~/.local/bin/codex-meter` launcher. This launcher supplies `--python` and `--worker-mode installed` for `estimate`, so only database, request, timestamp, and optional meter flags remain to be provided.

Prerequisites: `curl`, `tar`, and the OS SHA-256 utility. If Python 3.11–3.13 is not already installed, the installer can use an existing `uv` executable to acquire Python 3.13. On systems without `uv`, install a supported Python version and its `venv` component. To choose a particular trusted interpreter, set `CODEX_METER_PYTHON=/absolute/path/to/python3.13`.

The installer does not request root privileges or modify a shell startup file. If needed, add `$HOME/.local/bin` to `PATH` yourself. Re-running the script reinstalls the pinned wheel and executable.

## Windows x86_64

Review the [PowerShell installer source](../scripts/install.ps1), then download it:

```powershell
Invoke-WebRequest https://raw.githubusercontent.com/nghianguyen150612/Codex-Meter/main/scripts/install.ps1 -OutFile .\codex-meter-install.ps1
.\codex-meter-install.ps1
```

The PowerShell script requires Python 3.11–3.13 available through the Windows `py` launcher or an installed Python executable. It installs the signed-by-checksum (but **not code-signed**) v0.1.0 assets under your user profile, creates its own virtual environment, and adds the binary directory to your **user** `PATH`. Reopen your terminal afterward.

If your PowerShell execution policy blocks running downloaded scripts, review the script and your organization's policy before choosing an allowed execution method; the installer does not change execution policy or disable system protections.

Windows currently requires passing the installed interpreter explicitly to `estimate`:

```powershell
codex-meter estimate --database PATH --request FILE --generated-at 2026-10-16T07:00:00Z --python "$env:LOCALAPPDATA\CodexMeter\venv\Scripts\python.exe" --worker-mode installed --five-hour-used-percent 24
```

## Test the installed command

```bash
codex-meter --version
codex-meter estimate --help
```

**Scope:** `codex-meter estimate` requires an existing compatible Codex Meter SQLite history database and a valid AnalyticsRequest v1 JSON file. Automatic tracking, quota acquisition, and background monitoring are not included in v0.1.0. Manual current used percentages remain unverified. See [CLI analytics instructions](CLI_ANALYTICS_INTEGRATION.md) for example requests and [installation reference](INSTALLATION.md) for manual steps.

**Security:** checksum pinning confirms file bytes against hashes checked into these scripts; it does not independently authenticate the GitHub publisher. Inspect remote scripts before execution. No credentials, provider APIs, live benchmarks, or production data are accessed by these installers.
