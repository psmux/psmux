# PR #861: with scroll-enter-copy-mode off the wheel scrolls a pane's view
# without entering copy mode (#193). Before this PR nothing but more wheel
# brought that view back, so a command typed into a direct-scrolled pane ran
# while its output and the next prompt stayed out of sight:
#
#   wheel up 5 notches        client rows: row 258 .. row 286
#   type "echo SNAP" Enter    client rows: row 258 .. row 286 (no SNAP)
#
# Typing from the attached client now snaps the pane to its live screen, as
# Windows Terminal's snapOnInput does. tmux has no direct scroll outside copy
# mode, so there is no tmux behaviour to match; send-keys from a script does
# not move the view.
#
# GROUND TRUTH: the attached client's console screen (tests\conread.cs), with
# the wheel and the keys injected into that console (mouse_injector.cs,
# injector.cs).
#
# Set PSMUX_TEST_EXE to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$script:TestsPassed = 0; $script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }
Write-Host "binary: $PSMUX" -ForegroundColor Cyan

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE') { Remove-Item "env:$v" -EA SilentlyContinue }
$NS   = "pr861_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$SESS = "snap"
$TMP  = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_pr861_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
function P { & $PSMUX -L $NS @args 2>&1 }

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$RD = Join-Path $TMP "conread.exe"; $INJ = Join-Path $TMP "injector.exe"; $MI = Join-Path $TMP "mouse_injector.exe"
foreach ($pair in @(@($RD, "conread.cs"), @($INJ, "injector.cs"), @($MI, "mouse_injector.cs"))) {
    & $csc /nologo /optimize /out:$($pair[0]) (Join-Path $PSScriptRoot $pair[1]) 2>&1 | Out-Null
    if (-not (Test-Path $pair[0])) { Write-Fail "could not build tests\$($pair[1])"; exit 1 }
}

$CONF = Join-Path $TMP "snap.conf"
$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
@("set -g default-shell $POWERSHELL", "set -g mouse on", "set -g scroll-enter-copy-mode off",
  "set -g status-left ''", "set -g status-right ''") | Set-Content -Path $CONF -Encoding ASCII

$client = Start-Process -FilePath $PSMUX -ArgumentList "-f",$CONF,"-L",$NS,"new-session","-s",$SESS,"-x","100","-y","30" -PassThru
$portFile = Join-Path $psmuxDir "${NS}__${SESS}.port"
for ($i = 0; $i -lt 80 -and -not (Test-Path $portFile); $i++) { Start-Sleep -Milliseconds 250 }
Start-Sleep -Seconds 3

function Cleanup {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 800
    try { if (-not $client.HasExited) { Stop-Process -Id $client.Id -Force -EA SilentlyContinue } } catch {}
    Get-ChildItem "$psmuxDir\${NS}__*" -EA SilentlyContinue | Remove-Item -Force -EA SilentlyContinue
    Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
}

function Screen {
    $o = Join-Path $TMP "screen.txt"
    Start-Process -FilePath $RD -ArgumentList "$($client.Id)" -Wait -WindowStyle Hidden -RedirectStandardOutput $o | Out-Null
    if (Test-Path $o) { return @(Get-Content $o) } else { return @() }
}
# The client's first row, which is the pane's top row (status is at the bottom).
function TopRow { $s = Screen; if ($s.Count) { return ([string]$s[0]).Trim() } else { return "" } }
function ScreenHas($re) { return (@(Screen | Where-Object { $_ -match $re })).Count -gt 0 }
function Wheel-Up($n) {
    $g = (P display-message -t $SESS -p '#{pane_left} #{pane_top} #{pane_width} #{pane_height}') -split ' '
    & $MI $client.Id up $n ([int]$g[0] + [int]([int]$g[2] / 2)) ([int]$g[1] + [int]([int]$g[3] / 2)) | Out-Null
    Start-Sleep -Milliseconds 800
}
function Wait-Screen($re, $ms) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) { if (ScreenHas $re) { return $true }; Start-Sleep -Milliseconds 200 }
    return $false
}

P send-keys -t $SESS '1..300 | % { "row $_" }' Enter | Out-Null
if (-not (Wait-Screen '^row 300\s*$' 8000)) { Write-Fail "the rows never reached the client"; Cleanup; exit 1 }
Start-Sleep -Milliseconds 500

Write-Head "1. the wheel scrolls the view without copy mode"
Wheel-Up 5
$top = TopRow
$mode = (P display-message -t $SESS -p '#{pane_in_mode}')
Check (($top -match '^row (\d+)$') -and [int]$Matches[1] -lt 270 -and "$mode" -eq "0") "view scrolled to [$top], pane_in_mode=$mode" "view after the wheel: [$top], pane_in_mode=$mode"

Write-Head "2. send-keys from a script leaves the view where it is"
P send-keys -t $SESS 'echo SCRIPTED' Enter | Out-Null
Start-Sleep -Milliseconds 1500
$top2 = TopRow
Check ($top2 -match '^row \d+$' -and -not (ScreenHas '^SCRIPTED\s*$')) "view still scrolled at [$top2]" "the view moved to the live screen on send-keys: top [$top2]"
$cap = (P capture-pane -t $SESS -p -S -40) -join "`n"
Check ($cap -match '(?m)^SCRIPTED\s*$') "the scripted command ran (capture-pane shows SCRIPTED)" "capture-pane has no SCRIPTED"

Write-Head "3. typing into the client snaps the pane to its live screen"
& $INJ $client.Id "echo SNAP{ENTER}" | Out-Null
$ok = Wait-Screen '^SNAP\s*$' 4000
Check $ok "the client shows the SNAP output and the new prompt" "the client still shows the scrolled view: top [$(TopRow)]"
$s = Screen
$lastPane = ($s | Where-Object { $_ -match '^PS .*>\s*$' } | Select-Object -Last 1)
Check ($null -ne $lastPane) "a fresh prompt is on the client screen" "no prompt on the client screen"

Write-Head "4. output after the snap stays on the live screen"
& $INJ $client.Id "echo AFTER1; echo AFTER2{ENTER}" | Out-Null
$ok = Wait-Screen '^AFTER2\s*$' 4000
if (-not $ok) { Screen | ForEach-Object { Write-Host "    |$_" } }
Check $ok "the client shows AFTER2" "the client lost the live screen: top [$(TopRow)]"

Write-Head "5. with scroll-enter-copy-mode on, a key in copy mode keeps the copy view"
P set-option -g scroll-enter-copy-mode on | Out-Null
Wheel-Up 5
$mode = (P display-message -t $SESS -p '#{pane_in_mode}')
$sp1 = (P display-message -t $SESS -p '#{scroll_position}')
& $INJ $client.Id "l" | Out-Null
Start-Sleep -Milliseconds 800
$mode2 = (P display-message -t $SESS -p '#{pane_in_mode}')
$sp2 = (P display-message -t $SESS -p '#{scroll_position}')
Check ("$mode" -eq "1" -and "$mode2" -eq "1" -and "$sp1" -eq "$sp2" -and [int]"$sp2" -gt 0) "copy mode kept scroll_position $sp2" "copy mode: in_mode $mode then $mode2, scroll_position $sp1 then $sp2"

Cleanup
Write-Host ""
Write-Host "=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
