[CmdletBinding()]
param(
  [Parameter(Mandatory)]
  [string]$InstallerPath,

  [string]$ExpectedSha256,

  [switch]$RequireTrustedSignature
)

$ErrorActionPreference = 'Stop'
$installer = (Resolve-Path -LiteralPath $InstallerPath).Path
$hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $installer).Hash.ToLowerInvariant()
$signature = Get-AuthenticodeSignature -LiteralPath $installer

if ($ExpectedSha256 -and $hash -ne $ExpectedSha256.Trim().ToLowerInvariant()) {
  throw "Installer SHA-256 mismatch. Expected $ExpectedSha256, got $hash."
}
if ($RequireTrustedSignature -and $signature.Status -ne 'Valid') {
  throw "Installer signature is not trusted: $($signature.Status)"
}

[pscustomobject]@{
  installer = Split-Path -Leaf $installer
  sha256 = $hash
  signature_status = [string]$signature.Status
  signer = if ($signature.SignerCertificate) { $signature.SignerCertificate.Subject } else { $null }
  timestamp_certificate = if ($signature.TimeStamperCertificate) { $signature.TimeStamperCertificate.Subject } else { $null }
} | ConvertTo-Json -Depth 3
