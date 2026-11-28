# Issue #1111: display-popup run from a command line waits for the popup to
# close and exits with its command's status, as tmux's does.
#
# tmux returns CMD_RETURN_WAIT once the popup is open (cmd-display-menu.c:
# 696-703 at 3.8), and popup_free_cb hands the popup's status to the command
# client and continues it (popup.c:266-283). The status starts at 128 + SIGHUP
# (popup.c:609), so a popup closed before its command exits reports 129.
# psmux sent the command and returned at once with exit 0, so a script that
# uses a popup as a step (an editor, a picker) went on while it was open.
#
# Every case runs with a real client attached in a conhost window. Without
# a client the call still returns at once (nobody could close the popup),
# which the last case checks.
#
# Safety: unique -L namespace, a private PSMUX_DATA_DIR when none is set,
# cleanup only with `-L <ns> kill-server` and by recorded PID.

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red;   $script:TestsFailed++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Test($m) { Write-Host "`n[$m]" -ForegroundColor Cyan }

$PSMUX = $env:PSMUX_TEST_EXE
if (-not $PSMUX) { $PSMUX = (Get-Command psmux -EA SilentlyContinue).Source }
if (-not $PSMUX) { Write-Host "psmux not found"; exit 1 }

$work = Join-Path $env:TEMP "psmux_popwait_$PID"
New-Item -ItemType Directory -Force -Path $work | Out-Null
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $work "data"
    New-Item -ItemType Directory -Force -Path $env:PSMUX_DATA_DIR | Out-Null
}
# An attached client refuses to nest.
foreach ($v in 'PSMUX_SESSION','PSMUX_SESSION_NAME','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') {
    Remove-Item "env:$v" -EA SilentlyContinue
}
$NS = "popwait_$PID"
$S = "pw"
$started = New-Object System.Collections.Generic.List[int]

Write-Host "=== display-popup from a command line waits for the popup ===" -ForegroundColor White
Write-Host "psmux: $PSMUX  namespace: $NS  data: $env:PSMUX_DATA_DIR"

function Get-PopupActive {
    $raw = (& $PSMUX -L $NS dump-state -t $S 2>&1 | Out-String)
    return ($raw -match '"popup_active":true')
}
function Wait-PopupActive([bool]$Want, [int]$Ms = 5000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $Ms) {
        if ((Get-PopupActive) -eq $Want) { return $true }
        Start-Sleep -Milliseconds 100
    }
    return $false
}
# One display-popup call in the foreground: how long it took and its exit code.
function Invoke-Popup([string[]]$PopArgs) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    & $PSMUX -L $NS display-popup -t $S @PopArgs 2>&1 | Out-Null
    $rc = $LASTEXITCODE
    $sw.Stop()
    return [pscustomobject]@{ Ms = $sw.ElapsedMilliseconds; Rc = $rc }
}
# One display-popup call in a background process, to act on the popup while
# it waits.
function Start-Popup([string]$ArgLine) {
    $p = Start-Process -FilePath $PSMUX -ArgumentList "-L $NS display-popup -t $S $ArgLine" -WindowStyle Hidden -PassThru
    # Holding the handle keeps ExitCode readable after the process ends.
    $null = $p.Handle
    $started.Add($p.Id)
    return $p
}

& $PSMUX -L $NS new-session -d -s $S -x 120 -y 30 2>&1 | Out-Null
Start-Sleep -Seconds 2
& $PSMUX -L $NS has-session -t $S 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) { Write-Fail "session $S did not start"; exit 1 }

