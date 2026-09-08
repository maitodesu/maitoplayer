[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

function Fail([string]$Message) {
  [Console]::Error.WriteLine("CONTRACT ERROR: $Message")
  $script:failed = $true
}

$failed = $false
$contractsPath = Join-Path $repoRoot 'crates\contracts\src\lib.rs'
$typescriptPath = Join-Path $repoRoot 'apps\desktop\ui\src\lib\contracts\generated.ts'
$catalogPath = Join-Path $repoRoot 'fixtures\contracts\error-catalog.json'
$uiRoot = Join-Path $repoRoot 'apps\desktop\ui\src'

$rust = Get-Content -Raw -LiteralPath $contractsPath
$typescript = Get-Content -Raw -LiteralPath $typescriptPath
$catalogObject = Get-Content -Raw -LiteralPath $catalogPath | ConvertFrom-Json
$catalog = @{}
foreach ($property in $catalogObject.PSObject.Properties) {
  $catalog[$property.Name] = $property.Value
}

# Every public V1 struct/enum is required in the checked-in TypeScript contract.
$rustTypeNames = [regex]::Matches($rust, 'pub\s+(?:struct|enum)\s+([A-Za-z0-9_]+V1)') |
  ForEach-Object { $_.Groups[1].Value } |
  Sort-Object -Unique
foreach ($name in $rustTypeNames) {
  if ($typescript -notmatch "export\s+(?:interface|type)\s+$([regex]::Escape($name))\b") {
    Fail "Generated TypeScript contract is missing Rust type $name."
  }
}
$typescriptTypeNames = [regex]::Matches(
  $typescript,
  'export\s+(?:interface|type)\s+([A-Za-z0-9_]+V1)\b'
) |
  ForEach-Object { $_.Groups[1].Value } |
  Sort-Object -Unique
foreach ($name in $typescriptTypeNames) {
  if ($name -notin $rustTypeNames) {
    Fail "Generated TypeScript contract contains stale type $name."
  }
}

# Struct field names cross the IPC boundary as snake_case and must remain exact.
$rustStructs = [regex]::Matches(
  $rust,
  '(?s)pub\s+struct\s+([A-Za-z0-9_]+V1)\s*\{(.*?)\n\}'
)
foreach ($rustStruct in $rustStructs) {
  $name = $rustStruct.Groups[1].Value
  $tsMatch = [regex]::Match(
    $typescript,
    "(?s)export\s+interface\s+$([regex]::Escape($name))\s*\{(.*?)\n\}"
  )
  if (-not $tsMatch.Success) {
    continue
  }
  $fields = [regex]::Matches($rustStruct.Groups[2].Value, 'pub\s+([a-z][a-z0-9_]*)\s*:') |
    ForEach-Object { $_.Groups[1].Value } |
    Sort-Object -Unique
  foreach ($field in $fields) {
    if ($tsMatch.Groups[1].Value -notmatch "(?m)^\s*$([regex]::Escape($field))\??\s*:") {
      Fail "TypeScript $name is missing Rust field $field."
    }
  }
  $typescriptFields = [regex]::Matches(
    $tsMatch.Groups[1].Value,
    '(?m)^\s*([a-z][a-z0-9_]*)\??\s*:'
  ) |
    ForEach-Object { $_.Groups[1].Value } |
    Sort-Object -Unique
  foreach ($field in $typescriptFields) {
    if ($field -notin $fields) {
      Fail "TypeScript $name contains stale field $field."
    }
  }
}

# All public enums use snake_case serialization; compare their full variant sets.
$rustEnums = [regex]::Matches(
  $rust,
  '(?s)pub\s+enum\s+([A-Za-z0-9_]+V1)\s*\{(.*?)\n\}'
)
foreach ($rustEnum in $rustEnums) {
  $name = $rustEnum.Groups[1].Value
  $rustVariants = [regex]::Matches(
    $rustEnum.Groups[2].Value,
    '(?m)^\s*([A-Z][A-Za-z0-9_]*)\s*,?\s*$'
  ) |
    ForEach-Object {
      [regex]::Replace($_.Groups[1].Value, '(?<!^)([A-Z])', '_$1').ToLowerInvariant()
    } |
    Sort-Object -Unique
  $tsType = [regex]::Match(
    $typescript,
    "(?s)export\s+type\s+$([regex]::Escape($name))\s*=\s*(.*?);"
  )
  if (-not $tsType.Success) {
    continue
  }
  $typescriptVariants = [regex]::Matches($tsType.Groups[1].Value, "'([^']+)'") |
    ForEach-Object { $_.Groups[1].Value } |
    Sort-Object -Unique
  foreach ($variant in $rustVariants) {
    if ($variant -notin $typescriptVariants) {
      Fail "TypeScript $name is missing Rust variant $variant."
    }
  }
  foreach ($variant in $typescriptVariants) {
    if ($variant -notin $rustVariants) {
      Fail "TypeScript $name contains stale variant $variant."
    }
  }
}

$errorModule = [regex]::Match($rust, '(?s)pub\s+mod\s+error_codes\s*\{(.*?)\n\}').Groups[1].Value
$errorCodes = [regex]::Matches($errorModule, 'pub\s+const\s+[A-Z0-9_]+:\s*&str\s*=\s*"([A-Z0-9_]+)"') |
  ForEach-Object { $_.Groups[1].Value } |
  Sort-Object -Unique
$catalogCodes = @($catalog.Keys | Sort-Object -Unique)

foreach ($code in $errorCodes) {
  if (-not $catalog.ContainsKey($code)) {
    Fail "Public error $code is missing from fixtures/contracts/error-catalog.json."
  }
}
foreach ($code in $catalogCodes) {
  if ($code -notin $errorCodes) {
    Fail "Error fixture contains unknown code $code."
  }
  if ([string]::IsNullOrWhiteSpace([string]$catalog[$code])) {
    Fail "Error fixture $code has no remediation copy."
  }
}

$uiText = Get-ChildItem -LiteralPath $uiRoot -Recurse -File -Include '*.svelte', '*.ts' |
  Where-Object { $_.Name -ne 'generated.ts' } |
  ForEach-Object { Get-Content -Raw -LiteralPath $_.FullName }
$uiText = $uiText -join "`n"
foreach ($code in $errorCodes) {
  if ($uiText -notmatch "\b$([regex]::Escape($code))\b") {
    Fail "Public error $code has no explicit frontend remediation/state."
  }
}

if ($failed) {
  exit 1
}

Write-Host "Contract boundary verification passed: $($rustTypeNames.Count) V1 types and $($errorCodes.Count) public errors."
