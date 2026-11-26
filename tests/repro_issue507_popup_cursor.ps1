# Issue #507 REPRODUCTION: "display-popup -E pwsh -> cursor is invisible inside popup"
#
# Ground truth oracle: the Windows console cursor state of the attached psmux
# client process (GetConsoleCursorInfo.bVisible + GetConsoleScreenBufferInfo
# .dwCursorPosition), read via tests/cursorprobe.cs. That flag and cell are
# literally what the user sees blinking, so no VT stream interpretation is
# involved. The probe is validated against a plain shell (visible) and an app
# that hides the cursor (hidden) before being trusted.
#
# tmux parity reference (popup.c popup_mode_cb + server-client.c
# server_client_reset_state): when an overlay is active tmux takes BOTH the mode
# (MODE_CURSOR) and the cursor cell from the overlay, mapping the popup child's
# cursor to px+1+s.cx / py+1+s.cy. So in tmux the cursor is visible and sits on
# the child's cursor cell inside the popup border.

param([string]$PsmuxExe = (Get-Command psmux -EA Stop).Source)
$ErrorActionPreference = "Continue"
$psmuxDir = "$env:USERPROFILE\.psmux"
$S = "repro507"
$probe = "$env:TEMP\cursorprobe.exe"
$injector = "$env:TEMP\psmux_injector.exe"

function Cleanup { & $PsmuxExe kill-session -t $S 2>&1 | Out-Null; Start-Sleep -Milliseconds 500; Remove-Item "$psmuxDir\$S.*" -Force -EA SilentlyContinue }
function Info($m){ Write-Host "  $m" -ForegroundColor DarkGray }

