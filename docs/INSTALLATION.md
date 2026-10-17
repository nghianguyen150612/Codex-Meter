# Codex Meter 0.1.0 installation

Codex Meter 0.1.0 is an unpublished distribution candidate for review. It is a native Rust
executable plus the `codex-meter-analytics` Python wheel. It is not a standalone binary or an
installer.

The installed command uses three local components:

```text
codex-meter executable
Python 3.11, 3.12, or 3.13 environment
codex-meter-analytics 0.1.0 wheel installed in that environment
```

Use the archive, wheel, `artifact-manifest.json`, and `SHA256SUMS.txt` from the same candidate
workflow run. The platform archive includes a copy of the shared wheel; CI verifies that copy is
byte-for-byte identical to the separately listed `py3-none-any` wheel. No administrator privileges
are needed for the steps below. The estimate command requires an existing compatible Codex Meter
SQLite database and never creates or migrates it.

## Verify downloaded files

Keep the candidate files together in one directory. Check all entries in `SHA256SUMS.txt` before
extracting or installing them.

Linux:

```bash
sha256sum -c SHA256SUMS.txt
```

macOS:

```bash
shasum -a 256 -c SHA256SUMS.txt
```

Windows PowerShell:

```powershell
Get-Content .\SHA256SUMS.txt | ForEach-Object {
    $expected, $name = $_ -split '  ', 2
    if ((Get-FileHash -Path $name -Algorithm SHA256).Hash.ToLower() -ne $expected) {
        throw "Checksum mismatch: $name"
    }
}
```

The SHA-256 list detects differences from the values you received. A checksum file downloaded beside
an archive does not, by itself, authenticate who published either file.

## Arch Linux and other Linux distributions

Choose the `linux-x86_64` archive. Make sure a supported Python interpreter and its `venv` and
`pip` modules are installed. On Arch Linux, use a user-managed Python 3.11–3.13 installation if the
system Python is newer than 3.13. A locally installed Python manager such as `uv` can install a
supported interpreter without changing the system Python. On distributions that split `venv` into a
separate system package, ask the administrator to provide that module or use a user-managed Python.

The following example assumes the archive, wheel, manifest, and checksum file are in
`$HOME/Downloads/codex-meter-candidate` and Python 3.13 is available as `python3.13`:

```bash
cd "$HOME/Downloads/codex-meter-candidate"
sha256sum -c SHA256SUMS.txt
tar -xzf codex-meter-0.1.0-linux-x86_64.tar.gz

mkdir -p "$HOME/.local/bin" "$HOME/.local/share/codex-meter"
install -m 0755 codex-meter-0.1.0-linux-x86_64/codex-meter \
  "$HOME/.local/bin/codex-meter"
python3.13 -m venv "$HOME/.local/share/codex-meter/venv"
"$HOME/.local/share/codex-meter/venv/bin/python" -m pip install \
  --no-index --no-deps codex-meter-analytics-0.1.0-py3-none-any.whl
"$HOME/.local/bin/codex-meter" --version
"$HOME/.local/bin/codex-meter" estimate --help
```

If `$HOME/.local/bin` is not already on `PATH`, add this line to `~/.bashrc` or `~/.zshrc`, then
open a new terminal:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

For Python 3.11 or 3.12, substitute the matching interpreter command and use the virtual
environment's `bin/python` path in the estimate command.

## Windows 11

Choose the `windows-x86_64` archive. Install Python 3.11, 3.12, or 3.13 for your user and enable its
Python launcher or command-line entry. Use a fresh PowerShell window after changing `PATH`.

This example assumes the candidate files are in `$HOME\Downloads\codex-meter-candidate` and the
Python launcher has Python 3.13 available:

```powershell
Set-Location "$HOME\Downloads\codex-meter-candidate"
Get-Content .\SHA256SUMS.txt | ForEach-Object {
    $expected, $name = $_ -split '  ', 2
    if ((Get-FileHash -Path $name -Algorithm SHA256).Hash.ToLower() -ne $expected) {
        throw "Checksum mismatch: $name"
    }
}

$installRoot = Join-Path $env:LOCALAPPDATA 'CodexMeter'
$programDirectory = Join-Path $env:LOCALAPPDATA 'Programs\Codex Meter\bin'
$environment = Join-Path $installRoot 'venv'
Expand-Archive .\codex-meter-0.1.0-windows-x86_64.zip -DestinationPath $installRoot
New-Item -ItemType Directory -Force $programDirectory | Out-Null
Copy-Item (Join-Path $installRoot 'codex-meter-0.1.0-windows-x86_64\codex-meter.exe') `
    (Join-Path $programDirectory 'codex-meter.exe')
py -3.13 -m venv $environment
& (Join-Path $environment 'Scripts\python.exe') -m pip install --no-index --no-deps `
    .\codex-meter-analytics-0.1.0-py3-none-any.whl
& (Join-Path $programDirectory 'codex-meter.exe') --version
& (Join-Path $programDirectory 'codex-meter.exe') estimate --help
```

