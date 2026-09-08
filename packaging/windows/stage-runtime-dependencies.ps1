[CmdletBinding()]
param(
  [string]$CacheDirectory,
  [switch]$Offline
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$lockPath = Join-Path $PSScriptRoot 'runtime-dependencies.lock.json'
$lock = Get-Content -Raw -LiteralPath $lockPath | ConvertFrom-Json
if ($lock.schemaVersion -ne 1) {
  throw "Unsupported runtime dependency lock schema: $($lock.schemaVersion)"
}

if (-not $CacheDirectory) {
  $CacheDirectory = Join-Path $repoRoot '.cache/release-dependencies'
}
[IO.Directory]::CreateDirectory($CacheDirectory) | Out-Null
$CacheDirectory = (Resolve-Path -LiteralPath $CacheDirectory).Path

function Assert-HashAndSize {
  param(
    [Parameter(Mandatory)][string]$Path,
    [Parameter(Mandatory)][string]$ExpectedSha256,
    [Nullable[long]]$ExpectedSize
  )
  $item = Get-Item -LiteralPath $Path
  if (-not $item.PSIsContainer -and $null -ne $ExpectedSize -and $item.Length -ne $ExpectedSize) {
    throw "Size mismatch for $Path. Expected $ExpectedSize, received $($item.Length)."
  }
  $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash
  if (-not $actual.Equals($ExpectedSha256, [StringComparison]::OrdinalIgnoreCase)) {
    throw "SHA-256 mismatch for $Path. Expected $ExpectedSha256, received $actual."
  }
}

function Get-PinnedArtifact {
  param(
    [Parameter(Mandatory)]$Artifact,
    [Parameter(Mandatory)][string]$Destination
  )
  if (-not ([Uri]$Artifact.url).Scheme.Equals('https', [StringComparison]::OrdinalIgnoreCase)) {
    throw "Only HTTPS dependency URLs are accepted: $($Artifact.url)"
  }
  if (Test-Path -LiteralPath $Destination) {
    Assert-HashAndSize -Path $Destination -ExpectedSha256 $Artifact.sha256 -ExpectedSize $Artifact.size
    return
  }
  if ($Offline) {
    throw "Offline staging requires the verified cache artifact: $Destination"
  }
  $partial = "$Destination.download-$([Guid]::NewGuid().ToString('N'))"
  try {
    Invoke-WebRequest -UseBasicParsing -Uri $Artifact.url -OutFile $partial
    Assert-HashAndSize -Path $partial -ExpectedSha256 $Artifact.sha256 -ExpectedSize $Artifact.size
    Move-Item -LiteralPath $partial -Destination $Destination
  } finally {
    if (Test-Path -LiteralPath $partial) {
      Remove-Item -Force -LiteralPath $partial
    }
  }
}

function Assert-ContainedPath {
  param(
    [Parameter(Mandatory)][string]$Parent,
    [Parameter(Mandatory)][string]$Child
  )
  $parentFull = [IO.Path]::GetFullPath($Parent).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
  $childFull = [IO.Path]::GetFullPath($Child)
  if (-not $childFull.StartsWith($parentFull, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing filesystem operation outside $parentFull`: $childFull"
  }
}

$ffmpegArchive = Join-Path $CacheDirectory $lock.ffmpeg.archive.fileName
$jmdictArchive = Join-Path $CacheDirectory $lock.jmdict.archive.fileName
$jmdictLicense = Join-Path $CacheDirectory $lock.jmdict.license.fileName
$pitchSource = Join-Path $CacheDirectory $lock.lexicalMetadata.pitch.source.fileName
$pitchAuthors = Join-Path $CacheDirectory $lock.lexicalMetadata.pitch.authors.fileName
$pitchLicense = Join-Path $CacheDirectory $lock.lexicalMetadata.pitch.license.fileName
$jlptArchive = Join-Path $CacheDirectory $lock.lexicalMetadata.jlpt.archive.fileName
$jlptLicense = Join-Path $CacheDirectory $lock.lexicalMetadata.jlpt.license.fileName
Get-PinnedArtifact -Artifact $lock.ffmpeg.archive -Destination $ffmpegArchive
Get-PinnedArtifact -Artifact $lock.jmdict.archive -Destination $jmdictArchive
Get-PinnedArtifact -Artifact $lock.jmdict.license -Destination $jmdictLicense
Get-PinnedArtifact -Artifact $lock.lexicalMetadata.pitch.source -Destination $pitchSource
Get-PinnedArtifact -Artifact $lock.lexicalMetadata.pitch.authors -Destination $pitchAuthors
Get-PinnedArtifact -Artifact $lock.lexicalMetadata.pitch.license -Destination $pitchLicense
Get-PinnedArtifact -Artifact $lock.lexicalMetadata.jlpt.archive -Destination $jlptArchive
Get-PinnedArtifact -Artifact $lock.lexicalMetadata.jlpt.license -Destination $jlptLicense

$workRoot = Join-Path $CacheDirectory "stage-$([Guid]::NewGuid().ToString('N'))"
Assert-ContainedPath -Parent $CacheDirectory -Child $workRoot
[IO.Directory]::CreateDirectory($workRoot) | Out-Null
try {
  $ffmpegExtract = Join-Path $workRoot 'ffmpeg'
  $jmdictExtract = Join-Path $workRoot 'jmdict'
  $jlptExtract = Join-Path $workRoot 'jlpt'
  Expand-Archive -LiteralPath $ffmpegArchive -DestinationPath $ffmpegExtract
  Expand-Archive -LiteralPath $jmdictArchive -DestinationPath $jmdictExtract
  Expand-Archive -LiteralPath $jlptArchive -DestinationPath $jlptExtract

  $ffmpegRoot = Join-Path $ffmpegExtract $lock.ffmpeg.archiveRoot
  $ffmpegExe = Join-Path $ffmpegRoot $lock.ffmpeg.tools.ffmpeg.sourcePath
  $ffprobeExe = Join-Path $ffmpegRoot $lock.ffmpeg.tools.ffprobe.sourcePath
  $ffmpegLicense = Join-Path $ffmpegRoot $lock.ffmpeg.license.sourcePath
  $jmdictSource = Join-Path $jmdictExtract $lock.jmdict.archiveSourcePath

  Assert-HashAndSize -Path $ffmpegExe -ExpectedSha256 $lock.ffmpeg.tools.ffmpeg.sha256 -ExpectedSize $lock.ffmpeg.tools.ffmpeg.size
  Assert-HashAndSize -Path $ffprobeExe -ExpectedSha256 $lock.ffmpeg.tools.ffprobe.sha256 -ExpectedSize $lock.ffmpeg.tools.ffprobe.size
  Assert-HashAndSize -Path $ffmpegLicense -ExpectedSha256 $lock.ffmpeg.license.sha256
  Assert-HashAndSize -Path $jmdictSource -ExpectedSha256 $lock.jmdict.source.sha256 -ExpectedSize $lock.jmdict.source.size
  Assert-HashAndSize -Path $pitchSource -ExpectedSha256 $lock.lexicalMetadata.pitch.source.sha256 -ExpectedSize $lock.lexicalMetadata.pitch.source.size

  $versionOutput = (& $ffmpegExe -hide_banner -version 2>&1 | Out-String)
  if ($LASTEXITCODE -ne 0 -or $versionOutput -notmatch [regex]::Escape("ffmpeg version $($lock.ffmpeg.version)")) {
    throw "FFmpeg identity check failed.`n$versionOutput"
  }
  foreach ($required in $lock.ffmpeg.requiredConfiguration) {
    if (-not $versionOutput.Contains($required, [StringComparison]::Ordinal)) {
      throw "FFmpeg is missing required configuration flag: $required"
    }
  }
  foreach ($forbidden in $lock.ffmpeg.forbiddenConfiguration) {
    if ($versionOutput.Contains($forbidden, [StringComparison]::Ordinal)) {
      throw "FFmpeg contains forbidden configuration flag: $forbidden"
    }
  }
  $encodersOutput = (& $ffmpegExe -hide_banner -encoders 2>&1 | Out-String)
  if ($LASTEXITCODE -ne 0) {
    throw 'Unable to inspect FFmpeg encoders.'
  }
  foreach ($encoder in $lock.ffmpeg.requiredEncoders) {
    if ($encodersOutput -notmatch "(?m)^ [A-Z\.]{6}\s+$([regex]::Escape($encoder))\s") {
      throw "FFmpeg is missing required encoder: $encoder"
    }
  }

  $databaseName = "jmdict-$($lock.jmdict.version).sqlite"
  $databaseCache = Join-Path $CacheDirectory $databaseName
  if (Test-Path -LiteralPath $databaseCache) {
    Assert-HashAndSize -Path $databaseCache -ExpectedSha256 $lock.jmdict.database.sha256 -ExpectedSize $lock.jmdict.database.size
  } else {
    Push-Location $repoRoot
    try {
      & cargo run --locked -p dictionary-builder -- $jmdictSource $lock.jmdict.source.sha256 $lock.jmdict.version $databaseCache
      if ($LASTEXITCODE -ne 0) {
        throw 'The pinned JMdict SQLite build failed.'
      }
    } finally {
      Pop-Location
    }
    Assert-HashAndSize -Path $databaseCache -ExpectedSha256 $lock.jmdict.database.sha256 -ExpectedSize $lock.jmdict.database.size
  }

  $metadataDatabaseCache = Join-Path $CacheDirectory $lock.lexicalMetadata.database.fileName
  if (Test-Path -LiteralPath $metadataDatabaseCache) {
    Assert-HashAndSize -Path $metadataDatabaseCache -ExpectedSha256 $lock.lexicalMetadata.database.sha256 -ExpectedSize $lock.lexicalMetadata.database.size
  } else {
    Push-Location $repoRoot
    try {
      & cargo run --locked -p dictionary-builder -- metadata `
        $pitchSource `
        $lock.lexicalMetadata.pitch.source.sha256 `
        $lock.lexicalMetadata.pitch.version `
        $jlptExtract `
        $lock.lexicalMetadata.jlpt.archive.sha256 `
        $lock.lexicalMetadata.jlpt.version `
        $metadataDatabaseCache
      if ($LASTEXITCODE -ne 0) {
        throw 'The pinned lexical metadata SQLite build failed.'
      }
    } finally {
      Pop-Location
    }
    Assert-HashAndSize -Path $metadataDatabaseCache -ExpectedSha256 $lock.lexicalMetadata.database.sha256 -ExpectedSize $lock.lexicalMetadata.database.size
  }

  $stageRoot = Join-Path $repoRoot 'apps/desktop/src-tauri/release-resources'
  Assert-ContainedPath -Parent $repoRoot -Child $stageRoot
  if (Test-Path -LiteralPath $stageRoot) {
    Remove-Item -Recurse -Force -LiteralPath $stageRoot
  }
  foreach ($relative in @('bin', 'data', 'third-party')) {
    [IO.Directory]::CreateDirectory((Join-Path $stageRoot $relative)) | Out-Null
  }
  Copy-Item -LiteralPath $ffmpegExe -Destination (Join-Path $stageRoot 'bin/ffmpeg.exe')
  Copy-Item -LiteralPath $ffprobeExe -Destination (Join-Path $stageRoot 'bin/ffprobe.exe')
  Copy-Item -LiteralPath $databaseCache -Destination (Join-Path $stageRoot 'data/jmdict.sqlite')
  Copy-Item -LiteralPath $metadataDatabaseCache -Destination (Join-Path $stageRoot 'data/lexical-metadata.sqlite')
  Copy-Item -LiteralPath $ffmpegLicense -Destination (Join-Path $stageRoot 'third-party/FFmpeg-LICENSE.txt')
  Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'third-party/FFMPEG-NOTICE.txt') -Destination (Join-Path $stageRoot 'third-party/FFMPEG-NOTICE.txt')
  Copy-Item -LiteralPath $jmdictLicense -Destination (Join-Path $stageRoot 'third-party/JMdict-LICENSE.txt')
  Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'third-party/JMDICT-NOTICE.txt') -Destination (Join-Path $stageRoot 'third-party/JMDICT-NOTICE.txt')
  Copy-Item -LiteralPath $pitchAuthors -Destination (Join-Path $stageRoot 'third-party/UniDic-AUTHORS.txt')
  Copy-Item -LiteralPath $pitchLicense -Destination (Join-Path $stageRoot 'third-party/UniDic-BSD.txt')
  Copy-Item -LiteralPath $jlptLicense -Destination (Join-Path $stageRoot 'third-party/JLPT-ESTIMATES-LICENSE.txt')
  Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'third-party/LEXICAL-METADATA-NOTICE.txt') -Destination (Join-Path $stageRoot 'third-party/LEXICAL-METADATA-NOTICE.txt')
  Copy-Item -LiteralPath $lockPath -Destination (Join-Path $stageRoot 'third-party/runtime-dependencies.lock.json')

  $tauriTarget = Join-Path $repoRoot 'apps/desktop/src-tauri/target'
  [IO.Directory]::CreateDirectory($tauriTarget) | Out-Null
  $overlayPath = Join-Path $tauriTarget 'tauri.self-contained.json'
  $overlay = [ordered]@{
    bundle = [ordered]@{
      useLocalToolsDir = $true
      resources = [ordered]@{
        'release-resources/bin/ffmpeg.exe' = 'bin/ffmpeg.exe'
        'release-resources/bin/ffprobe.exe' = 'bin/ffprobe.exe'
        'release-resources/data/jmdict.sqlite' = 'data/jmdict.sqlite'
        'release-resources/data/lexical-metadata.sqlite' = 'data/lexical-metadata.sqlite'
        'release-resources/third-party/FFmpeg-LICENSE.txt' = 'third-party/FFmpeg-LICENSE.txt'
        'release-resources/third-party/FFMPEG-NOTICE.txt' = 'third-party/FFMPEG-NOTICE.txt'
        'release-resources/third-party/JMdict-LICENSE.txt' = 'third-party/JMdict-LICENSE.txt'
        'release-resources/third-party/JMDICT-NOTICE.txt' = 'third-party/JMDICT-NOTICE.txt'
        'release-resources/third-party/UniDic-AUTHORS.txt' = 'third-party/UniDic-AUTHORS.txt'
        'release-resources/third-party/UniDic-BSD.txt' = 'third-party/UniDic-BSD.txt'
        'release-resources/third-party/JLPT-ESTIMATES-LICENSE.txt' = 'third-party/JLPT-ESTIMATES-LICENSE.txt'
        'release-resources/third-party/LEXICAL-METADATA-NOTICE.txt' = 'third-party/LEXICAL-METADATA-NOTICE.txt'
        'release-resources/third-party/runtime-dependencies.lock.json' = 'third-party/runtime-dependencies.lock.json'
      }
    }
  }
  [IO.File]::WriteAllText($overlayPath, ($overlay | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))

  & (Join-Path $repoRoot 'tools/ci/check-runtime-dependencies.ps1') -RequireStaged

  Write-Host "Staged verified FFmpeg $($lock.ffmpeg.version)."
  Write-Host "Staged verified JMdict $($lock.jmdict.version) ($($lock.jmdict.database.entryCount) entries)."
  Write-Host "Staged verified $($lock.lexicalMetadata.pitch.version) ($($lock.lexicalMetadata.database.pitchPatternCount) pitch patterns)."
  Write-Host "Staged verified $($lock.lexicalMetadata.jlpt.version) ($($lock.lexicalMetadata.database.jlptEstimateCount) estimates)."
  Write-Host "Tauri release overlay: $overlayPath"
} finally {
  Assert-ContainedPath -Parent $CacheDirectory -Child $workRoot
  if (Test-Path -LiteralPath $workRoot) {
    Remove-Item -Recurse -Force -LiteralPath $workRoot
  }
}
