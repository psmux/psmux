# Issue #471: display-popup closes immediately when pressing Esc inside nvim
#
# EXPECTED (tmux parity): Esc is forwarded to the app inside the popup; the popup
# stays open and only closes when the app itself exits (:q).
# BUG: pressing Esc closes the popup instead of reaching nvim.
#
# Verification via dump-state:
#   popup_active  = True  -> popup overlay open
#   popup_has_pty = True  -> the popup PTY (nvim) is running
# Test: open popup with nvim, confirm it loaded, inject Esc, then re-check popup_active.

param([string]$PsmuxExe = (Get-Command psmux -EA Stop).Source)
$ErrorActionPreference = "Continue"
$psmuxDir = "$env:USERPROFILE\.psmux"
$S = "repro471"
$injector = "$env:TEMP\psmux_injector.exe"

function Cleanup { & $PsmuxExe kill-session -t $S 2>&1 | Out-Null; Start-Sleep -Milliseconds 500; Remove-Item "$psmuxDir\$S.*" -Force -EA SilentlyContinue }
function Info($m){ Write-Host "  $m" -ForegroundColor DarkGray }
function Pass($m){ Write-Host "  [PASS] $m" -ForegroundColor Green }
function Fail($m){ Write-Host "  [FAIL] $m" -ForegroundColor Red }
function Dump {
    $port = (Get-Content "$psmuxDir\$S.port" -Raw).Trim(); $key = (Get-Content "$psmuxDir\$S.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port); $tcp.NoDelay=$true; $tcp.ReceiveTimeout=3000
    $st=$tcp.GetStream(); $w=[System.IO.StreamWriter]::new($st); $r=[System.IO.StreamReader]::new($st)
    $w.Write("AUTH $key`n"); $w.Flush(); $null=$r.ReadLine(); $w.Write("dump-state`n"); $w.Flush()
    $best=$null; for($j=0;$j -lt 60;$j++){ try{$l=$r.ReadLine()}catch{break}; if($null -eq $l){break}; if($l -ne "NC" -and $l.Length -gt 100){$best=$l;break} }
    $tcp.Close(); return $best
}
function PopupText {
    $d = Dump | ConvertFrom-Json
    if (-not $d.popup_rows) { return "" }
    ($d.popup_rows | ForEach-Object { ($_.runs.text -join '') }) -join "`n"
}

Cleanup
Write-Host "`n=== Issue #471 reproduction: Esc inside popup nvim ===" -ForegroundColor Cyan
$proc = Start-Process -FilePath $PsmuxExe -ArgumentList "new-session","-s",$S -PassThru
Start-Sleep -Seconds 4
& $PsmuxExe has-session -t $S 2>$null
if ($LASTEXITCODE -ne 0){ Fail "session did not start"; exit 1 }

Write-Host "`n[Step 1] Open display-popup -E nvim" -ForegroundColor Yellow
# display-popup waits for the popup to close while a client is attached
# (tmux parity), so it runs in a job while the test works on the popup.
Start-Job -ScriptBlock { param($p,$s) & $p display-popup -t $s -E "nvim" 2>&1 } -ArgumentList $PsmuxExe,$S | Out-Null
# Wait for nvim to load inside the popup
$loaded=$false
for($t=0;$t -lt 20;$t++){
    Start-Sleep -Milliseconds 500
    $d = Dump | ConvertFrom-Json
    if($d.popup_active -and $d.popup_has_pty){
        $txt = PopupText
        if($txt -match "NVIM|VIM|~"){ $loaded=$true; Info "nvim loaded (popup shows nvim UI)"; break }
    }
}
$d = Dump | ConvertFrom-Json
Info "popup_active=$($d.popup_active) popup_has_pty=$($d.popup_has_pty) loaded=$loaded"
if(-not $d.popup_active){ Fail "popup never opened"; Cleanup; try{Stop-Process -Id $proc.Id -Force -EA SilentlyContinue}catch{}; exit 1 }
Pass "popup with nvim is open"

Write-Host "`n[Step 2] Put nvim in INSERT mode, then press Esc (Esc should return to normal mode, NOT close popup)" -ForegroundColor Yellow
# enter insert mode and type a marker
& $injector $proc.Id "i" 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
& $injector $proc.Id "HELLO471" 2>&1 | Out-Null
Start-Sleep -Milliseconds 700
$beforeEsc = Dump | ConvertFrom-Json
Info "before Esc: popup_active=$($beforeEsc.popup_active)"

# THE KEY ACTION: press Esc
Info "injecting {ESC}..."
& $injector $proc.Id "{ESC}" 2>&1 | Out-Null

$stillOpen=$false
for($t=0;$t -lt 8;$t++){
    Start-Sleep -Milliseconds 400
    $d = Dump | ConvertFrom-Json
    if($d.popup_active){ $stillOpen=$true } else { $stillOpen=$false }
    Info "  t=$([math]::Round($t*0.4,1))s popup_active=$($d.popup_active) has_pty=$($d.popup_has_pty)"
    if(-not $d.popup_active){ break }
}

Write-Host "`n=== VERDICT ===" -ForegroundColor Cyan
if($stillOpen){
    Pass "NOT REPRODUCED: popup STAYED OPEN after Esc (Esc forwarded to nvim)"
} else {
    Fail "BUG REPRODUCED: popup CLOSED on Esc instead of forwarding it to nvim"
}

# Confirm proper close path still works: :q should exit nvim and close popup
if($stillOpen){
    Write-Host "`n[Step 3] Confirm :q exits nvim and THEN closes popup (correct close path)" -ForegroundColor Yellow
    & $injector $proc.Id "{ESC}" 2>&1 | Out-Null; Start-Sleep -Milliseconds 300
    & $injector $proc.Id ":q!{ENTER}" 2>&1 | Out-Null
    $closed=$false
    for($t=0;$t -lt 10;$t++){ Start-Sleep -Milliseconds 500; $d=Dump|ConvertFrom-Json; if(-not $d.popup_active){$closed=$true;break} }
    if($closed){ Pass ":q! exited nvim and popup closed (correct)" } else { Fail ":q! did not close popup" }
}

Cleanup
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
