param([string]$Out = 'C:\Users\godwin\Documents\workspace\psmux-killpane\suites_kp.txt')
$w = 'C:\Users\godwin\Documents\workspace\psmux-killpane'
"" | Set-Content $Out
$suites = @(
  @{ f = 'test_issue964_kill_pane_all.ps1'; arg = '-Exe' },
  @{ f = 'test_issue964_kill_pane_filter.ps1'; arg = '-Exe' },
  @{ f = 'test_issue686_pool_surge_and_reap.ps1'; arg = '-Binary' },
  @{ f = 'test_issue686_reaper_during_shutdown.ps1'; arg = $null },
  @{ f = 'test_robust_churn.ps1'; arg = $null }
)
foreach ($side in 'kbinm', 'kbinf') {
  $bin = "$w\$side\psmux.exe"
  foreach ($s in $suites) {
    $env:PSMUX_DATA_DIR = "$env:TEMP\kpsuite-$side"
    New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
    $env:PSMUX_TEST_BIN = $bin
    $a = @('-NoProfile', '-File', "$w\tests\$($s.f)")
    if ($s.arg) { $a += @($s.arg, $bin) }
    $o = & pwsh @a 2>&1
    $code = $LASTEXITCODE
    $pass = @($o | Select-String '(\[PASS\]|PASS:)').Count
    $fail = @($o | Select-String '(\[FAIL\]|FAIL:)')
    $line = "{0} {1} exit={2} pass={3} fail={4}" -f $side, $s.f, $code, $pass, $fail.Count
    $line | Add-Content $Out; $line
    $fail | Select-Object -First 6 | ForEach-Object { "    $($_.Line.Trim())" | Add-Content $Out }
    Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $bin } | ForEach-Object { "    LEFTOVER pid=$($_.ProcessId) $($_.CommandLine)" | Add-Content $Out; Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
    Remove-Item -Recurse -Force $env:PSMUX_DATA_DIR -EA SilentlyContinue
  }
}
