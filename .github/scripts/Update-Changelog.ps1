#!/usr/bin/env pwsh
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Append')]
    [string] $Mode,
    [Parameter(Mandatory = $true)]
    [string] $Range
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$python = $null
foreach ($candidate in @('python3', 'python', 'py')) {
    if (Get-Command $candidate -ErrorAction SilentlyContinue) { $python = $candidate; break }
}
if (-not $python) { throw 'No python interpreter on PATH.' }

& $python (Join-Path $PSScriptRoot 'update_changelog.py') --range $Range
if ($LASTEXITCODE -ne 0) { throw "update_changelog.py failed ($LASTEXITCODE)" }
