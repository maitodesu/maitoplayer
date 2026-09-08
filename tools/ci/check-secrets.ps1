$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$patterns = @(
  '-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----',
  'AKIA[0-9A-Z]{16}',
  'ghp_[A-Za-z0-9]{30,}',
  'xox[baprs]-[A-Za-z0-9-]{20,}'
)
$excludedNames = @('check-secrets.ps1', 'pnpm-lock.yaml', 'Cargo.lock')
$violations = Get-ChildItem -LiteralPath $root -Recurse -File |
  Where-Object {
    $_.Name -notin $excludedNames -and
    # Generated/downloaded binary trees are either ignored or verified by the
    # dedicated runtime supply-chain guard. Scanning their arbitrary bytes as
    # text produces false credentials while adding no source-secret coverage.
    $_.FullName -notmatch '[\\/](\.git[^\\/]*|\.cache|\.tauri|\.playwright-cli|output|target|node_modules|dist|generated|release-resources)[\\/]'
  } |
  Select-String -Pattern $patterns
if ($violations) {
  $violations | ForEach-Object { Write-Error "Possible secret: $($_.Path):$($_.LineNumber)" }
  exit 1
}
Write-Host 'Secret-pattern scan passed.'
