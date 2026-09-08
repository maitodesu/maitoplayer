param(
  [Parameter(Mandatory = $true)][string]$Ffmpeg,
  [Parameter(Mandatory = $true)][string]$Ffprobe,
  [string]$RemuxFixture = 'fixtures/media/generated/h264-aac.mkv',
  [string]$EmbeddedFixture = 'fixtures/media/generated/multi-track.mkv',
  [string]$AudioFixture = 'fixtures/media/generated/h264-flac.mkv',
  [string]$VideoFixture = 'fixtures/media/generated/av1.mp4',
  [string]$DirectFixture = 'fixtures/media/generated/h264-aac.mp4'
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$ffmpegPath = (Resolve-Path -LiteralPath $Ffmpeg).Path
$ffprobePath = (Resolve-Path -LiteralPath $Ffprobe).Path
$remuxPath = (Resolve-Path -LiteralPath (Join-Path $root $RemuxFixture)).Path
$embeddedPath = (Resolve-Path -LiteralPath (Join-Path $root $EmbeddedFixture)).Path
$audioPath = (Resolve-Path -LiteralPath (Join-Path $root $AudioFixture)).Path
$videoPath = (Resolve-Path -LiteralPath (Join-Path $root $VideoFixture)).Path
$directPath = (Resolve-Path -LiteralPath (Join-Path $root $DirectFixture)).Path

$env:MAITOPLAYER_TEST_FFMPEG = $ffmpegPath
$env:MAITOPLAYER_TEST_FFPROBE = $ffprobePath
$env:MAITOPLAYER_TEST_REMUX_FIXTURE = $remuxPath
$env:MAITOPLAYER_TEST_EMBEDDED_FIXTURE = $embeddedPath
$env:MAITOPLAYER_TEST_AUDIO_FIXTURE = $audioPath
$env:MAITOPLAYER_TEST_VIDEO_FIXTURE = $videoPath
$env:MAITOPLAYER_TEST_DIRECT_FIXTURE = $directPath

Push-Location $root
try {
  & cargo test -p media-engine real_tool_ -- --ignored --nocapture
  if ($LASTEXITCODE -ne 0) {
    throw 'The media-engine real-tool lane failed.'
  }
} finally {
  Pop-Location
  Remove-Item Env:MAITOPLAYER_TEST_FFMPEG -ErrorAction SilentlyContinue
  Remove-Item Env:MAITOPLAYER_TEST_FFPROBE -ErrorAction SilentlyContinue
  Remove-Item Env:MAITOPLAYER_TEST_REMUX_FIXTURE -ErrorAction SilentlyContinue
  Remove-Item Env:MAITOPLAYER_TEST_EMBEDDED_FIXTURE -ErrorAction SilentlyContinue
  Remove-Item Env:MAITOPLAYER_TEST_AUDIO_FIXTURE -ErrorAction SilentlyContinue
  Remove-Item Env:MAITOPLAYER_TEST_VIDEO_FIXTURE -ErrorAction SilentlyContinue
  Remove-Item Env:MAITOPLAYER_TEST_DIRECT_FIXTURE -ErrorAction SilentlyContinue
}
