# pane-base-index is a WINDOW option (tmux options-table.c:1552-1554), read
# from each window's own table (window.c:1124 window_pane_at_index, :1162
# window_pane_index). `set -w -t :1 pane-base-index 1` renumbers window 1 only.
#
# psmux stored the value but read one global number everywhere, so window 0
# was numbered 1,2 as well and `select-pane -t :0.0` failed with "can't find
# pane: 0". The global `set -g pane-base-index` is unchanged (see
# tests/test_pane_base_index_target.ps1).
#
# Isolation: PSMUX_DATA_DIR is a fresh temp dir, every command uses -L, and
# cleanup is `-L <ns> kill-server`. PSMUX_TEST_EXE picks the binary.
# Exit code is the fail count.

$ErrorActionPreference = "Continue"
$Exe = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }
$env:PSMUX_DATA_DIR = Join-Path $env:TEMP ("psmux_pbiw_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$ns = "pbiw_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$script:pass = 0; $script:fail = 0
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { $script:pass++; Write-Host "  [PASS] $name = '$got'" -ForegroundColor Green }
    else { $script:fail++; Write-Host "  [FAIL] $name expected '$want', got '$got'" -ForegroundColor Red }
}
function P { & $Exe -L $ns @args 2>&1 }
function Panes($t) { (P list-panes -t $t -F '#{pane_index}') -join ',' }
$tui = $null

try {
    P new-session -d -s a -x 120 -y 30 | Out-Null
    Start-Sleep -Seconds 2
    P split-window -t a | Out-Null; P new-window -t a | Out-Null; P split-window -t a | Out-Null
    Start-Sleep -Milliseconds 500

    Write-Host "`n[1] set -w on one window" -ForegroundColor Yellow
    P set -w -t a:1 pane-base-index 1 | Out-Null
    Check "window 0 panes" (Panes 'a:0') "0,1"
    Check "window 1 panes" (Panes 'a:1') "1,2"
    Check "global value" ((P show -gv pane-base-index) -join '') "0"
    Check "show -wv -t a:1" ((P show -wv -t a:1 pane-base-index) -join '') "1"
    Check "display -t a:1.1 pane_index" ((P display -t a:1.1 -p '#{pane_index}') -join '') "1"
    Check "display -t a:0.1 pane_index" ((P display -t a:0.1 -p '#{pane_index}') -join '') "1"

    Write-Host "`n[2] targets follow the window's own base" -ForegroundColor Yellow
    $r = P select-pane -t a:0.0; Check "select-pane -t a:0.0 exit" $LASTEXITCODE 0
    $r = P select-pane -t a:1.2; Check "select-pane -t a:1.2 exit" $LASTEXITCODE 0
    Check "active pane of window 1" ((P display -t a:1 -p '#{pane_index}') -join '') "2"
    $r = P select-pane -t a:1.0; Check "select-pane -t a:1.0 is can't find pane" ("$LASTEXITCODE " + ($r -join '')) "1 psmux: can't find pane: 0"

    Write-Host "`n[3] -u gives the window the global value back" -ForegroundColor Yellow
    P set -w -t a:1 -u pane-base-index | Out-Null
    Check "window 1 panes after -u" (Panes 'a:1') "0,1"

    Write-Host "`n[4] set -g still renumbers every window" -ForegroundColor Yellow
    P set -g pane-base-index 1 | Out-Null
    Check "window 0 panes" (Panes 'a:0') "1,2"
    Check "window 1 panes" (Panes 'a:1') "1,2"
    P set -w -t a:0 pane-base-index 5 | Out-Null
    Check "window 0 own value over the global" (Panes 'a:0') "5,6"
    Check "window 1 keeps the global" (Panes 'a:1') "1,2"
    P set -g pane-base-index 0 | Out-Null
    P set -w -t a:0 -u pane-base-index | Out-Null

    Write-Host "`n[5] TUI: attached client, the window's own base" -ForegroundColor Yellow
    $tui = Start-Process -FilePath $Exe -ArgumentList "-L", $ns, "new-session", "-s", "tui" -PassThru
    for ($i = 0; $i -lt 50; $i++) { P has-session -t tui | Out-Null; if ($LASTEXITCODE -eq 0) { break }; Start-Sleep -Milliseconds 200 }
    Start-Sleep -Seconds 2
    P split-window -t tui | Out-Null
    P set -w -t tui:0 pane-base-index 3 | Out-Null
    Check "attached window panes" (Panes 'tui:0') "3,4"
    $r = P select-pane -t tui:0.3; Check "select-pane -t tui:0.3 exit" $LASTEXITCODE 0
    Check "active pane_index" ((P display -t tui -p '#{pane_index}') -join '') "3"
}
finally {
    P kill-server | Out-Null
    if ($tui -and -not $tui.WaitForExit(5000)) { Stop-Process -Id $tui.Id -Force -EA SilentlyContinue }
    Remove-Item -Recurse -Force $env:PSMUX_DATA_DIR -EA SilentlyContinue
}
Write-Host "`n  Passed: $script:pass  Failed: $script:fail"
exit $script:fail
