# Issue #964: kill-pane -a killed the target pane instead of every other pane
# of its window. tmux 3.8 cmd_kill_pane_all (cmd-kill-pane.c:72-93) unzooms
# the window, keeps the target, removes the others and redraws; a one-pane
# window is left alone.
#
# Fully isolated: a private PSMUX_DATA_DIR and config under %TEMP% and a
# namespace of its own. Only one session lives at a time, so a bare
# `kill-pane -a` (no -t) reaches it. Cleans up with `-L <ns> kill-server`.
#
# Usage: pwsh tests\test_issue964_kill_pane_all.ps1 [-Exe path\to\psmux.exe]
param([string]$Exe = '')
$ErrorActionPreference = 'Continue'
if (-not $Exe) { $Exe = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { Join-Path $PSScriptRoot '..\target\release\psmux.exe' } }
if (-not (Test-Path -LiteralPath $Exe)) { Write-Output "psmux binary not found: $Exe"; exit 2 }
$Exe = (Resolve-Path -LiteralPath $Exe).Path

$saved = @{}
foreach ($n in 'PSMUX_DATA_DIR', 'PSMUX_CONFIG_FILE', 'PSMUX_SERVER_MODE', 'TMUX', 'TMUX_PANE', 'PSMUX_TARGET_SESSION') {
    $saved[$n] = [Environment]::GetEnvironmentVariable($n, 'Process')
}
$Root = Join-Path ([IO.Path]::GetTempPath()) "psmux-964-$PID"
New-Item -ItemType Directory -Force (Join-Path $Root 'data') | Out-Null
$cfg = Join-Path $Root 'empty.conf'; [IO.File]::WriteAllText($cfg, '')
$env:PSMUX_DATA_DIR = Join-Path $Root 'data'
$env:PSMUX_CONFIG_FILE = $cfg
foreach ($n in 'PSMUX_SERVER_MODE', 'TMUX', 'TMUX_PANE', 'PSMUX_TARGET_SESSION') { Remove-Item "Env:$n" -ErrorAction SilentlyContinue }

$NS = "t964$PID"
$script:fail = 0; $script:pass = 0
function Check([bool]$ok, [string]$name, [string]$detail = '') {
    if ($ok) { $script:pass++; Write-Output "PASS: $name" } else { $script:fail++; Write-Output "FAIL: $name $detail" }
}
function P { & $Exe -L $NS @args 2>&1 | ForEach-Object { "$_" } }
function Alive([string]$s) { P has-session -t $s | Out-Null; $LASTEXITCODE -eq 0 }
function Wait-For([scriptblock]$cond, [int]$ms = 8000) {
    $d = (Get-Date).AddMilliseconds($ms)
    while ((Get-Date) -lt $d) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 200 }
    [bool](& $cond)
}
function Fresh([string]$s) {
    P new-session -d -s $s -x 120 -y 30 | Out-Null
    Wait-For { Alive $s } | Out-Null
}
function Done([string]$s) {
    P kill-session -t $s | Out-Null
    Wait-For { -not (Alive $s) } 4000 | Out-Null
}
function Panes([string]$t) { (P list-panes -t $t -F '#{pane_id}:#{pane_active}') -join ' ' }
function Ids([string]$t) { @(P list-panes -t $t -F '#{pane_id}') }
# A session whose window 0 has three panes, the first one active.
function ThreePanes([string]$s) {
    Fresh $s
    P split-window -d -t $s | Out-Null
    P split-window -d -t $s | Out-Null
    Wait-For { (Ids "${s}:0").Count -eq 3 } | Out-Null
    Start-Sleep -Milliseconds 300
}
function Kept([string]$t, [string]$id) { (Panes $t) -eq "${id}:1" }

