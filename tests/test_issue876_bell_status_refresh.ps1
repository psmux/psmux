# Issue #876: a bell, activity or silence flag on a window that is not the
# current one shows in the status line of an attached client at once, as in
# tmux: alerts_check_bell, alerts_check_activity and alerts_check_silence
# call server_status_session when they set the flag (alerts.c:202-204, tag
# 3.7c).
#
# Measured in tmux 3.7c through a nested client, one second after each step
# with no other input:
#
#   printf "\a" in w1 (client on w0)            [i] 0:w0* 1:w1! 2:w2
#   printf "\a" in w0, the current window       [i] 0:w0* 1:w1! 2:w2
#   monitor-activity on, echo hi in w2          [i] 0:w0* 1:w1! 2:w2#
#
# psmux set the flag on the server at once (list-windows showed it), but the
# tabs a client draws come from a cached window list that was rebuilt only on
# some other change, such as a window switch or refresh-client, so the
# status line kept the old tab.
#
# GROUND TRUTH: the status line of the attached client's console
# (tests\conread.cs), read while nothing else is sent to the server, and
# #{window_flags} from list-windows afterwards.
#
# Set PSMUX_TEST_EXE (or PSMUX_TEST_BIN) to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } elseif ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$script:TestsPassed = 0; $script:TestsFailed = 0
$script:Opened = @()

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

$env:PSMUX_SESSION_NAME = $null
$env:PSMUX_SESSION      = $null
$env:PSMUX_PANE         = $null
$env:TMUX               = $null
$env:TMUX_PANE          = $null

$NS   = "bs-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$SESS = "bs"
$TMP  = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_bs_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null

function P { & $PSMUX -L $NS @args 2>&1 }

$csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue |
           Select-Object -First 1 -ExpandProperty FullName
}
$RD = Join-Path $TMP "conread.exe"
if ($csc -and (Test-Path $csc)) {
    & $csc /nologo /optimize /out:$RD (Join-Path $PSScriptRoot "conread.cs") 2>&1 | Out-Null
}
if (-not (Test-Path $RD)) {
    Write-Fail "could not build tests\conread.cs (csc.exe unavailable)"
    Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
    exit 1
}

function Stop-Opened {
    foreach ($id in $script:Opened) { try { Stop-Process -Id $id -Force -EA SilentlyContinue } catch {} }
    $script:Opened = @()
}

function Kill-Rig {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    Stop-Opened
    Get-ChildItem "$psmuxDir\${NS}__*" -EA SilentlyContinue | Remove-Item -Force -EA SilentlyContinue
}

$CONF = Join-Path $TMP "bs.conf"
$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
@(
    "set -g default-shell $POWERSHELL",
    "set -g base-index 0",
    "set -g status-left ''",
    "set -g status-right ''"
) | Set-Content -Path $CONF -Encoding ASCII

