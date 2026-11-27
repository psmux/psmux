# Issue #1105: DECRQM, `CSI ? Ps $ p`: a pane's mode is answered as it stood
# at the query, and modes 6, 12 and 1004 are known.
#
# tmux answers each query as its parser meets it (INPUT_CSI_QUERY_PRIVATE,
# input.c:1651-1722 at tag 3.8). psmux answered every DECRQM of one read from
# the screen as it stood after the whole read, so a mode set, asked and reset
# in one write came back reset. It also answered 0, "not recognised", for 6
# (DECOM), 12 (the cursor blink) and 1004 (focus reporting), which tmux knows.
# The values expected below were measured from tmux 3.8.
#
# 1004: the console host turns the mode on too, for focus events of its own,
# in the same output as the program's. Its own `CSI ? 1004 h` comes beside a
# `CSI ? 9001 h` when a pane starts and after an RIS, and, from OpenConsole,
# right after the program turns the mode off. Those do not count.
#
# 2031 stays 0 where tmux knows it: psmux sends no theme change notification.
#
#   1. the inbox console host: build 26300 answers DECRQM itself and never
#      passes it on; build 19045 (Windows 10) passes it on when it next
#      draws, and psmux answers. Either way one reply comes back, and 1004
#      is answered as the program set it
#   2. OpenConsole, with PSMUX_CONPTY_DIR_FOR_TEST (a directory with conpty.dll
#      and OpenConsole.exe; skipped without it), passes it on to psmux
#
# tests/reply_order_probe.cs runs in the pane with `raw`, writes the bytes
# given in one write and records the replies as they came.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per console
# host; cleanup is `-L <ns> kill-server`. PSMUX_TEST_BIN or PSMUX_EXE picks the
# binary (defaults to target\release\psmux.exe).

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { $env:PSMUX_EXE }
$ConptyDir = $env:PSMUX_CONPTY_DIR_FOR_TEST
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
# The panes run pwsh commands; SHELL would make psmux start them in bash.
Remove-Item Env:SHELL -EA SilentlyContinue
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkGray }
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_rqm_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -Last 1
$probe = Join-Path $TempHome 'reply_order_probe.exe'
if ($csc) {
    & $csc.FullName /nologo /optimize /out:$probe (Join-Path $PSScriptRoot 'reply_order_probe.cs') 2>&1 | Out-Null
}
if (-not (Test-Path $probe)) {
    Write-Fail "build tests\reply_order_probe.cs (csc.exe unavailable)"
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
    exit $script:TestsFailed
}

# `\e` is ESC to the probe. Single quotes keep the `$` of DECRQM literal.
$E = '\e'
$DA1 = '\e[?1;2;4c'
function Ask($mode) { '\e[?' + $mode + '$p' }
function Mode($mode, $n) { '\e[?' + $mode + ';' + $n + '$y' }
$script:NS = ""
$script:run = 0
function P { & $PSMUX -L $script:NS @args 2>&1 }

function New-Win {
    $script:run++
    $win = "s:w$($script:run)"
    P new-window -d -t s -n "w$($script:run)" | Out-Null
    # Type once the shell shows its prompt: on a slow machine it can take
    # longer to start than a fixed wait, and keys sent before may be lost.
    for ($k = 0; $k -lt 100 -and -not ((P capture-pane -p -t $win | Out-String) -match 'PS [^\r\n]*>'); $k++) {
        Start-Sleep -Milliseconds 200
    }
    Start-Sleep -Milliseconds 500
    $win
}

# Run the probe with $queries in window $win; returns the replies as they came.
function Probe-In($win, $queries) {
    $out = Join-Path $TempHome "r_$($script:NS)_$([guid]::NewGuid().ToString('N').Substring(0, 6)).txt"
    P send-keys -t $win "& '$probe' '$out' '$queries' raw" Enter | Out-Null
    # The first run of a freshly built probe can take well over ten seconds.
    for ($k = 0; $k -lt 150 -and -not (Test-Path $out); $k++) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Milliseconds 300
    if (Test-Path $out) { (Get-Content $out -Raw).Trim() } else { '(no reply file)' }
}

