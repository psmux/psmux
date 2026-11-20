# Issue #1063: a closed window or an exited pane hands the wrong mode to the
# pane that gets the focus, and link-window -k links the wrong window.
#
# A window or a pane that goes away takes its own clock or copy mode with it,
# and the pane that gets the focus keeps its own, as in tmux, where a mode
# belongs to its window_pane. psmux parks the modes of the panes without the
# focus, and the paths that remove a window (kill-window, kill-window -a,
# unlink-window, link-window -k, break-pane, join-pane) or reap an exited
# pane moved the focus without handing them over: the next pane's parked
# clock was drawn over a pane in no mode, so keys went to its shell unseen,
# and the closed pane's mode moved onto the next one. Every case below was
# measured on tmux 3.8 with the same commands.
#
# link-window -k also read the current and the source window one place off
# after removing an occupant before them: -d moved the focus, and the
# window after the source was linked.
#
# Commands only, no client. Private PSMUX_DATA_DIR and config, a -L
# namespace per run; cleanup is `-L <ns> kill-server`.
# PSMUX_EXE (or PSMUX_TEST_BIN) picks the binary.

$ErrorActionPreference = "Continue"
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -ne 'PSMUX_EXE' -and $_.Name -ne 'PSMUX_TEST_BIN' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
Remove-Item Env:SHELL -EA SilentlyContinue
$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = $env:PSMUX_EXE }
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_wcm_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
$env:PSMUX_DATA_DIR = $TMP
$env:PSMUX_NO_WARM = "1"
$SHELLNAME = if (Get-Command pwsh -EA SilentlyContinue) { 'pwsh' } else { 'powershell' }
$conf = Join-Path $TMP "wcm.conf"
"set -g default-shell '$SHELLNAME -NoLogo -NoProfile'" | Set-Content $conf -Encoding ASCII
$env:PSMUX_CONFIG_FILE = $conf

$script:ns = ""
function P { & $PSMUX -L $script:ns @args 2>&1 }
function Fresh {
    if ($script:ns) { P kill-server | Out-Null; Start-Sleep -Milliseconds 400 }
    $script:ns = "wcm_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s R -x 80 -y 20 | Out-Null
    for ($i = 0; $i -lt 40; $i++) { & $PSMUX -L $script:ns has-session -t R 2>$null; if ($LASTEXITCODE -eq 0) { break }; Start-Sleep -Milliseconds 250 }
    Start-Sleep 1
}
function NewWindow { P new-window @args | Out-Null; Start-Sleep -Milliseconds 800 }
# The focused pane and its mode, as `<window>.<pane> <mode or none>`.
function Focus { (P display -p '#{window_index}.#{pane_index} #{?pane_in_mode,#{pane_mode},none}' | Out-String).Trim() }
function Check($name, $want, [scriptblock]$body) {
    Fresh
    & $body
    Start-Sleep 2
    $got = Focus
    $mode = ($got -split ' ')[1]
    if ($mode -eq $want) { Write-Pass "$name : the focused pane ($got) is in $want" } else { Write-Fail "$name : the focused pane should be in $want, it is [$got]" }
}

Write-Host "`n=== a closed window or pane takes its own mode, the next pane keeps its own ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"

