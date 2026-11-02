#requires -Version 7.0
# pipe-pane runs its command under the run shell (pwsh -Command), so a sink
# that is itself a program is a GRANDCHILD of the server. When that sink stops
# reading, the pane's output stalls behind it. Stopping the pipe must release
# the pane: the stopped sink's whole process tree goes, not just the shell, or
# the grandchild keeps the inherited stdin open and the pane stays frozen.
# Isolated: private USERPROFILE (New-PsmuxTestEnv) and a unique -L namespace.
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\psmux_test_helpers.ps1"
$exe = Get-PsmuxExe
$ctx = New-PsmuxTestEnv -Tag 'pipegc' -Exe $exe
$marker = 'Start-Sleep -Seconds ' + (500 + (Get-Random -Maximum 400))
$fail = 0
function Check([bool]$ok, [string]$msg) {
    if ($ok) { Write-Host "PASS: $msg" } else { Write-Host "FAIL: $msg"; $script:fail++ }
}
function Sinks { @(Get-CimInstance Win32_Process -Filter "Name='pwsh.exe'" | Where-Object { $_.CommandLine -and $_.CommandLine.Contains($marker) }) }
try {
    $ns = Register-PsmuxNamespace -Ctx $ctx -Namespace ('pipegc-' + [guid]::NewGuid().ToString('N').Substring(0, 12))
    function P { & $exe -L $ns @args 2>&1 }
    function LastBulk { ((P capture-pane -p -t s) | ForEach-Object { "$_" } | Where-Object { $_ -match 'BULKLINE|BULK_DONE' } | Select-Object -Last 1) }
    P new-session -d -s s -x 120 -y 30 'pwsh -NoProfile -NoLogo' | Out-Null
    Start-Sleep 3
    # The sink never reads: an inner pwsh that only sleeps (a grandchild).
    P pipe-pane -O -t s "pwsh -NoProfile -c `"$marker`"" | Out-Null
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ((Sinks).Count -lt 2 -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 200 }
    Check ((Sinks).Count -ge 2) 'sink shell and its grandchild are running'
    P send-keys -t s '1..300000 | % { "BULKLINE $_ xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx" }; echo BULK_DONE' Enter | Out-Null
    # Wait until the pane output stops moving: the sink is backpressuring it.
    $last = ''; $stable = 0
    for ($i = 0; $i -lt 60 -and $stable -lt 3; $i++) {
        Start-Sleep -Milliseconds 500
        $now = LastBulk
        if ($now -and $now -eq $last) { $stable++ } else { $stable = 0 }
        $last = $now
    }
    Check ($stable -ge 3 -and $last -notmatch 'BULK_DONE') "pane output is stalled behind the nonreading sink ($last)"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    P pipe-pane -t s | Out-Null
    Check ($sw.ElapsedMilliseconds -lt 2000) "stopping the pipe answers promptly ($($sw.ElapsedMilliseconds) ms)"
    $done = $false
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    while (-not $done -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 500
        $done = ((P capture-pane -p -t s) -join "`n") -match '(?m)^BULK_DONE'
    }
    Check $done 'pane output resumes and finishes after the pipe is stopped'
    Check ((Sinks).Count -eq 0) 'no process of the stopped sink is left running'
    Check (((P display-message -t s -p 'ALIVE') -join '') -eq 'ALIVE') 'server still answers'
} finally {
    foreach ($p in (Sinks)) { Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue }
    Remove-PsmuxTestEnv -Ctx $ctx
}
if ($fail) { Write-Host "$fail check(s) failed"; exit 1 }
Write-Host 'All pipe-pane grandchild sink checks passed.'
