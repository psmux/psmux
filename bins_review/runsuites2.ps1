$ErrorActionPreference = 'Continue'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$wt = Split-Path $PSScriptRoot
$b = $PSScriptRoot
Set-Location $wt
"default before: " + (psmux ls 2>&1 | Out-String).Trim() | Out-File "$b\ls_before.txt"
function RunSuite($label, $script, $exe, $extra = @{}) {
    Remove-Item env:PSMUX_SERVER_MODE, env:PSMUX_BASELINE_EXE -EA SilentlyContinue
    $env:PSMUX_TEST_EXE = $exe
    $env:PSMUX_TEST_BIN = $exe
    foreach ($k in $extra.Keys) { Set-Item "env:$k" $extra[$k] }
    $log = "$b\log_$label.txt"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    & pwsh -NoProfile -File $script *>&1 | Out-File -Encoding utf8 $log
    "$label exit=$LASTEXITCODE $([int]$sw.Elapsed.TotalSeconds)s" | Out-File -Append "$b\summary.txt"
}
Remove-Item "$b\summary.txt" -EA SilentlyContinue
git checkout -q --detach pr910
RunSuite 'p910_shared_master' "$wt\tests\test_issue798_shared_server.ps1" "$b\master\psmux.exe"
RunSuite 'p910_shared_pr910' "$wt\tests\test_issue798_shared_server.ps1" "$b\pr910\psmux.exe"
RunSuite 'p910_switch_pr910' "$wt\tests\test_issue798_server_mode_switch.ps1" "$b\pr910\psmux.exe" @{ PSMUX_BASELINE_EXE = "$b\master\psmux.exe" }
RunSuite 'p910_897_pr910' "$wt\tests\test_issue897_shared_rare_cases.ps1" "$b\pr910\psmux.exe"
git checkout -q --detach pr909
RunSuite 'p909_897_master' "$wt\tests\test_issue897_shared_rare_cases.ps1" "$b\master\psmux.exe"
RunSuite 'p909_897_pr909' "$wt\tests\test_issue897_shared_rare_cases.ps1" "$b\pr909\psmux.exe"
RunSuite 'p909_shared_pr909' "$wt\tests\test_issue798_shared_server.ps1" "$b\pr909\psmux.exe"
RunSuite 'p909_switch_pr909' "$wt\tests\test_issue798_server_mode_switch.ps1" "$b\pr909\psmux.exe" @{ PSMUX_BASELINE_EXE = "$b\master\psmux.exe" }
Remove-Item env:PSMUX_TEST_EXE, env:PSMUX_TEST_BIN, env:PSMUX_BASELINE_EXE -EA SilentlyContinue
"default after: " + (psmux ls 2>&1 | Out-String).Trim() | Out-File "$b\ls_after.txt"
"ALLDONE" | Out-File -Append "$b\summary.txt"

