# Issue #597 (follow up): what happens to a reply psmux cannot inject.
#
# A reporter on Windows 10 19045 measured every reply psmux injects into a pane
# arriving as zero bytes, while the replies ConPTY answers itself (DA1, DSR)
# arrived normally.  Their mouse debug log carried one line:
#
#     [platform] send_bracketed_paste: AttachConsole(18572) FAILED err=5
#
# and never a success.  Two things in that report could not be followed up from
# a modern host, and both are fixed here rather than guessed at.
#
# 1. The log did not say whether the FreeConsole immediately above the attach
#    had taken.  err=5 is ERROR_ACCESS_DENIED, which AttachConsole returns both
#    when the caller still holds a console and when the target console refuses
#    the caller, and nothing in the line told those apart.  The attach now
#    reports the detach result, the console before and after it, the target's
#    liveness, image, parent and token, both processes' integrity, and the OS
#    build, and retries once behind a second verified detach.
#
# 2. The colour path was documented as falling back to the pane's input pipe
#    when injection fails, and the reporter saw the reply still not arrive.
#    Measured here by writing each reply shape straight into a live pane's
#    ConPTY input pipe and dumping the child's stdin on build 26200:
#
#       "A"                               -> "A"
#       CSI ESC [ ? 997 ; 1 n             -> byte exact
#       OSC ESC ] 4 ; 2 ; rgb:... ESC \   -> nothing at all
#       DCS ESC P > | tmux 9.9.9 ESC \    -> a bare ESC, body eaten
#
#    So the pipe carries a CSI reply and not an OSC one, which is why the
#    colour path injects at all, and a DCS on the pipe is worse than lost
#    because a lone ESC is an Escape keypress to whatever reads the pane.  The
#    OSC pipe write is gone on Windows when a child pid was known, the
#    XTVERSION reply deliberately never gained one, and both now log the loss.
#
# The failure is driven by PSMUX_FAKE_INJECT_FAIL=1, which points every console
# attach at a process id that does not exist, so the attach fails for real and
# the whole failure path runs.  Documented in docs/diagnostics.md.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "i597inj"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor Cyan }

$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path

foreach ($v in 'PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') {
    Remove-Item "env:$v" -EA SilentlyContinue
}

$savedDataDir  = $env:PSMUX_DATA_DIR
$savedMouseDbg = $env:PSMUX_MOUSE_DEBUG
$savedFail     = $env:PSMUX_FAKE_INJECT_FAIL

$root = Join-Path $env:TEMP "psmux_i597_inject"
Remove-Item -Recurse -Force $root -EA SilentlyContinue
New-Item -ItemType Directory -Force $root | Out-Null

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$probe = Join-Path $root "query_probe_child.exe"
& $csc /nologo /optimize /out:$probe (Join-Path $repoTests "query_probe_child.cs") 2>&1 | Out-Null
if (-not (Test-Path $probe)) {
    Write-Host "FATAL: could not compile query_probe_child.cs" -ForegroundColor Red
    exit 1
}

# The Windows 10 inbox conhost (build 19045) hands psmux a query only when it
# next draws something, so there the probe writes a letter after each query
# (see query_probe_child.cs).
$paint = if ([Environment]::OSVersion.Version.Build -lt 22000) { " paint" } else { "" }

