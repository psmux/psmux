# Cross session join-pane: who owns the moved pane, and for how long.
#
# A pane joined from session S into session T keeps its real ConPTY and child
# in S's server process; T gets a proxy pane that tunnels I/O to it over
# loopback TCP (src/proxy_pane.rs, src/cross_session_server.rs).
#
# tmux 3.4 (one server): the pane simply moves. If it was the last pane of S,
# S is destroyed when its last window goes and the pane lives on in T
# (server_destroy_session via session_detach). kill-session -t S afterwards
# (or with S still alive) does not touch a pane that now belongs to T.
#
# Defects this suite pins (reproduced on master 98392248):
#   1. join-pane of S's LAST pane: S's server exited because its window list
#      was empty, the real shell died with it, T kept a zombie pane
#      (pane_dead=0, keystrokes go nowhere) and S.pid / S.sid stayed on disk.
#   2. kill-session -t S killed the moved shell, while T's pane stayed
#      pane_dead=0 and no pane-exited fired.
#
# Contract after the fix:
#   * the source server keeps hosting a forwarded pane until that pane exits,
#     even when S has no windows of its own left; S is then not a user session
#     (not in ls, has-session fails, no S registry files, the name is free);
#   * when the hosted pane exits or T drops it, the host process ends;
#   * if the hosting process ends anyway, T sees the pane as dead and fires
#     pane-exited / pane-died, honouring remain-on-exit.
#
# Note on ids: pane ids are allocated per server today (docs/design/
# multi-session-server.md, section 3.2), so the moved pane gets a new %id in T.
# tmux keeps the id; psmux cannot until ids are server or namespace unique.
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_joinpane_proxy_ownership.ps1
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = if ($env:PSMUX_TEST_NS) { $env:PSMUX_TEST_NS } else { "jpown$PID" }
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkGray }
function Check($cond, $m) { if ($cond) { Write-Pass $m } else { Write-Fail $m } }

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','PSMUX_TARGET_SESSION','TMUX','TMUX_PANE') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$savedDataDir = $env:PSMUX_DATA_DIR
$savedNoWarm  = $env:PSMUX_NO_WARM
$root = Join-Path $env:TEMP "psmux_jpown_$PID"
Remove-Item -Recurse -Force $root -EA SilentlyContinue
New-Item -ItemType Directory -Force $root | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $root "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_NO_WARM = "1"
$DATA = $env:PSMUX_DATA_DIR

Write-Host "`n=== join-pane proxy: ownership of a pane moved across sessions ===" -ForegroundColor Cyan
Write-Info ("psmux: {0}  ({1})" -f $PSMUX, ((& $PSMUX -V) -join ' '))

