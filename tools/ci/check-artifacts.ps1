$ErrorActionPreference = 'Stop'
$limit = 5MB
$blocked = @('.mp4', '.mkv', '.webm', '.sqlite', '.db', '.exe', '.dll', '.msi', '.msix', '.appx')
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
& (Join-Path $PSScriptRoot 'check-runtime-dependencies.ps1')
$releaseResources = [IO.Path]::GetFullPath((Join-Path $repoRoot 'apps/desktop/src-tauri/release-resources')).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
$bundledFont = [IO.Path]::GetFullPath((Join-Path $repoRoot 'apps/desktop/ui/static/fonts/NotoSansJP-VF.ttf'))
$bad = Get-ChildItem -File -Recurse | Where-Object {
  -not $_.FullName.StartsWith($releaseResources, [StringComparison]::OrdinalIgnoreCase) -and
  -not $_.FullName.Equals($bundledFont, [StringComparison]::OrdinalIgnoreCase) -and
  $_.FullName -notmatch '[\\/](target|node_modules|\.git|\.tauri|\.cache|output)[\\/]' -and
  $_.FullName -notmatch '[\\/]fixtures[\\/]media[\\/]generated[\\/]' -and
  (($_.Length -gt $limit) -or ($blocked -contains $_.Extension.ToLowerInvariant()))
}
if ($bad) {
  $bad | ForEach-Object { Write-Error "Large/generated artifact: $($_.FullName)" }
  exit 1
}