# Each arm gets its own data dir so the server is cold (PSMUX_MOUSE_DEBUG and
# PSMUX_FAKE_INJECT_FAIL are read by the SERVER, once, at first use) and so the
# mouse debug log belongs to that arm alone.
function Invoke-Arm([string]$name, [bool]$forceFail) {
    $dir = Join-Path $root $name
    New-Item -ItemType Directory -Force $dir | Out-Null
    $env:PSMUX_DATA_DIR = $dir
    $env:PSMUX_MOUSE_DEBUG = "1"
    if ($forceFail) { $env:PSMUX_FAKE_INJECT_FAIL = "1" }
    else { Remove-Item env:PSMUX_FAKE_INJECT_FAIL -EA SilentlyContinue }

    $log = Join-Path $dir "probe.txt"
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    & $PSMUX -L $NS new-session -d -s $name -x 100 -y 30 "$probe $log 900$paint" 2>&1 | Out-Null
    $deadline = (Get-Date).AddSeconds(90)
    while ((Get-Date) -lt $deadline -and -not (Test-Path $log)) { Start-Sleep -Milliseconds 300 }
    Start-Sleep -Milliseconds 500
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400

    $probeText = if (Test-Path $log) { Get-Content $log -Raw } else { "" }
    $mouseLog  = Join-Path $dir "mouse_debug.log"
    $logText   = if (Test-Path $mouseLog) { Get-Content $mouseLog -Raw } else { "" }
    Remove-Item env:PSMUX_FAKE_INJECT_FAIL -EA SilentlyContinue
    return @{ Probe = $probeText; Log = $logText }
}

function Get-Bytes($text, $name) {
    foreach ($line in ($text -split "`r?`n")) {
        if ($line.StartsWith("$name ") -and $line -match 'got=(\d+) bytes') { return [int]$Matches[1] }
    }
    return -1
}
function Get-Hex($text, $name) {
    foreach ($line in ($text -split "`r?`n")) {
        if ($line.StartsWith("$name ") -and $line -match 'hex=\[([^\]]*)\]') { return $Matches[1] }
    }
    return "<missing>"
}

$version = ((& $PSMUX -V) | Select-Object -First 1) -replace '^tmux\s+', ''
$version = $version.Trim()
$expectedHex = (@(0x1b, 0x50, 0x3e, 0x7c) + ([System.Text.Encoding]::ASCII.GetBytes("tmux $version")) + @(0x1b, 0x5c) |
    ForEach-Object { $_.ToString('x2') }) -join ' '

# ---------------------------------------------------------------------------
Write-Host "`n[Part 1] a healthy pane: every reply psmux owns is delivered" -ForegroundColor Yellow
$healthy = Invoke-Arm "healthy" $false
if (-not $healthy.Probe) {
    Write-Host "FATAL: the probe never wrote its log" -ForegroundColor Red
    $env:PSMUX_DATA_DIR = $savedDataDir; $env:PSMUX_MOUSE_DEBUG = $savedMouseDbg; $env:PSMUX_FAKE_INJECT_FAIL = $savedFail
    exit 1
}
Write-Host ($healthy.Probe.TrimEnd())

if ((Get-Hex $healthy.Probe "XTVERSION") -eq $expectedHex) {
    Write-Pass "XTVERSION reply is byte exact: [$expectedHex]"
} else {
    Write-Fail "XTVERSION reply [$(Get-Hex $healthy.Probe 'XTVERSION')] != expected [$expectedHex]"
}

$osc = Get-Bytes $healthy.Probe "OSC_COLOR1"
if ($osc -gt 0) { Write-Pass "the OSC 4 palette reply reached the pane ($osc bytes)" }
else { Write-Fail "the OSC 4 palette reply did not reach the pane (got=$osc)" }

$csi = Get-Bytes $healthy.Probe "CSI_SCHEME"
if ($csi -gt 0) { Write-Pass "the CSI light/dark reply reached the pane ($csi bytes)" }
else { Write-Fail "the CSI light/dark reply did not reach the pane (got=$csi)" }

if ($healthy.Log -match 'send_vt_response: pid=\d+ text_len=\d+ records=\d+ written=\d+ ok=true') {
    Write-Pass "the server logged a successful injection"
} else {
    Write-Fail "the server logged no successful injection (this is the line a reporter on a healthy box should see)"
}

if ($healthy.Log -match 'AttachConsole\(\d+\) FAILED') {
    Write-Fail "a healthy pane still produced an AttachConsole failure"
} else {
    Write-Pass "no AttachConsole failure on a healthy pane"
}

