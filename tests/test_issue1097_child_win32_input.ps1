# Issue #1097: on the Windows 10 inbox conhost, a program that asks its console for win32 input mode gets no named key, Ctrl+letter or Escape from psmux.
#
# That host (10.0.19041.320 on 19045) does not parse win32 input mode, and it
# passes a program's own CSI ?9001h through to psmux at the start of the
# pane's stream, where a host that parses the mode writes its own request:
#
#   child asks (keyrec845.js w32):   ESC[?9001h ESC[2J ESC[m ESC[H keyrec845 ready
#   child does not:                  ESC[2J ESC[m ESC[H keyrec845 ready
#
# psmux took it as the host's (#845) and wrote the #729 win32 records, which
# that host drops: the child read nothing for Up, F5 or C-Right.  On an inbox
# host before build 22000 psmux no longer takes the request as the host's, so
# the child gets tmux's bytes, as a child that did not ask does.  On a host
# that parses the mode (Windows 11, or OpenConsole through PSMUX_CONPTY_DIR)
# the #729 records stay.
#
# Test 1: a raw mode node child that asks for the mode (an Ink based TUI on
#         Windows): each key arrives once, as tmux's bytes on the Windows 10
#         inbox host and as one win32 record of the key elsewhere.
# Test 2: the same child without the request gets tmux's bytes everywhere.
#
# Isolation: own -L namespace, own PSMUX_DATA_DIR, cleanup only with
# `-L <ns> kill-server`.  PSMUX_TEST_BIN picks the binary (default: psmux on PATH).
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue1097_child_win32_input.ps1
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } elseif ($env:PSMUX_BIN) { $env:PSMUX_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "i1097$PID"
$script:TestsPassed = 0; $script:TestsFailed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkGray }

$node = Get-Command node -EA SilentlyContinue
if (-not $node) { Write-Host "SKIP: node is not installed" -ForegroundColor Yellow; exit 0 }

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'TMUX', 'TMUX_PANE') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$savedDataDir = $env:PSMUX_DATA_DIR
$savedNoWarm = $env:PSMUX_NO_WARM
$root = Join-Path $env:TEMP "psmux_i1097_$PID"
Remove-Item -Recurse -Force $root -EA SilentlyContinue
New-Item -ItemType Directory -Force $root | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $root "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_NO_WARM = "1"
$emptyConf = Join-Path $root "empty.conf"
"" | Set-Content -Path $emptyConf -Encoding ASCII
$keyrec = Join-Path $PSScriptRoot "keyrec845.js"

# The Windows 10 inbox host, which neither parses the mode nor asks for it.
$build = [Environment]::OSVersion.Version.Build
$inboxWin10 = $build -lt 22000 -and -not $env:PSMUX_CONPTY_DIR
Write-Info ("psmux: {0}  ({1})  build={2}  inbox Windows 10 host: {3}" -f $PSMUX, ((& $PSMUX -V) -join ' '), $build, $inboxWin10)

$ESC = [char]27
function Show([string]$s) { return $s.Replace([string]$ESC, '^[') }
function Hex([string]$s) { return (-join ([Text.Encoding]::ASCII.GetBytes($s) | ForEach-Object { $_.ToString('x2') })) }
function Text([string]$hex) { -join ($(for ($i = 0; $i -lt $hex.Length; $i += 2) { [char][Convert]::ToByte($hex.Substring($i, 2), 16) })) }
function Count-Lines([string]$log) { return @(Get-Content $log -EA SilentlyContinue).Count }
function Read-Hex([string]$log, [int]$from) {
    $lines = @(Get-Content $log -EA SilentlyContinue)
    -join @(for ($i = $from; $i -lt $lines.Count; $i++) { if ($lines[$i] -match ' hex=([0-9a-f]*)$') { $Matches[1] } })
}
function Wait-Lines([string]$log, [int]$before, [int]$ms = 2500) {
    $sw = [Diagnostics.Stopwatch]::StartNew(); $last = -1
    while ($sw.ElapsedMilliseconds -lt $ms) {
        $n = Count-Lines $log
        if ($n -gt $before -and $n -eq $last) { break }
        $last = $n
        Start-Sleep -Milliseconds 150
    }
}
function Start-Keyrec([string]$Sess, [string]$Mode) {
    $log = Join-Path $root "$Sess.log"
    Remove-Item $log -EA SilentlyContinue
    & $PSMUX -L $NS -f $emptyConf new-session -d -s $Sess -x 120 -y 30 -- node $keyrec $log $Mode 2>&1 | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 15000) {
        if ((Test-Path $log) -and (((& $PSMUX -L $NS capture-pane -p -t $Sess) -join "`n") -match 'keyrec845 ready')) { break }
        Start-Sleep -Milliseconds 100
    }
    Start-Sleep -Milliseconds 300
    return $log
}

