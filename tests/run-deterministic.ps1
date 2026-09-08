[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

function Invoke-Gate([string]$Label, [scriptblock]$Command) {
  Write-Host "`n== $Label ==" -ForegroundColor Cyan
  & $Command
  if ($LASTEXITCODE -ne 0) {
    throw "$Label failed with exit code $LASTEXITCODE."
  }
}

Push-Location $repoRoot
try {
  Invoke-Gate 'Rust format' { cargo fmt --all -- --check }
  Invoke-Gate 'Rust clippy' { cargo clippy --workspace --all-targets -- -D warnings }
  Invoke-Gate 'Rust workspace tests' { cargo test --workspace }
  Invoke-Gate 'Tauri format' { cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml -- --check }
  Invoke-Gate 'Tauri clippy' { cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings }
  Invoke-Gate 'Tauri tests' { cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml }
  Invoke-Gate 'Generated contract freshness' {
    $generated = Join-Path $repoRoot 'apps/desktop/ui/src/lib/contracts/generated.ts'
    $before = [IO.File]::ReadAllText($generated)
    pnpm generate:contracts
    if ($LASTEXITCODE -eq 0 -and $before -cne [IO.File]::ReadAllText($generated)) {
      throw 'Generated TypeScript contracts were stale. Review and commit the regenerated binding.'
    }
  }
  Invoke-Gate 'Frontend format' { pnpm format:check }
  Invoke-Gate 'Frontend typecheck' { pnpm check }
  Invoke-Gate 'Frontend unit tests' { pnpm test }
  Invoke-Gate 'Frontend production build' { pnpm build }
  Invoke-Gate 'Contract boundary' { & ./tests/contract/verify-contract-boundaries.ps1 }
  Invoke-Gate 'Desktop security boundary' { & ./tests/security/verify-desktop-boundary.ps1 }
  Invoke-Gate 'Bundled runtime supply-chain guard' { & ./tools/ci/check-runtime-dependencies.ps1 }
  Invoke-Gate 'Repository artifact guard' { & ./tools/ci/check-artifacts.ps1 }
  Invoke-Gate 'Repository capability guard' { & ./tools/ci/check-capabilities.ps1 }
  Invoke-Gate 'Repository secret-pattern guard' { & ./tools/ci/check-secrets.ps1 }
} finally {
  Pop-Location
}

Write-Host "`nDeterministic verification passed." -ForegroundColor Green
