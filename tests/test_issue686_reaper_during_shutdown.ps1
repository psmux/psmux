# Issue #686 follow up: a psmux command run while another server shuts down
# must not terminate that server halfway through its shutdown.
#
# Every psmux invocation starts by reaping orphaned servers
# (session::reap_orphaned_servers): a psmux process that listens on loopback,
# is older than 10 s, is claimed by this data dir's ownership marker and is
# named by no .port file. shutdown_this_server removes its .port first and
# then waits up to 4 s for spares still inside CreateProcessW. In that window
# the server matched every rule, so any psmux command (another namespace, a
# prompt, a status job) terminated it with exit code 1, and the spares it was
# waiting for became orphan pwsh for ever.
#
# Deterministic: the session is aged past the reaper's grace period, every
# spawn is held 1500 ms (PSMUX_TEST_SLOW_CREATE_MS), and `ls` in another
# namespace runs during the shutdown.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$imgName = [IO.Path]::GetFileNameWithoutExtension($PSMUX)
$Ns = "i686reap_$(Get-Random)"
$Other = "i686reapother_$(Get-Random)"
$Rounds = 3
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkGray }

function Get-ServerPid {
    # By command line, never by image name alone.
    $p = Get-CimInstance Win32_Process -Filter "Name='$imgName.exe'" -EA SilentlyContinue |
         Where-Object { $_.CommandLine -match [regex]::Escape("-L $Ns") -and $_.CommandLine -match 'server' -and $_.CommandLine -notmatch '__warm__' }
    if ($p) { return @($p)[0].ProcessId }
    return 0
}
function Cleanup { & $PSMUX -L $Ns kill-server 2>&1 | Out-Null; & $PSMUX -L $Other kill-server 2>&1 | Out-Null }

Write-Host "`n=== #686 follow up: the orphan reaper during a shutdown ===" -ForegroundColor Cyan
Write-Host "  binary: $PSMUX  namespace: $Ns"

$env:PSMUX_TEST_SLOW_CREATE_MS = "1500"
$leaked = 0; $selfKilled = 0
for ($r = 1; $r -le $Rounds; $r++) {
    & $PSMUX -L $Ns new-session -d -s "reap$r" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 900
    $srv = Get-ServerPid
    if ($srv -eq 0) { Write-Fail "round ${r}: no server"; continue }
    $h = [Diagnostics.Process]::GetProcessById($srv); $null = $h.Handle
    # Past the reaper's 10 s grace, so the reaper would consider it.
    while (((Get-Date) - $h.StartTime).TotalSeconds -lt 11) { Start-Sleep -Milliseconds 200 }
    # A burst of creations leaves spares inside their (held) create.
    for ($i = 1; $i -le 3; $i++) { & $PSMUX -L $Ns new-window -t "reap${r}:" 2>&1 | Out-Null }
    $kill = Start-Process -FilePath $PSMUX -ArgumentList @('-L', $Ns, 'kill-server') -WindowStyle Hidden -PassThru
    # Any psmux command reaps; run a few in another namespace while the
    # server is inside its shutdown wait.
    Start-Sleep -Milliseconds 250
    for ($i = 0; $i -lt 4; $i++) { & $PSMUX -L $Other ls 2>&1 | Out-Null; Start-Sleep -Milliseconds 150 }
    $kill.WaitForExit(15000) | Out-Null
    $h.WaitForExit(8000) | Out-Null
    Start-Sleep -Seconds 3
    $code = $h.ExitCode
    # Only processes created after the server started: Windows reuses pids, and
    # boot processes (csrss.exe, wininit.exe) carry a long dead smss pid as their
    # parent. A server handed that pid once made test_issue686_pool_surge_and_reap
    # count them as orphans and try to Stop-Process them (sweep 2026-11-21_15-34-11).
    $srvStart = $h.StartTime
    $orph = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$srv" -EA SilentlyContinue |
              Where-Object { $_.CreationDate -ge $srvStart })
    Write-Info "round ${r}: server $srv exit code $code, $($orph.Count) orphan(s) $(($orph | ForEach-Object { "$($_.Name)/$($_.ProcessId)" }) -join ' ')"
    if ($code -ne 0) { $selfKilled++ }
    if ($orph.Count -gt 0) {
        $leaked += $orph.Count
        foreach ($o in $orph) { try { Stop-Process -Id $o.ProcessId -Force -EA Stop } catch {} }
    }
    Start-Sleep -Milliseconds 300
}
Remove-Item Env:PSMUX_TEST_SLOW_CREATE_MS -ErrorAction SilentlyContinue

if ($selfKilled -eq 0) { Write-Pass "every server finished its own shutdown (exit code 0) with psmux commands running beside it ($Rounds rounds)" }
else { Write-Fail "$selfKilled of $Rounds servers were terminated mid shutdown (exit code not 0): another psmux process reaped a server that was shutting down" }
if ($leaked -eq 0) { Write-Pass "no orphan pane shells over $Rounds rounds" }
else { Write-Fail "$leaked orphan pane shell(s) over $Rounds rounds: the shutdown was cut short before its in flight spares were reaped" }

Cleanup
Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)"
Write-Host "  Failed: $($script:TestsFailed)"
exit $script:TestsFailed