function P { & $PSMUX -L $NS @args 2>&1 }
function Wait-Until([scriptblock]$cond, [int]$ms) {
    $deadline = (Get-Date).AddMilliseconds($ms)
    while ((Get-Date) -lt $deadline) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 100 }
    return (& $cond)
}
function Alive([int]$id) { [bool](Get-Process -Id $id -EA SilentlyContinue) }
function NsServers { @(Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" -EA SilentlyContinue | Where-Object { $_.CommandLine -match "-L $NS\b" -and $_.CommandLine -match '\bserver\b' }) }
function ServerPid([string]$s) {
    $f = Join-Path $DATA "${NS}__$s.pid"
    if (Test-Path $f) { [int]((Get-Content $f -Raw).Split(':')[0]) } else { 0 }
}
function RegFiles([string]$s) { @(Get-ChildItem $DATA -Filter "${NS}__$s.*" -EA SilentlyContinue | ForEach-Object Name) }
function Echoes([string]$target) {
    $tok = "jp$(Get-Random -Maximum 99999999)"
    P send-keys -t $target "echo $tok" Enter | Out-Null
    return (Wait-Until { (([regex]::Matches(((P capture-pane -p -t $target) -join "`n"), $tok)).Count -ge 2) } 6000)
}
function New-Pwsh([string]$s) { P new-session -d -s $s -x 110 -y 30 pwsh -NoLogo -NoProfile | Out-Null }
function Ready([string]$t) { Wait-Until { ((P capture-pane -p -t $t) -join '') -match 'PS ' } 15000 | Out-Null }

$sessionsBefore = $null
try {
    # ── Case 1: join the LAST pane of S into T ──────────────────────────────
    Write-Host "`n[1] join-pane moves the last pane of the source session" -ForegroundColor Cyan
    New-Pwsh "S1"; New-Pwsh "T1"
    Ready "S1:0.0"; Ready "T1:0.0"
    $srcPid = ServerPid "S1"
    $shell = [int](P display-message -p -t S1:0.0 '#{pane_pid}')
    Write-Info "source server $srcPid, shell $shell"
    $out = P join-pane -s S1:0.0 -t T1:0
    Check ($LASTEXITCODE -eq 0) "1.1 join-pane exits 0 ($out)"
    Start-Sleep -Milliseconds 1500
    Check (Alive $shell) "1.2 the moved shell is still running"
    $moved = P list-panes -t T1:0 -F '#{pane_index} #{pane_pid} #{pane_dead}' | Where-Object { $_ -match "^\d+ $shell " }
    Check ([bool]$moved -and $moved -match ' 0$') "1.3 T1 lists the moved pane, alive, with the shell's pid ($moved)"
    Check (Echoes "T1:0.1") "1.4 typing into the moved pane echoes"
    P has-session -t S1 | Out-Null
    Check ($LASTEXITCODE -ne 0) "1.5 has-session -t S1 fails: the emptied source session is gone"
    $ls = (P ls) -join ' / '
    Check ($ls -notmatch '\bS1:') "1.6 ls does not list S1 ($ls)"
    $rf = RegFiles "S1"
    Check ($rf.Count -eq 0) "1.7 no S1 registry files remain ($($rf -join ' '))"
    New-Pwsh "S1"
    P has-session -t S1 | Out-Null
    Check ($LASTEXITCODE -eq 0) "1.8 the name S1 is free again: a new S1 starts"
    Check (Alive $shell) "1.9 the moved shell survives a new session taking the old name"
    P kill-session -t S1 | Out-Null
    Start-Sleep -Milliseconds 800
    Check (Alive $shell) "1.10 killing the NEW S1 does not touch the moved shell"
    Check (Echoes "T1:0.1") "1.11 the moved pane still echoes"
    # T drops the pane: the shell and the host process must end.
    P kill-pane -t T1:0.1 | Out-Null
    Check (Wait-Until { -not (Alive $shell) } 5000) "1.12 kill-pane in T ends the moved shell"
    Check (Wait-Until { -not (Alive $srcPid) } 5000) "1.13 the hosting process exits once its last hosted pane is gone"
    P kill-session -t T1 | Out-Null

    # ── Case 2: kill-session on a source that still has windows ─────────────
    Write-Host "`n[2] kill-session on the source does not kill the moved pane" -ForegroundColor Cyan
    New-Pwsh "S2"; New-Pwsh "T2"
    P new-window -d -t S2 | Out-Null
    Ready "S2:0.0"; Ready "T2:0.0"
    $srcPid = ServerPid "S2"
    $shell = [int](P display-message -p -t S2:0.0 '#{pane_pid}')
    $other = [int](P display-message -p -t S2:1.0 '#{pane_pid}')
    P join-pane -s S2:0.0 -t T2:0 | Out-Null
    Check ($LASTEXITCODE -eq 0) "2.1 join-pane exits 0"
    Start-Sleep -Milliseconds 1000
    P kill-session -t S2 | Out-Null
    Start-Sleep -Milliseconds 1500
    Check (-not (Alive $other)) "2.2 kill-session -t S2 ends S2's own pane"
    Check (Alive $shell) "2.3 the moved shell survives kill-session of its old session (tmux parity)"
    P has-session -t S2 | Out-Null
    Check ($LASTEXITCODE -ne 0) "2.4 S2 is gone"
    Check ((RegFiles "S2").Count -eq 0) "2.5 no S2 registry files remain ($((RegFiles 'S2') -join ' '))"
    Check (Echoes "T2:0.1") "2.6 the moved pane still echoes"
    # The shell exits by itself: T removes the pane (remain-on-exit off), host ends.
    P set-hook -t T2 pane-exited "set -g @jp_exited yes" | Out-Null
    P send-keys -t T2:0.1 "exit" Enter | Out-Null
    Check (Wait-Until { -not (Alive $shell) } 5000) "2.7 the moved shell exits on 'exit'"
    Check (Wait-Until { ((P list-panes -t T2:0 -F '#{pane_pid}') -notcontains "$shell") } 5000) "2.8 T2 removes the exited pane (remain-on-exit off)"
    Check (Wait-Until { ((P show-options -gv -t T2 '@jp_exited') -join '') -eq 'yes' } 4000) "2.9 pane-exited fired in T2"
    Check (Wait-Until { -not (Alive $srcPid) } 5000) "2.10 the hosting process exits after its hosted pane exited"
    P kill-session -t T2 | Out-Null

    # ── Case 3: the hosting process dies under the pane ─────────────────────
    Write-Host "`n[3] the host process ends: the target sees a dead pane" -ForegroundColor Cyan
    foreach ($roe in 'on', 'off') {
        $s = "S3$roe"; $t = "T3$roe"
        New-Pwsh $s; New-Pwsh $t
        P new-window -d -t $s | Out-Null
        Ready "${s}:0.0"; Ready "${t}:0.0"
        $srcPid = ServerPid $s
        $shell = [int](P display-message -p -t "${s}:0.0" '#{pane_pid}')
        P set-option -g -t $t remain-on-exit $roe | Out-Null
        P set-hook -t $t pane-exited "set -g @jp_exited yes" | Out-Null
        P set-hook -t $t pane-died "set -g @jp_died yes" | Out-Null
        P join-pane -s "${s}:0.0" -t "${t}:0" | Out-Null
        Start-Sleep -Milliseconds 1000
        $pid1 = (P list-panes -t "${t}:0" -F '#{pane_index} #{pane_pid}' | Where-Object { $_ -match " $shell$" })
        Check ([bool]$pid1) "3.$roe.1 the moved pane is in $t ($pid1)"
        # This suite started $srcPid's session, so it may end that process by PID.
        Stop-Process -Id $srcPid -Force -EA SilentlyContinue
        Check (Wait-Until { -not (Alive $srcPid) } 3000) "3.$roe.2 host process $srcPid ended"
        if ($roe -eq 'on') {
            Check (Wait-Until { ((P list-panes -t "${t}:0" -F '#{pane_index} #{pane_dead}') -contains '1 1') } 5000) "3.on.3 remain-on-exit on: the pane stays, pane_dead=1 ($((P list-panes -t "${t}:0" -F '#{pane_index} #{pane_dead}') -join ','))"
            Check (Wait-Until { ((P show-options -gv -t $t '@jp_died') -join '') -eq 'yes' } 4000) "3.on.4 pane-died fired"
        } else {
            Check (Wait-Until { @(P list-panes -t "${t}:0" -F '#{pane_index}').Count -eq 1 } 5000) "3.off.3 remain-on-exit off: the dead pane is removed"
            Check (Wait-Until { ((P show-options -gv -t $t '@jp_exited') -join '') -eq 'yes' } 4000) "3.off.4 pane-exited fired"
        }
        P kill-session -t $t | Out-Null
    }

    # ── Case 5: the joined pane's id is not handed out again ────────────────
    # The proxy pane used to get "highest id in the tree + 1", which is not
    # what the allocator hands out next, so a later split or new-window could
    # reuse it and two panes answered to the same %N.
    Write-Host "`n[5] pane ids stay unique in the target after a join" -ForegroundColor Cyan
    New-Pwsh "S5"; New-Pwsh "T5"
    P new-window -d -t S5 | Out-Null
    Ready "S5:0.0"; Ready "T5:0.0"
    P new-window -d -t T5 | Out-Null
    P join-pane -s S5:0.0 -t T5:0 | Out-Null
    Start-Sleep -Milliseconds 800
    P split-window -d -t T5:0.0 | Out-Null
    P new-window -d -t T5 | Out-Null
    $ids = @(P list-panes -a -F '#{session_name} #{pane_id}' | Where-Object { $_ -match '^T5 ' } | ForEach-Object { ($_ -split ' ')[1] })
    Check ($ids.Count -eq 5 -and ($ids | Sort-Object -Unique).Count -eq $ids.Count) "5.1 five panes in T5, five distinct ids ($($ids -join ' '))"
    P kill-session -t S5 | Out-Null
    P kill-session -t T5 | Out-Null

    # ── Case 4: kill-server reaches hosting processes ───────────────────────
    Write-Host "`n[4] kill-server leaves no hosting process behind" -ForegroundColor Cyan
    New-Pwsh "S4"; New-Pwsh "T4"
    Ready "S4:0.0"; Ready "T4:0.0"
    $srcPid = ServerPid "S4"
    $shell = [int](P display-message -p -t S4:0.0 '#{pane_pid}')
    P join-pane -s S4:0.0 -t T4:0 | Out-Null
    Start-Sleep -Milliseconds 1200
    Check (Alive $srcPid) "4.1 host $srcPid alive while it hosts the moved pane"
    P kill-server | Out-Null
    Check (Wait-Until { -not (Alive $srcPid) } 6000) "4.2 the host exits after kill-server"
    Check (Wait-Until { -not (Alive $shell) } 6000) "4.3 the moved shell is gone after kill-server"
    Check (Wait-Until { (NsServers).Count -eq 0 } 6000) "4.4 no server process of this namespace remains"
} finally {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 800
    foreach ($p in (NsServers)) { Stop-Process -Id $p.ProcessId -Force -EA SilentlyContinue }
    if ($null -ne $savedDataDir) { $env:PSMUX_DATA_DIR = $savedDataDir } else { Remove-Item Env:\PSMUX_DATA_DIR -EA SilentlyContinue }
    if ($null -ne $savedNoWarm) { $env:PSMUX_NO_WARM = $savedNoWarm } else { Remove-Item Env:\PSMUX_NO_WARM -EA SilentlyContinue }
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

Write-Host ("`nResult: {0} passed, {1} failed" -f $script:Pass, $script:Fail) -ForegroundColor $(if ($script:Fail) { 'Red' } else { 'Green' })
exit $(if ($script:Fail) { 1 } else { 0 })
