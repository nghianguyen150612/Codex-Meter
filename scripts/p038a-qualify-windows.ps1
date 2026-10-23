param()

$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $false
$script:TestFailures = 0
$script:NativeTestStatus = "UNVERIFIED"
$script:RuntimeProbeStatus = "UNVERIFIED"

function Write-CommandSummary {
    param(
        [string]$Label,
        [string[]]$Arguments
    )

    Write-Output ("command=" + $Label)
    $lines = @(& cargo @Arguments 2>&1 | ForEach-Object { $_.ToString() })
    $exitCode = $LASTEXITCODE
    Write-Output ("exit_code=" + $exitCode)
    foreach ($line in $lines) {
        if ($line -match "^test result: ") {
            Write-Output $line
        }
    }
    if ($exitCode -ne 0) {
        foreach ($line in $lines) {
            if ($line -match "^test (.+) \.\.\. FAILED$") {
                Write-Output ("failed_test=" + $Matches[1])
            }
        }
        Write-Output "failure_details=raw test output withheld from sanitized evidence"
        $script:TestFailures++
    }
}

function Restore-EnvironmentValue {
    param(
        [string]$Name,
        [AllowNull()][string]$Value
    )
    if ($null -eq $Value) {
        Remove-Item ("Env:" + $Name) -ErrorAction SilentlyContinue
    } else {
        Set-Item ("Env:" + $Name) $Value
    }
}

$isWindows = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)
$architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
Write-Output ("operating_system=Windows")
Write-Output ("windows_build=" + [Environment]::OSVersion.Version.ToString())
Write-Output ("architecture=" + $architecture)
Write-Output ("powershell=" + $PSVersionTable.PSVersion.ToString())

