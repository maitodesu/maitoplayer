[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

function Assert-True([bool]$Condition, [string]$Message) {
  if (-not $Condition) {
    throw $Message
  }
}

$configPath = Join-Path $repoRoot 'apps\desktop\src-tauri\tauri.conf.json'
$capabilityPath = Join-Path $repoRoot 'apps\desktop\src-tauri\capabilities\default.json'
$commandsPath = Join-Path $repoRoot 'apps\desktop\src-tauri\src\commands\mod.rs'
$uiRoot = Join-Path $repoRoot 'apps\desktop\ui\src'
$runnerPath = Join-Path $repoRoot 'crates\media-engine\src\toolchain\mod.rs'
$mediaEnginePath = Join-Path $repoRoot 'crates\media-engine\src\lib.rs'
$sessionPath = Join-Path $repoRoot 'crates\media-engine\src\session\mod.rs'
$ankiTransportPath = Join-Path $repoRoot 'crates\anki-connect\src\transport\mod.rs'
$ankiPublishPath = Join-Path $repoRoot 'crates\anki-connect\src\publish\mod.rs'
$ankiNotePath = Join-Path $repoRoot 'crates\anki-connect\src\note\mod.rs'
$diagnosticsPath = Join-Path $repoRoot 'apps\desktop\ui\src\lib\features\diagnostics\DiagnosticsPanel.svelte'

$config = Get-Content -Raw -LiteralPath $configPath | ConvertFrom-Json
$csp = [string]$config.app.security.csp
foreach ($directive in @("default-src 'self'", "script-src 'self'", "object-src 'none'", "frame-src 'none'", "form-action 'none'")) {
  Assert-True ($csp.Contains($directive)) "CSP is missing required directive: $directive"
}
Assert-True (-not $csp.Contains("'unsafe-eval'")) 'CSP permits unsafe-eval.'
Assert-True (-not $csp.Contains('https:')) 'CSP permits remote HTTPS content.'

$capability = Get-Content -Raw -LiteralPath $capabilityPath | ConvertFrom-Json
$permissions = @($capability.permissions)
Assert-True ($permissions.Count -eq 1 -and $permissions[0] -eq 'core:default') 'Desktop capability is broader than core:default.'
Assert-True (-not (($permissions -join ' ') -match '(?i)fs:|shell:|process:')) 'Filesystem, shell, or process permissions are exposed to the webview.'

$commands = Get-Content -Raw -LiteralPath $commandsPath
$commandBlocks = [regex]::Matches($commands, '(?s)#\[tauri::command\]\s*pub\s+(?:async\s+)?fn\s+\w+\s*\((.*?)\)\s*->')
foreach ($block in $commandBlocks) {
  Assert-True ($block.Groups[1].Value -notmatch '\b(?:PathBuf|Path|OsString)\b') 'A public Tauri command accepts an arbitrary filesystem path.'
}

$uiFiles = Get-ChildItem -LiteralPath $uiRoot -Recurse -File -Include '*.svelte', '*.ts'
Assert-True (-not ($uiFiles | Select-String -SimpleMatch '{@html')) 'Frontend contains untrusted Svelte {@html} rendering.'
Assert-True (-not ($uiFiles | Select-String -Pattern '\bfetch\s*\(')) 'Frontend performs direct network/media fetches.'

$runner = Get-Content -Raw -LiteralPath $runnerPath
Assert-True ($runner -match 'Command::new\(&request\.executable\)') 'Media runner no longer uses the validated executable directly.'
Assert-True ($runner -notmatch '(?i)cmd\.exe|powershell(?:\.exe)?|/c\b|-command\b') 'Media runner invokes a command shell.'
Assert-True ($runner -match 'max_stdout_bytes' -and $runner -match 'max_stderr_bytes' -and $runner -match 'request\.timeout') 'Media runner output/time bounds are missing.'
Assert-True ($runner -match 'limit_kill_on_job_close' -and $runner -match 'assign_process') 'Windows media children are not assigned to a kill-on-close Job Object.'

$mediaEngine = Get-Content -Raw -LiteralPath $mediaEnginePath
$closeImplementation = [regex]::Match(
  $mediaEngine,
  '(?s)fn\s+close\s*\(&self,\s*session_id:.*?self\.sessions\.close\(session_id\)'
).Value
Assert-True ($closeImplementation -match 'conversions' -and $closeImplementation -match 'mining_operations') 'Session close does not cancel both playback conversion and mining extraction.'

$session = Get-Content -Raw -LiteralPath $sessionPath
Assert-True ($session -match 'MAX_PROTOCOL_CHUNK(?:_BYTES)?:\s*usize\s*=\s*8\s*\*\s*1024\s*\*\s*1024') 'Scoped media response cap is absent or changed from 8 MiB.'
Assert-True ($session -match 'sessions:\s*RwLock<HashMap<MediaSessionId') 'Scoped media protocol is not backed by opaque session IDs.'

$ankiTransport = Get-Content -Raw -LiteralPath $ankiTransportPath
Assert-True ($ankiTransport -match 'Ipv4Addr::LOCALHOST') 'Anki transport is not pinned to IPv4 loopback.'
Assert-True ($ankiTransport -match 'connect_timeout' -and $ankiTransport -match 'set_read_timeout' -and $ankiTransport -match 'set_write_timeout') 'Anki transport timeout controls are missing.'
Assert-True ($ankiTransport -match 'MAX_RESPONSE_BYTES' -and $ankiTransport -match 'MAX_HEADER_BYTES') 'Anki response bounds are missing.'
Assert-True ($ankiTransport -match '!is_read_only_action\(action\)') 'Anki automatic retries are not restricted to read-only actions.'

$ankiPublish = Get-Content -Raw -LiteralPath $ankiPublishPath
Assert-True ($ankiPublish -match 'MAX_TOTAL_MEDIA_BYTES' -and $ankiPublish -match 'Sha256::digest\(&media\.data\)') 'Anki media size or content-address validation is missing.'

$ankiNote = Get-Content -Raw -LiteralPath $ankiNotePath
$editableFieldsRestricted = (
  $ankiNote -match 'editable_fields\.keys\(\)\.any' -and
  $ankiNote -match '"expression"\s*\|\s*"reading"\s*\|\s*"sentence"\s*\|\s*"definition"'
)
Assert-True $editableFieldsRestricted 'Frontend-editable fields are not restricted before trusted audio/image markup and mining metadata are rendered.'

$diagnostics = Get-Content -Raw -LiteralPath $diagnosticsPath
Assert-True ($diagnostics -notmatch '\bsource_path\b|\bsubtitle_path\b') 'Diagnostics references an unredacted source/subtitle path.'
Assert-True ($diagnostics -notmatch '(?m)^\s*health,\s*$') 'Diagnostics serializes raw health errors, including provider-supplied diagnostic text.'

Write-Host 'Desktop security-boundary verification passed.'
