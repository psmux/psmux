# Issue #470: display-popup does not work when invoked from display-menu
#
# Verification via dump-state:
#   menu_active   = True  -> menu overlay open
#   popup_active  = True  -> popup overlay opened
#   popup_has_pty = True  -> popup command PTY spawned
#
# Menu item selection driven by injecting the item's shortcut key 'g'.

param([string]$PsmuxExe = (Get-Command psmux -EA Stop).Source)

$ErrorActionPreference = "Continue"
$psmuxDir = "$env:USERPROFILE\.psmux"
$S = "repro470"
$injector = "$env:TEMP\psmux_injector.exe"

function Cleanup { & $PsmuxExe kill-session -t $S 2>&1 | Out-Null; Start-Sleep -Milliseconds 500; Remove-Item "$psmuxDir\$S.*" -Force -EA SilentlyContinue }
function Write-Info($m){ Write-Host "  $m" -ForegroundColor DarkGray }
function Write-Pass($m){ Write-Host "  [PASS] $m" -ForegroundColor Green }
function Write-Fail($m){ Write-Host "  [FAIL] $m" -ForegroundColor Red }
function Dump {
    $port = (Get-Content "$psmuxDir\$S.port" -Raw).Trim(); $key = (Get-Content "$psmuxDir\$S.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port); $tcp.NoDelay=$true; $tcp.ReceiveTimeout=3000
    $st=$tcp.GetStream(); $w=[System.IO.StreamWriter]::new($st); $r=[System.IO.StreamReader]::new($st)
    $w.Write("AUTH $key`n"); $w.Flush(); $null=$r.ReadLine(); $w.Write("dump-state`n"); $w.Flush()
    $best=$null; for($j=0;$j -lt 60;$j++){ try{$l=$r.ReadLine()}catch{break}; if($null -eq $l){break}; if($l -ne "NC" -and $l.Length -gt 100){$best=$l;break} }
    $tcp.Close(); return $best
}

Cleanup
Write-Host "`n=== Issue #470 reproduction ===" -ForegroundColor Cyan
Write-Host "psmux: $PsmuxExe" -ForegroundColor DarkGray
$proc = Start-Process -FilePath $PsmuxExe -ArgumentList "new-session","-s",$S -PassThru
Start-Sleep -Seconds 4
& $PsmuxExe has-session -t $S 2>$null
if ($LASTEXITCODE -ne 0){ Write-Fail "session did not start"; exit 1 }

# Baseline: standalone display-popup
Write-Host "`n[Baseline] standalone display-popup -E" -ForegroundColor Yellow
# display-popup waits for the popup to close while a client is attached
# (tmux parity), so it runs in a job while the test works on the popup.
Start-Job -ScriptBlock { param($p,$s) & $p display-popup -t $s -E "ping -n 20 127.0.0.1" 2>&1 } -ArgumentList $PsmuxExe,$S | Out-Null
$baseActive=$false
for($t=0;$t -lt 10;$t++){ Start-Sleep -Milliseconds 400; $d=Dump|ConvertFrom-Json; if($d.popup_active){$baseActive=$true;break} }
Write-Info "standalone popup_active=$baseActive"
if($baseActive){Write-Pass "standalone display-popup works"}else{Write-Fail "standalone display-popup broken"}
& $injector $proc.Id "{ESC}" 2>&1 | Out-Null; Start-Sleep -Milliseconds 800

# Bug path
Write-Host "`n[Bug path] display-menu item 'g' -> display-popup -E" -ForegroundColor Yellow
& $PsmuxExe display-menu -t $S -T "Menu" "popuptest" g "display-popup -E 'ping -n 20 127.0.0.1'" 2>&1 | Out-Null
Start-Sleep -Milliseconds 1000
$d = Dump | ConvertFrom-Json
Write-Info "STEP 1 menu opened: menu_active=$($d.menu_active) title=$($d.menu_title)"
if(-not $d.menu_active){ Write-Fail "menu never opened - cannot test selection"; Cleanup; try{Stop-Process -Id $proc.Id -Force -EA SilentlyContinue}catch{}; exit 1 }
Write-Pass "menu opened with the item present"

Write-Info "STEP 2 injecting shortcut key 'g' to select the item..."
& $injector $proc.Id "g" 2>&1 | Out-Null

$popupOpened=$false; $menuClosed=$false
for($t=0;$t -lt 16;$t++){
    Start-Sleep -Milliseconds 400
    $d = Dump | ConvertFrom-Json
    if(-not $d.menu_active){ $menuClosed=$true }
    if($d.popup_active){ $popupOpened=$true }
    Write-Info "  t=$([math]::Round($t*0.4,1))s menu_active=$($d.menu_active) popup_active=$($d.popup_active) popup_has_pty=$($d.popup_has_pty)"
    if($popupOpened){ break }
}

Write-Host "`n=== VERDICT ===" -ForegroundColor Cyan
Write-Info "menu closed after selection: $menuClosed"
Write-Info "popup opened after selection: $popupOpened"
if(-not $baseActive){
    Write-Fail "Baseline broken - can't isolate"
} elseif($popupOpened){
    Write-Pass "NOT REPRODUCED: menu-fired display-popup opened"
} else {
    Write-Fail "BUG REPRODUCED: item selected (menu_closed=$menuClosed) but display-popup never fired"
}

Cleanup
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
