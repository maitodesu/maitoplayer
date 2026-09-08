[CmdletBinding()]
param(
  [Parameter(Mandatory)]
  [string]$Ffmpeg,

  [Parameter(Mandatory)]
  [string]$Ffprobe,

  [string]$FixtureDirectory = 'fixtures/media/generated'
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$ffmpegPath = (Resolve-Path -LiteralPath $Ffmpeg).Path
$ffprobePath = (Resolve-Path -LiteralPath $Ffprobe).Path
$fixtureRoot = (Resolve-Path -LiteralPath (Join-Path $repoRoot $FixtureDirectory)).Path

foreach ($tool in @($ffmpegPath, $ffprobePath)) {
  if (-not (Test-Path -LiteralPath $tool -PathType Leaf)) {
    throw "Media tool is not a file: $tool"
  }
}

$required = @{
  MIGAKU_TEST_REMUX_FIXTURE = 'h264-aac.mkv'
  MIGAKU_TEST_EMBEDDED_FIXTURE = 'multi-track.mkv'
  MIGAKU_TEST_AUDIO_FIXTURE = 'h264-flac.mkv'
  MIGAKU_TEST_VIDEO_FIXTURE = 'vp9-opus.webm'
  MIGAKU_TEST_DIRECT_FIXTURE = 'known-markers.mp4'
}
foreach ($entry in $required.GetEnumerator()) {
  $path = Join-Path $fixtureRoot $entry.Value
  if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
    throw "Required generated fixture is missing: $($entry.Value)"
  }
}

$sumPath = Join-Path $fixtureRoot 'SHA256SUMS'
if (-not (Test-Path -LiteralPath $sumPath -PathType Leaf)) {
  throw 'Generated fixture SHA256SUMS is missing.'
}
$files = Get-ChildItem -LiteralPath $fixtureRoot -Recurse -File |
  Where-Object Name -ne 'SHA256SUMS'
$actual = @{}
foreach ($file in $files) {
  $relativeName = [IO.Path]::GetRelativePath($fixtureRoot, $file.FullName).Replace('\', '/')
  $actual[$relativeName] = (Get-FileHash -Algorithm SHA256 -LiteralPath $file.FullName).Hash.ToLowerInvariant()
}
$expectedNames = [System.Collections.Generic.HashSet[string]]::new(
  [System.StringComparer]::Ordinal
)
foreach ($line in Get-Content -LiteralPath $sumPath) {
  if ($line -notmatch '^([0-9a-f]{64})  (.+)$') {
    throw "Malformed SHA256SUMS line: $line"
  }
  $expectedHash = $Matches[1]
  $name = $Matches[2]
  if ($name.Contains('\') -or [IO.Path]::IsPathFullyQualified($name) -or
      $name -match '(^|/)\.\.(/|$)' -or $name.StartsWith('/')) {
    throw "SHA256SUMS entry is not a safe relative path: $name"
  }
  if (-not $expectedNames.Add($name)) {
    throw "Duplicate SHA256SUMS entry: $name"
  }
  if (-not $actual.ContainsKey($name) -or $actual[$name] -ne $expectedHash) {
    throw "Generated fixture checksum mismatch: $name"
  }
}
if ($expectedNames.Count -ne $actual.Count) {
  $missing = @($actual.Keys | Where-Object { -not $expectedNames.Contains($_) } | Sort-Object)
  throw "Generated files are missing from SHA256SUMS: $($missing -join ', ')"
}

$saved = @{}
$names = @('MIGAKU_TEST_FFMPEG', 'MIGAKU_TEST_FFPROBE') + @($required.Keys)
foreach ($name in $names) {
  $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
try {
  $env:MIGAKU_TEST_FFMPEG = $ffmpegPath
  $env:MIGAKU_TEST_FFPROBE = $ffprobePath
  foreach ($entry in $required.GetEnumerator()) {
    [Environment]::SetEnvironmentVariable(
      $entry.Key,
      (Join-Path $fixtureRoot $entry.Value),
      'Process'
    )
  }
  Push-Location $repoRoot
  try {
    cargo test -p media-engine -- --ignored --nocapture
    if ($LASTEXITCODE -ne 0) {
      throw "Real media-tool integration tests failed with exit code $LASTEXITCODE."
    }
  } finally {
    Pop-Location
  }
} finally {
  foreach ($name in $names) {
    [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process')
  }
}

Write-Host "Real media-tool lane passed with $($actual.Count) verified generated files."