Add `$programDirectory` to your **user** `Path` through Windows Environment Variables. Do not edit
the system `Path` or run the installer steps as Administrator. If Windows SmartScreen warns about an
unsigned candidate, do not disable Windows security protections to continue; these artifacts have
not been Authenticode signed.

## macOS Intel and Apple Silicon

Choose the `macos-x86_64` archive for Intel Macs or the `macos-arm64` archive for Apple Silicon.
Install Python 3.11, 3.12, or 3.13 for your architecture. The shell steps below assume the archive,
wheel, manifest, and checksum file are in `~/Downloads/codex-meter-candidate` and `python3.13` is
available:

```bash
cd "$HOME/Downloads/codex-meter-candidate"
shasum -a 256 -c SHA256SUMS.txt
tar -xzf codex-meter-0.1.0-macos-arm64.tar.gz

mkdir -p "$HOME/.local/bin" "$HOME/.local/share/codex-meter"
install -m 0755 codex-meter-0.1.0-macos-arm64/codex-meter \
  "$HOME/.local/bin/codex-meter"
python3.13 -m venv "$HOME/.local/share/codex-meter/venv"
"$HOME/.local/share/codex-meter/venv/bin/python" -m pip install \
  --no-index --no-deps codex-meter-analytics-0.1.0-py3-none-any.whl
"$HOME/.local/bin/codex-meter" --version
"$HOME/.local/bin/codex-meter" estimate --help
```

For an Intel Mac, use the `macos-x86_64` archive name and executable directory in the extraction
and copy commands. If `$HOME/.local/bin` is not on `PATH`, add
`export PATH="$HOME/.local/bin:$PATH"` to `~/.zshrc` and open a new terminal.

The candidate executable is not signed or notarized by Apple. Do not disable Gatekeeper or other
macOS protections to run it. A future public distribution needs its own reviewed signing and
notarization setup if seamless default-policy installation is required.

## Run an estimate

Check both help screens and the product version after installing. The database and request paths
below are placeholders; the database must already exist and contain compatible Codex Meter history.
Create an AnalyticsRequest v1 JSON file using the synthetic example in the
[CLI analytics guide](https://github.com/nghianguyen150612/Codex-Meter/blob/main/docs/CLI_ANALYTICS_INTEGRATION.md).
The CLI never seeds, migrates, or silently creates a database.

Linux and macOS:

```bash
codex-meter --version
codex-meter estimate --help
codex-meter estimate \
  --database /path/to/existing.sqlite \
  --request /path/to/request.json \
  --generated-at 2026-10-16T07:00:00Z \
  --five-hour-used-percent 24 \
  --python "$HOME/.local/share/codex-meter/venv/bin/python" \
  --worker-mode installed \
  --json
```

Windows PowerShell:

```powershell
$cli = Join-Path $env:LOCALAPPDATA 'Programs\Codex Meter\bin\codex-meter.exe'
$python = Join-Path $env:LOCALAPPDATA 'CodexMeter\venv\Scripts\python.exe'
& $cli --version
& $cli estimate --help
& $cli estimate `
    --database 'C:\path\to\existing.sqlite' `
    --request 'C:\path\to\request.json' `
    --generated-at '2026-10-16T07:00:00Z' `
    --five-hour-used-percent 24 `
    --python $python `
    --worker-mode installed `
    --json
```

`--five-hour-used-percent` is a percentage supplied manually by the user. Codex Meter does not
acquire or verify the current provider meter value. An estimate is empirical and is not a provider
quota allowance.

## Update or uninstall

Install the executable and wheel from the same candidate version together. For an update, stop
using the CLI, replace the user-local executable, and reinstall the matching wheel in the same
virtual environment with `python -m pip install --no-index --no-deps --force-reinstall <wheel>`.
Virtual environments are tied to their Python installation and are not guaranteed to be relocatable.

To uninstall, remove the user-local executable and the Codex Meter virtual environment. On Linux
and macOS, remove `$HOME/.local/bin/codex-meter` and `$HOME/.local/share/codex-meter`. On Windows,
remove `%LOCALAPPDATA%\Programs\Codex Meter\bin` and `%LOCALAPPDATA%\CodexMeter`; remove the user
`Path` entry if it points to the deleted binary directory.

## Troubleshooting

- **No supported Python interpreter:** create a Python 3.11–3.13 environment, then pass its exact
  interpreter path with `--python`. Python 3.14 and later are not yet in the supported matrix.
- **`python_worker_resolution_failed`:** install the matching wheel in the selected virtual
  environment and pass both `--python` and `--worker-mode installed`. The CLI fails closed if the
  wheel is missing, duplicated, or has a version different from the executable.
- **`database_missing`:** check the path. Codex Meter requires an existing compatible SQLite file
  and will not create one.
- **`database_open_failed`:** check user read permissions and whether another process is holding
  an incompatible database lock. The estimate process opens history read-only and runs no
  migrations.

The bundle's build manifest records its exact source SHA, target triple, component hashes, and
workflow run. Native executables are unsigned, wheel package contents are allowlisted, and the
candidate has no standalone publisher signature or verified GitHub artifact attestation.
