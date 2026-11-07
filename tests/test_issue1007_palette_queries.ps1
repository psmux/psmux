# Issue #1007: palette queries past the sixteen ANSI colours, and colour
# replies that end the way their query did.
#
# tmux's input_osc_4 (input.c:2925-2970 at tag 3.8) answers `OSC 4 ; N ; ?`
# for every index from 0 to 255 from the pane's own palette, the OSC 4 entries
# over the pane-colours defaults (colour_palette_get), and asks the outer
# terminal only when the pane has none. Every colour reply ends as its query
# did, BEL or ST (input_osc_colour_reply with ictx->input_end). psmux answered
# only the sixteen ANSI colours, and ended every reply in ST.
#
# tests/reply_order_probe.cs runs in the pane with `raw`: it writes the queries
# of a case in one write and records the replies as they came. No client is
# attached, so the sixteen colours, the foreground and the background are
# psmux's own defaults (Campbell).
#   1. the inbox console host, which passes OSC 4 on as written, BEL included,
#      and drops OSC 10 and 11 (pipe-pane -o).
#   2. OpenConsole, with PSMUX_CONPTY_DIR_FOR_TEST (a directory with
#      conpty.dll and OpenConsole.exe; skipped without it), which passes all of
#      them on.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per route;
# cleanup is `-L <ns> kill-server`. PSMUX_TEST_BIN or PSMUX_EXE picks the
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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_pal_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -Last 1
$probe = Join-Path $TempHome 'reply_order_probe.exe'
if ($csc) { & $csc.FullName /nologo /optimize /out:$probe (Join-Path $PSScriptRoot 'reply_order_probe.cs') 2>&1 | Out-Null }
if (-not (Test-Path $probe)) {
    Write-Fail "build tests\reply_order_probe.cs (csc.exe unavailable)"
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
    exit $script:TestsFailed
}

$E = '\e'; $A = '\a'; $ST = '\e\'
$C1 = 'rgb:c5c5/0f0f/1f1f'; $C2 = 'rgb:1313/a1a1/0e0e'
# name, the queries (\e for ESC, \a for BEL), what tmux gives for a pane whose
# entries are as the case sets them, and, where the inbox console host drops
# the query, what it gives.
$cases = @(
    @{ n = 'index 1 asked with BEL';  q = "$E]4;1;?$A";   a = "$E]4;1;$C1$A" }
    @{ n = 'index 1 asked with ST';   q = "$E]4;1;?$ST";  a = "$E]4;1;$C1$ST" }
    @{ n = 'index 20, no entry';      q = "$E]4;20;?$A";  a = '' }
    @{ n = 'index 255, no entry';     q = "$E]4;255;?$ST"; a = '' }
    @{ n = 'index 20 set by OSC 4';   q = "$E]4;20;rgb:11/22/33$ST$E]4;20;?$A"; a = "$E]4;20;rgb:1111/2222/3333$A" }
    @{ n = 'index 255 set by OSC 4';  q = "$E]4;255;#abcdef$A$E]4;255;?$ST"; a = "$E]4;255;rgb:abab/cdcd/efef$ST" }
    @{ n = 'low and high in one OSC 4'; q = "$E]4;20;#112233;1;?;20;?;21;?;2;?$ST"; a = "$E]4;1;$C1$ST$E]4;20;rgb:1111/2222/3333$ST$E]4;2;$C2$ST" }
    @{ n = 'index 20 reset by OSC 104'; q = "$E]4;20;#112233$A$E]104;20$A$E]4;20;?$A"; a = '' }
    @{ n = 'foreground asked with BEL'; q = "$E]10;?$A";  a = "$E]10;rgb:cccc/cccc/cccc$A"; inbox = '' }
    @{ n = 'background asked with ST';  q = "$E]11;?$ST"; a = "$E]11;rgb:0c0c/0c0c/0c0c$ST"; inbox = '' }
    @{ n = 'index 20 from set -p pane-colours'; q = "$E]4;20;?$A"; a = "$E]4;20;rgb:4444/5555/6666$A"; set = @('-p', 'pane-colours[20]', '#445566') }
    @{ n = 'index 200 from set -g pane-colours'; q = "$E]4;200;?$ST"; a = "$E]4;200;rgb:1212/3434/5656$ST"; set = @('-g', 'pane-colours[200]', '#123456') }
)

$script:NS = ""
function P { & $PSMUX -L $script:NS @args 2>&1 }
function Run-Cases($route) {
    $script:NS = "pal_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 120 -y 20 | Out-Null
    Start-Sleep -Milliseconds 2500
    $i = 0
    foreach ($c in $cases) {
        $i++
        P new-window -d -t s -n "w$i" | Out-Null
        Start-Sleep -Milliseconds 2500
        if ($c.ContainsKey('set')) {
            $scope = $c.set[0]
            if ($scope -eq '-p') { P set -p -t "s:w$i" $c.set[1] $c.set[2] | Out-Null } else { P set $scope $c.set[1] $c.set[2] | Out-Null }
            Start-Sleep -Milliseconds 300
        }
        $out = Join-Path $TempHome "r_${route}_$i.txt"
        P send-keys -t "s:w$i" "& '$probe' '$out' '$($c.q)' raw" Enter | Out-Null
        for ($k = 0; $k -lt 60 -and -not (Test-Path $out); $k++) { Start-Sleep -Milliseconds 200 }
        Start-Sleep -Milliseconds 300
        $got = if (Test-Path $out) { (Get-Content $out -Raw).Trim() } else { '(no reply file)' }
        $want = if ($route -eq 'inbox conhost' -and $c.ContainsKey('inbox')) { $c.inbox } else { $c.a }
        Check "${route}: $($c.n)" $got $want
        if ($c.ContainsKey('set') -and $c.set[0] -eq '-g') { P set -gu $c.set[1] | Out-Null }
    }
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== palette queries and colour reply endings ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    Write-Host "`n[1] the inbox conhost" -ForegroundColor Yellow
    Run-Cases 'inbox conhost'

    Write-Host "`n[2] OpenConsole" -ForegroundColor Yellow
    if ($ConptyDir -and (Test-Path (Join-Path $ConptyDir 'conpty.dll'))) {
        $env:PSMUX_CONPTY_DIR = $ConptyDir
        Run-Cases 'OpenConsole'
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
