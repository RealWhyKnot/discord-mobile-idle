#!/usr/bin/env pwsh
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Tag
)

$ErrorActionPreference = 'Stop'

if ($Tag -notmatch '^v(\d{4})\.(\d+)\.(\d+)\.(\d+)(-beta)?$') {
    throw "Release tag must be vYYYY.M.D.N or vYYYY.M.D.N-beta, got '$Tag'."
}

$dateStamp = "$($Matches[1]).$($Matches[2]).$($Matches[3])"
$actual = [int] $Matches[4]
$pattern = "^v$([regex]::Escape($dateStamp))\.(\d+)(-beta)?$"
$highest = -1

foreach ($existing in @(git tag --list "v$dateStamp.*")) {
    if ($existing -eq $Tag) { continue }
    if ($existing -match $pattern -and [int]$Matches[1] -gt $highest) { $highest = [int]$Matches[1] }
}

$expected = $highest + 1
if ($actual -ne $expected) {
    throw "Release tag $Tag uses revision $actual, expected $expected for $dateStamp. Use .0 when no release exists for the day, otherwise increment the highest same-day revision by one."
}

Write-Host "Release tag $Tag uses the expected same-day revision $expected."
