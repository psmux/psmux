# Issue #507: display-popup: cursor invisible
#
# Reported: `psmux display-popup -E pwsh` -> "Cursor is invisible inside popup."
#
# ROOT CAUSE: the client computed the terminal cursor ONLY from the active pane
# (client.rs post-draw atomic cursor write). A PTY popup is modal and takes the
# keystrokes, but nothing ever moved the cursor into it, so it stayed parked on
# the pane underneath. Measured before the fix: cursor frozen at screen (46,0)
# (the background pane's prompt) while the user typed inside the popup box that
# occupied rows 2..25. The server did not even publish the popup child's cursor.
#
# FIX: popup.rs publishes popup_cursor_row / popup_cursor_col / popup_hide_cursor
# for PTY popups, and the client maps them through popup_cursor_screen_pos() into
# the popup interior, taking precedence over the pane. This mirrors tmux, where
# popup_mode_cb returns the popup's own screen and server_client_reset_state
# takes both the cursor and the cursor mode from the overlay.
#
# GROUND TRUTH: the Windows console cursor of the attached client process
# (GetConsoleCursorInfo.bVisible + GetConsoleScreenBufferInfo.dwCursorPosition)
# read by tests/cursorprobe.cs. That flag and cell are literally what the user
# sees blinking. The popup box is located by scanning the console text for its
# border glyphs, NOT by calling psmux's own geometry code.

param(
    [string]$PsmuxExe = (Get-Command psmux -EA Stop).Source,
    [string]$Session = "t507cursor"
)
$ErrorActionPreference = "Continue"
$psmuxDir = "$env:USERPROFILE\.psmux"
$S = $Session
$probe = "$env:TEMP\cursorprobe.exe"
$injector = "$env:TEMP\psmux_injector.exe"
$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path
$script:Pass = 0; $script:Fail = 0

function Pass($m){ Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Fail($m){ Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Info($m){ Write-Host "  $m" -ForegroundColor DarkGray }
function Cleanup { & $PsmuxExe kill-session -t $S 2>&1 | Out-Null; Start-Sleep -Milliseconds 500; Remove-Item "$psmuxDir\$S.*" -Force -EA SilentlyContinue }

# Compile the probe / injector on demand.
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $probe))    { & $csc /nologo /optimize /out:$probe    "$repoTests\cursorprobe.cs" 2>&1 | Out-Null }
if (-not (Test-Path $injector)) { & $csc /nologo /optimize /out:$injector "$repoTests\injector.cs"    2>&1 | Out-Null }
if (-not (Test-Path $probe)) { Write-Host "cursorprobe.exe failed to build" -ForegroundColor Red; exit 1 }

