# Issue #1111: display-popup closes as tmux's does: -EE only on exit status 0,
# no -E keeps the popup open, Escape and C-c close it, -k lets any key.
#
# tmux 3.8 (cmd-display-menu.c:676-690, popup.c:430-436 and 500-501):
#   - no -E: the popup stays open when its command exits; Escape or C-c closes
#     it, at once even while the command runs
#   - -E: it closes when the command exits; while the command runs, Escape and
#     C-c go to the command
#   - -EE: it closes only when the command exits with 0, else it stays open
#   - once the command has exited, Escape or C-c closes it, and with -k any key
# psmux closed every popup a command line opened when its command exited,
# unless psmux's -K was given, closed -EE ones on any status, read no -k, and
# let only q close a popup left open. A menu item's popup (run in the server
# itself) stayed open without -E but dropped -K. -K stays: it keeps the popup
# open even with -E.
# Each case below was measured against tmux 3.8 in WSL with the same flags
# (one client attached through script(1), keys on its input).
#
# Every case runs with a real client attached in a conhost window and keys
# written into its console input (tests/injector.cs). Each popup is opened by
# a command line in a background process, which returns when the popup closes,
# with the popup command's exit status (129 when closed before it exited).
#
# Safety: unique -L namespace, a private PSMUX_DATA_DIR when none is set, an
# empty config, cleanup only with `-L <ns> kill-server` and by recorded PID.

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red;   $script:TestsFailed++ }
function Write-Test($m) { Write-Host "`n[$m]" -ForegroundColor Cyan }

$PSMUX = $env:PSMUX_TEST_EXE
if (-not $PSMUX) { $PSMUX = (Get-Command psmux -EA SilentlyContinue).Source }
if (-not $PSMUX) { Write-Host "psmux not found"; exit 1 }

$work = Join-Path $env:TEMP "psmux_popclose_$PID"
New-Item -ItemType Directory -Force -Path $work | Out-Null
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $work "data"
    New-Item -ItemType Directory -Force -Path $env:PSMUX_DATA_DIR | Out-Null
}
$env:PSMUX_CONFIG_FILE = Join-Path $work "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""
# An attached client refuses to nest.
foreach ($v in 'PSMUX_SESSION','PSMUX_SESSION_NAME','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') {
    Remove-Item "env:$v" -EA SilentlyContinue
}
$NS = "popclose_$PID"
$S = "pc"
$started = New-Object System.Collections.Generic.List[int]

$injector = Join-Path $work "injector.exe"
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
& $csc /nologo /optimize /out:$injector (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
if (-not (Test-Path $injector)) { Write-Fail "tests\injector.cs did not compile"; exit 1 }

Write-Host "=== display-popup closes as tmux's does ===" -ForegroundColor White
Write-Host "psmux: $PSMUX  namespace: $NS  data: $env:PSMUX_DATA_DIR"

function P { & $PSMUX -L $NS @args 2>&1 }
function Get-PopupActive { return ((P dump-state -t $S | Out-String) -match '"popup_active":true') }
function Wait-PopupActive([bool]$Want, [int]$Ms = 5000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $Ms) {
        if ((Get-PopupActive) -eq $Want) { return $true }
        Start-Sleep -Milliseconds 100
    }
    return $false
}
# One display-popup command line in a background process.
function Start-Popup([string]$ArgLine) {
    $p = Start-Process -FilePath $PSMUX -ArgumentList "-L $NS display-popup -t $S $ArgLine" -WindowStyle Hidden -PassThru
    # Holding the handle keeps ExitCode readable after the process ends.
    $null = $p.Handle
    $started.Add($p.Id)
    return $p
}
function Send-Keys([string]$Keys) { & $injector $script:Client $Keys 2>&1 | Out-Null; Start-Sleep -Milliseconds 800 }
# After the keys: the popup should still be open and its command line waiting.
function Check-Open([string]$When, $p) {
    if ((Get-PopupActive) -and -not $p.HasExited) { Write-Pass "$When`: still open" }
    else { Write-Fail "$When`: closed (popup_active=$(Get-PopupActive), returned=$($p.HasExited))" }
}
# The popup should be closed and its command line returned with $Rc.
function Check-Closed([string]$When, $p, [int]$Rc) {
    $closed = Wait-PopupActive $false 3000
    $returned = $p.WaitForExit(3000)
    if ($closed -and $returned -and $p.ExitCode -eq $Rc) { Write-Pass "$When`: closed, exit code $Rc" }
    elseif ($closed -and $returned) { Write-Fail "$When`: closed, exit code $($p.ExitCode), expected $Rc" }
    else { Write-Fail "$When`: popup_active=$(-not $closed), returned=$returned" }
}
function Close-Leftover {
    if (Get-PopupActive) { P display-popup -t $S -C | Out-Null; Wait-PopupActive $false 2000 | Out-Null }
}
# Open a popup, wait for it and, when $AfterExit, for its command to end.
function Open-Popup([string]$ArgLine, [switch]$AfterExit) {
    Close-Leftover
    $p = Start-Popup $ArgLine
    if (-not (Wait-PopupActive $true)) { Write-Fail "popup did not open: $ArgLine"; return $null }
    if ($AfterExit) { Start-Sleep -Seconds 3 }
    return $p
}

