param(
  [Parameter(Mandatory = $true)][string]$Ffmpeg,
  [Parameter(Mandatory = $true)][string]$Ffprobe,
  [string]$OutputDirectory = 'fixtures/media/generated',
  [switch]$IncludeLargeSparse
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$fixtureOutputDirectory = [System.IO.Path]::GetFullPath((Join-Path $root $OutputDirectory))
if (-not $fixtureOutputDirectory.StartsWith($root, [System.StringComparison]::OrdinalIgnoreCase)) {
  throw 'Fixture output escaped repository root.'
}
New-Item -ItemType Directory -Force -Path $fixtureOutputDirectory | Out-Null

function Invoke-Fixture {
  param([string]$Name, [string[]]$Arguments)
  Write-Host "Generating $Name"
  & $Ffmpeg @Arguments
  if ($LASTEXITCODE -ne 0) { throw "FFmpeg failed while generating $Name" }
}

$mp4 = Join-Path $fixtureOutputDirectory 'h264-aac.mp4'
Invoke-Fixture 'H.264/AAC MP4' @(
  '-nostdin', '-hide_banner', '-y',
  '-f', 'lavfi', '-i', 'testsrc2=size=640x360:rate=24:duration=5',
  '-f', 'lavfi', '-i', 'sine=frequency=880:sample_rate=48000:duration=5',
  '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-preset', 'veryfast', '-crf', '20',
  '-c:a', 'aac', '-b:a', '128k', '-shortest', '-movflags', '+faststart', $mp4
)

Invoke-Fixture 'Known color and audio marker MP4' @(
  '-nostdin', '-hide_banner', '-y',
  '-f', 'lavfi', '-i', 'color=c=red:size=640x360:rate=24:duration=1',
  '-f', 'lavfi', '-i', 'color=c=green:size=640x360:rate=24:duration=1',
  '-f', 'lavfi', '-i', 'color=c=blue:size=640x360:rate=24:duration=1',
  '-f', 'lavfi', '-i', 'color=c=white:size=640x360:rate=24:duration=1',
  '-f', 'lavfi', '-i', 'color=c=black:size=640x360:rate=24:duration=1',
  '-f', 'lavfi', '-i', 'aevalsrc=if(lt(mod(t\,1)\,0.08)\,0.6*sin(2*PI*880*t)\,0):s=48000:d=5',
  '-filter_complex', '[0:v][1:v][2:v][3:v][4:v]concat=n=5:v=1:a=0[v]',
  '-map', '[v]', '-map', '5:a:0', '-c:v', 'libx264', '-pix_fmt', 'yuv420p',
  '-c:a', 'aac', '-shortest', '-movflags', '+faststart',
  (Join-Path $fixtureOutputDirectory 'known-markers.mp4')
)

Invoke-Fixture 'VP9/Opus WebM' @(
  '-nostdin', '-hide_banner', '-y',
  '-f', 'lavfi', '-i', 'testsrc2=size=640x360:rate=24:duration=5',
  '-f', 'lavfi', '-i', 'sine=frequency=660:sample_rate=48000:duration=5',
  '-c:v', 'libvpx-vp9', '-deadline', 'realtime', '-cpu-used', '8', '-b:v', '700k',
  '-c:a', 'libopus', '-b:a', '96k', '-shortest', (Join-Path $fixtureOutputDirectory 'vp9-opus.webm')
)

Invoke-Fixture 'H.264/AAC Matroska remux' @(
  '-nostdin', '-hide_banner', '-y', '-i', $mp4, '-map', '0', '-c', 'copy',
  (Join-Path $fixtureOutputDirectory 'h264-aac.mkv')
)

Invoke-Fixture 'H.264/FLAC Matroska' @(
  '-nostdin', '-hide_banner', '-y', '-i', $mp4, '-c:v', 'copy', '-c:a', 'flac',
  (Join-Path $fixtureOutputDirectory 'h264-flac.mkv')
)

Invoke-Fixture 'Multiple audio and embedded text subtitles' @(
  '-nostdin', '-hide_banner', '-y', '-i', $mp4,
  '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000:duration=5',
  '-i', (Join-Path $root 'fixtures/media/subtitles/synthetic.srt'),
  '-map', '0:v:0', '-map', '0:a:0', '-map', '1:a:0', '-map', '2:0',
  '-c:v', 'copy', '-c:a', 'aac', '-c:s', 'srt',
  '-metadata:s:a:0', 'language=jpn', '-metadata:s:a:1', 'language=eng',
  '-metadata:s:s:0', 'language=jpn', (Join-Path $fixtureOutputDirectory 'multi-track.mkv')
)

$encoderInventory = (& $Ffmpeg '-hide_banner' '-encoders' 2>&1) -join "`n"
$hevcEncoder = if ($encoderInventory -match '\blibx265\b') {
  'libx265'
} elseif ($encoderInventory -match '\bhevc_nvenc\b') {
  'hevc_nvenc'
} else {
  $null
}
if ($hevcEncoder) {
  Invoke-Fixture 'HEVC Main 8-bit Matroska' @(
    '-nostdin', '-hide_banner', '-y',
    '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=24:duration=2',
    '-c:v', $hevcEncoder, '-profile:v', 'main', '-pix_fmt', 'yuv420p', '-an',
    (Join-Path $fixtureOutputDirectory 'hevc-main8.mkv')
  )

  $hevc10PixelFormat = if ($hevcEncoder -eq 'hevc_nvenc') { 'p010le' } else { 'yuv420p10le' }
  Invoke-Fixture 'HEVC Main 10-bit Matroska' @(
    '-nostdin', '-hide_banner', '-y',
    '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=24:duration=2',
    '-c:v', $hevcEncoder, '-profile:v', 'main10', '-pix_fmt', $hevc10PixelFormat, '-an',
    (Join-Path $fixtureOutputDirectory 'hevc-main10.mkv')
  )
} else {
  Set-Content -LiteralPath (Join-Path $fixtureOutputDirectory 'HEVC-SKIPPED.txt') -Encoding utf8NoBOM `
    -Value 'No libx265 or usable HEVC hardware encoder was available.'
}

Invoke-Fixture 'H.264 High 10 Matroska' @(
  '-nostdin', '-hide_banner', '-y',
  '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=24:duration=2',
  '-vf', 'format=yuv420p10le', '-c:v', 'libx264', '-profile:v', 'high10', '-an',
  (Join-Path $fixtureOutputDirectory 'h264-high10.mkv')
)

Invoke-Fixture 'AV1 MP4' @(
  '-nostdin', '-hide_banner', '-y',
  '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=24:duration=2',
  '-c:v', 'libaom-av1', '-cpu-used', '8', '-crf', '40', '-b:v', '0', '-an',
  (Join-Path $fixtureOutputDirectory 'av1.mp4')
)

$unicodeDirectory = Join-Path $fixtureOutputDirectory '日本語 media with spaces'
New-Item -ItemType Directory -Force -Path $unicodeDirectory | Out-Null
Copy-Item -LiteralPath $mp4 -Destination (Join-Path $unicodeDirectory '映像 sample.mp4') -Force

$longDirectory = $fixtureOutputDirectory
1..5 | ForEach-Object {
  $longDirectory = Join-Path $longDirectory ("long-path-segment-{0:D2}-abcdefghijklmnopqrstuvwxyz" -f $_)
}
New-Item -ItemType Directory -Force -Path $longDirectory | Out-Null
Copy-Item -LiteralPath $mp4 -Destination (Join-Path $longDirectory 'long-path-video.mp4') -Force

if ($IncludeLargeSparse) {
  $largePath = Join-Path $fixtureOutputDirectory 'sparse-over-4gib.mp4'
  Copy-Item -LiteralPath $mp4 -Destination $largePath -Force
  & fsutil sparse setflag $largePath | Out-Null
  if ($LASTEXITCODE -ne 0) { throw 'Could not mark the large fixture as sparse.' }
  $largeStream = [System.IO.File]::Open($largePath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Write)
  try {
    $largeStream.SetLength(4GB + 1MB)
  } finally {
    $largeStream.Dispose()
  }
}

$mediaFiles = Get-ChildItem -LiteralPath $fixtureOutputDirectory -Recurse -File |
  Where-Object { $_.Extension -in @('.mp4', '.mkv', '.webm') }
foreach ($mediaFile in $mediaFiles) {
  $inventory = "$($mediaFile.FullName).probe.json"
  & $Ffprobe '-v' 'error' '-show_format' '-show_streams' '-of' 'json' $mediaFile.FullName |
    Set-Content -LiteralPath $inventory -Encoding utf8NoBOM
  if ($LASTEXITCODE -ne 0) { throw "FFprobe inventory failed for $($mediaFile.Name)." }
}

Get-ChildItem -LiteralPath $fixtureOutputDirectory -Recurse -File |
  Where-Object { $_.Name -ne 'SHA256SUMS' } |
  Get-FileHash -Algorithm SHA256 |
  Sort-Object Path |
  ForEach-Object {
    $relativePath = [System.IO.Path]::GetRelativePath($fixtureOutputDirectory, $_.Path).Replace('\', '/')
    "{0}  {1}" -f $_.Hash.ToLowerInvariant(), $relativePath
  } |
  Set-Content -LiteralPath (Join-Path $fixtureOutputDirectory 'SHA256SUMS') -Encoding utf8NoBOM