try {
    Write-Output "== CLI: the issue's repros"
    ThreePanes p1
    $ids = @(Ids 'p1:0')
    P select-pane -t $ids[2] | Out-Null
    Start-Sleep -Milliseconds 300
    $before = Panes 'p1:0'
    P kill-pane -a | Out-Null
    $rc = $LASTEXITCODE
    Wait-For { (Ids 'p1:0').Count -lt 3 } 3000 | Out-Null
    Start-Sleep -Milliseconds 300
    Check ((Kept 'p1:0' $ids[2]) -and $rc -eq 0) "kill-pane -a (no -t) keeps the active pane $($ids[2])" "(before $before, after $(Panes 'p1:0'), rc $rc)"
    Done p1

    ThreePanes p2
    $ids = @(Ids 'p2:0')
    P kill-pane -a -t $ids[1] | Out-Null
    $rc = $LASTEXITCODE
    Wait-For { (Ids 'p2:0').Count -lt 3 } 3000 | Out-Null
    Start-Sleep -Milliseconds 300
    Check ((Kept 'p2:0' $ids[1]) -and $rc -eq 0) "kill-pane -a -t $($ids[1]) keeps that pane (not the active one)" "($(Panes 'p2:0'), rc $rc)"
    Done p2

    Write-Output "== CLI: other spellings"
    ThreePanes p3
    $ids = @(Ids 'p3:0')
    P kill-pane -a -t p3:0.2 | Out-Null
    Wait-For { (Ids 'p3:0').Count -lt 3 } 3000 | Out-Null
    Start-Sleep -Milliseconds 300
    Check (Kept 'p3:0' $ids[2]) 'kill-pane -a -t sess:0.2 keeps pane index 2' "($(Panes 'p3:0'))"
    Done p3

    ThreePanes p4
    $ids = @(Ids 'p4:0')
    P select-pane -t $ids[1] | Out-Null
    Start-Sleep -Milliseconds 300
    P killp -a -t p4 | Out-Null
    Wait-For { (Ids 'p4:0').Count -lt 3 } 3000 | Out-Null
    Start-Sleep -Milliseconds 300
    Check (Kept 'p4:0' $ids[1]) 'killp -a -t sess keeps the active pane' "($(Panes 'p4:0'))"
    Done p4

    ThreePanes p5
    $ids = @(Ids 'p5:0')
    P kill-pane -at $ids[2] | Out-Null
    Wait-For { (Ids 'p5:0').Count -lt 3 } 3000 | Out-Null
    Start-Sleep -Milliseconds 300
    Check (Kept 'p5:0' $ids[2]) "kill-pane -at $($ids[2]) is -a -t $($ids[2])" "($(Panes 'p5:0'))"
    Done p5

    ThreePanes p6
    $ids = @(Ids 'p6:0')
    P kill-pane -t $ids[1] | Out-Null
    Wait-For { (Ids 'p6:0').Count -lt 3 } 3000 | Out-Null
    Start-Sleep -Milliseconds 300
    $left = (Ids 'p6:0') -join ' '
    Check ($left -eq "$($ids[0]) $($ids[2])") "kill-pane -t $($ids[1]) without -a still kills only that pane" "($left)"
    Done p6

    Write-Output "== a window that is not the current one"
    Fresh w1
    P split-window -d -t w1:0 | Out-Null
    P new-window -t w1 | Out-Null
    P split-window -d -t w1:1 | Out-Null
    P split-window -d -t w1:1 | Out-Null
    Wait-For { (Ids 'w1:0').Count -eq 2 -and (Ids 'w1:1').Count -eq 3 } | Out-Null
    Start-Sleep -Milliseconds 300
    $ids0 = @(Ids 'w1:0')
    P kill-pane -a -t w1:0.1 | Out-Null
    Wait-For { (Ids 'w1:0').Count -lt 2 } 3000 | Out-Null
    Start-Sleep -Milliseconds 300
    $wins = (P list-windows -t w1 -F '#{window_index}:#{window_active}') -join ' '
    Check (((Ids 'w1:0') -join ' ') -eq $ids0[1]) 'kill-pane -a -t :0.1 keeps pane 0.1' "($(Panes 'w1:0'))"
    Check ((Ids 'w1:1').Count -eq 3) 'the current window keeps its three panes' "($(Panes 'w1:1'))"
    Check ($wins -eq '0:0 1:1') 'window 1 is still the current window' "($wins)"
    Done w1

    # -t %N in a window that is not current: the kill-pane hooks run in the
    # target's window, as for any other target. A before-kill-pane hook
    # running kill-pane -a in the current window 1 would close one of its
    # two panes.
    Fresh w2
    P split-window -d -t w2:0 | Out-Null
    P new-window -t w2 | Out-Null
    P split-window -d -t w2:1 | Out-Null
    Wait-For { (Ids 'w2:0').Count -eq 2 -and (Ids 'w2:1').Count -eq 2 } | Out-Null
    Start-Sleep -Milliseconds 300
    $ids0 = @(Ids 'w2:0')
    P set-hook -t w2 before-kill-pane 'kill-pane -a' | Out-Null
    P kill-pane -a -t $ids0[1] | Out-Null
    Wait-For { (Ids 'w2:0').Count -lt 2 } 3000 | Out-Null
    Start-Sleep -Milliseconds 800
    Check (((Ids 'w2:0') -join ' ') -eq $ids0[1]) "kill-pane -a -t $($ids0[1]) in window 0 keeps it" "($(Panes 'w2:0'))"
    Check ((Ids 'w2:1').Count -eq 2) "before-kill-pane runs in the target's window, not the current one" "($(Panes 'w2:1'))"
    P set-hook -u -t w2 before-kill-pane | Out-Null
    Done w2

    Write-Output "== zoom and one-pane windows"
    ThreePanes z1
    $ids = @(Ids 'z1:0')
    P resize-pane -Z -t $ids[0] | Out-Null
    Start-Sleep -Milliseconds 300
    $zoomed = (P display-message -p -t z1:0 '#{window_zoomed_flag}') -join ''
    P kill-pane -a -t $ids[0] | Out-Null
    Wait-For { (Ids 'z1:0').Count -lt 3 } 3000 | Out-Null
    Start-Sleep -Milliseconds 300
    $after = (P display-message -p -t z1:0 '#{window_zoomed_flag}') -join ''
    Check ((Kept 'z1:0' $ids[0]) -and $zoomed -eq '1' -and $after -eq '0') 'kill-pane -a in a zoomed window keeps the target and unzooms' "($(Panes 'z1:0'), zoomed $zoomed then $after)"
    Done z1

    Fresh o1
    $ids = @(Ids 'o1:0')
    P kill-pane -a -t o1 | Out-Null
    $rc = $LASTEXITCODE
    Start-Sleep -Milliseconds 1000
    Check ((Alive o1) -and (Kept 'o1:0' $ids[0]) -and $rc -eq 0) 'kill-pane -a in a one-pane window leaves it alone' "(alive $(Alive o1), $(Panes 'o1:0'), rc $rc)"
    Done o1

    Write-Output "== hook: the in server route"
    ThreePanes h1
    $ids = @(Ids 'h1:0')
    P set-hook -t h1 after-split-window 'kill-pane -a' | Out-Null
    P split-window -t h1 | Out-Null
    Wait-For { (Ids 'h1:0').Count -eq 1 } 5000 | Out-Null
    Start-Sleep -Milliseconds 500
    $p = @(P list-panes -t 'h1:0' -F '#{pane_id}:#{pane_active}')
    $kept = if ($p.Count -eq 1) { ($p[0] -split ':')[0] } else { '' }
    Check ($p.Count -eq 1 -and $p[0] -like '*:1' -and $kept -notin $ids) 'hook kill-pane -a keeps only the new active pane' "($($p -join ' '))"
    Done h1

    ThreePanes h2
    $ids = @(Ids 'h2:0')
    P set-hook -t h2 after-split-window "kill-pane -a -t $($ids[1])" | Out-Null
    P split-window -d -t h2 | Out-Null
    Wait-For { (Ids 'h2:0').Count -eq 1 } 5000 | Out-Null
    Start-Sleep -Milliseconds 500
    Check (Kept 'h2:0' $ids[1]) "hook kill-pane -a -t $($ids[1]) keeps that pane" "($(Panes 'h2:0'))"
    Done h2

    # The hook's target is window 1 while window 0 stays current: the hook's
    # line runs against window 1, not against whatever is current later.
    Fresh h3
    P split-window -d -t h3:0 | Out-Null
    P new-window -d -t h3 | Out-Null
    Wait-For { (Ids 'h3:0').Count -eq 2 -and (Ids 'h3:1').Count -eq 1 } | Out-Null
    Start-Sleep -Milliseconds 300
    $ids0 = @(Ids 'h3:0'); $ids1 = @(Ids 'h3:1')
    P set-hook -t h3 after-split-window 'kill-pane -a' | Out-Null
    P split-window -d -t h3:1.0 | Out-Null
    Wait-For { (Ids 'h3:1').Count -eq 1 } 5000 | Out-Null
    Start-Sleep -Milliseconds 800
    Check (((Ids 'h3:0') -join ' ') -eq ($ids0 -join ' ')) 'hook kill-pane -a leaves the current window alone' "($(Panes 'h3:0'))"
    Check (((Ids 'h3:1') -join ' ') -eq $ids1[0]) "hook kill-pane -a keeps $($ids1[0]) in the hook's window" "($(Panes 'h3:1'))"
    Done h3

    # An after-kill-pane hook running kill-pane -a fires once, not forever.
    ThreePanes h4
    $ids = @(Ids 'h4:0')
    $count = Join-Path $Root 'h4.count'
    P set-hook -t h4 after-kill-pane 'kill-pane -a' | Out-Null
    P set-hook -a -t h4 after-kill-pane "run-shell 'cmd /c echo x>>$count'" | Out-Null
    P kill-pane -t $ids[2] | Out-Null
    Wait-For { (Ids 'h4:0').Count -eq 1 } 5000 | Out-Null
    Start-Sleep -Milliseconds 3000
    $fired = if (Test-Path $count) { @(Get-Content $count).Count } else { 0 }
    Check ((Alive h4) -and (Ids 'h4:0').Count -eq 1 -and $fired -eq 1) 'after-kill-pane kill-pane -a runs once' "(fired $fired, $(Panes 'h4:0'))"
    P set-hook -u -t h4 after-kill-pane | Out-Null
    Done h4

    Write-Output "== floats"
    # A float's shell goes with its whole process tree, as a tiled pane's does.
    Fresh f1
    P split-window -d -t f1 | Out-Null
    Wait-For { (Ids 'f1:0').Count -eq 2 } | Out-Null
    $marker = "127.0.0.$((Get-Random -Minimum 2 -Maximum 250))"
    P new-pane -d -t f1 -x 40 -y 10 "Start-Process -WindowStyle Hidden ping.exe -ArgumentList '-t','-w','3000','$marker'; Start-Sleep 600" | Out-Null
    $pings = { @(Get-CimInstance Win32_Process -Filter "Name='PING.EXE'" | Where-Object { $_.CommandLine -like "*$marker*" }) }
    Wait-For { (& $pings).Count -ge 1 } 8000 | Out-Null
    $before = (& $pings).Count
    $ids = @(Ids 'f1:0')
    P kill-pane -a -t $ids[0] | Out-Null
    Wait-For { (& $pings).Count -eq 0 } 5000 | Out-Null
    $left = (& $pings).Count
    Check ($before -eq 1 -and $left -eq 0 -and ((Ids 'f1:0') -join ' ') -eq $ids[0]) "kill-pane -a ends a float's descendants" "(ping before $before, after $left, $(Panes 'f1:0'))"
    & $pings | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    Done f1

    Write-Output "== control mode"
    ThreePanes c1
    $ids = @(Ids 'c1:0')
    $psi = [Diagnostics.ProcessStartInfo]::new($Exe)
    foreach ($a in @('-L', $NS, '-C', 'attach', '-t', 'c1')) { $psi.ArgumentList.Add($a) }
    $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $cp = [Diagnostics.Process]::Start($psi)
    $outTask = $cp.StandardOutput.ReadToEndAsync()
    Start-Sleep -Milliseconds 1500
    $cp.StandardInput.WriteLine("kill-pane -a -t $($ids[2])")
    $cp.StandardInput.Flush()
    Wait-For { (Ids 'c1:0').Count -lt 3 } 4000 | Out-Null
    Start-Sleep -Milliseconds 800
    $cp.StandardInput.Close()
    if (-not $cp.WaitForExit(4000)) { $cp.Kill() }
    $ctl = if ($outTask.Wait(2000)) { $outTask.Result } else { '' }
    Check (Kept 'c1:0' $ids[2]) "control mode kill-pane -a -t $($ids[2]) keeps that pane" "($(Panes 'c1:0'))"
    Check ($ctl -match '%layout-change') 'control mode client gets %layout-change' "(output: $(($ctl -split "`n" | Select-Object -Last 4) -join ' | '))"
    # tmux notifies a layout change from each layout_close_pane: two panes
    # went, so the final layout comes at least twice.
    $lays = @($ctl -split "`r?`n" | Where-Object { $_ -like '%layout-change *' })
    $last = if ($lays.Count) { $lays[-1] } else { '' }
    $same = @($lays | Where-Object { $_ -eq $last }).Count
    Check ($same -ge 2) 'control mode gets a %layout-change for each removed pane' "($same of $($lays.Count): $last)"
    Done c1

    # `--` ends the flags: the -a after it is an operand, not the flag.
    ThreePanes c2
    $psi2 = [Diagnostics.ProcessStartInfo]::new($Exe)
    foreach ($a in @('-L', $NS, '-C', 'attach', '-t', 'c2')) { $psi2.ArgumentList.Add($a) }
    $psi2.RedirectStandardInput = $true; $psi2.RedirectStandardOutput = $true; $psi2.RedirectStandardError = $true
    $psi2.UseShellExecute = $false
    $cp = [Diagnostics.Process]::Start($psi2)
    $null = $cp.StandardOutput.ReadToEndAsync()
    Start-Sleep -Milliseconds 1500
    $cp.StandardInput.WriteLine('kill-pane -- -a')
    $cp.StandardInput.Flush()
    Start-Sleep -Milliseconds 1500
    $cp.StandardInput.Close()
    if (-not $cp.WaitForExit(4000)) { $cp.Kill() }
    Check ((Ids 'c2:0').Count -ge 2) 'control mode kill-pane -- -a does not kill the other panes' "($(Panes 'c2:0'))"
    Done c2
} finally {
    P kill-server | Out-Null
    foreach ($n in $saved.Keys) {
        if ($null -eq $saved[$n]) { Remove-Item "Env:$n" -ErrorAction SilentlyContinue }
        else { [Environment]::SetEnvironmentVariable($n, $saved[$n], 'Process') }
    }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $Root -ErrorAction SilentlyContinue
}
Write-Output "passed $script:pass, failed $script:fail"
exit $script:fail
