# Issue #1081: DECRQSS, `DCS $ q Pt ST`: the cursor style a pane's program set is answered.
#
# tmux answers `Pt` of `SP q` with the DECSCUSR style the program set, its
# shape with the blink of `CSI ? 12 h`, else with the `cursor-style` option,
# and any other setting with `DCS 0 $ r ST` (input_handle_decrqss,
# input.c:2554-2617 at tag 3.8). RIS and respawn-pane keep the shape and drop
# the blink, clear-history keeps both. psmux answered none of them. The
# numbers expected below were measured from tmux 3.8. psmux writes them the
# VT510's DECRPSS way, `DCS 1 $ r Ps SP q ST`, where tmux writes
# `DCS 1 $ r SP q Ps SP q ST`.
#
#   1. the inbox console host never passes DECRQSS on: build 26300 answers it
#      itself, and build 19045 (Windows 10) drops the DCS and its ST, shows
#      the rest as text and answers nothing
#   2. OpenConsole, with PSMUX_CONPTY_DIR_FOR_TEST (a directory with conpty.dll
#      and OpenConsole.exe; skipped without it), passes it on to psmux
#
# tests/reply_order_probe.cs runs in the pane with `raw` and records the replies
# as they came.
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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_rqss_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
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

# `\e` is ESC to the probe. Single quotes keep the `$` of DECRQSS literal.
$E = '\e'
$ASK = '\eP$q q\e\'
$DA1 = '\e[?1;2;4c'
$UNKNOWN = '\eP0$r\e\'
function Style($ps) { '\eP1$r' + $ps + ' q\e\' }
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
    $script:NS = "rqss_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 2500
    $one = '^(\\eP[01]\$r[^\\]*\\e\\)?$'
    $got = Probe "$E[4 q$ASK"
    Check "inbox conhost: the cursor style gets the host's DECRPSS or none" ($got -cmatch $one) $true
    Write-Info $(if ($got) { "the host answered '$got'" } else { 'nothing came back' })
    $got = Probe '\eP$qm\e\'
    Check "inbox conhost: so does SGR" ($got -cmatch $one) $true
    Write-Info $(if ($got) { "the host answered '$got'" } else { 'nothing came back' })
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

function Run-OpenConsole {
    $script:NS = "rqss_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 2500
    $r = 'OpenConsole'

    Write-Host "  -- the style the program set" -ForegroundColor DarkCyan
    foreach ($c in @(
            @('nothing set', '', 0),
            @('blinking block', "$E[1 q", 1),
            @('steady block', "$E[2 q", 2),
            @('blinking underline', "$E[3 q", 3),
            @('underline', "$E[4 q", 4),
            @('blinking bar', "$E[5 q", 5),
            @('bar', "$E[6 q", 6),
            @('0 after underline', "$E[4 q$E[0 q", 0),
            @('no parameter after underline', "$E[4 q$E[ q", 0),
            @('7 after underline, ignored', "$E[4 q$E[7 q", 4),
            @('blink mode with no shape', "$E[?12h", 0),
            @('steady block, then blink mode on', "$E[2 q$E[?12h", 1),
            @('blinking block, then blink mode off', "$E[1 q$E[?12l", 2),
            @('RIS after a blinking block', "$E[1 q${E}c", 2),
            @('underline, then the alternate screen', "$E[4 q$E[?1049h", 4)
        )) {
        $got = Probe "$($c[1])$ASK"
        Check "${r}: $($c[0])" $got (Style $c[2])
    }

    Write-Host "  -- the cursor-style option when the program set none" -ForegroundColor DarkCyan
    P set -g cursor-style bar | Out-Null
    Check "${r}: cursor-style bar" (Probe $ASK) (Style 6)
    Check "${r}: and the program's wins" (Probe "$E[3 q$ASK") (Style 3)
    P set -g cursor-style blinking-underline | Out-Null
    Check "${r}: cursor-style blinking-underline" (Probe $ASK) (Style 3)
    # psmux's own cursor-blink, which tmux spells into the style.
    P set -g cursor-style block | Out-Null
    P set -g cursor-blink on | Out-Null
    Check "${r}: cursor-style block with cursor-blink on" (Probe $ASK) (Style 1)
    P set -gu cursor-blink | Out-Null
    P set -gu cursor-style | Out-Null
    Check "${r}: back to the default" (Probe $ASK) (Style 0)

    Write-Host "  -- other settings, and the query's form" -ForegroundColor DarkCyan
    Check "${r}: SGR" (Probe '\eP$qm\e\') $UNKNOWN
    Check "${r}: DECSTBM" (Probe '\eP$qr\e\') $UNKNOWN
    Check "${r}: DECSCA" (Probe '\eP$q"q\e\') $UNKNOWN
    Check "${r}: nothing asked" (Probe '\eP$q\e\') $UNKNOWN
    Check "${r}: more after SP q" (Probe ("$E[6 q" + '\eP$q qx\e\')) (Style 6)
    Check "${r}: a parameter before the dollar" (Probe '\eP1$q q\e\') (Style 0)
    Check "${r}: DCS dollar p is no query, DA1 still comes" (Probe ('\eP$p q\e\' + "$E[c")) $DA1
    # tmux reads everything up to the ST as the string, a DA1 too.
    Check "${r}: a DA1 inside the string is not answered" (Probe '\eP$q q\e[c\e\') (Style 0)
    Check "${r}: an OSC inside the string" (Probe '\eP$q q\e]0;x\e\') (Style 0)

    Write-Host "  -- order" -ForegroundColor DarkCyan
    Check "${r}: before DA1" (Probe "$ASK$E[c") ((Style 0) + $DA1)
    Check "${r}: each at its point" (Probe "$E[5 q$ASK$E[3 q$ASK") ((Style 5) + (Style 3))
    P set -g allow-passthrough on | Out-Null
    Check "${r}: allow-passthrough on changes nothing" (Probe "$E[4 q$ASK") (Style 4)
    P set -gu allow-passthrough | Out-Null

    Write-Host "  -- a new screen" -ForegroundColor DarkCyan
    $w = New-Win
    Probe-In $w "$E[4 q$E[c" | Out-Null
    P clear-history -t $w | Out-Null
    Start-Sleep -Milliseconds 500
    Check "${r}: clear-history keeps the style" (Probe-In $w $ASK) (Style 4)
    $w = New-Win
    Probe-In $w "$E[1 q$E[c" | Out-Null
    P respawn-pane -k -t $w | Out-Null
    Start-Sleep -Milliseconds 3000
    Check "${r}: respawn-pane keeps the shape, not the blink" (Probe-In $w $ASK) (Style 2)

    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== DECRQSS ===" -ForegroundColor Cyan
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
