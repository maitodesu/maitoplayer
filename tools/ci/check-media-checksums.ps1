param(
  [string]$Directory = 'fixtures/media/generated'
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$fixtureRoot = (Resolve-Path -LiteralPath (Join-Path $root $Directory)).Path
$checksumPath = Join-Path $fixtureRoot 'SHA256SUMS'
if (-not (Test-Path -LiteralPath $checksumPath -PathType Leaf)) {
  throw 'Generated media SHA256SUMS is missing.'
}

$actual = Get-ChildItem -LiteralPath $fixtureRoot -Recurse -File |
  Where-Object { $_.Name -ne 'SHA256SUMS' } |
  Get-FileHash -Algorithm SHA256 |
  Sort-Object Path |
  ForEach-Object {
    $relativePath = $_.Path.Substring($fixtureRoot.Length).TrimStart('\').Replace('\', '/')
    "{0}  {1}" -f $_.Hash.ToLowerInvariant(), $relativePath
  }
$expected = Get-Content -LiteralPath $checksumPath -Encoding UTF8
$differences = @(Compare-Object -ReferenceObject $expected -DifferenceObject $actual)
if ($differences.Count -ne 0) {
  throw 'Generated media checksums do not match the fixture inventory.'
}
Write-Host "Verified $($actual.Count) generated media artifacts and probe inventories."
