# PR #878 (datbird): move-pane inside one window, edge cases against tmux.
#
# tmux (cmd-join-pane.c, 3.2 and later) refuses only a move of a pane onto
# itself ("source and target panes must be different"). Every expected layout
# below was taken from tmux 3.8 and tmux 3.4 under WSL on the same start:
# a 120x40 window, status off, split -h then split -v, so
#   A = 0,0 60x40   B = 61,0 59x20   C = 61,21 59x19 (active)
#
# Layers: CLI (one-shot TCP), a control mode client (%layout-change), hooks.
# Safety: -L namespace, honours PSMUX_DATA_DIR, never kills by name.
# Run:  pwsh -NoProfile -File tests\test_pr878_move_pane_edges.ps1
#       PSMUX_TEST_EXE selects the binary under test.

$ErrorActionPreference = 'Continue'
$env:PSMUX_NO_WARM = "1"
$PSMUX = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }
$NS = "pr878_$PID"
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }
function Check($cond, $msg) { if ($cond) { Write-Pass $msg } else { Write-Fail $msg } }
Write-Host "binary: $PSMUX" -ForegroundColor Cyan

function P { & $PSMUX -L $NS @args 2>&1 }
$F = '#{pane_id}:#{pane_index}:#{pane_left},#{pane_top},#{pane_width}x#{pane_height}#{?pane_active,*,}'
function Ids([string]$t) { @(P list-panes -t $t -F '#{pane_id}' | ForEach-Object { "$_".Trim() } | Where-Object { $_ -match '^%' }) }
function Wait-Panes([string]$t, [int]$n) {
    $deadline = (Get-Date).AddSeconds(10)
    while ((Get-Date) -lt $deadline) { if ((Ids $t).Count -eq $n) { return $true }; Start-Sleep -Milliseconds 150 }
    return $false
}
function Base([string]$s) {
    P kill-session -t $s | Out-Null
    for ($try = 0; $try -lt 3; $try++) {
        P new-session -d -s $s -x 120 -y 40 | Out-Null
        if (Wait-Panes $s 1) { break }
    }
    P set-option -t $s status off | Out-Null
    P split-window -h -t $s | Out-Null
    [void](Wait-Panes $s 2)
    P split-window -v -t $s | Out-Null
    [void](Wait-Panes $s 3)
    Start-Sleep -Milliseconds 300
    return , (Ids $s)
}
# Layout with ids renamed to A B C, in pane order.
function Shape([string]$s, [string[]]$ids) {
    $map = @{ $ids[0] = 'A'; $ids[1] = 'B'; $ids[2] = 'C' }
    (@(P list-panes -t $s -F $F) | ForEach-Object {
        $l = "$_".Trim()
        if ($l -match '^(%\d+)(:.*)$') { $(if ($map.ContainsKey($matches[1])) { $map[$matches[1]] } else { $matches[1] }) + $matches[2] } else { $l }
    }) -join ' '
}
function Sub([string]$cmd, [string[]]$ids) { $cmd.Replace('{A}', $ids[0]).Replace('{B}', $ids[1]).Replace('{C}', $ids[2]) }

$cases = @(
    @{ N='-h';            Cmd='move-pane -s {C} -t {A} -h';        Want='A:0:0,0,30x40 C:1:31,0,29x40* B:2:61,0,59x40' },
    @{ N='-h -b';         Cmd='move-pane -s {C} -t {A} -h -b';     Want='C:0:0,0,30x40* A:1:31,0,29x40 B:2:61,0,59x40' },
    @{ N='-v';            Cmd='move-pane -s {C} -t {A} -v';        Want='A:0:0,0,60x20 C:1:0,21,60x19* B:2:61,0,59x40' },
    @{ N='-b (vertical)'; Cmd='move-pane -s {C} -t {A} -b';        Want='C:0:0,0,60x20* A:1:0,21,60x19 B:2:61,0,59x40' },
    @{ N='-h -l 10';      Cmd='move-pane -s {C} -t {A} -h -l 10';  Want='A:0:0,0,49x40 C:1:50,0,10x40* B:2:61,0,59x40' },
    @{ N='-h -l 25%';     Cmd='move-pane -s {C} -t {A} -h -l 25%'; Want='A:0:0,0,44x40 C:1:45,0,15x40* B:2:61,0,59x40' },
    @{ N='sibling -h';    Cmd='move-pane -s {B} -t {C} -h';        Want='A:0:0,0,60x40 C:1:61,0,29x40 B:2:91,0,29x40*' },
    @{ N='movep alias';   Cmd='movep -s {C} -t {A} -h';            Want='A:0:0,0,30x40 C:1:31,0,29x40* B:2:61,0,59x40' }
)

