$ErrorActionPreference = 'Stop'
$config = Get-Content -LiteralPath 'apps/desktop/src-tauri/tauri.conf.json' -Raw
if ($config -match 'assetProtocol[^}]*enable[^:]*:\s*true' -and
    $config -match '(\*\*|\$HOME|%USERPROFILE%)') {
  Write-Error 'Tauri asset protocol scope is overbroad.'
  exit 1
}
$sources = Get-ChildItem -LiteralPath 'apps/desktop/ui/src' -File -Recurse
if ($sources | Select-String -Pattern '{@html') {
  Write-Error 'Untrusted Svelte {@html} rendering is forbidden.'
  exit 1
}

