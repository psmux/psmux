# Issue #471: display-popup closed immediately when pressing Esc inside nvim.
#
# ROOT CAUSE: client.rs popup input handling special-cased KeyCode::Esc in the
# PTY-popup branch to send "overlay-close", so Esc killed the popup instead of
# being forwarded to the app running inside it. Fixed by forwarding Esc as \x1b
# to the popup PTY (tmux parity); the popup now closes only when the child exits.
#
# PROOF via dump-state (popup_active / popup_has_pty / popup_rows) driven by REAL
# keystrokes injected with WriteConsoleInput.

param([string]$PsmuxExe = (Get-Command psmux -EA Stop).Source)
$ErrorActionPreference = "Continue"
$psmuxDir = "$env:USERPROFILE\.psmux"
$S = "test471"
$injector = "$env:TEMP\psmux_injector.exe"
$script:Pass = 0; $script:Fail = 0

function Cleanup { & $PsmuxExe kill-session -t $S 2>&1 | Out-Null; Start-Sleep -Milliseconds 500; Remove-Item "$psmuxDir\$S.*" -Force -EA SilentlyContinue }
function Info($m){ Write-Host "  $m" -ForegroundColor DarkGray }
function Pass($m){ Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Fail($m){ Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Dump {
    $port = (Get-Content "$psmuxDir\$S.port" -Raw).Trim(); $key = (Get-Content "$psmuxDir\$S.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port); $tcp.NoDelay=$true; $tcp.ReceiveTimeout=3000
    $st=$tcp.GetStream(); $w=[System.IO.StreamWriter]::new($st); $r=[System.IO.StreamReader]::new($st)
    $w.Write("AUTH $key`n"); $w.Flush(); $null=$r.ReadLine(); $w.Write("dump-state`n"); $w.Flush()
    $best=$null; for($j=0;$j -lt 60;$j++){ try{$l=$r.ReadLine()}catch{break}; if($null -eq $l){break}; if($l -ne "NC" -and $l.Length -gt 100){$best=$l;break} }
    $tcp.Close(); return $best
}
function PopupText { $d = Dump | ConvertFrom-Json; if(-not $d.popup_rows){return ""}; ($d.popup_rows | ForEach-Object { ($_.runs.text -join '') }) -join "`n" }

Cleanup
if (-not (Get-Command nvim -EA SilentlyContinue)) { Write-Host "nvim not installed - skipping (this test requires nvim)" -ForegroundColor Yellow; exit 0 }

Write-Host "`n=== Issue #471: Esc inside a PTY popup ===" -ForegroundColor Cyan
$proc = Start-Process -FilePath $PsmuxExe -ArgumentList "new-session","-s",$S -PassThru
Start-Sleep -Seconds 4
& $PsmuxExe has-session -t $S 2>$null
if ($LASTEXITCODE -ne 0){ Fail "session did not start"; exit 1 }

# --- Open nvim in a popup ---
# display-popup waits for the popup to close while a client is attached
# (tmux parity), so it runs in a job while the test works on the popup.
Start-Job -ScriptBlock { param($p,$s) & $p display-popup -t $s -E "nvim" 2>&1 } -ArgumentList $PsmuxExe,$S | Out-Null
$loaded=$false
for($t=0;$t -lt 20;$t++){ Start-Sleep -Milliseconds 500; $d=Dump|ConvertFrom-Json; if($d.popup_active -and $d.popup_has_pty){ if((PopupText) -match "NVIM|VIM|~"){$loaded=$true;break} } }
if(-not $loaded){ Fail "nvim did not load in popup"; Cleanup; try{Stop-Process -Id $proc.Id -Force -EA SilentlyContinue}catch{}; exit 1 }
Pass "nvim loaded inside popup (popup_active + popup_has_pty)"

# --- TEST 1: Esc in INSERT mode returns to normal mode, popup stays open ---
Write-Host "`n[Test 1] Esc from insert mode keeps popup open" -ForegroundColor Yellow
& $injector $proc.Id "i" 2>&1 | Out-Null; Start-Sleep -Milliseconds 400
& $injector $proc.Id "HELLO471" 2>&1 | Out-Null; Start-Sleep -Milliseconds 600
& $injector $proc.Id "{ESC}" 2>&1 | Out-Null
$open=$true
for($t=0;$t -lt 6;$t++){ Start-Sleep -Milliseconds 400; $d=Dump|ConvertFrom-Json; $open=$d.popup_active; Info "t=$([math]::Round($t*0.4,1))s popup_active=$($d.popup_active)"; if(-not $open){break} }
if($open){ Pass "popup STAYED OPEN after Esc (Esc forwarded to nvim)" } else { Fail "popup closed on Esc (bug present)" }

# --- TEST 2: multiple Esc presses in normal mode do not close popup ---
if($open){
    Write-Host "`n[Test 2] Repeated Esc in normal mode keeps popup open" -ForegroundColor Yellow
    & $injector $proc.Id "{ESC}{ESC}{ESC}" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    $d=Dump|ConvertFrom-Json
    if($d.popup_active){ Pass "popup still open after 3x Esc" } else { Fail "popup closed after repeated Esc" }
}

# --- TEST 3: :q! exits nvim -> popup closes (correct close path preserved) ---
Write-Host "`n[Test 3] :q! exits nvim and closes the popup" -ForegroundColor Yellow
& $injector $proc.Id "{ESC}" 2>&1 | Out-Null; Start-Sleep -Milliseconds 300
& $injector $proc.Id ":q!{ENTER}" 2>&1 | Out-Null
$closed=$false
for($t=0;$t -lt 12;$t++){ Start-Sleep -Milliseconds 500; $d=Dump|ConvertFrom-Json; if(-not $d.popup_active){$closed=$true;break} }
if($closed){ Pass ":q! exited nvim and popup closed (child-exit close path works)" } else { Fail ":q! did not close popup" }

# --- TEST 4 (regression guard): static (non-PTY) popup STILL closes on Esc ---
Write-Host "`n[Test 4] Regression: static text popup still closes on Esc" -ForegroundColor Yellow
# prefix+: then 'list-keys' opens a static (non-PTY) popup of key bindings.
& $injector $proc.Id "^b{SLEEP:400}:{SLEEP:400}list-keys{ENTER}" 2>&1 | Out-Null
$staticOpen=$false
for($t=0;$t -lt 12;$t++){ Start-Sleep -Milliseconds 500; $d=Dump|ConvertFrom-Json; if($d.popup_active -and -not $d.popup_has_pty){$staticOpen=$true;break} }
if($staticOpen){
    Info "static popup open (popup_active=True, popup_has_pty=False)"
    & $injector $proc.Id "{ESC}" 2>&1 | Out-Null
    $staticClosed=$false
    for($t=0;$t -lt 8;$t++){ Start-Sleep -Milliseconds 400; $d=Dump|ConvertFrom-Json; if(-not $d.popup_active){$staticClosed=$true;break} }
    if($staticClosed){ Pass "static popup closed on Esc (non-PTY branch unchanged)" } else { Fail "static popup did NOT close on Esc (regression)" }
} else {
    Info "could not open a static popup to test (skipping regression guard)"
}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:Pass)" -ForegroundColor Green
Write-Host "  Failed: $($script:Fail)" -ForegroundColor $(if($script:Fail){'Red'}else{'Green'})
Cleanup
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
exit $script:Fail
