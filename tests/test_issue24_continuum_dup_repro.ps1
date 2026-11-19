# Issue #24 (psmux-plugins): psmux-continuum auto_save.ps1 used to spawn a
# duplicate background process on EVERY client attach, with no single-instance
# guard.
#
# FIXED: auto_save.ps1 now takes a named Mutex ('Local\psmux-continuum-autosave')
# before starting its save loop, so repeat client-attached firings start a
# script instance that finds the mutex held and exits (after waiting up to
# MUTEX_HANDOVER_MS for it) instead of accumulating. See scripts/auto_save.ps1 header comment for issue #24.
#
# This is now a REGRESSION test (inverted from the original bug-reproduction
# script): it sources the real continuum plugin.conf into a live psmux
# session, attaches/detaches a client N times via Win32, and counts the pwsh
# processes running auto_save.ps1 after each attach. It PASSES when the count
# comes back to 1 once the handover wait is over (the fix holds) and FAILS if
# more than one is still running then (the duplicate-spawn bug has regressed).

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SESSION = "issue24_repro"
$psmuxDir = "$env:USERPROFILE\.psmux"
# Resolve the plugin the same way psmux does. src/config.rs searches BOTH
# ~/.psmux/plugins/<name>/ and ~/.psmux/plugins/psmux-plugins/<name>/, because
# the plugin collection is normally cloned as one repo and each plugin then sits
# one level deeper. This hardcoded only the flat path, so with a normal install
# it aborted at "plugin.conf not found" without running a single check.
$PLUGINROOTS = @("$psmuxDir\plugins\psmux-continuum", "$psmuxDir\plugins\psmux-plugins\psmux-continuum")
$PLUGINDIR = $PLUGINROOTS | Where-Object { Test-Path (Join-Path $_ "plugin.conf") } | Select-Object -First 1
if (-not $PLUGINDIR) { $PLUGINDIR = $PLUGINROOTS[0] }
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

Write-Host "=== Issue #24 reproduction: duplicate auto_save processes per attach ===" -ForegroundColor Cyan
if (-not (Test-Path $PLUGINCONF)) { Write-Fail "plugin.conf not found at $PLUGINCONF"; exit 1 }

Cleanup
# Use a tiny interval so the script reaches its first save quickly (not required for the
# count, but proves the loop is alive). The bug is about process accumulation regardless.
& $PSMUX new-session -d -s $SESSION
Start-Sleep -Seconds 3
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { Write-Fail "session create failed"; exit 1 }

# source the real continuum plugin.conf -> registers the client-attached hook
& $PSMUX source-file -t $SESSION $PLUGINCONF 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

$base = Count-AutoSave
Write-Host "  baseline auto_save processes (no client attached): $base"

$counts = @()
$ATTACHES = 4
for ($i = 1; $i -le $ATTACHES; $i++) {
    # attach a client (fires client-attached -> run-shell auto_save.ps1, server-side)
    $proc = Start-Process -FilePath $PSMUX -ArgumentList "attach","-t",$SESSION -PassThru -WindowStyle Minimized
    Start-Sleep -Seconds 3   # let the hook fire and pwsh spawn
    $c = Count-AutoSave
    $counts += $c
    Write-Host "  after attach #$i : auto_save processes = $c"
    # detach by killing the client process (and any conhost child)
    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
    Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" -EA SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine -like '*attach*' -and $_.CommandLine -like "*$SESSION*" } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
    Start-Sleep -Seconds 2
    $after = Count-AutoSave
    Write-Host "    after detach #$i : auto_save processes = $after (should persist if bug present)"
}

# The counts above include copies still waiting for the mutex. Give the
# last one its handover wait, then count what is left.
Wait-Handover
$final = Count-AutoSave
Write-Host "`n  final auto_save process count: $final after $ATTACHES attaches and the $HANDOVER_MS ms handover wait" -ForegroundColor Yellow

# REGRESSED if more than one is still running after the handover wait; the
# mutex guard in auto_save.ps1 should leave exactly 1 no matter how many
# times client-attached fires.
if ($final -ge 2) {
    Write-Fail "REGRESSION: auto_save processes accumulate per attach (final=$final, progression: $($counts -join ','))"
} elseif ($final -eq 1 -and ($counts | Where-Object { $_ -ge 1 }).Count -eq $ATTACHES) {
    Write-Pass "Single-instance guard holds: 1 auto_save process left after $ATTACHES attaches (progression while waiting: $($counts -join ','))"
} else {
    Write-Fail "Inconclusive: final=$final progression=$($counts -join ',') (hook may not have fired -- check client-attached wiring)"
}

Cleanup
Write-Host "`n=== Results: Passed=$($script:Pass) Failed=$($script:Fail) ===" -ForegroundColor Cyan
exit $script:Fail
