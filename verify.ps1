[CmdletBinding()]
param(
    [switch]$SkipExe,
    [switch]$SkipDocker
)

$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

$python = Join-Path $PSScriptRoot '.venv\Scripts\python.exe'
if (-not (Test-Path $python)) { $python = 'python' }

$failures = @()

function Start-Step {
    param([string]$Name)
    Write-Host ''
    Write-Host "== $Name" -ForegroundColor Cyan
}

function Assert-ExitCode {
    param([string]$Name, [int]$Expected = 0)
    if ($LASTEXITCODE -ne $Expected) {
        throw "$Name exited $LASTEXITCODE, expected $Expected"
    }
}

Start-Step 'Lint'
& $python -m ruff format --check .
Assert-ExitCode 'ruff format'
& $python -m ruff check .
Assert-ExitCode 'ruff check'

Start-Step 'Tests'
& $python -m pytest
Assert-ExitCode 'pytest'

Start-Step 'Container code'
$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $cargo) {
    Write-Host 'cargo is not on PATH, skipping the Rust checks'
    $failures += 'Rust checks not run (no cargo)'
} else {
    cargo fmt --check
    Assert-ExitCode 'cargo fmt'
    if ($env:OS -eq 'Windows_NT') {
        Write-Host 'the TLS library needs NASM to build on Windows, leaving clippy and the tests to verify.local.ps1 and CI'
    } else {
        cargo clippy --locked --all-targets --quiet -- -D warnings
        Assert-ExitCode 'cargo clippy'
        cargo test --locked --quiet
        Assert-ExitCode 'cargo test'
        Write-Host 'Rust code is formatted, lint clean and passes its tests'
    }
}

Start-Step 'Container image'
$docker = Get-Command docker -ErrorAction SilentlyContinue
if ($SkipDocker) {
    Write-Host 'skipped by request'
} elseif (-not $docker) {
    Write-Host 'docker is not on PATH, leaving the image build to verify.local.ps1'
} else {
    docker build -t discord-mobile-idle:verify .
    Assert-ExitCode 'docker build'
    docker run --rm -e DISCORD_TOKEN= discord-mobile-idle:verify
    Assert-ExitCode 'container smoke' 2
    Write-Host 'image builds and rejects an empty token'
}

Start-Step 'Windows client'
if ($SkipExe) {
    Write-Host 'skipped by request'
} elseif ($env:OS -ne 'Windows_NT') {
    Write-Host 'not Windows, skipping the executable'
    $failures += 'executable not built (not Windows)'
} else {
    & $python -m PyInstaller --onefile --clean --noconfirm --noconsole --log-level WARN `
        --name discord-mobile-idle --distpath dist/verify --workpath build/verify `
        --paths . --icon packaging/icon.ico `
        --collect-all curl_cffi --collect-all discord_protos `
        --collect-submodules discord --collect-submodules pystray `
        --hidden-import audioop packaging/entry.py
    Assert-ExitCode 'pyinstaller'
    $selfTestLog = Join-Path $env:LOCALAPPDATA 'discord-mobile-idle\self-test.log'
    Remove-Item $selfTestLog -Force -ErrorAction SilentlyContinue
    $run = Start-Process -FilePath 'dist/verify/discord-mobile-idle.exe' -ArgumentList '--self-test' -Wait -PassThru
    if ($run.ExitCode -ne 0) {
        if (Test-Path $selfTestLog) { Get-Content $selfTestLog }
        throw "self test exited $($run.ExitCode)"
    }
    Write-Host 'executable builds and self-tests clean'
}

Start-Step 'Local extras'
$local = Join-Path $PSScriptRoot 'verify.local.ps1'
if (-not (Test-Path $local)) {
    Write-Host 'no verify.local.ps1, nothing extra to run'
} else {
    & $local
    if ($LASTEXITCODE -ne 0) { throw "verify.local.ps1 exited $LASTEXITCODE" }
}

Write-Host ''
if ($failures.Count -gt 0) {
    Write-Host 'Incomplete:' -ForegroundColor Yellow
    foreach ($item in $failures) { Write-Host "  $item" -ForegroundColor Yellow }
    exit 1
}
Write-Host 'Both entry points verified.' -ForegroundColor Green
exit 0
