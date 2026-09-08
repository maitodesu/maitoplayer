[CmdletBinding()]
param([switch]$RequireStaged)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$lockPath = Join-Path $repoRoot 'packaging/windows/runtime-dependencies.lock.json'
$lock = Get-Content -Raw -LiteralPath $lockPath | ConvertFrom-Json

function Assert-Sha256([string]$Label, [string]$Value) {
  if ($Value -notmatch '^[0-9A-Fa-f]{64}$') {
    throw "$Label must be an exact SHA-256 value."
  }
}

function Assert-ImmutableHttpsUrl([string]$Label, [string]$Value) {
  $uri = [Uri]$Value
  if ($uri.Scheme -ne 'https' -or $Value -match '/latest/' -or $Value -match '[?&](ref|version)=') {
    throw "$Label must be a pinned immutable HTTPS URL."
  }
}

if ($lock.schemaVersion -ne 1) { throw 'Unexpected runtime dependency lock schema.' }
Assert-ImmutableHttpsUrl 'FFmpeg archive' $lock.ffmpeg.archive.url
Assert-ImmutableHttpsUrl 'FFmpeg source commit' $lock.ffmpeg.sourceCommit.url
Assert-ImmutableHttpsUrl 'FFmpeg build scripts commit' $lock.ffmpeg.buildScriptsCommit.url
Assert-ImmutableHttpsUrl 'JMdict archive' $lock.jmdict.archive.url
Assert-ImmutableHttpsUrl 'JMdict source commit' $lock.jmdict.sourceRepositoryCommit.url
Assert-ImmutableHttpsUrl 'JMdict licence' $lock.jmdict.license.url
Assert-ImmutableHttpsUrl 'UniDic pitch source' $lock.lexicalMetadata.pitch.source.url
Assert-ImmutableHttpsUrl 'UniDic source page' $lock.lexicalMetadata.pitch.sourcePage
Assert-ImmutableHttpsUrl 'UniDic authors' $lock.lexicalMetadata.pitch.authors.url
Assert-ImmutableHttpsUrl 'UniDic BSD licence' $lock.lexicalMetadata.pitch.license.url
Assert-ImmutableHttpsUrl 'JLPT estimate archive' $lock.lexicalMetadata.jlpt.archive.url
Assert-ImmutableHttpsUrl 'JLPT estimate source' $lock.lexicalMetadata.jlpt.sourceRepository
Assert-ImmutableHttpsUrl 'JLPT estimate licence' $lock.lexicalMetadata.jlpt.license.url
Assert-ImmutableHttpsUrl 'Noto Sans JP source commit' $lock.notoSansJp.sourceCommit.url
Assert-ImmutableHttpsUrl 'Noto Sans JP font' $lock.notoSansJp.font.url
Assert-ImmutableHttpsUrl 'Noto Sans JP licence' $lock.notoSansJp.license.url

@{
  'FFmpeg archive' = $lock.ffmpeg.archive.sha256
  'FFmpeg executable' = $lock.ffmpeg.tools.ffmpeg.sha256
  'FFprobe executable' = $lock.ffmpeg.tools.ffprobe.sha256
  'FFmpeg licence' = $lock.ffmpeg.license.sha256
  'JMdict archive' = $lock.jmdict.archive.sha256
  'JMdict JSON' = $lock.jmdict.source.sha256
  'JMdict licence' = $lock.jmdict.license.sha256
  'JMdict database' = $lock.jmdict.database.sha256
  'UniDic pitch source' = $lock.lexicalMetadata.pitch.source.sha256
  'UniDic authors' = $lock.lexicalMetadata.pitch.authors.sha256
  'UniDic BSD licence' = $lock.lexicalMetadata.pitch.license.sha256
  'JLPT estimate archive' = $lock.lexicalMetadata.jlpt.archive.sha256
  'JLPT estimate licence' = $lock.lexicalMetadata.jlpt.license.sha256
  'Lexical metadata database' = $lock.lexicalMetadata.database.sha256
  'Noto Sans JP font' = $lock.notoSansJp.font.sha256
  'Noto Sans JP licence' = $lock.notoSansJp.license.sha256
}.GetEnumerator() | ForEach-Object { Assert-Sha256 $_.Key $_.Value }