P new-session -d -s $S -x 120 -y 30 | Out-Null
Start-Sleep -Seconds 2
P has-session -t $S | Out-Null
if ($LASTEXITCODE -ne 0) { Write-Fail "session $S did not start"; exit 1 }

$con = Start-Process conhost.exe -ArgumentList "`"$PSMUX`" -L $NS attach -t $S" -PassThru
$started.Add($con.Id)
$attached = $false
for ($i = 0; $i -lt 50; $i++) {
    Start-Sleep -Milliseconds 200
    if ((P display-message -t $S -p '#{session_attached}' | Out-String).Trim() -eq '1') { $attached = $true; break }
}
$script:Client = 0
foreach ($c in @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($con.Id)" -EA SilentlyContinue)) {
    $started.Add([int]$c.ProcessId)
    if ($c.Name -eq 'psmux.exe') { $script:Client = [int]$c.ProcessId }
}
if (-not $attached -or $script:Client -eq 0) {
    Write-Fail "no client attached to $S"
} else {
    Write-Pass "a client is attached to $S (pid $($script:Client))"

    Write-Test "1. -E, exit 3: closes when the command exits"
    $p = Open-Popup "-E `"Start-Sleep -Milliseconds 500; exit 3`""
    if ($p) { Check-Closed "after the exit" $p 3 }

    Write-Test "2. -EE, exit 0: closes when the command exits"
    $p = Open-Popup "-EE `"Start-Sleep -Milliseconds 500; exit 0`""
    if ($p) { Check-Closed "after the exit" $p 0 }

    Write-Test "3. -EE, exit 3: stays open; x does not close it, Escape does"
    $p = Open-Popup "-EE `"Start-Sleep -Milliseconds 500; exit 3`"" -AfterExit
    if ($p) {
        Check-Open "after the exit" $p
        Send-Keys "x"
        Check-Open "after x" $p
        Send-Keys "{ESC}"
        Check-Closed "after Escape" $p 3
    }

    Write-Test "4. No -E, exit 3: stays open; C-c closes it"
    $p = Open-Popup "`"Start-Sleep -Milliseconds 500; exit 3`"" -AfterExit
    if ($p) {
        Check-Open "after the exit" $p
        Send-Keys "^c"
        Check-Closed "after C-c" $p 3
    }

    Write-Test "5. No -E, the command still running: Escape closes it at once"
    $p = Open-Popup "`"Start-Sleep -Seconds 30`""
    if ($p) {
        Start-Sleep -Seconds 2
        Send-Keys "{ESC}"
        Check-Closed "after Escape" $p 129
    }

    Write-Test "6. -E, the command still running: Escape goes to it, the popup stays"
    $p = Open-Popup "-E `"Start-Sleep -Seconds 5`""
    if ($p) {
        Start-Sleep -Seconds 1
        Send-Keys "{ESC}"
        Check-Open "after Escape" $p
        Start-Sleep -Seconds 4
        Check-Closed "after the exit" $p 0
    }

    Write-Test "7. -E -k, exit 3: closes when the command exits"
    $p = Open-Popup "-E -k `"Start-Sleep -Milliseconds 500; exit 3`""
    if ($p) { Check-Closed "after the exit" $p 3 }

    Write-Test "8. -EE -k, exit 3: stays open; x closes it"
    $p = Open-Popup "-EE -k `"Start-Sleep -Milliseconds 500; exit 3`"" -AfterExit
    if ($p) {
        Check-Open "after the exit" $p
        Send-Keys "x"
        Check-Closed "after x" $p 3
    }

    Write-Test "9. No -E, exit 0: q closes it (psmux's own; tmux takes only Escape and C-c)"
    $p = Open-Popup "`"Start-Sleep -Milliseconds 500; exit 0`"" -AfterExit
    if ($p) {
        Check-Open "after the exit" $p
        Send-Keys "q"
        Check-Closed "after q" $p 0
    }

    Write-Test "10. psmux -K with -E, exit 3: stays open; Escape closes it"
    $p = Open-Popup "-K -E `"Start-Sleep -Milliseconds 500; exit 3`"" -AfterExit
    if ($p) {
        Check-Open "after the exit" $p
        Send-Keys "{ESC}"
        Check-Closed "after Escape" $p 3
    }

    Write-Test "11. A key binding's -EE popup, exit 3: stays open; Escape closes it"
    Close-Leftover
    P bind-key -n F5 display-popup -EE "Start-Sleep -Milliseconds 500; exit 3" | Out-Null
    Send-Keys "{F5}"
    if (Wait-PopupActive $true) {
        Start-Sleep -Seconds 3
        if (Get-PopupActive) { Write-Pass "after the exit: still open" } else { Write-Fail "after the exit: closed" }
        Send-Keys "{ESC}"
        if (Wait-PopupActive $false 3000) { Write-Pass "after Escape: closed" } else { Write-Fail "after Escape: still open" }
    } else { Write-Fail "F5 opened no popup" }

    # A menu item runs its command in the server itself, a path that read
    # only whether -E was given and dropped -K.
    Write-Test "12. A menu item's -EE popup, exit 3: stays open; Escape closes it"
    Close-Leftover
    P display-menu -t $S -T Menu "popup" g "display-popup -EE 'Start-Sleep -Milliseconds 500; exit 3'" | Out-Null
    Start-Sleep -Milliseconds 800
    Send-Keys "g"
    if (Wait-PopupActive $true) {
        Start-Sleep -Seconds 3
        if (Get-PopupActive) { Write-Pass "after the exit: still open" } else { Write-Fail "after the exit: closed" }
        Send-Keys "{ESC}"
        if (Wait-PopupActive $false 3000) { Write-Pass "after Escape: closed" } else { Write-Fail "after Escape: still open" }
    } else { Write-Fail "the menu item opened no popup" }

    Write-Test "13. A menu item's -K -E popup, exit 3: stays open"
    Close-Leftover
    P display-menu -t $S -T Menu "popup" g "display-popup -K -E 'Start-Sleep -Milliseconds 500; exit 3'" | Out-Null
    Start-Sleep -Milliseconds 800
    Send-Keys "g"
    if (Wait-PopupActive $true) {
        Start-Sleep -Seconds 3
        if (Get-PopupActive) { Write-Pass "after the exit: still open" } else { Write-Fail "after the exit: closed" }
        Close-Leftover
    } else { Write-Fail "the menu item opened no popup" }
}

# Cleanup: this namespace only, and the PIDs this suite started.
P kill-server | Out-Null
Start-Sleep -Milliseconds 800
foreach ($p in $started) { Stop-Process -Id $p -Force -EA SilentlyContinue }
Start-Sleep -Milliseconds 500
$left = @($started | Where-Object { Get-Process -Id $_ -EA SilentlyContinue })
if ($left.Count -eq 0) { Write-Pass "no window or process this suite started is left" } else { Write-Fail "left running: $($left -join ', ')" }
Remove-Item -Recurse -Force $work -EA SilentlyContinue

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed"
exit $script:TestsFailed
