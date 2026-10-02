# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Run in a Visual Studio developer PowerShell with the prerequisites in WINDOWS.md.
# Produces an unsigned client build; does not install, enable startup or host services.
[CmdletBinding()]
param([switch] $DebugBuild, [switch] $NoBundle)
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Run this script on Windows.' }
foreach ($tool in @('cargo', 'rustc', 'node', 'pnpm', 'protoc', 'wasm-bindgen', 'cl', 'link')) {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        throw "Missing $tool. See WINDOWS.md; use a Visual Studio developer PowerShell."
    }
}
$appDir = Split-Path $PSScriptRoot -Parent
if (-not $env:PROTOC_INCLUDE) {
    $protocBin = (Get-Command protoc).Source
    $includeDir = Join-Path (Split-Path $protocBin -Parent) '../include'
    if (-not (Test-Path -LiteralPath $includeDir)) {
        throw 'Set PROTOC_INCLUDE to the protoc package include directory.'
    }
    $env:PROTOC_INCLUDE = (Resolve-Path -LiteralPath $includeDir).Path
}
Push-Location $appDir
try {
    & pnpm install --frozen-lockfile
    if ($LASTEXITCODE -ne 0) { throw 'Dependency installation failed' }
    & "$PSScriptRoot/prepare-cli-sidecar.ps1" -DebugBuild:$DebugBuild
    $tauriArgs = @('tauri', 'build', '--config', 'src-tauri/tauri.sidecar.conf.json',
        '--config', 'src-tauri/tauri.windows-port.conf.json')
    if ($DebugBuild) { $tauriArgs += '--debug' }
    if ($NoBundle) { $tauriArgs += '--no-bundle' } else { $tauriArgs += @('--bundles', 'nsis,msi') }
    & pnpm @tauriArgs
    if ($LASTEXITCODE -ne 0) { throw 'Windows app build failed' }
} finally { Pop-Location }