if (-not ($lock.ffmpeg.forbiddenConfiguration -contains '--enable-gpl') -or
    -not ($lock.ffmpeg.forbiddenConfiguration -contains '--enable-nonfree')) {
  throw 'The FFmpeg lock must explicitly reject GPL and nonfree build flags.'
}
if ($lock.jmdict.database.entryCount -lt 200000) {
  throw 'The bundled dictionary lock unexpectedly describes a sample/incomplete database.'
}
if ($lock.lexicalMetadata.schemaVersion -ne '1' -or
    $lock.lexicalMetadata.database.pitchPatternCount -lt 500000 -or
    $lock.lexicalMetadata.database.jlptEstimateCount -lt 5000) {
  throw 'The lexical metadata lock unexpectedly describes a sample or incompatible database.'
}

function Assert-RepoFile([string]$Label, [string]$RelativePath, [string]$Sha256, [Nullable[long]]$Size) {
  $path = Join-Path $repoRoot $RelativePath
  if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
    throw "$Label is missing: $RelativePath"
  }
  $item = Get-Item -LiteralPath $path
  if ($null -ne $Size -and $item.Length -ne $Size) {
    throw "$Label size mismatch: $RelativePath"
  }
  $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash
  if (-not $actual.Equals($Sha256, [StringComparison]::OrdinalIgnoreCase)) {
    throw "$Label SHA-256 mismatch: $RelativePath"
  }
}

Assert-RepoFile 'Bundled Noto Sans JP font' $lock.notoSansJp.font.path $lock.notoSansJp.font.sha256 $lock.notoSansJp.font.size
Assert-RepoFile 'Bundled Noto Sans JP licence' $lock.notoSansJp.license.path $lock.notoSansJp.license.sha256 $lock.notoSansJp.license.size
$fontNoticePath = Join-Path $repoRoot $lock.notoSansJp.noticePath
if (-not (Test-Path -LiteralPath $fontNoticePath -PathType Leaf)) {
  throw "Bundled Noto Sans JP notice is missing: $($lock.notoSansJp.noticePath)"
}
$fontNotice = Get-Content -Raw -LiteralPath $fontNoticePath
if ($fontNotice -notmatch [Regex]::Escape($lock.notoSansJp.sourceCommit.sha) -or
    $fontNotice -notmatch [Regex]::Escape($lock.notoSansJp.font.sha256)) {
  throw 'The Noto Sans JP notice must identify the pinned source commit and font hash.'
}

$tauri = Get-Content -Raw -LiteralPath (Join-Path $repoRoot 'apps/desktop/src-tauri/tauri.conf.json') | ConvertFrom-Json
if ($tauri.bundle.windows.webviewInstallMode.type -ne 'offlineInstaller' -or
    $tauri.bundle.windows.webviewInstallMode.silent -ne $true) {
  throw 'Windows packaging must silently include the offline WebView2 installer.'
}

$uiDistRoot = Join-Path $repoRoot 'apps/desktop/ui/dist'
if (Test-Path -LiteralPath $uiDistRoot) {
  Assert-RepoFile 'Built Noto Sans JP font' 'apps/desktop/ui/dist/fonts/NotoSansJP-VF.ttf' $lock.notoSansJp.font.sha256 $lock.notoSansJp.font.size
  foreach ($requiredDistFile in @('NotoSansJP-OFL.txt', 'NotoSansJP-NOTICE.txt')) {
    $path = Join-Path $uiDistRoot "fonts/$requiredDistFile"
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
      throw "Built Noto Sans JP attribution is missing: fonts/$requiredDistFile"
    }
  }
}

$stageRoot = Join-Path $repoRoot 'apps/desktop/src-tauri/release-resources'
if (-not (Test-Path -LiteralPath $stageRoot)) {
  if ($RequireStaged) { throw "Staged runtime resources are required: $stageRoot" }
  Write-Host 'Runtime dependency lock passed (staged resources are absent).'
  return
}

