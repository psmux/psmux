param([string]$Out = 'C:\Users\godwin\Documents\workspace\psmux-killpane\rerun_kp.txt', [int]$Rounds = 3)
$w = 'C:\Users\godwin\Documents\workspace\psmux-killpane'
"" | Set-Content $Out
for ($r = 1; $r -le $Rounds; $r++) {
  foreach ($side in 'kbinm', 'kbinf') {
    $bin = "$w\$side\psmux.exe"
    foreach ($s in @(
        @{ f = 'test_issue686_reaper_during_shutdown.ps1'; arg = $null },
        @{ f = 'test_issue686_pool_surge_and_reap.ps1'; arg = '-Binary' },
        @{ f = 'test_kill_pane_loop_stall.ps1'; arg = '-Exe' })) {
      $data = "$env:TEMP\kprr-$side"
      New-Item -ItemType Directory -Force $data | Out-Null
      # the stall suite refuses nothing but sets its own data dir; the others use this one
      if ($s.f -ne 'test_kill_pane_loop_stall.ps1') { $env:PSMUX_DATA_DIR = $data } else { Remove-Item Env:PSMUX_DATA_DIR -EA SilentlyContinue }
      $env:PSMUX_TEST_BIN = $bin
      $a = @('-NoProfile', '-File', "$w\tests\$($s.f)")
      if ($s.arg) { $a += @($s.arg, $bin) }
      $o = & pwsh @a 2>&1
      $code = $LASTEXITCODE
      $fails = @($o | Select-String '(\[FAIL\]|FAIL:)' | ForEach-Object { $_.Line.Trim() })
      $extra = @($o | Select-String 'display-message with kill-pane' | ForEach-Object { $_.Line.Trim() })
      $line = "r{0} {1} {2} exit={3} {4} {5}" -f $r, $side, $s.f, $code, ($extra -join ''), ($fails -join ' ; ')
      $line | Add-Content $Out; $line
      Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $bin } | ForEach-Object { "    LEFTOVER pid=$($_.ProcessId) $($_.CommandLine)" | Add-Content $Out; Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
      Remove-Item -Recurse -Force $data -EA SilentlyContinue
    }
  }
}
