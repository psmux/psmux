# Issue #802: `set prefix None` must disable the prefix, like tmux.
#
#   set -g prefix C-a
#   set prefix None
#   show -v prefix        # psmux 3.4.0 printed C-a, and C-a c still made a window
#
# tmux: None is a real key (key-string.c:254, KEYC_NONE), the prefix check in
# server_client_handle_key (server-client.c:1484) never matches it, and
# options_from_string (options.c:1305) rejects any other unparsable key with
# "bad key: <value>". show prints KEYC_NONE back as "None" (key-string.c:355).
#
# Part A drives the CLI. Part B and C attach a real client and type into it
# with tests/injector.cs (WriteConsoleInput); proof is the window count.
# Runs in its own -L namespace; never touches the default one.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
# A private data dir: source-file below writes config-warnings.log there, and
# the user's ~/.psmux must not be touched.
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $env:TEMP ("psmux_issue802_data_" + $PID)
    New-Item -ItemType Directory -Force -Path $env:PSMUX_DATA_DIR | Out-Null
}
$dataDir = $env:PSMUX_DATA_DIR
$TMP = Join-Path $env:TEMP "psmux_issue802"
New-Item -ItemType Directory -Force -Path $TMP | Out-Null
$SOCK = "i802_$PID"
$S = "p802"
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }
function Write-Ascii([string]$p, [string]$t) { [IO.File]::WriteAllText($p, $t, (New-Object System.Text.ASCIIEncoding)) }
function P { & $PSMUX -L $SOCK @args 2>&1 | Out-String | ForEach-Object { $_.Trim() } }
function Q([string]$fmt) { P display-message -p -t $S $fmt }
function Wins { [int](Q '#{session_windows}') }

Write-Host "binary:  $PSMUX" -ForegroundColor Cyan
Write-Host "socket:  $SOCK" -ForegroundColor Cyan

# ---------------------------------------------------------------- Part A
Write-Host "`n=== Part A: option values over the CLI ===" -ForegroundColor Cyan
$confA = Join-Path $TMP "a.conf"
Write-Ascii $confA "set -g @x 1`n"
& $PSMUX -L $SOCK -f $confA new-session -d -s $S -x 100 -y 30 2>&1 | Out-Null
Start-Sleep -Milliseconds 1500

$v = P show -gv prefix
if ($v -eq "C-b") { Write-Pass "default prefix is C-b" } else { Write-Fail "default prefix '$v'" }

P set -g prefix C-a | Out-Null
P set prefix None | Out-Null
$v = P show -v prefix
if ($v -ceq "None") { Write-Pass "set prefix None: show -v prefix prints None" } else { Write-Fail "set prefix None: show -v prefix = '$v' (expected None)" }