function Dump {
    $port = (Get-Content "$psmuxDir\$S.port" -Raw).Trim(); $key = (Get-Content "$psmuxDir\$S.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port); $tcp.NoDelay=$true; $tcp.ReceiveTimeout=3000
    $st=$tcp.GetStream(); $w=[System.IO.StreamWriter]::new($st); $r=[System.IO.StreamReader]::new($st)
    $w.Write("AUTH $key`n"); $w.Flush(); $null=$r.ReadLine(); $w.Write("dump-state`n"); $w.Flush()
    $best=$null; for($j=0;$j -lt 60;$j++){ try{$l=$r.ReadLine()}catch{break}; if($null -eq $l){break}; if($l -ne "NC" -and $l.Length -gt 100){$best=$l;break} }
    $tcp.Close(); return $best
}
function PopupText { $d = Dump | ConvertFrom-Json; if(-not $d.popup_rows){return ""}; ($d.popup_rows | ForEach-Object { ($_.runs.text -join '') }) -join "`n" }

function Probe([switch]$Screen) {
    $f = "$env:TEMP\probe507_run.json"
    Remove-Item $f -Force -EA SilentlyContinue
    if ($Screen) { & $probe $script:ClientPid $f 6 120 1 | Out-Null } else { & $probe $script:ClientPid $f 6 120 0 | Out-Null }
    if (-not (Test-Path $f)) { return $null }
    return (Get-Content $f -Raw | ConvertFrom-Json)
}

# Locate the popup box from the console text itself (border glyphs), so the
# assertion does not depend on psmux's own rect maths.
function Find-PopupBox($screen) {
    $top = -1; $bottom = -1; $left = -1; $right = -1
    for ($i = 0; $i -lt $screen.Count; $i++) {
        $line = $screen[$i]
        if ($top -lt 0 -and $line.Contains([char]0x250C)) { $top = $i; $left = $line.IndexOf([char]0x250C); $right = $line.IndexOf([char]0x2510) }
        if ($line.Contains([char]0x2514)) { $bottom = $i }
    }
    if ($top -lt 0 -or $bottom -lt 0) { return $null }
    return @{ Top=$top; Bottom=$bottom; Left=$left; Right=$right }
}

Cleanup
Write-Host "`n=== Issue #507: cursor inside display-popup ===" -ForegroundColor Cyan

# Start the attached client and poll: a cold start with no warm server can take
# well over five seconds. If a session NAME turns out to be unusable on this
# machine (a previously force-killed run can leave the name held), fall back to
# a suffixed name rather than reporting a cursor failure that is really a
# startup failure.
$proc = $null
$up = $false
foreach ($suffix in @("", "_b", "_c")) {
    $S = "$Session$suffix"
    & $PsmuxExe kill-session -t $S 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    Remove-Item "$psmuxDir\$S.*" -Force -EA SilentlyContinue
    $proc = Start-Process -FilePath $PsmuxExe -ArgumentList "new-session","-s",$S -PassThru
    $script:ClientPid = $proc.Id
    for ($t = 0; $t -lt 30; $t++) {
        Start-Sleep -Milliseconds 750
        & $PsmuxExe has-session -t $S 2>$null
        if ($LASTEXITCODE -eq 0) { $up = $true; break }
        if (-not (Get-Process -Id $proc.Id -EA SilentlyContinue)) { break }  # client died, try next name
    }
    if ($up) { break }
    Info "session name '$S' would not start on this machine, trying another"
    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
}
if (-not $up){ Fail "session did not start under any name"; exit 1 }
Info "session '$S' up (client pid $($proc.Id))"
Start-Sleep -Seconds 2   # let the shell paint its first prompt

# ── Test 1: baseline, cursor visible on the pane (probe sanity) ──
Write-Host "`n[Test 1] Baseline: cursor visible on the pane" -ForegroundColor Yellow
$base = Probe -Screen
if ($null -eq $base) { Fail "cursor probe could not attach to the client console"; Cleanup; exit 1 }
if ($base.visible -eq $base.samples -and $base.hidden -eq 0) { Pass "cursor visible in all $($base.samples) samples (no popup)" }
else { Fail "baseline cursor not visible: visible=$($base.visible) hidden=$($base.hidden)" }
$baseX = $base.cursorX; $baseY = $base.cursorY
Info "baseline cursor = ($baseX,$baseY)"

# ── Test 2: open the exact command from the report ──
Write-Host "`n[Test 2] display-popup -E pwsh reaches a live prompt" -ForegroundColor Yellow
# display-popup waits for the popup to close while a client is attached
# (tmux parity), so it runs in a job while the test works on the popup.
Start-Job -ScriptBlock { param($p,$s) & $p display-popup -t $s -E "pwsh" 2>&1 } -ArgumentList $PsmuxExe,$S | Out-Null
$ready=$false
for($t=0;$t -lt 30;$t++){
    Start-Sleep -Milliseconds 500
    $d = Dump | ConvertFrom-Json
    if($d.popup_active -and $d.popup_has_pty -and (PopupText) -match "PS [A-Z]:\\"){ $ready=$true; break }
}
if($ready){ Pass "popup open with a live pwsh prompt (popup_active + popup_has_pty)" }
else { Fail "pwsh never reached a prompt inside the popup"; Cleanup; try{Stop-Process -Id $proc.Id -Force -EA SilentlyContinue}catch{}; exit 1 }

# ── Test 3 (TCP/server path): the popup child's cursor is published ──
Write-Host "`n[Test 3] Server publishes the popup child's cursor over TCP" -ForegroundColor Yellow
$d = Dump | ConvertFrom-Json
if ($null -ne $d.popup_cursor_row -and $null -ne $d.popup_cursor_col) {
    Pass "dump-state carries popup_cursor_row=$($d.popup_cursor_row) popup_cursor_col=$($d.popup_cursor_col)"
} else {
    Fail "dump-state has no popup cursor fields (server never publishes them)"
}
if ($d.popup_hide_cursor -eq $false) { Pass "popup_hide_cursor=false (pwsh wants a visible cursor)" }
else { Fail "popup_hide_cursor=$($d.popup_hide_cursor), expected false for a shell" }

# ── Test 4 (THE BUG): console cursor is inside the popup box ──
Write-Host "`n[Test 4] Console cursor sits INSIDE the popup border" -ForegroundColor Yellow
$pop = Probe -Screen
$box = Find-PopupBox $pop.screen
if ($null -eq $box) { Fail "could not locate the popup border on the console" }
else {
    Info "popup box: rows $($box.Top)..$($box.Bottom), cols $($box.Left)..$($box.Right)"
    Info "cursor = ($($pop.cursorX),$($pop.cursorY)), visible=$($pop.visible)/$($pop.samples)"
    if ($pop.visible -eq $pop.samples) { Pass "cursor visible in all $($pop.samples) samples while the popup is open" }
    else { Fail "cursor hidden while popup open: visible=$($pop.visible) hidden=$($pop.hidden)" }

    $insideY = ($pop.cursorY -gt $box.Top) -and ($pop.cursorY -lt $box.Bottom)
    $insideX = ($pop.cursorX -gt $box.Left) -and ($pop.cursorX -lt $box.Right)
    if ($insideY -and $insideX) { Pass "cursor is strictly inside the popup border" }
    else { Fail "cursor ($($pop.cursorX),$($pop.cursorY)) is OUTSIDE the popup box (bug #507)" }

    if ($pop.cursorY -ne $baseY) { Pass "cursor left the pane row underneath (was row $baseY, now row $($pop.cursorY))" }
    else { Fail "cursor still on the background pane row $baseY (bug #507)" }

    # It must be on the popup's prompt row, not just anywhere in the box, and
    # not on the identical-looking prompt of the pane underneath: require the
    # row to be inside the box AND to carry the popup's own border glyph.
    $row = $pop.screen[$pop.cursorY]
    if ($insideY -and $row -match "PS [A-Z]:\\" -and $row.Contains([char]0x2502)) {
        Pass "cursor is on the popup's prompt row (inside the border)"
    } else {
        Fail "cursor row is not the popup's prompt row: '$row'"
    }
}

# ── Test 5 (keystroke injection): cursor tracks what is typed in the popup ──
Write-Host "`n[Test 5] Cursor tracks typing inside the popup (WriteConsoleInput)" -ForegroundColor Yellow
if (Test-Path $injector) {
    $before = Probe
    $text = "echo AAAAAAAAAA"   # 15 characters
    & $injector $proc.Id $text 2>&1 | Out-Null
    Start-Sleep -Milliseconds 900
    $after = Probe -Screen
    $dx = $after.cursorX - $before.cursorX
    $dy = $after.cursorY - $before.cursorY
    Info "cursor ($($before.cursorX),$($before.cursorY)) -> ($($after.cursorX),$($after.cursorY))  delta x=$dx y=$dy"
    if ($dx -eq $text.Length -and $dy -eq 0) { Pass "cursor advanced exactly $($text.Length) columns with the typed text" }
    else { Fail "cursor delta x=$dx y=$dy, expected x=$($text.Length) y=0" }

    if ((PopupText) -match [regex]::Escape($text)) { Pass "the text really landed in the popup" }
    else { Fail "typed text did not reach the popup" }

    $box2 = Find-PopupBox $after.screen
    if ($box2 -and $after.cursorX -gt $box2.Left -and $after.cursorX -lt $box2.Right `
             -and $after.cursorY -gt $box2.Top -and $after.cursorY -lt $box2.Bottom) {
        Pass "cursor still inside the popup after typing"
    } else { Fail "cursor is not inside the popup box after typing" }
} else { Info "injector unavailable, skipping keystroke layer" }

# ── Test 6 (regression): closing the popup returns the cursor to the pane ──
Write-Host "`n[Test 6] Closing the popup hands the cursor back to the pane" -ForegroundColor Yellow
if (Test-Path $injector) {
    & $injector $proc.Id "{ESC}" 2>&1 | Out-Null       # discard the typed line
    Start-Sleep -Milliseconds 300
    & $injector $proc.Id "exit{ENTER}" 2>&1 | Out-Null
    $closed=$false
    for($t=0;$t -lt 16;$t++){ Start-Sleep -Milliseconds 500; $d=Dump|ConvertFrom-Json; if(-not $d.popup_active){$closed=$true;break} }
    if($closed){
        Pass "popup closed when its child exited"
        Start-Sleep -Milliseconds 600
        $back = Probe -Screen
        if ($back.visible -eq $back.samples) { Pass "cursor still visible after the popup closed" }
        else { Fail "cursor went missing after the popup closed: visible=$($back.visible)" }
        if ($back.cursorY -eq $baseY) { Pass "cursor returned to the pane row $baseY" }
        else { Fail "cursor did not return to the pane: row $($back.cursorY), expected $baseY" }
    } else { Fail "popup did not close on child exit" }
} else { Info "injector unavailable, skipping close-path layer" }

# ── Test 7 (no regression): a static popup must NOT steal the cursor ──
Write-Host "`n[Test 7] Static (non-PTY) popup leaves the pane cursor alone" -ForegroundColor Yellow
if (Test-Path $injector) {
    $preStatic = Probe
    & $injector $proc.Id "^b{SLEEP:400}:{SLEEP:400}list-keys{ENTER}" 2>&1 | Out-Null
    $staticOpen=$false
    for($t=0;$t -lt 12;$t++){ Start-Sleep -Milliseconds 500; $d=Dump|ConvertFrom-Json; if($d.popup_active -and -not $d.popup_has_pty){$staticOpen=$true;break} }
    if ($staticOpen) {
        $st = Probe
        Info "static popup open; cursor ($($st.cursorX),$($st.cursorY)) vs pre ($($preStatic.cursorX),$($preStatic.cursorY))"
        if ($st.cursorX -eq $preStatic.cursorX -and $st.cursorY -eq $preStatic.cursorY) {
            Pass "static popup did not move the cursor (PTY-only change, no regression)"
        } else {
            Fail "static popup moved the cursor unexpectedly"
        }
        & $injector $proc.Id "{ESC}" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 600
    } else { Info "could not open a static popup, skipping" }
} else { Info "injector unavailable, skipping static-popup layer" }

# ── Test 8 (TUI health): the session still works after all of it ──
Write-Host "`n[Test 8] TUI still functional (CLI-driven)" -ForegroundColor Yellow
& $PsmuxExe split-window -v -t $S 2>&1 | Out-Null
Start-Sleep -Milliseconds 800
$panes = (& $PsmuxExe display-message -t $S -p '#{window_panes}' 2>&1).Trim()
if ($panes -eq "2") { Pass "split-window still works (2 panes)" } else { Fail "expected 2 panes, got '$panes'" }
$fin = Probe
if ($fin.visible -eq $fin.samples) { Pass "cursor visible in the new pane" } else { Fail "cursor lost after split: visible=$($fin.visible)" }

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:Pass)" -ForegroundColor Green
Write-Host "  Failed: $($script:Fail)" -ForegroundColor $(if($script:Fail){'Red'}else{'Green'})
Cleanup
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
exit $script:Fail
