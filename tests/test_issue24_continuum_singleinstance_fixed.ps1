# Issue #24 FIX verification: auto_save.ps1 now holds a named mutex so only one
# auto-save loop runs regardless of how many times client-attached fires.
#
# Two independent proofs:
#   A) Direct guard check: launch auto_save.ps1 twice by hand; the second must
#      exit within the handover wait, leaving the first one running.
#   B) End-to-end via psmux: attach a client N times; the count must reach 1
#      (proving the hook fires) and be exactly 1 once the handover wait is over
#      (proving no accumulation).

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SESSION = "issue24_fixed"
$psmuxDir = "$env:USERPROFILE\.psmux"
# Resolve the plugin the same way psmux does: src/config.rs searches both
# ~/.psmux/plugins/<name>/ and ~/.psmux/plugins/psmux-plugins/<name>/, since the
# collection is normally cloned as one repo with each plugin a level deeper.
# Hardcoding the flat path aborted the suite before any check ran.
$PLUGINROOTS = @("$psmuxDir\plugins\psmux-continuum", "$psmuxDir\plugins\psmux-plugins\psmux-continuum")
$PLUGINDIR = $PLUGINROOTS | Where-Object { Test-Path (Join-Path $_ "plugin.conf") } | Select-Object -First 1
if (-not $PLUGINDIR) { $PLUGINDIR = $PLUGINROOTS[0] }
$AUTOSAVE = Join-Path $PLUGINDIR "scripts\auto_save.ps1"
$PLUGINCONF = Join-Path $PLUGINDIR "plugin.conf"
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m){ Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m){ Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }

function Count-AutoSave {
    @(Get-CimInstance Win32_Process -Filter "Name='pwsh.exe'" -EA SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine -like '*auto_save*' }).Count
}
function Kill-AutoSave {
    Get-CimInstance Win32_Process -Filter "Name='pwsh.exe'" -EA SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine -like '*auto_save*' } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
}
# plugin.conf's hook runs ~/.psmux/plugins/psmux-continuum/scripts/auto_save.ps1,
# where ppm installs it, and that script exits at once unless psmux-resurrect
# sits beside it. With continuum only under plugins\psmux-plugins\ (a clone of
# the whole collection) the hook starts nothing, and every count below is 0.
# That is a missing prerequisite, not a result.
$HOOKSCRIPT = "$psmuxDir\plugins\psmux-continuum\scripts\auto_save.ps1"
$RESURRECT = "$psmuxDir\plugins\psmux-resurrect\scripts\save.ps1"
foreach ($need in $HOOKSCRIPT, $RESURRECT) {
    if (-not (Test-Path $need)) {
        Write-Host "[SKIP] $need not found - install psmux-continuum and psmux-resurrect with ppm (prefix + I)" -ForegroundColor Yellow
        exit 0
    }
}
# A second auto_save.ps1 waits up to MUTEX_HANDOVER_MS for the mutex before it
# exits (psmux-plugins 45cc03fd, so a restarted server takes the loop over), so
# for that long after a start a waiting copy is still running and counted.
$hm = Select-String -LiteralPath $HOOKSCRIPT -Pattern 'MUTEX_HANDOVER_MS\s*=\s*(\d+)' | Select-Object -First 1
$HANDOVER_MS = if ($hm) { [int]$hm.Matches[0].Groups[1].Value } else { 0 }
function Wait-Handover {
    # Until at most one is left, or the handover wait (plus slack) is over.
    $deadline = (Get-Date).AddMilliseconds($HANDOVER_MS + 5000)
    while ((Count-AutoSave) -gt 1 -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 500 }
}

function Cleanup {
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Kill-AutoSave
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
}

Write-Host "=== Issue #24 FIX verification ===" -ForegroundColor Cyan
Cleanup
Start-Sleep -Seconds 1

# --- Proof A: direct single-instance guard (no psmux needed) ---
Write-Host "`n[A] Direct mutex guard: launch auto_save.ps1 twice" -ForegroundColor Yellow
# Need a running psmux server so the loop's 'psmux ls' check passes and it stays alive
& $PSMUX new-session -d -s $SESSION
Start-Sleep -Seconds 3
Start-Process pwsh -ArgumentList "-NoProfile","-File",$AUTOSAVE,"-IntervalMinutes","1" -WindowStyle Hidden | Out-Null
Start-Sleep -Seconds 2
$afterFirst = Count-AutoSave
$second = Start-Process pwsh -ArgumentList "-NoProfile","-File",$AUTOSAVE,"-IntervalMinutes","1" -WindowStyle Hidden -PassThru
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$secondExited = $second.WaitForExit($HANDOVER_MS + 5000)
$sw.Stop()
$afterSecond = Count-AutoSave
Write-Host "  count after 1st launch: $afterFirst; 2nd launch exited: $secondExited after $($sw.ElapsedMilliseconds) ms (handover wait $HANDOVER_MS ms); count then: $afterSecond"
if ($afterFirst -eq 1 -and $secondExited -and $afterSecond -eq 1) {
    Write-Pass "Second instance exited within the handover wait (mutex guard holds, count back to 1)"
} else {
    Write-Fail "Guard failed: afterFirst=$afterFirst secondExited=$secondExited afterSecond=$afterSecond (expected 1, True, 1)"
}
Kill-AutoSave
Start-Sleep -Seconds 1

# --- Proof B: end-to-end via repeated client attach ---
Write-Host "`n[B] End-to-end: attach a client 4 times via the client-attached hook" -ForegroundColor Yellow
& $PSMUX source-file -t $SESSION $PLUGINCONF 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
$reached1 = $false
$counts = @()
for ($i = 1; $i -le 4; $i++) {
    $proc = Start-Process -FilePath $PSMUX -ArgumentList "attach","-t",$SESSION -PassThru -WindowStyle Minimized
    Start-Sleep -Seconds 3
    $c = Count-AutoSave
    $counts += $c
    if ($c -ge 1) { $reached1 = $true }
    Write-Host "  after attach #$i : auto_save processes = $c"
    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
    Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" -EA SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine -like '*attach*' -and $_.CommandLine -like "*$SESSION*" } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
    Start-Sleep -Seconds 2
}
# The counts above include copies still waiting for the mutex.
Wait-Handover
$final = Count-AutoSave
Write-Host "  progression while waiting: $($counts -join ','); final after the handover wait=$final"
if ($reached1 -and $final -eq 1) {
    Write-Pass "Hook fired each attach yet exactly ONE auto_save survives (no accumulation)"
} else {
    Write-Fail "Unexpected: reached1=$reached1 final=$final"
}

Cleanup
Write-Host "`n=== Results: Passed=$($script:Pass) Failed=$($script:Fail) ===" -ForegroundColor Cyan
exit $script:Fail