$con = Start-Process conhost.exe -ArgumentList "`"$PSMUX`" -L $NS attach -t $S" -PassThru
$started.Add($con.Id)
$attached = $false
for ($i = 0; $i -lt 50; $i++) {
    Start-Sleep -Milliseconds 200
    if ((& $PSMUX -L $NS display-message -t $S -p '#{session_attached}' 2>&1 | Out-String).Trim() -eq '1') { $attached = $true; break }
}
foreach ($c in @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($con.Id)" -EA SilentlyContinue)) { $started.Add([int]$c.ProcessId) }
if (-not $attached) {
    Write-Fail "no client attached to $S"
} else {
    Write-Pass "a client is attached to $S"

    Write-Test "1. -E: waits for the command and exits with its status"
    $r = Invoke-Popup @('-E', 'Start-Sleep -Seconds 3; exit 7')
    if ($r.Ms -ge 2500) { Write-Pass "returned after $($r.Ms) ms, once the 3 s command had ended" }
    else { Write-Fail "returned after $($r.Ms) ms, before the 3 s command ended" }
    if ($r.Rc -eq 7) { Write-Pass "exit code 7, the popup command's" } else { Write-Fail "exit code $($r.Rc), expected 7" }
    if (Wait-PopupActive $false 2000) { Write-Pass "the popup is closed when it returns" } else { Write-Fail "popup still open" }

    Write-Test "2. A command that ends well: exit 0 after the wait"
    $r = Invoke-Popup @('-E', 'Start-Sleep -Seconds 2')
    if ($r.Ms -ge 1500 -and $r.Rc -eq 0) { Write-Pass "returned after $($r.Ms) ms with exit code 0" }
    else { Write-Fail "returned after $($r.Ms) ms with exit code $($r.Rc), expected about 2 s and 0" }

    Write-Test "3. Closed with -C before the command ends: 129 (tmux's 128 + SIGHUP)"
    $p = Start-Popup "-E `"Start-Sleep -Seconds 20`""
    if (Wait-PopupActive $true) {
        Start-Sleep -Milliseconds 500
        if (-not $p.HasExited) { Write-Pass "still waiting while the popup is open" } else { Write-Fail "returned while the popup is open (exit $($p.ExitCode))" }
        $r = Invoke-Popup @('-C')
        if ($r.Ms -lt 3000 -and $r.Rc -eq 0) { Write-Pass "-C itself returns at once ($($r.Ms) ms, exit 0)" } else { Write-Fail "-C took $($r.Ms) ms, exit $($r.Rc)" }
        if ($p.WaitForExit(3000)) {
            if ($p.ExitCode -eq 129) { Write-Pass "the waiting call returned with 129" } else { Write-Fail "the waiting call returned with $($p.ExitCode), expected 129" }
        } else { Write-Fail "the waiting call did not return after -C" }
    } else { Write-Fail "popup did not open" }

    Write-Test "4. A popup opened over it: the first call returns, the second waits"
    $p = Start-Popup "-E `"Start-Sleep -Seconds 20`""
    if (Wait-PopupActive $true) {
        Start-Sleep -Milliseconds 500
        $q = Start-Popup "-E `"Start-Sleep -Seconds 2; exit 5`""
        if ($p.WaitForExit(3000)) {
            if ($p.ExitCode -eq 129) { Write-Pass "the first call returned with 129 when its popup was replaced" } else { Write-Fail "the first call returned with $($p.ExitCode), expected 129" }
        } else { Write-Fail "the first call did not return when its popup was replaced" }
        if ($q.WaitForExit(8000)) {
            if ($q.ExitCode -eq 5) { Write-Pass "the second call returned with its own status 5" } else { Write-Fail "the second call returned with $($q.ExitCode), expected 5" }
        } else { Write-Fail "the second call did not return" }
    } else { Write-Fail "popup did not open" }

    Write-Test "5. -K keeps the popup open: the status is kept until it closes"
    $p = Start-Popup "-K `"exit 3`""
    if (Wait-PopupActive $true) {
        Start-Sleep -Seconds 2
        if (-not $p.HasExited) { Write-Pass "still waiting while the -K popup stays open after its command" } else { Write-Fail "returned while the popup is open (exit $($p.ExitCode))" }
        & $PSMUX -L $NS display-popup -t $S -C 2>&1 | Out-Null
        if ($p.WaitForExit(3000)) {
            if ($p.ExitCode -eq 3) { Write-Pass "returned with the command's status 3 when closed" } else { Write-Fail "returned with $($p.ExitCode), expected 3" }
        } else { Write-Fail "did not return after -C" }
    } else { Write-Fail "popup did not open" }
}

Write-Test "5b. Two sessions, two waiting calls: each settles on its own"
# Each session has its own server and its own waiters: the client of one
# detaching, or -C on one, must not answer the call waiting on the other.
$S2 = "pw2"
if ($attached) {
    if (Get-PopupActive) { & $PSMUX -L $NS display-popup -t $S -C 2>&1 | Out-Null; Wait-PopupActive $false 2000 | Out-Null }
    & $PSMUX -L $NS new-session -d -s $S2 -x 120 -y 30 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    $con2 = Start-Process conhost.exe -ArgumentList "`"$PSMUX`" -L $NS attach -t $S2" -PassThru
    $started.Add($con2.Id)
    $att2 = $false
    for ($i = 0; $i -lt 50; $i++) {
        Start-Sleep -Milliseconds 200
        if ((& $PSMUX -L $NS display-message -t $S2 -p '#{session_attached}' 2>&1 | Out-String).Trim() -eq '1') { $att2 = $true; break }
    }
    $kids2 = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($con2.Id)" -EA SilentlyContinue | ForEach-Object { [int]$_.ProcessId })
    foreach ($k in $kids2) { $started.Add($k) }
    if (-not $att2) { Write-Fail "no client attached to $S2" } else {
        $a = Start-Popup "-E `"Start-Sleep -Seconds 30`""
        $b = Start-Process -FilePath $PSMUX -ArgumentList "-L $NS display-popup -t $S2 -E `"Start-Sleep -Seconds 30`"" -WindowStyle Hidden -PassThru
        $null = $b.Handle; $started.Add($b.Id)
        $open2 = $false
        for ($i = 0; $i -lt 50; $i++) {
            Start-Sleep -Milliseconds 100
            if ((& $PSMUX -L $NS dump-state -t $S2 2>&1 | Out-String) -match '"popup_active":true') { $open2 = $true; break }
        }
        if ((Wait-PopupActive $true) -and $open2) {
            Start-Sleep -Milliseconds 500
            # The client of the second session detaches: only its call returns.
            foreach ($k in $kids2) { Stop-Process -Id $k -Force -EA SilentlyContinue }
            Stop-Process -Id $con2.Id -Force -EA SilentlyContinue
            if ($b.WaitForExit(4000) -and $b.ExitCode -eq 129) { Write-Pass "$S2's call returned with 129 when its client detached" } else { Write-Fail "$S2's call: returned=$($b.HasExited) exit=$(if ($b.HasExited) { $b.ExitCode })" }
            Start-Sleep -Milliseconds 500
            if (-not $a.HasExited) { Write-Pass "$S's call still waits, its client is still there" } else { Write-Fail "$S's call returned with $($a.ExitCode) when the other session's client detached" }
            # -C on the first session: only its call returns.
            & $PSMUX -L $NS display-popup -t $S -C 2>&1 | Out-Null
            if ($a.WaitForExit(3000) -and $a.ExitCode -eq 129) { Write-Pass "$S's call returned with 129 after its own -C" } else { Write-Fail "$S's call: returned=$($a.HasExited) exit=$(if ($a.HasExited) { $a.ExitCode })" }
        } else { Write-Fail "the two popups did not open" }
    }
    & $PSMUX -L $NS kill-session -t $S2 2>&1 | Out-Null
}

