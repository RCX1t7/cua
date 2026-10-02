# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

<#
Stage the Spaces CLI on Windows without requiring a Unix shell.
Like prepare-cli-sidecar.sh, this builds cua-spaces-cli, not cua-cli.
#>
[CmdletBinding()]
param(
    [string] $Target,
    [string] $Binary,
    [switch] $DebugBuild
)
$ErrorActionPreference = 'Stop'
$appDir = Split-Path $PSScriptRoot -Parent
$repoDir = (Resolve-Path (Join-Path $appDir '../..')).Path
if (-not $Target) {
    $hostInfo = & rustc -vV
    if ($LASTEXITCODE -ne 0) { throw 'rustc failed' }
    $Target = ($hostInfo | Where-Object { $_ -match '^host: ' }) -replace '^host: ', ''
}
if ($Target -notmatch '^\w+-pc-windows-(msvc|gnu)$') {
    throw 'This script stages a Windows target. Use prepare-cli-sidecar.sh for other platforms.'
}
$profile = if ($DebugBuild) { 'debug' } else { 'release' }
if (-not $Binary) {
    $cargoArgs = @('build', '--locked', '-p', 'cua-spaces-cli', '--target', $Target,
        '--manifest-path', (Join-Path $repoDir 'libs/cua/Cargo.toml'))
    if (-not $DebugBuild) { $cargoArgs += '--release' }
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw 'Spaces CLI build failed' }
    $targetDir = if ($env:CARGO_TARGET_DIR) {
        [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
    } else { Join-Path $repoDir 'libs/cua/target' }
    $Binary = Join-Path $targetDir "$Target/$profile/cua-spaces-cli.exe"
}
$source = (Resolve-Path -LiteralPath $Binary).Path
$outDir = Join-Path $appDir 'src-tauri/binaries'
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$outFile = Join-Path $outDir "cua-$Target.exe"
Copy-Item -LiteralPath $source -Destination $outFile -Force
Write-Output "staged $outFile"