function Probe($queries) { Probe-In (New-Win) $queries }

function Run-Inbox {
    $script:NS = "rqm_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 2500
    # The text after the queries makes the build 19045 host draw, and so
    # pass them on.
    foreach ($m in 6, 12, 1004, 2031) {
        $got = Probe "$E[?${m}h$(Ask $m)$E[?${m}l."
        Check "inbox conhost: $m gets one reply" ($got -cmatch ('^\\e\[\?' + $m + ';\d\$y$')) $true
        Write-Info "the reply was '$got'"
        if ($m -eq 1004) {
            # The host's own answer, or psmux's from the program's mode.
            Check "inbox conhost: 1004 set is answered" $got (Mode 1004 1)
        }
    }
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

function Run-OpenConsole {
    $script:NS = "rqm_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 2500
    $r = 'OpenConsole'

    Write-Host "  -- set, asked and reset in one write" -ForegroundColor DarkCyan
    foreach ($c in @(
            @('DECCKM', 1, 'h', 'l', 1),
            @('DECTCEM hidden', 25, 'l', 'h', 2),
            @('DECAWM off', 7, 'l', 'h', 2),
            @('bracketed paste', 2004, 'h', 'l', 1),
            @('mouse 1000', 1000, 'h', 'l', 1),
            @('alternate screen', 1049, 'h', 'l', 1)
        )) {
        Check "${r}: $($c[0])" (Probe "$E[?$($c[1])$($c[2])$(Ask $c[1])$E[?$($c[1])$($c[3])") (Mode $c[1] $c[4])
    }
    Check "${r}: IRM" (Probe ('\e[4h\e[4$p\e[4l')) '\e[4;1$y'
    Check "${r}: asked before and after a set" (Probe "$(Ask 1)$E[?1h$(Ask 1)") ((Mode 1 2) + (Mode 1 1))

    Write-Host "  -- 6, origin mode" -ForegroundColor DarkCyan
    foreach ($c in @(
            @('nothing set', '', 2),
            @('set', "$E[?6h", 1),
            @('set, then reset', "$E[?6h$E[?6l", 2),
            @('set, then RIS', "$E[?6h${E}c", 2)
        )) {
        Check "${r}: 6 $($c[0])" (Probe "$($c[1])$(Ask 6)$E[?6l") (Mode 6 $c[2])
    }

    Write-Host "  -- 12, the cursor blink" -ForegroundColor DarkCyan
    foreach ($c in @(
            @('nothing set', '', 2),
            @('blink on', "$E[?12h", 1),
            @('blink off', "$E[?12l", 2),
            @('blinking block', "$E[1 q", 1),
            @('steady block', "$E[2 q", 2),
            @('bar, then blink on', "$E[6 q$E[?12h", 1),
            @('blink on, then DECSCUSR 0', "$E[?12h$E[0 q", 2),
            @('blinking bar, then RIS', "$E[5 q${E}c", 2),
            @('blink on, then the alternate screen', "$E[?12h$E[?1049h", 1),
            @('blink on, then DECSCUSR 7', "$E[?12h$E[7 q", 1)
        )) {
        Check "${r}: 12 $($c[0])" (Probe "$($c[1])$(Ask 12)") (Mode 12 $c[2])
    }
    P set -g cursor-style blinking-bar | Out-Null
    foreach ($c in @(
            @('nothing set', '', 1),
            @('steady block, then DECSCUSR 0', "$E[2 q$E[0 q", 1),
            @('blink off, then RIS', "$E[?12l${E}c", 1),
            @('blink off, then DECSCUSR 0', "$E[?12l$E[0 q", 1),
            @('blink off', "$E[?12l", 2)
        )) {
        Check "${r}: cursor-style blinking-bar, 12 $($c[0])" (Probe "$($c[1])$(Ask 12)") (Mode 12 $c[2])
    }
    P set -g cursor-style underline | Out-Null
    Check "${r}: cursor-style underline" (Probe (Ask 12)) (Mode 12 2)
    P set -g cursor-style blinking-underline | Out-Null
    Check "${r}: cursor-style blinking-underline" (Probe (Ask 12)) (Mode 12 1)
    P set -gu cursor-style | Out-Null
    Check "${r}: cursor-style back to the default" (Probe (Ask 12)) (Mode 12 2)

    Write-Host "  -- 1004, focus reporting, beside the host's own" -ForegroundColor DarkCyan
    foreach ($c in @(
            @('nothing set', '', 2),
            @('set', "$E[?1004h", 1),
            @('set, then reset', "$E[?1004h$E[?1004l", 2),
            @('set, then RIS', "$E[?1004h${E}c", 2),
            @('reset, then set', "$E[?1004l$E[?1004h", 1),
            @('set twice', "$E[?1004h$E[?1004h", 1),
            @('reset twice', "$E[?1004h$E[?1004l$E[?1004l", 2),
            @('set and reset with 1000', "$E[?1000;1004h$E[?1004;1000l", 2),
            @('reset with 1000, then text', "$E[?1004h$E[?1004;1000lxy", 2)
        )) {
        Check "${r}: 1004 $($c[0])" (Probe "$($c[1])$(Ask 1004)$E[?1004l") (Mode 1004 $c[2])
    }

    Write-Host "  -- 2031, theme updates, stays unknown" -ForegroundColor DarkCyan
    Check "${r}: 2031 nothing set" (Probe (Ask 2031)) (Mode 2031 0)
    Check "${r}: 2031 set" (Probe "$E[?2031h$(Ask 2031)$E[?2031l") (Mode 2031 0)

    Write-Host "  -- order" -ForegroundColor DarkCyan
    Check "${r}: each at its point, before DA1" (Probe "$E[?12h$(Ask 6)$(Ask 12)$(Ask 1004)$(Ask 2031)$E[c") `
        ((Mode 6 2) + (Mode 12 1) + (Mode 1004 2) + (Mode 2031 0) + $DA1)

    Write-Host "  -- a new screen" -ForegroundColor DarkCyan
    $w = New-Win
    Probe-In $w "$E[?12h$E[c" | Out-Null
    P clear-history -t $w | Out-Null
    Start-Sleep -Milliseconds 500
    Check "${r}: clear-history keeps the blink" (Probe-In $w (Ask 12)) (Mode 12 1)
    $w = New-Win
    Probe-In $w "$E[?6h$E[?1004h$E[?12h$E[c" | Out-Null
    P respawn-pane -k -t $w | Out-Null
    Start-Sleep -Milliseconds 3000
    Check "${r}: respawn-pane resets 6, 1004 and 12" (Probe-In $w "$(Ask 6)$(Ask 1004)$(Ask 12)") `
        ((Mode 6 2) + (Mode 1004 2) + (Mode 12 2))

    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== DECRQM ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    Write-Host "`n[1] the inbox conhost" -ForegroundColor Yellow
    Run-Inbox

    Write-Host "`n[2] OpenConsole" -ForegroundColor Yellow
    if ($ConptyDir -and (Test-Path (Join-Path $ConptyDir 'conpty.dll'))) {
        $env:PSMUX_CONPTY_DIR = $ConptyDir
        Run-OpenConsole
        Remove-Item Env:PSMUX_CONPTY_DIR
    } else {
        Write-Info "skipped: set PSMUX_CONPTY_DIR_FOR_TEST to a folder holding conpty.dll and OpenConsole.exe"
    }
}
catch {
    Write-Fail "exception: $_"
}
finally {
    if ($script:NS) { P kill-server | Out-Null }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