$expectedFiles = @(
  'bin/ffmpeg.exe',
  'bin/ffprobe.exe',
  'data/jmdict.sqlite',
  'data/lexical-metadata.sqlite',
  'third-party/FFmpeg-LICENSE.txt',
  'third-party/FFMPEG-NOTICE.txt',
  'third-party/JMdict-LICENSE.txt',
  'third-party/JMDICT-NOTICE.txt',
  'third-party/UniDic-AUTHORS.txt',
  'third-party/UniDic-BSD.txt',
  'third-party/JLPT-ESTIMATES-LICENSE.txt',
  'third-party/LEXICAL-METADATA-NOTICE.txt',
  'third-party/runtime-dependencies.lock.json'
)
$actualFiles = @(Get-ChildItem -File -Recurse -LiteralPath $stageRoot | ForEach-Object {
  $_.FullName.Substring($stageRoot.Length + 1).Replace('\', '/')
})
$difference = @(Compare-Object -ReferenceObject $expectedFiles -DifferenceObject $actualFiles)
if ($difference.Count -ne 0) {
  $details = $difference | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" }
  throw "Staged resources differ from the exact allowlist:`n$($details -join "`n")"
}

function Assert-StagedFile([string]$RelativePath, [string]$Sha256, [Nullable[long]]$Size) {
  $path = Join-Path $stageRoot $RelativePath
  $item = Get-Item -LiteralPath $path
  if ($null -ne $Size -and $item.Length -ne $Size) {
    throw "Staged size mismatch: $RelativePath"
  }
  $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash
  if (-not $actual.Equals($Sha256, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Staged SHA-256 mismatch: $RelativePath"
  }
}

Assert-StagedFile 'bin/ffmpeg.exe' $lock.ffmpeg.tools.ffmpeg.sha256 $lock.ffmpeg.tools.ffmpeg.size
Assert-StagedFile 'bin/ffprobe.exe' $lock.ffmpeg.tools.ffprobe.sha256 $lock.ffmpeg.tools.ffprobe.size
Assert-StagedFile 'data/jmdict.sqlite' $lock.jmdict.database.sha256 $lock.jmdict.database.size
Assert-StagedFile 'data/lexical-metadata.sqlite' $lock.lexicalMetadata.database.sha256 $lock.lexicalMetadata.database.size
Assert-StagedFile 'third-party/FFmpeg-LICENSE.txt' $lock.ffmpeg.license.sha256 $null
Assert-StagedFile 'third-party/JMdict-LICENSE.txt' $lock.jmdict.license.sha256 $null
Assert-StagedFile 'third-party/UniDic-AUTHORS.txt' $lock.lexicalMetadata.pitch.authors.sha256 $lock.lexicalMetadata.pitch.authors.size
Assert-StagedFile 'third-party/UniDic-BSD.txt' $lock.lexicalMetadata.pitch.license.sha256 $lock.lexicalMetadata.pitch.license.size
Assert-StagedFile 'third-party/JLPT-ESTIMATES-LICENSE.txt' $lock.lexicalMetadata.jlpt.license.sha256 $lock.lexicalMetadata.jlpt.license.size

$trackedPairs = @(
  @('packaging/windows/third-party/FFMPEG-NOTICE.txt', 'third-party/FFMPEG-NOTICE.txt'),
  @('packaging/windows/third-party/JMDICT-NOTICE.txt', 'third-party/JMDICT-NOTICE.txt'),
  @('packaging/windows/third-party/LEXICAL-METADATA-NOTICE.txt', 'third-party/LEXICAL-METADATA-NOTICE.txt'),
  @('packaging/windows/runtime-dependencies.lock.json', 'third-party/runtime-dependencies.lock.json')
)
foreach ($pair in $trackedPairs) {
  $sourceHash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $repoRoot $pair[0])).Hash
  Assert-StagedFile $pair[1] $sourceHash $null
}

Write-Host 'Bundled runtime supply-chain guard passed.'
