# Codex Meter v0.1.0 user-local installer for Windows x86_64.
# Usage: powershell -NoProfile -File .\install.ps1
[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$version = "0.1.0"
$base = "https://github.com/nghianguyen150612/Codex-Meter/releases/download/v0.1.0"
$archive = "codex-meter-0.1.0-windows-x86_64.zip"
$archiveSha = "b6295c8522ca7b4c0bea03d8582d7153d6dba6a5aa372dccfa429b9982024d68"
$wheel = "codex_meter_analytics-0.1.0-py3-none-any.whl"
$wheelSha = "98777ada1c620ecc390b8d1f67d0c6b5312b13e30b6211eff95b33879e62c39f"

if ([Environment]::Is64BitOperatingSystem -ne $true) {
    throw "Codex Meter v0.1.0 requires Windows x86_64."
}
$root = Join-Path $env:LOCALAPPDATA "CodexMeter"
$programDir = Join-Path $env:LOCALAPPDATA "Programs\Codex Meter\bin"
$venv = Join-Path $root "venv"
$venvPython = Join-Path $venv "Scripts\python.exe"
$temp = Join-Path ([IO.Path]::GetTempPath()) ("codex-meter-install-" + [Guid]::NewGuid().ToString("N"))

function Download-Verified([string]$Name, [string]$Sha) {
    $destination = Join-Path $temp $Name
    Invoke-WebRequest -Uri "$base/$Name" -OutFile $destination -UseBasicParsing
    $actual = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $Sha) {
        throw "SHA-256 verification failed for $Name. Nothing was installed."
    }
    return $destination
}

function Find-Python {
    $launcher = Get-Command py -ErrorAction SilentlyContinue
    if ($launcher) {
        foreach ($v in @("3.13", "3.12", "3.11")) {
            & $launcher.Source "-$v" -c "import sys; sys.exit(0 if (3, 11) <= sys.version_info[:2] <= (3, 13) else 1)" *> $null
            if ($LASTEXITCODE -eq 0) {
                return @($launcher.Source, "-$v")
            }
        }
    }
    foreach ($name in @("python3.13", "python3.12", "python3.11", "python")) {
        $exe = Get-Command $name -ErrorAction SilentlyContinue
        if (-not $exe) { continue }
        & $exe.Source -c "import sys; sys.exit(0 if (3, 11) <= sys.version_info[:2] <= (3, 13) else 1)" *> $null
        if ($LASTEXITCODE -eq 0) {
            return @($exe.Source)
        }
    }
    throw "Python 3.11, 3.12 or 3.13 is required. Install one for your user and retry."
}

New-Item -ItemType Directory -Path $temp -Force | Out-Null
try {
    Write-Host "Downloading checksum-pinned Codex Meter $version..."
    $archivePath = Download-Verified $archive $archiveSha
    $wheelPath = Download-Verified $wheel $wheelSha

    $pythonArgs = @(Find-Python)
    $pythonExe = $pythonArgs[0]
    $pythonOptions = @($pythonArgs | Select-Object -Skip 1)
    New-Item -ItemType Directory -Path $root, $programDir -Force | Out-Null
    if (-not (Test-Path -LiteralPath (Join-Path $venv "pyvenv.cfg"))) {
        & $pythonExe @pythonOptions -m venv $venv
        if ($LASTEXITCODE -ne 0) { throw "Failed to create Python virtual environment." }
    }

    if (-not (Test-Path -LiteralPath $venvPython -PathType Leaf)) {
        throw "The Codex Meter virtual environment is missing Python."
    }
    & $venvPython -c "import sys; sys.exit(0 if (3, 11) <= sys.version_info[:2] <= (3, 13) else 1)"
    if ($LASTEXITCODE -ne 0) {
        throw "Existing virtual environment uses unsupported Python. Recreate it and retry."
    }
    & $venvPython -m pip install --no-index --no-deps --force-reinstall $wheelPath
    if ($LASTEXITCODE -ne 0) { throw "Python analytics wheel installation failed." }
    & $venvPython -c "from importlib.metadata import version; assert version('codex-meter-analytics') == '0.1.0'"
    if ($LASTEXITCODE -ne 0) { throw "Python analytics version verification failed." }

    Expand-Archive -LiteralPath $archivePath -DestinationPath $temp -Force
    $binary = Join-Path $temp "codex-meter-0.1.0-windows-x86_64\codex-meter.exe"
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
        throw "Release archive does not contain the expected executable."
    }
    $destination = Join-Path $programDir "codex-meter.exe"
    Copy-Item -LiteralPath $binary -Destination $destination -Force

    & $destination --version
    if ($LASTEXITCODE -ne 0) { throw "Installed executable failed version check." }

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $entries = @($userPath -split ";" | Where-Object { $_ })
    if ($entries -notcontains $programDir) {
        $newPath = (@($entries) + $programDir) -join ";"
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    }

    Write-Host "Codex Meter $version installed for the current user."
    Write-Host "Executable: $destination"
    Write-Host "Python:     $venvPython"
    Write-Host "Reopen your terminal to use the updated user PATH."
    Write-Host ('Example: codex-meter estimate --database PATH --request FILE --generated-at UTC --python "' + $venvPython + '" --worker-mode installed')
    Write-Host "Existing Codex Meter SQLite data is required; automatic tracking is not included."
}
finally {
    if (Test-Path -LiteralPath $temp) {
        Remove-Item -LiteralPath $temp -Recurse -Force
    }
}