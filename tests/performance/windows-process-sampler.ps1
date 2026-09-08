[CmdletBinding()]
param(
  [Parameter(Mandatory)]
  [int]$ProcessId,

  [ValidateRange(10, 86400)]
  [int]$DurationSeconds = 7200,

  [ValidateRange(1, 60)]
  [int]$IntervalSeconds = 5,

  [Parameter(Mandatory)]
  [string]$OutputCsv
)

$ErrorActionPreference = 'Stop'
$resolvedOutput = [System.IO.Path]::GetFullPath($OutputCsv)
$parent = Split-Path -Parent $resolvedOutput
if ($parent) {
  New-Item -ItemType Directory -Path $parent -Force | Out-Null
}

$samples = [System.Collections.Generic.List[object]]::new()
$rootStartUtc = (Get-Process -Id $ProcessId -ErrorAction Stop).StartTime.ToUniversalTime()
$started = [DateTimeOffset]::UtcNow
$deadline = $started.AddSeconds($DurationSeconds)
while ([DateTimeOffset]::UtcNow -lt $deadline) {
  $rootProcess = Get-Process -Id $ProcessId -ErrorAction Stop
  if ($rootProcess.StartTime.ToUniversalTime() -ne $rootStartUtc) {
    throw "Process ID $ProcessId was reused after the sampled application exited."
  }
  $processRows = @(Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId)
  $treeIds = [System.Collections.Generic.HashSet[int]]::new()
  [void]$treeIds.Add($ProcessId)
  do {
    $added = 0
    foreach ($row in $processRows) {
      if ($treeIds.Contains([int]$row.ParentProcessId) -and
          $treeIds.Add([int]$row.ProcessId)) {
        $added++
      }
    }
  } while ($added -gt 0)
  $treeProcesses = @(Get-Process -Id @($treeIds) -ErrorAction SilentlyContinue)
  $treeWorkingSet = ($treeProcesses | Measure-Object -Property WorkingSet64 -Sum).Sum
  $treePrivateMemory = ($treeProcesses | Measure-Object -Property PrivateMemorySize64 -Sum).Sum
  $treeHandles = ($treeProcesses | Measure-Object -Property HandleCount -Sum).Sum
  $treeThreads = ($treeProcesses | ForEach-Object { $_.Threads.Count } | Measure-Object -Sum).Sum
  $treeCpu = ($treeProcesses | ForEach-Object { $_.TotalProcessorTime.TotalSeconds } | Measure-Object -Sum).Sum
  $samples.Add([pscustomobject]@{
      timestamp_utc = [DateTimeOffset]::UtcNow.ToString('O')
      elapsed_seconds = [math]::Round(([DateTimeOffset]::UtcNow - $started).TotalSeconds, 3)
      root_pid = $ProcessId
      root_start_utc = $rootStartUtc.ToString('O')
      process_count = $treeProcesses.Count
      root_working_set_mib = [math]::Round($rootProcess.WorkingSet64 / 1MB, 3)
      root_private_memory_mib = [math]::Round($rootProcess.PrivateMemorySize64 / 1MB, 3)
      tree_working_set_mib = [math]::Round($treeWorkingSet / 1MB, 3)
      tree_private_memory_mib = [math]::Round($treePrivateMemory / 1MB, 3)
      tree_handles = [int64]$treeHandles
      tree_threads = [int64]$treeThreads
      tree_total_cpu_seconds = [math]::Round($treeCpu, 3)
    })
  Start-Sleep -Seconds $IntervalSeconds
}

$samples | Export-Csv -LiteralPath $resolvedOutput -NoTypeInformation -Encoding utf8
Write-Host "Wrote $($samples.Count) process samples to $resolvedOutput"