Write-Test "6. The client detaches while a call waits: it returns with 129, the popup stays"
# tmux frees the popup with its client, which answers the waiting command with
# 128 + SIGHUP. psmux keeps the popup for the next attach, but the call must not
# wait on a popup nobody attached could close.
$w = $null
if ($attached) {
    if (Get-PopupActive) { & $PSMUX -L $NS display-popup -t $S -C 2>&1 | Out-Null; Wait-PopupActive $false 2000 | Out-Null }
    $w = Start-Popup "-E `"Start-Sleep -Seconds 30`""
    if (Wait-PopupActive $true) {
        Start-Sleep -Milliseconds 500
        if (-not $w.HasExited) { Write-Pass "waiting while the client shows the popup" } else { Write-Fail "returned while the client shows the popup (exit $($w.ExitCode))" }
    } else { Write-Fail "popup did not open"; $w = $null }
}
foreach ($c in @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($con.Id)" -EA SilentlyContinue)) { Stop-Process -Id $c.ProcessId -Force -EA SilentlyContinue }
Stop-Process -Id $con.Id -Force -EA SilentlyContinue
$detached = $false
for ($i = 0; $i -lt 50; $i++) {
    Start-Sleep -Milliseconds 200
    if ((& $PSMUX -L $NS display-message -t $S -p '#{session_attached}' 2>&1 | Out-String).Trim() -eq '0') { $detached = $true; break }
}
if ($w) {
    if ($w.WaitForExit(3000)) {
        if ($w.ExitCode -eq 129) { Write-Pass "the waiting call returned with 129 when the client detached" } else { Write-Fail "the waiting call returned with $($w.ExitCode), expected 129" }
    } else { Write-Fail "the waiting call still waits after the client detached" }
    if (Get-PopupActive) { Write-Pass "the popup stays on the session for the next attach" } else { Write-Fail "the popup went away with the client" }
    & $PSMUX -L $NS display-popup -t $S -C 2>&1 | Out-Null
    Wait-PopupActive $false 2000 | Out-Null
}

Write-Test "7. No client attached: returns at once, as before"
if ($detached) {
    $r = Invoke-Popup @('-E', 'Start-Sleep -Seconds 3; exit 7')
    if ($r.Ms -lt 2000 -and $r.Rc -eq 0) { Write-Pass "returned after $($r.Ms) ms with exit code 0" }
    else { Write-Fail "returned after $($r.Ms) ms with exit code $($r.Rc), expected at once and 0" }
    if (Wait-PopupActive $true 1000) { Write-Pass "the popup is open on the detached session" } else { Write-Fail "popup did not open" }
} else { Write-Fail "the client did not detach" }

# Cleanup: this namespace only, and the PIDs this suite started.
& $PSMUX -L $NS kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 800
foreach ($p in $started) { Stop-Process -Id $p -Force -EA SilentlyContinue }
Start-Sleep -Milliseconds 500
$left = @($started | Where-Object { Get-Process -Id $_ -EA SilentlyContinue })
if ($left.Count -eq 0) { Write-Pass "no window or process this suite started is left" } else { Write-Fail "left running: $($left -join ', ')" }
Remove-Item -Recurse -Force $work -EA SilentlyContinue

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed  Skipped: $script:TestsSkipped"
exit $script:TestsFailed
