# Issue #708: respawn-pane threw the pane's history away and dropped -e.
#
# OpenRig hands a pane over to a successor agent with
#   respawn-pane -t %id -e IDENTITY=... -- <agent>
# and needs the successor to (a) see the predecessor's scrollback and (b) see
# the -e variables.
#
# Measured on master 39ac085, 5 loops each, 120x30 pane:
#   history kept after respawn-pane (no -k), explicit command   0/5
#   respawn-pane -k -e OPENRIG_TEST=hello -e SECOND=two         0/5
#     child printed ENV=[] []
#   new-window -e (control, same helper)                        5/5
#
# Real tmux 3.4 (WSL), same scenario:
#   history_size 33 before respawn, 33 after, MARKER_A still in capture -S -3000
#   visible rows at death (fill33..fill60, "Pane is dead") are gone
#   cursor back at 0,0; alternate screen left; copy mode left
#   -e applies to that process only (a later respawn without -e does not see it)
#   -k on a live shell keeps the scrolled history, drops the visible rows
#
# tmux source: spawn.c spawn_pane (SPAWN_RESPAWN) keeps the window_pane and
# calls window_pane_reset_mode_all + screen_reinit (screen.c), which runs
# grid_clear_lines(hsize, sy): visible rows cleared, history kept. The child
# environment is environ_for_session then environ_copy(sc->environ, child).
#
# Set PSMUX_TEST_BIN to test a binary that is not first on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$script:TestsPassed = 0; $script:TestsFailed = 0
$script:Opened = @()

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

$env:PSMUX_SESSION_NAME = $null
$env:PSMUX_SESSION      = $null
$env:PSMUX_PANE         = $null
$env:TMUX               = $null
$env:TMUX_PANE          = $null
$env:OPENRIG_TEST       = $null
$env:SECOND             = $null

$NS   = "rs708_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$SESS = "s708"
$TMP  = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_i708_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null

function P { & $PSMUX -L $NS @args 2>&1 }
function Run-P {
    param([string[]]$PArgs)
    $out = & $PSMUX -L $NS @PArgs 2>&1 | Out-String
    return [pscustomobject]@{ Code = $LASTEXITCODE; Out = $out.Trim() }
}

# ---- helper programs the panes run -----------------------------------------
# mark.cmd waits a moment (so the pane scope remain-on-exit can be set, the way
# the reporter does it), prints a marker and then enough lines to push the
# marker into history, and exits.
$MARK = Join-Path $TMP "mark.cmd"
Set-Content -Path $MARK -Encoding ASCII -Value @(
    '@echo off',
    'ping -n 2 127.0.0.1 >nul',
    'echo MARKER_%1',
    'for /L %%n in (1,1,60) do @echo fill%%n'
)
$ENVC = Join-Path $TMP "envecho.cmd"
Set-Content -Path $ENVC -Encoding ASCII -Value @(
    '@echo off',
    'echo ENV=[%OPENRIG_TEST%] [%SECOND%] CWD=[%CD%]'
)
$ALT = Join-Path $TMP "alt.ps1"
Set-Content -Path $ALT -Encoding ASCII -Value @(
    '1..40 | ForEach-Object { "pre$_" }',
    '[Console]::Out.Write([char]27 + "[?1049h")',
    '"ALTSCREEN"',
    'Start-Sleep -Milliseconds 1500'
)

# The attached client's console screen is read with tests\conread.cs (the
# same reader the #702 suite uses), so the TUI check sees what a user sees.
$csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue |
           Select-Object -First 1 -ExpandProperty FullName
}
$RD = Join-Path $TMP "conread.exe"
if ($csc -and (Test-Path $csc)) {
    & $csc /nologo /optimize /out:$RD (Join-Path $PSScriptRoot "conread.cs") 2>&1 | Out-Null
}
function Get-ClientScreen($procId) {
    if (-not (Test-Path $RD)) { return @() }
    $o = Join-Path $TMP "screen.txt"; $e = Join-Path $TMP "screen_err.txt"
    Start-Process -FilePath $RD -ArgumentList "$procId" -Wait -WindowStyle Hidden `
        -RedirectStandardOutput $o -RedirectStandardError $e | Out-Null
    if (Test-Path $o) { return @(Get-Content $o) }
    return @()
}

function Cap($t)      { (P capture-pane -p -S -3000 -t $t) -join "`n" }
function CapVis($t)   { (P capture-pane -p -t $t) -join "`n" }
function Field($t, $f) { ((P display-message -p -t $t $f) -join '').Trim() }

function Wait-Match($t, $pat, $ms = 15000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) {
        if ((Cap $t) -match $pat) { return $true }
        Start-Sleep -Milliseconds 150
    }
    return $false
}
function Wait-Dead($t, $ms = 15000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) {
        if ((Field $t '#{pane_dead}') -eq '1') { return $true }
        Start-Sleep -Milliseconds 150
    }
    return $false
}
# A new window running $cmd, with remain-on-exit set on the PANE (the reporter's
# setup). Returns the pane id.
function New-Pane($cmd) {
    $id = ((P new-window -d -P -F '#{pane_id}' -t "${SESS}:" $cmd) -join '').Trim()
    P set-option -p -t $id remain-on-exit on | Out-Null
    return $id
}