try {
    Write-Host "`n[1] CLI move-pane inside the window, layout against tmux" -ForegroundColor Yellow
    foreach ($c in $cases) {
        $ids = Base 'm'
        $cmd = Sub $c.Cmd $ids
        $out = P @($cmd -split ' ')
        $rc = $LASTEXITCODE
        $got = Shape 'm' $ids
        Check ($rc -eq 0 -and $got -eq $c.Want) "$($c.N): rc=$rc out=<$($out -join ' ')>`n         got  $got`n         tmux $($c.Want)"
        Check (@(P list-windows -t m).Count -eq 1) "$($c.N): still one window"
    }

    Write-Host "`n[2] -d keeps the active pane" -ForegroundColor Yellow
    $ids = Base 'm'
    P select-pane -t $ids[1] | Out-Null
    $out = P move-pane -d -s $ids[2] -t $ids[0] -h
    $rc = $LASTEXITCODE
    $act = "$(P display -p -t m '#{pane_id}')".Trim()
    Check ($rc -eq 0 -and $act -eq $ids[1]) "-d: active pane stays $($ids[1]) (now $act), layout $(Shape 'm' $ids)"
    $ids = Base 'm'
    P select-pane -t $ids[1] | Out-Null
    P move-pane -s $ids[2] -t $ids[0] -h | Out-Null
    $act = "$(P display -p -t m '#{pane_id}')".Trim()
    Check ($act -eq $ids[2]) "without -d the moved pane becomes active ($act)"

    Write-Host "`n[3] zoomed window" -ForegroundColor Yellow
    $ids = Base 'm'
    P resize-pane -Z -t $ids[2] | Out-Null
    $z = "$(P display -p -t m '#{window_zoomed_flag}')".Trim()
    $out = P move-pane -s $ids[2] -t $ids[0] -h
    $rc = $LASTEXITCODE
    $z2 = "$(P display -p -t m '#{window_zoomed_flag}')".Trim()
    $got = Shape 'm' $ids
    Check ($z -eq '1' -and $rc -eq 0 -and $z2 -eq '0' -and $got -eq 'A:0:0,0,30x40 C:1:31,0,29x40* B:2:61,0,59x40') "zoomed: unzooms and moves (zoom $z -> $z2, rc=$rc): $got"

    Write-Host "`n[4] refusals" -ForegroundColor Yellow
    $ids = Base 'm'
    $out = P move-pane -s $ids[1] -t $ids[1]
    Check ($LASTEXITCODE -ne 0 -and ($out -join ' ') -match 'source and target panes must be different') "onto itself: $($out -join ' ')"
    Check ((Shape 'm' $ids) -eq 'A:0:0,0,60x40 B:1:61,0,59x20 C:2:61,21,59x19*') "onto itself leaves the layout alone"
    P kill-session -t one | Out-Null
    P new-session -d -s one -x 120 -y 40 | Out-Null
    [void](Wait-Panes 'one' 1)
    $out = P move-pane -s one:0.0 -t one:0.0
    Check ($LASTEXITCODE -ne 0 -and ($out -join ' ') -match 'source and target panes must be different') "only pane, -s and -t: $($out -join ' ')"
    $out = P move-pane -t one:0.0
    Check ($LASTEXITCODE -ne 0) "only pane, no -s and nothing marked: rc=$LASTEXITCODE <$($out -join ' ')>"
    P has-session -t one | Out-Null
    $hs = $LASTEXITCODE
    Check ((Ids 'one').Count -eq 1 -and $hs -eq 0) "only pane survives"
    $ids = Base 'm'
    $out = P join-pane -s $ids[2] -t $ids[0] -h
    Write-Info "join-pane in one window: rc=$LASTEXITCODE <$($out -join ' ')> (tmux 3.2+ allows it, psmux refuses)"

    Write-Host "`n[5] marked pane is the default source" -ForegroundColor Yellow
    $ids = Base 'm'
    P select-pane -m -t $ids[2] | Out-Null
    P select-pane -t $ids[0] | Out-Null
    $out = P move-pane -t $ids[0] -h
    $rc = $LASTEXITCODE
    $got = Shape 'm' $ids
    Check ($rc -eq 0 -and $got -eq 'A:0:0,0,30x40 C:1:31,0,29x40* B:2:61,0,59x40') "marked source: rc=$rc <$($out -join ' ')> $got"

    Write-Host "`n[6] pane-base-index 1, index targets" -ForegroundColor Yellow
    $ids = Base 'm'
    P set-option -g pane-base-index 1 | Out-Null
    $out = P move-pane -s m:0.3 -t m:0.1 -h
    $rc = $LASTEXITCODE
    $got = Shape 'm' $ids
    Check ($rc -eq 0 -and $got -eq 'A:1:0,0,30x40 C:2:31,0,29x40* B:3:61,0,59x40') "m:0.3 -> m:0.1: rc=$rc <$($out -join ' ')> $got"
    P set-option -g pane-base-index 0 | Out-Null

    Write-Host "`n[7] window-layout-changed hook" -ForegroundColor Yellow
    $ids = Base 'm'
    P set-option -g '@wlc' no | Out-Null
    P set-hook -g window-layout-changed "set-option -g @wlc yes" | Out-Null
    P move-pane -s $ids[2] -t $ids[0] -h | Out-Null
    Start-Sleep -Milliseconds 600
    $w = "$(P show-options -gv '@wlc')".Trim()
    # psmux fires window-layout-changed for no command yet (split-window
    # neither), so this is reported, not asserted.
    Write-Info "window-layout-changed hook after move-pane: @wlc=$w (tmux: yes)"
    P set-hook -gu window-layout-changed | Out-Null

    Write-Host "`n[8] control client: no %error, %layout-change, layout" -ForegroundColor Yellow
    $ids = Base 'cc'
    $psi = [Diagnostics.ProcessStartInfo]::new($PSMUX, "-L $NS -C attach-session -t cc")
    $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.UseShellExecute = $false
    $cc = [Diagnostics.Process]::Start($psi)
    $read = $cc.StandardOutput.ReadToEndAsync()
    Start-Sleep -Milliseconds 1500
    $cc.StandardInput.WriteLine("move-pane -s `"$($ids[2])`" -t `"$($ids[0])`" -h"); $cc.StandardInput.Flush()
    Start-Sleep -Milliseconds 1500
    $cc.StandardInput.Close()
    if (-not $cc.WaitForExit(5000)) { $cc.Kill() }
    $lines = $read.Result -split "`r?`n"
    Check (@($lines | Where-Object { $_ -match '^%error ' }).Count -eq 0) "no %error ($(($lines | Where-Object { $_ -notmatch '^%(output|begin|end)' }) -join ' / '))"
    Check (@($lines | Where-Object { $_ -match '^%layout-change ' }).Count -ge 1) "%layout-change follows the move"
    $got = Shape 'cc' $ids
    Check ($got -eq 'A:0:0,0,30x40 C:1:31,0,29x40* B:2:61,0,59x40') "control move layout: $got"
} finally {
    & $PSMUX -L $NS kill-server 2>$null | Out-Null
}

Write-Host "`n=== PR #878 move-pane edges: $script:Pass passed, $script:Fail failed ===" -ForegroundColor Cyan
if ($script:Fail -gt 0) { exit 1 } else { exit 0 }