function Start-Attached {
    $p = Start-Process -FilePath $PSMUX `
        -ArgumentList "-f",$CONF,"-L",$NS,"new-session","-s",$SESS,"-n","w0","-x","100","-y","30" -PassThru
    $script:Opened += $p.Id
    $portFile = Join-Path $psmuxDir "${NS}__${SESS}.port"
    for ($i = 0; $i -lt 80; $i++) {
        Start-Sleep -Milliseconds 250
        if (Test-Path $portFile) {
            $port = (Get-Content $portFile -Raw).Trim()
            try {
                $t = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port); $t.Close()
                Start-Sleep -Milliseconds 2500
                return $p
            } catch {}
        }
    }
    return $null
}

function Screen {
    $o = Join-Path $TMP "screen.txt"
    Start-Process -FilePath $RD -ArgumentList "$script:cpid" -Wait -WindowStyle Hidden -RedirectStandardOutput $o | Out-Null
    if (Test-Path $o) { return @(Get-Content $o) }
    return @()
}

# The flags drawn after window $name in the status line, which is the last
# row of the client's console holding "<index>:<name>". $null when the tab
# is not on the screen.
function Tab-Flags($name) {
    $screen = Screen
    for ($i = $screen.Count - 1; $i -ge 0; $i--) {
        $line = ([string]$screen[$i]) -replace '[^ -~]', ' '
        if ($line -match "\d+:$name([*\-#!~MZ]*)") { return $Matches[1] }
    }
    return $null
}

# Read the status line until window $name's tab carries $flag, up to
# $timeoutMs. Only the client's console is read, so nothing here reaches the
# server and makes it rebuild the window list.
function Wait-TabFlag($name, $flag, $timeoutMs) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $flags = $null
    while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
        $flags = Tab-Flags $name
        if ($flags -ne $null -and $flags.Contains($flag)) { return @($true, $flags, $sw.ElapsedMilliseconds) }
        Start-Sleep -Milliseconds 250
    }
    return @($false, $flags, $sw.ElapsedMilliseconds)
}

function Window-Flags($name) {
    foreach ($l in (P list-windows -t $SESS -F '#{window_name} #{window_flags}')) {
        $parts = ([string]$l).Trim() -split ' ', 2
        if ($parts[0] -eq $name) { if ($parts.Count -gt 1) { return $parts[1] } else { return "" } }
    }
    return $null
}

# -- Rig --

Kill-Rig
$client = Start-Attached
if (-not $client) {
    Write-Fail "the attached client did not come up"
    Kill-Rig
    Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
    exit 1
}
$script:cpid = $client.Id
foreach ($w in "w1", "w2", "w3") { P new-window -d -t $SESS -n $w | Out-Null }
Start-Sleep -Seconds 3

$flags = Tab-Flags "w1"
Check ($flags -ne $null -and -not $flags.Contains('!')) "w1's tab is drawn with no flag: [$flags]" "w1's tab before the bell: [$flags]"

# -- 1. bell --

Write-Head "1. a bell in w1 while the client is on w0"
P send-keys -t "${SESS}:w1" 'Write-Host -NoNewline ([char]7)' Enter | Out-Null
$r = Wait-TabFlag "w1" "!" 4000
Check $r[0] "the status line shows w1! after $($r[2]) ms" "the status line still shows w1 with [$($r[1])] after $($r[2]) ms"
$wf = Window-Flags "w1"
Check ($wf -like '*!*') "list-windows gives w1 the flags [$wf]" "list-windows gives w1 the flags [$wf]"

# -- 2. activity --

Write-Head "2. output in w2 with monitor-activity on"
P set-option -g monitor-activity on | Out-Null
Start-Sleep -Milliseconds 1500
P send-keys -t "${SESS}:w2" 'echo hi' Enter | Out-Null
$r = Wait-TabFlag "w2" "#" 4000
Check $r[0] "the status line shows w2# after $($r[2]) ms" "the status line still shows w2 with [$($r[1])] after $($r[2]) ms"
$wf = Window-Flags "w2"
Check ($wf -like '*#*') "list-windows gives w2 the flags [$wf]" "list-windows gives w2 the flags [$wf]"

# -- 3. silence --

Write-Head "3. w3 silent for three seconds with monitor-silence 3"
# w3 prints first, so the flag is not already due when set-option rebuilds
# the window list; it comes later, from the one second alert check alone.
P set-option -g monitor-activity off | Out-Null
P send-keys -t "${SESS}:w3" 'echo quiet' Enter | Out-Null
Start-Sleep -Milliseconds 1200
P set-option -w -t "${SESS}:w3" monitor-silence 3 | Out-Null
$flags = Tab-Flags "w3"
Check ($flags -ne $null -and -not $flags.Contains('~')) "w3's tab has no ~ yet: [$flags]" "w3's tab already reads [$flags]"
$r = Wait-TabFlag "w3" "~" 7000
Check $r[0] "the status line shows w3~ after $($r[2]) ms" "the status line still shows w3 with [$($r[1])] after $($r[2]) ms"
$wf = Window-Flags "w3"
Check ($wf -like '*~*') "list-windows gives w3 the flags [$wf]" "list-windows gives w3 the flags [$wf]"

# -- 4. the current window --

Write-Head "4. a bell in w0, the current window, sets no flag"
P send-keys -t "${SESS}:w0" 'Write-Host -NoNewline ([char]7)' Enter | Out-Null
$r = Wait-TabFlag "w0" "!" 2500
Check (-not $r[0]) "w0's tab has no ! after $($r[2]) ms: [$($r[1])]" "w0's tab shows [$($r[1])]"

# -- 5. selecting the window clears its flag --

Write-Head "5. selecting w1 clears its bell flag"
P select-window -t "${SESS}:w1" | Out-Null
Start-Sleep -Milliseconds 1500
$flags = Tab-Flags "w1"
Check ($flags -ne $null -and $flags.Contains('*') -and -not $flags.Contains('!')) "w1's tab reads [$flags]" "w1's tab reads [$flags]"

# -- Done --

Kill-Rig
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue

Write-Host ""
Write-Host "=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