# name, send-keys key, tmux's bytes, virtual key and control key state of
# the win32 record.  Escape comes after the named keys, which latch the
# ConPTY on a host that parses the mode (#588).
$cases = @(
    @('Up', 'Up', "$ESC[A", 38, 256),
    @('Left', 'Left', "$ESC[D", 37, 256),
    @('Home', 'Home', "$ESC[H", 36, 256),
    @('F5', 'F5', "$ESC[15~", 116, 0),
    @('C-Right', 'C-Right', "$ESC[1;5C", 39, 264),
    @('C-a', 'C-a', [string][char]1, 65, 8),
    @('Escape', 'Escape', [string]$ESC, 27, 0)
)

try {
    Write-Host "`n=== Issue #1097: a child that asks for win32 input mode ===" -ForegroundColor Cyan

    Write-Host "`n[Test 1] raw mode child that asked for the mode" -ForegroundColor Yellow
    $log = Start-Keyrec "w" "w32"
    foreach ($c in $cases) {
        $before = Count-Lines $log
        & $PSMUX -L $NS send-keys -t w $c[1] 2>&1 | Out-Null
        Wait-Lines $log $before
        $hex = Read-Hex $log $before
        $s = Text $hex
        if ($inboxWin10) {
            if ($hex -eq (Hex $c[2])) { Write-Pass ("{0}: tmux's bytes {1}" -f $c[0], (Show $c[2])) }
            else { Write-Fail ("{0}: want tmux's bytes {1}, read '{2}'" -f $c[0], (Show $c[2]), (Show $s)) }
        } else {
            $recs = [regex]::Matches($s, "\x1b\[(\d+);(\d+);(\d+);(\d+);(\d+);(\d+)_")
            $vk = @($recs | Where-Object { [int]$_.Groups[1].Value -eq $c[3] -and [int]$_.Groups[5].Value -eq $c[4] })
            if ($vk.Count -eq 2 -and $recs.Count -eq 2) { Write-Pass ("{0}: one vk={1} press and release" -f $c[0], $c[3]) }
            else { Write-Fail ("{0}: want one vk={1} cs={2} record, read '{3}'" -f $c[0], $c[3], $c[4], (Show $s)) }
        }
    }
    & $PSMUX -L $NS kill-session -t w 2>&1 | Out-Null

    Write-Host "`n[Test 2] the same child without the request" -ForegroundColor Yellow
    $log = Start-Keyrec "p" "-"
    foreach ($c in $cases) {
        $before = Count-Lines $log
        & $PSMUX -L $NS send-keys -t p $c[1] 2>&1 | Out-Null
        Wait-Lines $log $before
        $hex = Read-Hex $log $before
        if ($hex -eq (Hex $c[2])) { Write-Pass ("{0}: tmux's bytes {1}" -f $c[0], (Show $c[2])) }
        else { Write-Fail ("{0}: want tmux's bytes {1}, read '{2}'" -f $c[0], (Show $c[2]), (Show (Text $hex))) }
    }
}
finally {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    $env:PSMUX_DATA_DIR = $savedDataDir
    $env:PSMUX_NO_WARM = $savedNoWarm
    for ($i = 0; $i -lt 10 -and (Test-Path $root); $i++) {
        Start-Sleep -Milliseconds 300
        Remove-Item -Recurse -Force $root -EA SilentlyContinue
    }
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