# The default namespace is judged on which sessions exist, not on the whole
# `ls` line: a live session of the person running this may gain a window or
# an attached client while the suite runs, and that is not this suite's doing.
function Get-DefaultSessionNames {
    $names = @(& $PSMUX ls -F '#{session_name}' 2>$null | Where-Object { $_ -and $_ -notmatch '^psmux:' } | Sort-Object)
    return ($names -join ',')
}
$before = Get-DefaultSessionNames

try {
    P new-session -d -s $SESS -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 1500
    if ((Run-P @('has-session', '-t', $SESS)).Code -ne 0) { Write-Fail "session did not start"; throw "no session" }

    # ---- 1. history survives respawn-pane (no -k) of a dead pane -------------
    Write-Head "1. dead pane, explicit command, respawn-pane without -k"
    $id = New-Pane "$MARK A"
    if (-not (Wait-Dead $id)) { Write-Fail "pane $id never died" }
    $preCap = Cap $id
    $preHist = [int](Field $id '#{history_size}')
    Write-Info "before respawn: history_size=$preHist, MARKER_A in capture: $($preCap -match 'MARKER_A')"
    $pidBefore = Field $id '#{pane_pid}'
    $r = Run-P @('respawn-pane', '-t', $id, "$MARK R")
    Write-Info "respawn rc=$($r.Code) out=[$($r.Out)]"
    $null = Wait-Match $id 'MARKER_R'
    $null = Wait-Dead $id
    $post = Cap $id
    $vis = CapVis $id
    if ($post -match '(?m)^MARKER_A$') { Write-Pass "MARKER_A (in history at death) is still in capture-pane -S -3000" }
    else { Write-Fail "MARKER_A was discarded by respawn-pane; capture starts: $((($post -split "`n") | Select-Object -First 2) -join ' | ')" }
    if ($post.IndexOf('MARKER_A') -ge 0 -and $post.IndexOf('MARKER_R') -gt $post.IndexOf('MARKER_A')) {
        Write-Pass "the new process's output follows the kept history"
    } else { Write-Fail "old history does not precede the new output" }
    # tmux clears the rows that were VISIBLE at death (grid_clear_lines) rather
    # than pushing them into history: fill60 was on screen, so it is gone.
    $fill60 = ([regex]::Matches($post, '(?m)^fill60$')).Count
    if ($fill60 -eq 1) { Write-Pass "rows visible at death were cleared, not pushed to history (fill60 once, from the new run)" }
    else { Write-Fail "fill60 appears $fill60 times; expected once (tmux clears the visible rows)" }
    if ($vis -notmatch 'MARKER_A') { Write-Pass "visible screen does not show the old marker" } else { Write-Fail "old marker still visible" }
    if ((Field $id '#{pane_id}') -eq $id) { Write-Pass "pane id unchanged ($id)" } else { Write-Fail "pane id changed" }

    # history_size right after a respawn equals what it was at death
    $id1b = New-Pane "$MARK H"
    $null = Wait-Dead $id1b
    $h0 = [int](Field $id1b '#{history_size}')
    P respawn-pane -t $id1b "cmd /c ping -n 30 127.0.0.1 >nul" | Out-Null
    Start-Sleep -Milliseconds 700
    $h1 = [int](Field $id1b '#{history_size}')
    $cur = Field $id1b '#{cursor_x},#{cursor_y}'
    Write-Info "history_size at death=$h0, right after respawn=$h1, cursor=$cur"
    if ($h0 -gt 0 -and $h1 -eq $h0) { Write-Pass "history_size kept across respawn ($h0 -> $h1), like tmux (33 -> 33)" }
    else { Write-Fail "history_size $h0 -> $h1 across respawn" }
    if ($cur -eq '0,0') { Write-Pass "cursor homed by the respawn (tmux screen_reinit)" } else { Write-Info "cursor after respawn is $cur (child may have written)" }
    P respawn-pane -k -t $id1b "cmd /c exit" | Out-Null

    # ---- 2. -e reaches the respawned process --------------------------------
    Write-Head "2. respawn-pane -e"
    $r = Run-P @('respawn-pane', '-k', '-t', $id, '-e', 'OPENRIG_TEST=hello', '--', $ENVC)
    if ($r.Code -eq 0) { Write-Pass "respawn-pane -k -e exits 0" } else { Write-Fail "rc=$($r.Code) [$($r.Out)]" }
    if (Wait-Match $id 'ENV=\[hello\]' 8000) { Write-Pass "-e OPENRIG_TEST=hello is visible in the child" }
    else { Write-Fail "child did not see -e: $(((Cap $id) -split "`n" | Select-String 'ENV=') -join ' | ')" }

    P respawn-pane -k -t $id -e OPENRIG_TEST=one -e SECOND=two -- $ENVC | Out-Null
    if (Wait-Match $id 'ENV=\[one\] \[two\]' 8000) { Write-Pass "two -e flags both reach the child" }
    else { Write-Fail "multiple -e: $(((Cap $id) -split "`n" | Select-String 'ENV=') -join ' | ')" }

    $winDir = $env:SystemRoot
    P respawn-pane -k -t $id -c $winDir -e OPENRIG_TEST=withc -- $ENVC | Out-Null
    if (Wait-Match $id ('ENV=\[withc\] \[\] CWD=\[' + [regex]::Escape($winDir) + '\]') 8000) { Write-Pass "-e together with -c: both applied" }
    else { Write-Fail "-e with -c: $(((Cap $id) -split "`n" | Select-String 'ENV=') -join ' | ')" }

    # -e without -k on a DEAD pane (the handover shape), positional command form
    $null = Wait-Dead $id
    P respawn-pane -t $id -e OPENRIG_TEST=dead "$ENVC" | Out-Null
    if (Wait-Match $id 'ENV=\[dead\]' 8000) { Write-Pass "-e on a dead pane with a positional command" }
    else { Write-Fail "-e on dead pane: $(((Cap $id) -split "`n" | Select-String 'ENV=') -join ' | ')" }

    # tmux: -e is for that process only. A later respawn without -e does not see it.
    $null = Wait-Dead $id
    P respawn-pane -t $id -- $ENVC | Out-Null
    Start-Sleep -Milliseconds 1500
    $lastEnv = ((Cap $id) -split "`n" | Select-String 'ENV=' | Select-Object -Last 1).ToString()
    if ($lastEnv -match 'ENV=\[\] \[\]') { Write-Pass "-e does not stick to later respawns (tmux: ENV2=[])" }
    else { Write-Fail "later respawn saw: $lastEnv" }

    # -e overrides the session environment (tmux: environ_copy after environ_for_session)
    P set-environment -t $SESS OPENRIG_TEST fromsession | Out-Null
    $null = Wait-Dead $id
    P respawn-pane -t $id -e OPENRIG_TEST=override -- $ENVC | Out-Null
    if (Wait-Match $id 'ENV=\[override\]' 8000) { Write-Pass "-e overrides set-environment" }
    else { Write-Fail "override: $(((Cap $id) -split "`n" | Select-String 'ENV=' | Select-Object -Last 1))" }
    P set-environment -u -t $SESS OPENRIG_TEST | Out-Null

    # respawn-window takes -e too
    $null = Wait-Dead $id
    $wid = Field $id '#{window_id}'
    $r = Run-P @('respawn-window', '-k', '-t', $wid, '-e', 'OPENRIG_TEST=window', $ENVC)
    if (Wait-Match $id 'ENV=\[window\]' 8000) { Write-Pass "respawn-window -e reaches the child" }
    else { Write-Fail "respawn-window -e: rc=$($r.Code) $(((Cap $id) -split "`n" | Select-String 'ENV=' | Select-Object -Last 1))" }

    # ---- 3. -k on a live shell keeps the scrolled history --------------------
    Write-Head "3. respawn-pane -k on a live shell pane"
    $sid = ((P new-window -d -P -F '#{pane_id}' -t "${SESS}:") -join '').Trim()
    Start-Sleep -Milliseconds 2500
    P send-keys -t $sid 'echo LIVEHIST; 1..50 | % { "f$_" }' Enter | Out-Null
    $null = Wait-Match $sid '(?m)^f50'
    $refuse = Run-P @('respawn-pane', '-t', $sid)
    if ($refuse.Code -ne 0 -and $refuse.Out -match 'still active') { Write-Pass "live pane without -k still refused: $($refuse.Out)" }
    else { Write-Fail "live pane without -k: rc=$($refuse.Code) [$($refuse.Out)]" }
    $pid1 = Field $sid '#{pane_pid}'
    P respawn-pane -k -t $sid | Out-Null
    Start-Sleep -Milliseconds 2500
    $pid2 = Field $sid '#{pane_pid}'
    $c = Cap $sid
    if ($c -match '(?m)^LIVEHIST') { Write-Pass "respawn-pane -k kept the live shell's scrolled history" }
    else { Write-Fail "respawn-pane -k lost the history" }
    if ($pid1 -and $pid2 -and $pid1 -ne $pid2) { Write-Pass "a new process runs (pane_pid $pid1 -> $pid2)" } else { Write-Fail "pane_pid $pid1 -> $pid2" }

    # ---- 4. copy mode on the dead pane is left by the respawn ----------------
    Write-Head "4. copy mode on a dead pane"
    $cid = New-Pane "$MARK C"
    $null = Wait-Dead $cid
    P select-window -t $cid | Out-Null
    P copy-mode -t $cid | Out-Null
    Start-Sleep -Milliseconds 300
    $inBefore = Field $cid '#{pane_in_mode}'
    P respawn-pane -t $cid "$MARK D" | Out-Null
    $null = Wait-Match $cid 'MARKER_D'
    $inAfter = Field $cid '#{pane_in_mode}'
    Write-Info "pane_in_mode before=$inBefore after=$inAfter"
    if ($inAfter -eq '0') { Write-Pass "copy mode left on respawn (tmux: in_mode 1 -> 0)" } else { Write-Fail "still in mode after respawn" }
    if ((Cap $cid) -match '(?m)^MARKER_C$') { Write-Pass "history kept through a respawn from copy mode" } else { Write-Fail "copy mode respawn lost the history" }

    # ---- 5. alternate screen at death --------------------------------------
    Write-Head "5. process dies on the alternate screen"
    $aid = New-Pane "pwsh -NoProfile -File $ALT"
    $null = Wait-Dead $aid 20000
    Write-Info "alternate_on at death: $(Field $aid '#{alternate_on}')"
    P respawn-pane -t $aid "cmd /c echo AFTERALT" | Out-Null
    $null = Wait-Match $aid 'AFTERALT'
    $ac = Cap $aid
    $altAfter = Field $aid '#{alternate_on}'
    if ($altAfter -ne '1') { Write-Pass "alternate screen left on respawn" } else { Write-Fail "alternate screen still on" }
    if ($ac -match '(?m)^pre1$') { Write-Pass "main screen history survives a death on the alternate screen" } else { Write-Fail "main history lost after alt screen death" }
    if ($ac -notmatch 'ALTSCREEN') { Write-Pass "alternate screen contents are not in history" } else { Write-Fail "ALTSCREEN leaked into history" }

    # ---- 6. TUI: a visible attached client shows the kept history ------------
    Write-Head "6. attached client (visible window) scrolls into the kept history"
    $tuiSess = "t708"
    $cli = Start-Process -FilePath $PSMUX -ArgumentList "-L",$NS,"new-session","-s",$tuiSess,"-x","100","-y","30" -PassThru
    $script:Opened += $cli.Id
    $ok = $false
    for ($i = 0; $i -lt 60; $i++) {
        Start-Sleep -Milliseconds 250
        if ((Run-P @('has-session', '-t', $tuiSess)).Code -eq 0) { $ok = $true; break }
    }
    if (-not $ok) { Write-Fail "attached client session never came up" }
    else {
        Start-Sleep -Milliseconds 2500
        $tp = ((P display-message -p -t "${tuiSess}:" '#{pane_id}') -join '').Trim()
        P set-option -p -t $tp remain-on-exit on | Out-Null
        P respawn-pane -k -t $tp "$MARK T" | Out-Null
        $null = Wait-Dead $tp
        P respawn-pane -t $tp "cmd /c echo TUI_RESPAWNED" | Out-Null
        $null = Wait-Match $tp 'TUI_RESPAWNED'
        $n = ([regex]::Matches((Cap $tp), '(?m)^MARKER_T$')).Count
        if ($n -eq 1) { Write-Pass "attached session pane: MARKER_T kept in history after respawn" } else { Write-Fail "attached session pane: MARKER_T count $n" }
        # Scroll the attached client's copy mode to the top of the history and
        # read the client's own console: the first pane row must be the
        # predecessor's marker.
        P copy-mode -t $tp | Out-Null
        P send-keys -t $tp -X history-top | Out-Null
        $screen = @()
        for ($i = 0; $i -lt 20; $i++) {
            Start-Sleep -Milliseconds 250
            $screen = Get-ClientScreen $cli.Id
            if (($screen | Select-Object -First 2) -match 'MARKER_T') { break }
        }
        Write-Info "client screen rows 0..2: $((($screen | Select-Object -First 3) | ForEach-Object { $_.TrimEnd() }) -join ' | ')"
        if (-not (Test-Path $RD)) { Write-Fail "conread.exe could not be built; the client screen cannot be read" }
        elseif (($screen | Select-Object -First 2) -match '^MARKER_T\b') { Write-Pass "attached client in copy mode at history-top shows MARKER_T on its screen" }
        else { Write-Fail "attached client's top rows do not show the predecessor's marker" }
        P send-keys -t $tp -X cancel | Out-Null
        Start-Sleep -Milliseconds 500
        $live = Get-ClientScreen $cli.Id
        # The respawned `echo` exits at once and the pane's remain-on-exit
        # keeps it, so its bottom line is remain-on-exit-format's text. Writing
        # it scrolls the screen by one line, which puts TUI_RESPAWNED (the top
        # row) into the history, as tmux 3.7c and 3.8 do (server-fn.c:406-408 at 3.8).
        if (($live -join "`n") -match 'Pane is dead \(status 0' -and ($live -join "`n") -notmatch 'MARKER_T' -and (Cap $tp) -match 'TUI_RESPAWNED') { Write-Pass "after leaving copy mode the client shows the new process, not the old screen" }
        else { Write-Fail "client live view: $((($live | Where-Object { $_.Trim() }) | Select-Object -First 3) -join ' | ')" }
        if ((Run-P @('has-session', '-t', $tuiSess)).Code -eq 0) { Write-Pass "attached client session alive after the respawns" } else { Write-Fail "attached session died" }
    }

    # ---- 7. respawn of a ONE row pane that prints long lines -----------------
    # Found while fixing #708. A respawn opens a new ConPTY at the pane's size;
    # born at one row, it leaves long lines to the terminal's autowrap, and
    # vt100 Grid::col_wrap computed `0 - 1` for the row that wrapped and
    # panicked the server (master 39ac085: died at the first respawn in 4 of
    # 5 runs; crash.log `grid.rs:813 unwrap on None`).
    Write-Head "7. respawn-pane -k of a one row pane printing long lines"
    $FLOOD = Join-Path $TMP "flood.cmd"
    Set-Content -Path $FLOOD -Encoding ASCII -Value @(
        '@echo off',
        ('for /L %%n in (1,1,20000) do @echo flood_%1_%%n_' + ('x' * 140))
    )
    $fa = ((P new-window -d -P -F '#{pane_id}' -t "${SESS}:" "$FLOOD A") -join '').Trim()
    $fb = ((P split-window -d -P -F '#{pane_id}' -t $fa "$FLOOD B") -join '').Trim()
    P resize-pane -t $fb -y 1 | Out-Null
    Write-Info "one row pane $fb height: $(Field $fb '#{pane_height}')"
    $diedAt = -1
    for ($i = 0; $i -lt 8; $i++) {
        foreach ($t in @($fa, $fb)) {
            Start-Sleep -Milliseconds (Get-Random -Minimum 50 -Maximum 250)
            P respawn-pane -k -t $t "$FLOOD R$i" | Out-Null
        }
        if ((Run-P @('has-session', '-t', $SESS)).Code -ne 0) { $diedAt = $i; break }
    }
    if ($diedAt -lt 0) { Write-Pass "server alive after 16 respawns of flooding panes, one of them a single row" }
    else { Write-Fail "server died at respawn round $diedAt (one row pane, long lines)" }
}
finally {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    foreach ($pid_ in $script:Opened) { try { Stop-Process -Id $pid_ -Force -EA SilentlyContinue } catch {} }
    Get-ChildItem "$psmuxDir\${NS}__*" -EA SilentlyContinue | Remove-Item -Force -EA SilentlyContinue
    Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
}

$after = Get-DefaultSessionNames
if ($before -eq $after) { Write-Pass "default namespace untouched" } else { Write-Fail "default namespace session list changed: before=[$before] after=[$after]" }

Write-Host ""
Write-Host "Passed: $script:TestsPassed  Failed: $script:TestsFailed"
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