try {
    Write-Host "`n[1] kill-window" -ForegroundColor Yellow
    Check 'kill-window, the next window has a clock' 'clock-mode' { NewWindow; P clock-mode | Out-Null; P select-window -t :0 | Out-Null; P kill-window | Out-Null }
    Check 'kill-window, the next window is in copy mode' 'copy-mode' { NewWindow; P copy-mode | Out-Null; P select-window -t :0 | Out-Null; P kill-window | Out-Null }
    Check 'kill-window of a window with a clock' 'none' { NewWindow; P select-window -t :0 | Out-Null; P clock-mode | Out-Null; P kill-window | Out-Null }
    Check 'kill-window of a window in copy mode' 'none' { NewWindow; P select-window -t :0 | Out-Null; P copy-mode | Out-Null; P kill-window | Out-Null }

    # The report: a key on the window left reaches its clock, not its shell.
    Fresh
    NewWindow; P clock-mode | Out-Null; P select-window -t :0 | Out-Null; P kill-window | Out-Null
    Start-Sleep 1
    P send-keys q | Out-Null
    Start-Sleep 1
    $after = Focus
    $typed = @(P capture-pane -p | Where-Object { $_ -match '> *q\s*$' }).Count
    if ($after -match ' none$' -and $typed -eq 0) { Write-Pass "q on the window left closes its clock and types nothing ($after)" } else { Write-Fail "q should close the clock and type nothing, focus [$after], q typed on $typed row(s)" }

    Write-Host "`n[2] a pane or a window that exits" -ForegroundColor Yellow
    Check 'exit of a window, the next window has a clock' 'clock-mode' { NewWindow; P clock-mode | Out-Null; P select-window -t :0 | Out-Null; P send-keys exit Enter | Out-Null }
    Check 'a window with a clock that exits' 'none' { NewWindow -d; NewWindow -t :5 "$SHELLNAME -NoProfile -Command Start-Sleep -Seconds 4"; P clock-mode | Out-Null; Start-Sleep 5 }
    Check 'exit of a pane, the other pane has a clock' 'clock-mode' { P clock-mode | Out-Null; P split-window -h | Out-Null; Start-Sleep 1; P send-keys exit Enter | Out-Null }
    Check 'exit of a pane, the other pane is in copy mode' 'copy-mode' { P copy-mode | Out-Null; P split-window -h | Out-Null; Start-Sleep 1; P send-keys exit Enter | Out-Null }

    Write-Host "`n[3] the other ways a window goes" -ForegroundColor Yellow
    Check 'kill-window -a, the kept window has a clock' 'clock-mode' { NewWindow; P clock-mode | Out-Null; P select-window -t :0 | Out-Null; P kill-window -a -t :1 | Out-Null }
    Check 'unlink-window -k, the next window has a clock' 'clock-mode' { NewWindow; P clock-mode | Out-Null; P select-window -t :0 | Out-Null; P unlink-window -k | Out-Null }
    Check 'break-pane -d, the pane left has a clock' 'clock-mode' { P clock-mode | Out-Null; P split-window -h | Out-Null; Start-Sleep 1; P break-pane -d | Out-Null }
    Check 'join-pane -d of a window''s last pane, the focus lands on a window with a clock' 'clock-mode' { NewWindow; P clock-mode | Out-Null; NewWindow; P select-window -t :0 | Out-Null; P join-pane -d -t :2 | Out-Null }

    Write-Host "`n[4] link-window -k over a window before the current one" -ForegroundColor Yellow
    foreach ($detach in @($true, $false)) {
        Fresh
        P rename-window zero | Out-Null
        foreach ($n in 'one', 'two', 'three', 'four') { NewWindow -n $n }
        P select-window -t :1 | Out-Null
        if ($detach) { P link-window -d -k -s :3 -t :0 | Out-Null } else { P link-window -k -s :3 -t :0 | Out-Null }
        Start-Sleep 1
        $cur = (P display -p '#{window_index}:#{window_name}' | Out-String).Trim()
        $first = (P display -p -t :0 '#{window_name}' | Out-String).Trim()
        # Measured: tmux links `three` at 0; -d keeps 1:one, without -d the
        # link is selected.
        $wantCur = if ($detach) { '1:one' } else { '0:three' }
        $flag = if ($detach) { '-d -k' } else { '-k' }
        if ($first -eq 'three') { Write-Pass "link-window $flag -s :3 -t :0 links three" } else { Write-Fail "link-window $flag -s :3 -t :0 should link three, window 0 is [$first]" }
        if ($cur -eq $wantCur) { Write-Pass "link-window $flag leaves the focus on $wantCur" } else { Write-Fail "link-window $flag should leave the focus on $wantCur, it is on [$cur]" }
    }

    # Both routes: the CLI goes through the server request, source-file
    # through the in server command. tmux selects the new link when -k
    # removes the current window, -d or not (server-fn.c, "Force
    # select/redraw if current"), and -d otherwise hands the focus back to
    # the current window with its own mode.
    $srcFile = Join-Path $TMP "lw.conf"
    foreach ($route in 'cli', 'source-file') {
        function LW([string]$line) {
            if ($route -eq 'cli') { P @($line -split ' ') | Out-Null }
            else { Set-Content -Path $srcFile -Value $line -Encoding ASCII; P source-file $srcFile | Out-Null }
        }
        Write-Host "`n[5] link-window -d, $route" -ForegroundColor Yellow
        Fresh
        P rename-window zero | Out-Null
        foreach ($n in 'one', 'two', 'three') { NewWindow -n $n }
        P select-window -t :0 | Out-Null
        LW 'link-window -d -k -s :3 -t :0'
        Start-Sleep 1
        $cur = (P display -p '#{window_index}:#{window_name}' | Out-String).Trim()
        if ($cur -eq '0:three') { Write-Pass "link-window -d -k over the current window selects the link ($cur, $route)" } else { Write-Fail "link-window -d -k over the current window should select 0:three, it is on [$cur] ($route)" }

        Fresh
        NewWindow -d
        P copy-mode | Out-Null
        LW 'link-window -d -s :1 -t :5'
        Start-Sleep 1
        $got = Focus
        $n5 = (P display -p -t :5 '#{window_index}' | Out-String).Trim()
        if ($got -eq '0.0 copy-mode' -and $n5 -eq '5') { Write-Pass "link-window -d keeps the current pane's copy mode ($got, $route)" } else { Write-Fail "link-window -d should keep 0.0 in copy-mode and link 5, focus [$got] link [$n5] ($route)" }
    }
}
catch {
    Write-Fail "exception: $_"
}
finally {
    if ($script:ns) { P kill-server | Out-Null }
    Remove-Item -Recurse -Force $TMP -EA SilentlyContinue
}

Write-Host "`n  Passed: $script:TestsPassed  Failed: $script:TestsFailed"
exit $script:TestsFailed