# ---------------------------------------------------------------------------
Write-Host "`n[Part 2] injection forced to fail: the reporter's box, reproduced" -ForegroundColor Yellow
$failed = Invoke-Arm "forcefail" $true
if (-not $failed.Probe) {
    Write-Host "FATAL: the probe never wrote its log under the fault seam" -ForegroundColor Red
    $env:PSMUX_DATA_DIR = $savedDataDir; $env:PSMUX_MOUSE_DEBUG = $savedMouseDbg; $env:PSMUX_FAKE_INJECT_FAIL = $savedFail
    exit 1
}
Write-Host ($failed.Probe.TrimEnd())

$xtv = Get-Bytes $failed.Probe "XTVERSION"
if ($xtv -eq 0) { Write-Pass "XTVERSION goes undelivered when injection fails, and nothing is written to the pipe in its place" }
else { Write-Fail "XTVERSION still returned $xtv bytes with injection forced to fail (the fault seam did not take)" }

$oscF = Get-Bytes $failed.Probe "OSC_COLOR1"
if ($oscF -eq 0) { Write-Pass "the OSC 4 reply goes undelivered when injection fails" }
else { Write-Fail "the OSC 4 reply still returned $oscF bytes with injection forced to fail (the fault seam did not take)" }

# The pipe itself is not what breaks: the CSI reply is a plain pipe write and
# it still lands.  This is what makes "ConPTY eats an OSC" a measurement rather
# than an assumption.
$csiF = Get-Bytes $failed.Probe "CSI_SCHEME"
if ([Environment]::OSVersion.Version.Build -lt 22000) {
    # The Windows 10 inbox conhost (build 19045) drops a CSI written to its
    # input pipe: measured by writing `ESC [ ? 997 ; 1 n`, `ESC [ 0 n`, DA2,
    # DECRPM and `CSI 6 ; 20 ; 10 t` straight into a pseudoconsole's input,
    # with a letter between each, under the flags 0, 0x4 and 0x6, and a child
    # in VT input mode read only the letters. So there the pipe carries no
    # CSI reply, and the one this arm leaves to it does not arrive.
    if ($csiF -eq 0) { Write-Pass "the CSI reply does not arrive: this host drops a CSI written to its input pipe" }
    else { Write-Fail "the CSI reply arrived on the pipe ($csiF bytes), which this host was measured to drop" }
} else {
    if ($csiF -gt 0) { Write-Pass "the CSI reply still arrives on the pipe ($csiF bytes), so the pipe is not the broken half" }
    else { Write-Fail "the CSI reply stopped arriving too (got=$csiF): the pipe write regressed" }
}

foreach ($field in @(
    'AttachConsole\(\d+\) FAILED err=\d+',
    'ERROR_INVALID_PARAMETER|ERROR_ACCESS_DENIED|ERROR_INVALID_HANDLE',
    'server pid=\d+',
    'console before FreeConsole:',
    'FreeConsole ok=(true|false) err=\d+',
    'console after:',
    'target alive=(true|false)',
    'integrity=0x[0-9A-F]+',
    'os build='
)) {
    if ($failed.Log -match $field) { Write-Pass "the failure report carries /$field/" }
    else { Write-Fail "the failure report is missing /$field/" }
}

if ($failed.Log -match 'XTVERSION reply LOST: \d+ bytes') {
    Write-Pass "the XTVERSION reply that could not be delivered is named as lost"
} else {
    Write-Fail "an XTVERSION reply was dropped with nothing in the log to say so"
}

if ($failed.Log -match 'OSC colour reply LOST: \d+ bytes') {
    Write-Pass "the OSC reply that could not be delivered is named as lost"
} else {
    Write-Fail "an OSC reply was dropped with nothing in the log to say so"
}

# ---------------------------------------------------------------------------
& $PSMUX -L $NS kill-server 2>&1 | Out-Null
$env:PSMUX_DATA_DIR = $savedDataDir
$env:PSMUX_MOUSE_DEBUG = $savedMouseDbg
$env:PSMUX_FAKE_INJECT_FAIL = $savedFail

Write-Host "`n================ SUMMARY ================" -ForegroundColor Yellow
Write-Host "  Passed: $script:TestsPassed" -ForegroundColor Green
Write-Host "  Failed: $script:TestsFailed" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