if (-not $isWindows -or $architecture -ne "X64") {
    Write-Output "windows_native_tests=UNVERIFIED"
    Write-Output "windows_live_runtime=UNVERIFIED"
    Write-Output "diagnostic=Run this procedure in native Windows x64 PowerShell"
    exit 2
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$commit = (& git -C $repoRoot rev-parse HEAD 2>$null | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or -not $commit) {
    Write-Output "windows_native_tests=UNVERIFIED"
    Write-Output "diagnostic=Git could not identify the source commit"
    exit 2
}
Write-Output ("commit=" + $commit)
$gitStatus = @(& git -C $repoRoot status --porcelain 2>$null)
if ($LASTEXITCODE -ne 0 -or $gitStatus.Count -ne 0) {
    Write-Output "working_tree=dirty_or_unavailable"
    Write-Output "windows_native_tests=UNVERIFIED"
    Write-Output "diagnostic=Use a clean checkout so evidence identifies the tested source"
    exit 2
}
Write-Output "working_tree=clean"

$localDataPath = [System.Environment]::GetFolderPath(
    [System.Environment+SpecialFolder]::LocalApplicationData
)
try {
    $volumeRoot = [System.IO.Path]::GetPathRoot($localDataPath)
    $localVolume = [System.IO.DriveInfo]::new($volumeRoot)
    Write-Output ("local_journal_volume_type=" + $localVolume.DriveType.ToString())
    Write-Output ("local_journal_filesystem=" + $localVolume.DriveFormat)
    if ($localVolume.DriveType -ne [System.IO.DriveType]::Fixed) {
        Write-Output "windows_live_runtime=UNVERIFIED"
        Write-Output "diagnostic=LocalAppData is not on a fixed local drive"
        exit 2
    }
} catch {
    Write-Output "local_journal_volume_type=unknown"
    Write-Output "windows_live_runtime=UNVERIFIED"
    Write-Output "diagnostic=LocalAppData volume characteristics could not be verified"
    exit 2
}

try {
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [System.Security.Principal.WindowsPrincipal]::new($identity)
    $isAdministrator = $principal.IsInRole(
        [System.Security.Principal.WindowsBuiltInRole]::Administrator
    )
    Write-Output ("administrator_role=" + $isAdministrator.ToString().ToLowerInvariant())
    Write-Output "owner_sid=checked_by_native_security_tests; value_not_recorded"
} catch {
    Write-Output "administrator_role=unknown"
}

$pythonCommand = Get-Command python -CommandType Application -ErrorAction SilentlyContinue
$codexCommand = Get-Command codex -CommandType Application -ErrorAction SilentlyContinue
if ($null -ne $codexCommand) {
    $versionLines = @(& $codexCommand.Source --version 2>$null)
    $versionCode = $LASTEXITCODE
    if ($versionCode -eq 0 -and $versionLines.Count -gt 0) {
        Write-Output ("codex_cli_version=" + $versionLines[0].ToString().Trim())
    } else {
        Write-Output "codex_cli_version=unavailable"
    }

    if ($null -ne $pythonCommand) {
        $probeScript = Join-Path $repoRoot "scripts/p038a_probe_app_server.py"
        $probeArguments = @($probeScript, "--program", $codexCommand.Source)
        $probeLines = @(& $pythonCommand.Source @probeArguments 2>$null)
        $probeCode = $LASTEXITCODE
        foreach ($line in $probeLines) {
            if ($line -match "^(app_server_probe|app_server_user_agent|app_server_platform_family|app_server_platform_os|codex_home_path|diagnostic)=") {
                Write-Output $line
            }
        }
        if ($probeCode -eq 0) {
            $agentLine = $probeLines | Where-Object { $_ -match "^app_server_user_agent=" } |
                Select-Object -First 1
            if ($agentLine -eq "app_server_user_agent=codex_cli_rs/0.163.0") {
                $script:RuntimeProbeStatus = "PASS"
                Write-Output "audited_runtime_compatibility=PASS"
            } else {
                $script:RuntimeProbeStatus = "FAIL"
                Write-Output "audited_runtime_compatibility=FAIL"
            }
        }
    } else {
        Write-Output "app_server_probe=UNVERIFIED"
        Write-Output "diagnostic=Python is unavailable for the read-only runtime probe"
    }
} else {
    Write-Output "codex_cli=unavailable"
    Write-Output "app_server_probe=UNVERIFIED"
}
Write-Output "authentication_state=not_read_or_recorded; confirm locally in Codex"

$cargoCommand = Get-Command cargo -CommandType Application -ErrorAction SilentlyContinue
if ($null -eq $cargoCommand) {
    Write-Output "windows_native_tests=UNVERIFIED"
    Write-Output "diagnostic=Cargo is unavailable"
    exit 2
}
$rustcCommand = Get-Command rustc -CommandType Application -ErrorAction SilentlyContinue
if ($null -eq $rustcCommand) {
    Write-Output "windows_native_tests=UNVERIFIED"
    Write-Output "diagnostic=Rust is unavailable"
    exit 2
}
Write-Output ("rustc=" + (& rustc --version))
$hostLine = (& rustc -Vv | Select-String "^host:").ToString()
Write-Output ("rust_host=" + $hostLine.Substring(6))
$rustupCommand = Get-Command rustup -CommandType Application -ErrorAction SilentlyContinue
if ($null -ne $rustupCommand) {
    $toolchainLine = (& rustup show active-toolchain 2>$null | Select-Object -First 1)
    if ($toolchainLine) {
        Write-Output ("rust_toolchain=" + $toolchainLine.ToString().Split(" ")[0])
    }
    $targets = @(& rustup target list --installed 2>$null)
    if ($LASTEXITCODE -eq 0) {
        Write-Output ("rust_targets=" + ($targets -join ","))
    }
}
if ($null -eq $pythonCommand) {
    Write-Output "windows_native_tests=UNVERIFIED"
    Write-Output "diagnostic=Python is unavailable for the existing Windows fixture tests"
    exit 2
}

$testRoot = Join-Path $localDataPath ("codex-meter-p038a-" + [Guid]::NewGuid().ToString("N"))
$temporaryLocalData = Join-Path $testRoot "local-app-data"
$savedLocalAppData = $env:LOCALAPPDATA
$savedTemp = $env:TEMP
$savedTmp = $env:TMP
$locationPushed = $false

try {
    New-Item -ItemType Directory -Path $testRoot | Out-Null
    New-Item -ItemType Directory -Path $temporaryLocalData | Out-Null
    Set-Item Env:LOCALAPPDATA $temporaryLocalData
    Set-Item Env:TEMP $testRoot
    Set-Item Env:TMP $testRoot
    Push-Location $repoRoot
    $locationPushed = $true
    Write-CommandSummary "cargo fmt --manifest-path rust/Cargo.toml --all -- --check" @("fmt", "--manifest-path", "rust/Cargo.toml", "--all", "--", "--check")
    Write-CommandSummary "cargo clippy --manifest-path rust/Cargo.toml --workspace --all-targets --all-features -- -D warnings" @("clippy", "--manifest-path", "rust/Cargo.toml", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings")
    Write-CommandSummary "cargo test --manifest-path rust/Cargo.toml -p codex-meter --lib codex_app_server::execution_recovery" @("test", "--manifest-path", "rust/Cargo.toml", "-p", "codex-meter", "--lib", "codex_app_server::execution_recovery")
    Write-CommandSummary "cargo test --manifest-path rust/Cargo.toml -p codex-meter --test pty_execution_recovery_windows -- --test-threads=1" @("test", "--manifest-path", "rust/Cargo.toml", "-p", "codex-meter", "--test", "pty_execution_recovery_windows", "--", "--test-threads=1")
    Write-CommandSummary "cargo test --manifest-path rust/Cargo.toml --workspace --all-targets" @("test", "--manifest-path", "rust/Cargo.toml", "--workspace", "--all-targets")
    Write-CommandSummary "cargo test --manifest-path rust/Cargo.toml --workspace --all-targets --all-features" @("test", "--manifest-path", "rust/Cargo.toml", "--workspace", "--all-targets", "--all-features")
} finally {
    if ($locationPushed) {
        Pop-Location
    }
    Restore-EnvironmentValue "LOCALAPPDATA" $savedLocalAppData
    Restore-EnvironmentValue "TEMP" $savedTemp
    Restore-EnvironmentValue "TMP" $savedTmp
    if (Test-Path -LiteralPath $testRoot) {
        Remove-Item -LiteralPath $testRoot -Recurse -Force
    }
}

if ($script:TestFailures -eq 0) {
    $script:NativeTestStatus = "PASS"
    Write-Output "windows_native_security=PASS"
    Write-Output "windows_conpty_synthetic_tests=PASS"
} else {
    $script:NativeTestStatus = "FAIL"
    Write-Output "windows_native_security=FAIL"
    Write-Output "windows_conpty_synthetic_tests=FAIL"
}
Write-Output "windows_live_runtime=UNVERIFIED"
Write-Output "live_request_authorized=false"
Write-Output "diagnostic=The probe initializes the installed runtime only; live task scenarios require separate bounded authorization"

if ($script:NativeTestStatus -eq "FAIL") {
    exit 1
}
exit 2