P set -g prefix C-a | Out-Null
P set -g prefix none | Out-Null
$v = P show -gv prefix
if ($v -ceq "None") { Write-Pass "set -g prefix none (lower case): show -gv prints None" } else { Write-Fail "set -g prefix none: show -gv = '$v'" }
$lst = P show -g
if ($lst -cmatch '(?m)^prefix\s+None\s*$') { Write-Pass "show -g lists 'prefix None'" } else { Write-Fail "show -g prefix line: $(($lst -split "`n" | Select-String '^prefix\s') -join ' | ')" }
$fmt = Q '#{prefix}'
if ($fmt -ceq "None") { Write-Pass "#{prefix} expands to None" } else { Write-Fail "#{prefix} = '$fmt'" }

P set -gu prefix | Out-Null
$v = P show -gv prefix
if ($v -eq "C-b") { Write-Pass "set -gu prefix restores C-b" } else { Write-Fail "set -gu prefix gave '$v'" }

P set -g prefix2 None | Out-Null
$v = P show -gv prefix2
if ($v -ceq "None") { Write-Pass "prefix2 None shows None" } else { Write-Fail "prefix2 None shows '$v'" }

# Unparsable keys are an error, and the value is left alone.
P set -g prefix C-a | Out-Null
$err = P set -g prefix NotAKey
$v = P show -gv prefix
if ($err -match 'bad key: NotAKey') { Write-Pass "set -g prefix NotAKey reports 'bad key: NotAKey'" } else { Write-Fail "set -g prefix NotAKey said '$err'" }
if ($v -eq "C-a") { Write-Pass "rejected prefix leaves C-a in place" } else { Write-Fail "prefix after rejected set: '$v'" }
P set -g prefix2 C-x | Out-Null
$err = P set -g prefix2 Bogus-Key
$v = P show -gv prefix2
if ($err -match 'bad key: Bogus-Key') { Write-Pass "set -g prefix2 Bogus-Key reports 'bad key: Bogus-Key'" } else { Write-Fail "set -g prefix2 Bogus-Key said '$err'" }
if ($v -eq "C-x") { Write-Pass "rejected prefix2 leaves C-x in place" } else { Write-Fail "prefix2 after rejected set: '$v'" }

# A config file with a bad prefix key reports it and keeps loading.
$confBad = Join-Path $TMP "bad.conf"
Write-Ascii $confBad "set -g prefix NoSuchKey`nset -g @after 1`n"
$warnLog = Join-Path $dataDir "config-warnings.log"
Remove-Item $warnLog -Force -EA SilentlyContinue
P source-file $confBad | Out-Null
$v = P show -gv prefix; $after = P show -gv '@after'
$warn = if (Test-Path $warnLog) { Get-Content $warnLog -Raw } else { "" }
if ($warn -match 'bad key: NoSuchKey') { Write-Pass "config-warnings.log records 'bad key: NoSuchKey'" } else { Write-Fail "config-warnings.log: '$warn'" }
if ($v -eq "C-a" -and $after -eq "1") { Write-Pass "bad config line left prefix C-a and the next line still ran" } else { Write-Fail "after bad config: prefix='$v' @after='$after'" }
$confNone = Join-Path $TMP "none.conf"
Write-Ascii $confNone "set -g prefix None`n"
P source-file $confNone | Out-Null
$v = P show -gv prefix
if ($v -ceq "None") { Write-Pass "config 'set -g prefix None' sets None" } else { Write-Fail "config prefix None gave '$v'" }
P kill-server | Out-Null
Start-Sleep -Milliseconds 800

# ---------------------------------------------------------------- Part B / C
$conf = Join-Path $TMP "kb.conf"
Write-Ascii $conf @"
set -g prefix C-a
set -g status-left "GL"
bind -T root F12 set prefix None \; set key-table off \; set status-left "OFF"
bind -T off  F12 set -u prefix \; set -u key-table \; set -u status-left
"@

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$inj = Join-Path $TMP "keys_802.exe"
Remove-Item $inj -Force -EA SilentlyContinue
& $csc /nologo /optimize /out:$inj (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
if (-not (Test-Path $inj)) { Write-Fail "could not compile tests/injector.cs"; exit 1 }

$launch = Join-Path $TMP "launch_802.cmd"
Write-Ascii $launch @"
@echo off
set PSMUX_SESSION=
set PSMUX_SESSION_NAME=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set PSMUX_NO_WARM=1
set PSMUX_DATA_DIR=$dataDir
"$PSMUX" -L $SOCK -f "$conf" new-session -s $S -x 120 -y 30 cmd
"@
$lp = Start-Process -FilePath $launch -PassThru
for ($i = 0; $i -lt 100; $i++) { if (Test-Path "$dataDir\${SOCK}__$S.port") { break }; Start-Sleep -Milliseconds 250 }
Start-Sleep -Seconds 3
$cpid = 0
for ($k = 0; $k -lt 12; $k++) {
    $cli = Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
        Where-Object { $_.CommandLine -match "$SOCK\b.*new-session -s\s+$S\b" } | Select-Object -First 1
    if ($cli) { $cpid = [int]$cli.ProcessId; break }
    Start-Sleep -Milliseconds 600
}
if ($cpid -eq 0) {
    Write-Fail "attached client did not start"
} else {
    Write-Info "attached client pid=$cpid"
    function Inject([string]$k, [int]$ms = 1200) { & $inj $cpid $k 2>&1 | Out-Null; Start-Sleep -Milliseconds $ms }

    Write-Host "`n=== Part B: an attached client obeys set prefix None ===" -ForegroundColor Cyan
    $w0 = Wins
    Inject "^a{SLEEP:300}c"
    $w1 = Wins
    if ($w1 -eq $w0 + 1) { Write-Pass "baseline: C-a c made a window ($w0 -> $w1)" } else { Write-Fail "baseline C-a c: windows $w0 -> $w1" }

    P set -t $S prefix None | Out-Null
    Start-Sleep -Milliseconds 800
    Inject "^a{SLEEP:300}c"
    $w2 = Wins
    if ($w2 -eq $w1) { Write-Pass "after set prefix None, C-a c made no window ($w2)" } else { Write-Fail "after set prefix None, C-a c still made a window ($w1 -> $w2)" }
    Inject "^b{SLEEP:300}c"
    $w3 = Wins
    if ($w3 -eq $w2) { Write-Pass "C-b c made no window either" } else { Write-Fail "C-b c made a window with prefix None ($w2 -> $w3)" }
    $kt = Q '#{client_key_table}'
    if ($kt -eq "root") { Write-Pass "client never entered the prefix table ($kt)" } else { Write-Fail "client key table '$kt'" }
    Inject "{ESC}" 300

    P set -t $S prefix C-a | Out-Null
    Start-Sleep -Milliseconds 800
    Inject "^a{SLEEP:300}c"
    $w4 = Wins
    if ($w4 -eq $w3 + 1) { Write-Pass "prefix back to C-a: C-a c made a window again" } else { Write-Fail "after restoring C-a: windows $w3 -> $w4" }

    Write-Host "`n=== Part C: nested toggle (F12) ===" -ForegroundColor Cyan
    P set -g prefix C-a | Out-Null
    Start-Sleep -Milliseconds 500
    Inject "{F12}"
    $pv = P show -v -t $S prefix; $kt = P show -v -t $S key-table; $ckt = Q '#{client_key_table}'
    if ($pv -ceq "None") { Write-Pass "F12 off: prefix None" } else { Write-Fail "F12 off: prefix '$pv'" }
    if ($kt -eq "off" -and $ckt -eq "off") { Write-Pass "F12 off: key-table off, client in off" } else { Write-Fail "F12 off: key-table '$kt' client '$ckt'" }
    $w5 = Wins
    Inject "^a{SLEEP:300}c"
    $w6 = Wins
    if ($w6 -eq $w5) { Write-Pass "outer level off: C-a c made no window ($w6)" } else { Write-Fail "outer level off but C-a c made a window ($w5 -> $w6)" }
    $ckt = Q '#{client_key_table}'
    if ($ckt -eq "off") { Write-Pass "client stayed in off after C-a c ($ckt)" } else { Write-Fail "client table after C-a c: '$ckt'" }
    Inject "{ESC}" 300
    Inject "{F12}"
    $pv = P show -v -t $S prefix; $ckt = Q '#{client_key_table}'
    if ($pv -ne "None" -and $pv -ne "") { Write-Pass "F12 on: prefix armed again ($pv)" } else { Write-Fail "F12 on: prefix '$pv'" }
    if ($ckt -eq "root") { Write-Pass "F12 on: client back in root" } else { Write-Fail "F12 on: client in '$ckt'" }
    $w7 = Wins
    $pk = if ($pv -eq "C-a") { "^a" } else { "^b" }
    Inject "$pk{SLEEP:300}c"
    $w8 = Wins
    if ($w8 -eq $w7 + 1) { Write-Pass "F12 on: $pv c made a window" } else { Write-Fail "F12 on: $pv c windows $w7 -> $w8" }

    try { Stop-Process -Id $cpid -Force -EA SilentlyContinue } catch {}
}
& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
if ($lp -and -not $lp.HasExited) { try { Stop-Process -Id $lp.Id -Force -EA SilentlyContinue } catch {} }

Write-Host "`n=== issue802 results: $script:Pass passed, $script:Fail failed ===" -ForegroundColor Cyan
if ($script:Fail -gt 0) { exit 1 }
exit 0
