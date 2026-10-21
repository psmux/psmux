# set-option run from a hook (the server's own command path) ran twice:
# `set -ga @x a` from after-new-window left "aa", `set -ga status-right Z`
# from after-split-window left "ZZ", and a bare `set -g mouse` toggled and
# toggled back. tmux 3.8 runs it once (cmd-set-option.c:347, :351-353).
#
# Isolated: PSMUX_DATA_DIR is a fresh temp dir and every command uses -L.
# PSMUX_EXE picks the binary (defaults to psmux on PATH).

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$env:PSMUX_DATA_DIR = Join-Path $env:TEMP ("psmux_setonce_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$NS = "setonce_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function P { & $PSMUX -L $NS @args 2>&1 }
function Check($name, $got, $want) {
    if ("$got" -eq "$want") { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}
$tui = $null

try {
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Seconds 2
    P has-session -t s | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Fail "session did not start"; exit 1 }

    Write-Host "`n[1] set -ga on a user option from after-new-window" -ForegroundColor Yellow
    P set-hook -g after-new-window 'set -ga @x a' | Out-Null
    P new-window -t s | Out-Null
    Start-Sleep -Milliseconds 800
    Check "@x after one new-window" ((P show -gv '@x') -join '') "a"
    P new-window -t s | Out-Null
    Start-Sleep -Milliseconds 800
    Check "@x after two new-windows" ((P show -gv '@x') -join '') "aa"
    P set-hook -gu after-new-window | Out-Null

    Write-Host "`n[2] set -ga status-right from after-split-window" -ForegroundColor Yellow
    P set -g status-right X | Out-Null
    P set-hook -g after-split-window 'set -ga status-right Z' | Out-Null
    P split-window -t s | Out-Null
    Start-Sleep -Milliseconds 800
    Check "status-right after one split" ((P show -gv status-right) -join '') "XZ"
    P set-hook -gu after-split-window | Out-Null

    Write-Host "`n[3] a bare boolean toggles once" -ForegroundColor Yellow
    P set -g mouse on | Out-Null
    P set-hook -g after-new-window 'set -g mouse' | Out-Null
    P new-window -t s | Out-Null
    Start-Sleep -Milliseconds 800
    Check "mouse after one toggle" ((P show -gv mouse) -join '') "off"
    P set-hook -gu after-new-window | Out-Null

    Write-Host "`n[4] a chained set is seen by the next command, each runs once" -ForegroundColor Yellow
    $conf = Join-Path $env:PSMUX_DATA_DIR "hook.conf"
    Set-Content -Path $conf -Value "set-hook -g after-new-window 'set -g @k v \; set -gF @k2 `"#{@k}`" \; set -ga @n z'"
    P source-file $conf | Out-Null
    P new-window -t s | Out-Null
    Start-Sleep -Milliseconds 800
    Check "@k2 (set -F of the value set just before)" ((P show -gv '@k2') -join '') "v"
    Check "@n" ((P show -gv '@n') -join '') "z"
    P set-hook -gu after-new-window | Out-Null

    Write-Host "`n[5] after-write work still runs: pane-border-status from a hook resizes the pane" -ForegroundColor Yellow
    $before = [int]((P display -p '#{pane_height}') -join '')
    P set-hook -g after-new-window 'set -g pane-border-status top' | Out-Null
    P new-window -t s | Out-Null
    Start-Sleep -Milliseconds 800
    $wh = [int]((P display -p '#{window_height}') -join '')
    $ph = [int]((P display -p '#{pane_height}') -join '')
    if ($ph -eq $wh - 1) { Write-Pass "pane_height $ph = window_height $wh - 1" } else { Write-Fail "pane_height $ph, window_height $wh (before $before)" }
    P set-hook -gu after-new-window | Out-Null
    P set -g pane-border-status off | Out-Null

    Write-Host "`n[6] TUI: attached client, hook appends once" -ForegroundColor Yellow
    $tui = Start-Process -FilePath $PSMUX -ArgumentList "-L", $NS, "new-session", "-s", "tui" -PassThru
    Start-Sleep -Seconds 4
    P has-session -t tui | Out-Null
    if ($LASTEXITCODE -eq 0) {
        P set-hook -t tui -g after-new-window 'set -ga @t q' | Out-Null
        P new-window -t tui | Out-Null
        Start-Sleep -Milliseconds 800
        Check "attached session @t" ((P show -t tui -gv '@t') -join '') "q"
        Check "attached session window count" ((P display -t tui -p '#{session_windows}') -join '') "2"
    } else {
        Write-Fail "attached session did not start"
    }
}
finally {
    P kill-server | Out-Null
    if ($tui -and -not $tui.HasExited) { Stop-Process -Id $tui.Id -Force -EA SilentlyContinue }
    Remove-Item -Recurse -Force $env:PSMUX_DATA_DIR -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