function Dump {
    $port = (Get-Content "$psmuxDir\$S.port" -Raw).Trim(); $key = (Get-Content "$psmuxDir\$S.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port); $tcp.NoDelay=$true; $tcp.ReceiveTimeout=3000
    $st=$tcp.GetStream(); $w=[System.IO.StreamWriter]::new($st); $r=[System.IO.StreamReader]::new($st)
    $w.Write("AUTH $key`n"); $w.Flush(); $null=$r.ReadLine(); $w.Write("dump-state`n"); $w.Flush()
    $best=$null; for($j=0;$j -lt 60;$j++){ try{$l=$r.ReadLine()}catch{break}; if($null -eq $l){break}; if($l -ne "NC" -and $l.Length -gt 100){$best=$l;break} }
    $tcp.Close(); return $best
}
function PopupText { $d = Dump | ConvertFrom-Json; if(-not $d.popup_rows){return ""}; ($d.popup_rows | ForEach-Object { ($_.runs.text -join '') }) -join "`n" }

function Probe($tag, [switch]$Screen) {
    $f = "$env:TEMP\probe507_$tag.json"
    Remove-Item $f -Force -EA SilentlyContinue
    if ($Screen) { & $probe $script:Pid507 $f 8 120 1 | Out-Null }
    else { & $probe $script:Pid507 $f 8 120 0 | Out-Null }
    if (-not (Test-Path $f)) { return $null }
    return (Get-Content $f -Raw | ConvertFrom-Json)
}

Cleanup
Write-Host "`n=== Issue #507 reproduction: popup cursor visibility ===" -ForegroundColor Cyan

$proc = Start-Process -FilePath $PsmuxExe -ArgumentList "new-session","-s",$S -PassThru
$script:Pid507 = $proc.Id
Start-Sleep -Seconds 5
& $PsmuxExe has-session -t $S 2>$null
if ($LASTEXITCODE -ne 0){ Write-Host "session did not start" -ForegroundColor Red; exit 1 }
Info "psmux client pid=$($proc.Id)"

# --- STEP 1: BASELINE, no popup. Cursor must be visible at the shell prompt. ---
Write-Host "`n[Step 1] BASELINE (no popup): console cursor state" -ForegroundColor Yellow
$base = Probe "baseline" -Screen
if ($null -eq $base) { Write-Host "probe failed" -ForegroundColor Red; Cleanup; exit 1 }
Write-Host ("  visible={0}/{1}  hidden={2}  cursor=({3},{4})  cursorSize={5}" -f $base.visible,$base.samples,$base.hidden,$base.cursorX,$base.cursorY,$base.cursorSize)
Info "screen line at cursor row: '$($base.screen[$base.cursorY])'"

# --- STEP 2: open the EXACT command from the issue: display-popup -E pwsh ---
Write-Host "`n[Step 2] Opening: psmux display-popup -E pwsh" -ForegroundColor Yellow
# display-popup waits for the popup to close while a client is attached
# (tmux parity), so it runs in a job while the test works on the popup.
Start-Job -ScriptBlock { param($p,$s) & $p display-popup -t $s -E "pwsh" 2>&1 } -ArgumentList $PsmuxExe,$S | Out-Null
$ready=$false
for($t=0;$t -lt 30;$t++){
    Start-Sleep -Milliseconds 500
    $d = Dump | ConvertFrom-Json
    if($d.popup_active -and $d.popup_has_pty){
        $txt = PopupText
        if($txt -match "PS [A-Z]:\\"){ $ready=$true; break }
    }
}
if(-not $ready){ Write-Host "  pwsh did not reach a prompt inside the popup" -ForegroundColor Red; Cleanup; try{Stop-Process -Id $proc.Id -Force -EA SilentlyContinue}catch{}; exit 1 }
Info "popup is open with a live pwsh prompt inside it"
Write-Host "  --- popup content as psmux itself reports it (dump-state popup_rows) ---" -ForegroundColor DarkGray
(PopupText) -split "`n" | ForEach-Object { Info "  |$_" }

# --- STEP 3: THE MEASUREMENT. Is the console cursor visible, and where? ---
Write-Host "`n[Step 3] POPUP OPEN: console cursor state (THE CLAIM)" -ForegroundColor Yellow
$pop = Probe "popup" -Screen
Write-Host ("  visible={0}/{1}  hidden={2}  cursor=({3},{4})  cursorSize={5}" -f $pop.visible,$pop.samples,$pop.hidden,$pop.cursorX,$pop.cursorY,$pop.cursorSize)
Write-Host "  sample trace [x,y,visible]: $($pop.trace | ForEach-Object { '[' + ($_ -join ',') + ']' })" -ForegroundColor DarkGray

Write-Host "`n  --- what is actually on the client console (rows around cursor) ---" -ForegroundColor DarkGray
for($i=0; $i -lt $pop.screen.Count; $i++){
    $mark = if($i -eq $pop.cursorY){ ">>" } else { "  " }
    if($pop.screen[$i].Trim().Length -gt 0){ Write-Host ("  {0}{1,3}: {2}" -f $mark,$i,$pop.screen[$i]) -ForegroundColor DarkGray }
}

# --- STEP 4: type inside the popup; a correct cursor TRACKS the child's cursor ---
Write-Host "`n[Step 4] Type inside the popup, cursor must move with the text" -ForegroundColor Yellow
if (Test-Path $injector) {
    & $injector $proc.Id "echo AAAAAAAAAA" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 900
    $typed = Probe "typed" -Screen
    Write-Host ("  after typing 16 chars: visible={0}/{1}  cursor=({2},{3})" -f $typed.visible,$typed.samples,$typed.cursorX,$typed.cursorY)
    Write-Host ("  baseline-popup cursor was ({0},{1}); delta x={2} y={3}" -f $pop.cursorX,$pop.cursorY,($typed.cursorX - $pop.cursorX),($typed.cursorY - $pop.cursorY))
    Info "popup text now:"
    (PopupText) -split "`n" | ForEach-Object { Info "  |$_" }
} else { Info "injector not compiled, skipping typing step" }

# --- VERDICT ---
Write-Host "`n=== VERDICT ===" -ForegroundColor Cyan
if ($pop.hidden -eq $pop.samples) {
    Write-Host "  CONFIRMED: cursor is HIDDEN in every sample while the popup is open" -ForegroundColor Red
    Write-Host "  (baseline without popup was visible=$($base.visible)/$($base.samples))" -ForegroundColor Red
} elseif ($pop.visible -eq $pop.samples) {
    Write-Host "  Cursor IS visible while popup is open; check whether it sits INSIDE the popup box" -ForegroundColor Yellow
} else {
    Write-Host "  Cursor flickers: visible=$($pop.visible) hidden=$($pop.hidden)" -ForegroundColor Yellow
}

Cleanup
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
