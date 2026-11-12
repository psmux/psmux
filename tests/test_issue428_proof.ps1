# Issue #428 PROOF: prefix+] / paste-buffer bridges OS clipboard, with NO regression
# to internal-buffer or named-buffer priority.
$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$psmuxDir = "$env:USERPROFILE\.psmux"
$S = "issue428_proof"
$pass=0; $fail=0
function P($m){Write-Host "  [PASS] $m" -ForegroundColor Green; $script:pass++}
function F($m){Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:fail++}
function I($m){Write-Host "  [INFO] $m" -ForegroundColor Cyan}
# pwsh Set-Clipboard is a silent no-op while another window holds the clipboard;
# every seed is verified (clipboard_test_helpers.ps1).
. (Join-Path $PSScriptRoot "clipboard_test_helpers.ps1")
function Seed($v){ if(-not (Set-ClipboardVerified $v)){ I "could not seed the clipboard with $v (held by another process)" } }

& $PSMUX kill-session -t $S 2>&1 | Out-Null
Start-Sleep -Milliseconds 400
Remove-Item "$psmuxDir\$S.*" -Force -EA SilentlyContinue

$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$S -PassThru
$ok=$false
for($i=0;$i -lt 30;$i++){ Start-Sleep -Milliseconds 400; & $PSMUX has-session -t $S 2>$null; if($LASTEXITCODE -eq 0){$ok=$true;break} }
if(-not $ok){ F "no session"; exit 1 }
P "session started"

function Reset-Pane { & $PSMUX send-keys -t $S "cls" Enter 2>&1 | Out-Null; Start-Sleep -Milliseconds 700 }
function Clear-Buffers { for($i=0;$i -lt 25;$i++){ & $PSMUX delete-buffer 2>&1 | Out-Null } }

Write-Host "`n=== REGRESSION: internal buffer still wins ===" -ForegroundColor Yellow

# R1: set-buffer non-empty + paste-buffer -> internal, NOT clipboard
Clear-Buffers
Seed "CLIP_SHOULD_NOT_APPEAR_R1"
Start-Sleep -Milliseconds 200
& $PSMUX set-buffer "INTERNAL_WINS_R1" 2>&1 | Out-Null
Start-Sleep -Milliseconds 200
Reset-Pane
& $PSMUX paste-buffer -t $S 2>&1 | Out-Null
Start-Sleep -Milliseconds 700
$c = & $PSMUX capture-pane -t $S -p 2>&1 | Out-String
if($c -match "INTERNAL_WINS_R1" -and $c -notmatch "CLIP_SHOULD_NOT_APPEAR_R1"){ P "R1: non-empty internal buffer takes priority over clipboard" }
else { F "R1: regression - internal buffer did not win" ; I ("tail: "+(($c -split "`n"|?{$_ -match '\S'}|Select -Last 2) -join ' | ')) }

# R2: named buffer paste ignores clipboard
Clear-Buffers
Seed "CLIP_SHOULD_NOT_APPEAR_R2"
Start-Sleep -Milliseconds 200
& $PSMUX set-buffer -b mybuf "NAMED_WINS_R2" 2>&1 | Out-Null
Start-Sleep -Milliseconds 200
Reset-Pane
& $PSMUX paste-buffer -b mybuf -t $S 2>&1 | Out-Null
Start-Sleep -Milliseconds 700
$c = & $PSMUX capture-pane -t $S -p 2>&1 | Out-String
if($c -match "NAMED_WINS_R2" -and $c -notmatch "CLIP_SHOULD_NOT_APPEAR_R2"){ P "R2: named buffer (-b) takes priority over clipboard" }
else { F "R2: regression - named buffer did not win"; I ("tail: "+(($c -split "`n"|?{$_ -match '\S'}|Select -Last 2) -join ' | ')) }

Write-Host "`n=== FIX: empty buffer -> OS clipboard ===" -ForegroundColor Yellow

# F1: empty buffer + CLI paste-buffer -> clipboard
Clear-Buffers
Seed "OSCLIP_FIX_F1"
Start-Sleep -Milliseconds 200
Reset-Pane
& $PSMUX paste-buffer -t $S 2>&1 | Out-Null
Start-Sleep -Milliseconds 700
$c = & $PSMUX capture-pane -t $S -p 2>&1 | Out-String
if($c -match "OSCLIP_FIX_F1"){ P "F1: CLI paste-buffer pastes OS clipboard when buffer empty" }
else { F "F1: clipboard not pasted"; I ("tail: "+(($c -split "`n"|?{$_ -match '\S'}|Select -Last 2) -join ' | ')) }

# F2: empty buffer + prefix+] -> clipboard
Clear-Buffers
Seed "OSCLIP_FIX_F2"
Start-Sleep -Milliseconds 200
Reset-Pane
& "$env:TEMP\psmux_injector.exe" $proc.Id "^b{SLEEP:400}]"
Start-Sleep -Seconds 1
$c = & $PSMUX capture-pane -t $S -p 2>&1 | Out-String
if($c -match "OSCLIP_FIX_F2"){ P "F2: prefix+] pastes OS clipboard when buffer empty" }
else { F "F2: prefix+] did not paste clipboard"; I ("tail: "+(($c -split "`n"|?{$_ -match '\S'}|Select -Last 2) -join ' | ')) }

# F3: -b naming a non-existent buffer should NOT fall back to clipboard (explicit request)
Clear-Buffers
Seed "CLIP_SHOULD_NOT_APPEAR_F3"
Start-Sleep -Milliseconds 200
Reset-Pane
& $PSMUX paste-buffer -b nonexistent_buf -t $S 2>&1 | Out-Null
Start-Sleep -Milliseconds 700
$c = & $PSMUX capture-pane -t $S -p 2>&1 | Out-String
if($c -notmatch "CLIP_SHOULD_NOT_APPEAR_F3"){ P "F3: explicit -b missing buffer does NOT leak clipboard" }
else { F "F3: explicit -b wrongly fell back to clipboard"; I ("tail: "+(($c -split "`n"|?{$_ -match '\S'}|Select -Last 2) -join ' | ')) }

& $PSMUX kill-session -t $S 2>&1 | Out-Null
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
Remove-Item "$psmuxDir\$S.*" -Force -EA SilentlyContinue

Write-Host "`n=== Results: $pass passed, $fail failed ===" -ForegroundColor Cyan
exit $fail
