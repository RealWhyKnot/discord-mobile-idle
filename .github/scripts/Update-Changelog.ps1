#!/usr/bin/env pwsh
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Append', 'Promote')]
    [string] $Mode,
    [string] $Range,
    [string] $Version
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$python = $null
foreach ($candidate in @('python3', 'python', 'py')) {
    if (Get-Command $candidate -ErrorAction SilentlyContinue) { $python = $candidate; break }
}
if (-not $python) { throw 'No python interpreter on PATH.' }

if ($Mode -eq 'Promote') {
    if (-not $Version) { throw 'Promote mode needs -Version, for example v2026.9.18.0.' }
    $scriptArgs = @('--promote', $Version)
} else {
    if (-not $Range) { throw 'Append mode needs -Range, for example abc123..def456.' }
    $scriptArgs = @('--range', $Range)
}

& $python (Join-Path $PSScriptRoot 'update_changelog.py') @scriptArgs
if ($LASTEXITCODE -ne 0) { throw "update_changelog.py failed ($LASTEXITCODE)" }
