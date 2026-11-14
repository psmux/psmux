# The cursor across the alternate screen: where it is after DECSET 1049 and
# 47, and after DECRST 1049 and 47, as tmux keeps one cursor for both screens
# (screen_alternate_on and screen_alternate_off, screen.c at tag 3.8). psmux
# kept a cursor per grid, so 1049 started the alternate screen at 0,0 and 47
# at wherever the last alternate session left it, and DECRST 47 jumped back
# to the main screen's cursor (fixed in 43e82838, which has unit tests only).
#
# Every expectation is what tmux 3.8 did with the same bytes in an 80 by 24
# pane, read with #{cursor_x},#{cursor_y} and #{alternate_on} while the
# program still runs, except where a comment says otherwise.
#
# tests/reply_order_probe.cs runs in the pane with `raw`: it writes a case's
# bytes in one write and stays for 2 s, while the suite reads the pane's
# cursor; the last reading before it ends is the one checked.
#   1. the inbox conhost.
#   2. OpenConsole, when PSMUX_TEST_CONPTY_DIR or PSMUX_CONPTY_DIR_FOR_TEST
#      names a folder holding conpty.dll, skipped otherwise.
#
# The inbox conhost of Windows 10 (19045) keeps 1049 to itself and starts its
# alternate screen with the cursor at the top left, and hands psmux only its
# drawing, so there psmux stays off the alternate screen with the host's
# cursor; it passes 47 on but ignores a 1049 reset that follows.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per route;
# cleanup is `-L <ns> kill-server`. PSMUX_TEST_BIN or PSMUX_EXE picks the
# binary (defaults to target\release\psmux.exe).

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { $env:PSMUX_EXE }
$ConptyDir = if ($env:PSMUX_TEST_CONPTY_DIR) { $env:PSMUX_TEST_CONPTY_DIR } elseif ($env:PSMUX_CONPTY_DIR_FOR_TEST) { $env:PSMUX_CONPTY_DIR_FOR_TEST } else { '' }
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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_altcur_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
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

# The inbox conhost before Windows 11 (measured on 19045).
$Win10 = [Environment]::OSVersion.Version.Build -lt 22000
$script:NS = ""
$script:run = 0
function P { & $PSMUX -L $script:NS @args 2>&1 }

# Type once the shell in $target shows its prompt: on a slow machine it can
# take longer to start than a fixed wait, and keys sent before may be lost.
function Wait-Prompt($target) {
    for ($k = 0; $k -lt 100 -and -not ((P capture-pane -p -t $target | Out-String) -match 'PS [^\r\n]*>'); $k++) {
        Start-Sleep -Milliseconds 200
    }
    Start-Sleep -Milliseconds 500
}

# Run $c.q from a new window and check where psmux has the cursor, and
# whether it is on the alternate screen, while the program still runs.
function Case($route, $c) {
    $script:run++
    $window = "w$($script:run)"
    P new-window -t s -n $window | Out-Null
    Wait-Prompt "s:$window"
    $out = Join-Path $TempHome "r_$($script:NS)_$($script:run).txt"
    P send-keys -t "s:$window" "& '$probe' '$out' '$($c.q)' raw" Enter | Out-Null
    $last = '(no reading)'
    for ($k = 0; $k -lt 150 -and -not (Test-Path $out); $k++) {
        $v = "$(P display -p -t "s:$window" '#{cursor_x},#{cursor_y} alt=#{alternate_on}')".Trim()
        # Only a reading the program outlived: once it ends, the shell draws
        # its prompt and moves the cursor.
        if (-not (Test-Path $out)) { $last = $v }
        Start-Sleep -Milliseconds 100
    }
    if (-not (Test-Path $out)) { $last = '(the probe never ended)' }
    $want = if ($c.want -isnot [hashtable]) { $c.want }
            elseif ($route -eq 'OpenConsole') { $c.want.oc }
            elseif ($Win10 -and $c.want.ContainsKey('win10')) { $c.want.win10 }
            else { $c.want.inbox }
    Check "${route}: $($c.n)" $last $want
}

$cases = @(
    @{ n = '1049 on keeps the cursor'; q = '\e[5;10H\e[?1049h'; want = @{ inbox = '9,4 alt=1'; win10 = '0,0 alt=0'; oc = '9,4 alt=1' } }
    @{ n = '1049 off puts back the cursor it saved'; q = '\e[5;10H\e[?1049h\e[2;3H\e[?1049l'; want = '9,4 alt=0' }
    @{ n = '47 on keeps the cursor'; q = '\e[5;10H\e[?47h'; want = '9,4 alt=1' }
    @{ n = '47 off keeps the alternate screen''s cursor'; q = '\e[5;10H\e[?47h\e[2;3H\e[?47l'; want = '2,1 alt=0' }
    # psmux: tmux takes 1047 as 47; both console hosts ignore it, and so does
    # psmux (43e82838).
    @{ n = '1047 on is ignored'; q = '\e[5;10H\e[?1047h'; want = '9,4 alt=0' }
    @{ n = '1047 on and off are ignored'; q = '\e[5;10H\e[?1047h\e[2;3H\e[?1047l'; want = '2,1 alt=0' }
    @{ n = '1049 on again keeps the alternate cursor'; q = '\e[5;10H\e[?1049h\e[2;3H\e[?1049h'; want = @{ inbox = '2,1 alt=1'; win10 = '0,0 alt=0'; oc = '2,1 alt=1' } }
    # The inbox conhost draws the alternate screen itself and puts the cursor
    # on the last column, `ESC[5;80H`, where the wrap still waits.
    @{ n = '1049 on with a wrap pending'; q = '\e[5;80Hx\e[?1049h'; want = @{ inbox = '79,4 alt=1'; win10 = '0,0 alt=0'; oc = '80,4 alt=1' } }
    @{ n = '1049 on in origin mode'; q = '\e[5;20r\e[?6h\e[3;4H\e[?1049h'; want = @{ inbox = '3,6 alt=1'; win10 = '0,0 alt=0'; oc = '3,6 alt=1' } }
    # The inbox conhost passes 47 on but takes this 1049 reset for itself: it
    # homes its cursor (`ESC[H`) and hands psmux no `ESC[?1049l`, so psmux
    # stays on the alternate screen, where the host's cursor now is.
    @{ n = '47 on, then 1049 off'; q = '\e[5;10H\e[?47h\e[2;3H\e[?1049l'; want = @{ inbox = '0,0 alt=1'; win10 = '2,1 alt=1'; oc = '2,1 alt=0' } }
)

function Run-Cases($route) {
    $script:NS = "ac_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $script:run = 0
    P new-session -d -s s -x 80 -y 24 | Out-Null
    Start-Sleep -Milliseconds 2500
    foreach ($c in $cases) { Case $route $c }
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== The cursor across the alternate screen ===" -ForegroundColor Cyan
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
        Write-Info "skipped: set PSMUX_TEST_CONPTY_DIR to a folder holding conpty.dll and OpenConsole.exe"
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
