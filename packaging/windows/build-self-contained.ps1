[CmdletBinding()]
param(
  [string]$CacheDirectory,
  [switch]$Offline
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$stageArguments = @{}
if ($CacheDirectory) { $stageArguments.CacheDirectory = $CacheDirectory }
if ($Offline) { $stageArguments.Offline = $true }
& (Join-Path $PSScriptRoot 'stage-runtime-dependencies.ps1') @stageArguments

Push-Location (Join-Path $repoRoot 'apps/desktop/src-tauri')
try {
  & '..\ui\node_modules\.bin\tauri.CMD' build --ci --bundles nsis --config target\tauri.self-contained.json
  if ($LASTEXITCODE -ne 0) {
    throw "Tauri self-contained NSIS build failed with exit code $LASTEXITCODE."
  }
} finally {
  Pop-Location
}
