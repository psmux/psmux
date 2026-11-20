# test_runner_leftover_audit.ps1
#
# PROOF SUITE for the runner's per suite desktop audit. It does not test psmux;
# it tests the runner, by deliberately being the kind of badly behaved suite the
# audit exists to catch.
#
# BACKGROUND
# After three full sweeps the desktop held about 33 Windows Terminal windows with
# six dead tabs each, hosting 121 processes whose command line was exactly
# `C:\WINDOWS\system32\cmd.exe /c pause`, all parked at "Press any key". None of
# them were descendants of any suite: the runner puts every suite in a Job Object
# with KILL_ON_JOB_CLOSE, and the job kill had already run. Without attribution
# the only evidence was the pile of tabs, so run_all_tests.ps1 now snapshots the
# desktop around every suite, names whatever is new, ends it and closes its
# window. This suite is what proves that machinery actually fires.
#
# IT LEAKS TWO CONSOLES ON PURPOSE, BY TWO DIFFERENT ROUTES
#
#   A. IN JOB. A plain `Start-Process cmd.exe /c pause`. It is a descendant of
#      this script, therefore a member of the suite's job object, therefore the
#      job kill should reap it. This is the control: it proves the existing job
#      teardown still works and that the audit does not invent leftovers.
#
#   B. OUTSIDE THE JOB. Created through Win32_Process.Create, so the new process
#      is a child of WmiPrvSE.exe and has never been a member of this suite's job
#      object. TerminateJobObject cannot touch it, and it gets a real visible
#      console window in the interactive session. This is the same SHAPE as the
#      real world leak (a console whose creator is not in the job, left parked on
#      a keypress that never comes), and the only thing that can clean it up is
#      the runner's audit.
#
#   Win32_Process.Create also reproduces the exact command line shape seen in the
#   wild, `cmd.exe /c pause` with no quoting, rather than the quoted form
#   PowerShell produces. Both shapes must be recognised, so the suite leaks one
#   of each.
#
# WHY IT SKIPS WHEN RUN BY HAND
# A suite that leaks a console window would, run standalone, leave that window on
# the desktop: exactly the thing being fixed. So it only leaks when the runner is
# the caller, which it detects through PSMUX_TEST_RUNNER=1 (set by
# run_all_tests.ps1 for every suite it starts). Run by hand it SKIPS and touches
# nothing.

$ErrorActionPreference = 'Continue'

$pass = 0
$fail = 0
function P { param([string]$m) Write-Host "[PASS] $m" -ForegroundColor Green; $script:pass++ }
function F { param([string]$m) Write-Host "[FAIL] $m" -ForegroundColor Red; $script:fail++ }
function S { param([string]$m) Write-Host "[SKIP] $m" -ForegroundColor Yellow }

Write-Host "=== runner leftover audit: synthetic leaking suite ===" -ForegroundColor Cyan

if ($env:PSMUX_TEST_RUNNER -ne '1') {
    S "not started by run_all_tests.ps1 (PSMUX_TEST_RUNNER is not 1)"
    Write-Host ""
    Write-Host "This suite deliberately leaks console windows so the runner's per suite" -ForegroundColor DarkGray
    Write-Host "audit has something to catch. Leaking them outside a run would strand" -ForegroundColor DarkGray
    Write-Host "them on the desktop, so nothing is spawned here." -ForegroundColor DarkGray
    Write-Host "  Run it through the runner:  tests\run_full_interactive.cmd -Only runner_leftover_audit" -ForegroundColor DarkGray
    Write-Host ""
    Write-Host "PASS: 0  FAIL: 0  (skipped)"
    exit 0
}

# Record what we create so the audit's own output can be checked against it.
$pidFile = Join-Path $env:TEMP 'psmux_leftover_audit_pids.txt'
Remove-Item $pidFile -Force -ErrorAction SilentlyContinue

function Get-PausePids {
    @(Get-CimInstance -Query "SELECT ProcessId,ParentProcessId,CommandLine FROM Win32_Process WHERE Name='cmd.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -match '/c\s+pause\b' })
}

$before = @(Get-PausePids | ForEach-Object { [int]$_.ProcessId })
Write-Host "  parked consoles already present before this suite: $($before.Count)" -ForegroundColor DarkGray

# ── Case A: a leak the job object can reach ──────────────────────────────────
Write-Host "`n[Case A] in-job leak: Start-Process cmd.exe /c pause" -ForegroundColor Yellow
$aPid = 0
try {
    # No -WindowStyle and no -NoNewWindow on purpose: this must get a real console
    # of its own, which on Windows 11 is a Terminal tab or a conhost window.
    $a = Start-Process -FilePath 'cmd.exe' -ArgumentList '/c', 'pause' -PassThru -ErrorAction Stop
    $aPid = $a.Id
    Start-Sleep -Milliseconds 800
    if (Get-Process -Id $aPid -ErrorAction SilentlyContinue) {
        P "in-job parked console is alive (pid=$aPid); the job object owns it"
    } else {
        F "in-job parked console died immediately (pid=$aPid)"
    }
} catch {
    F "could not start the in-job parked console: $_"
}

# ── Case B: a leak the job object cannot reach ───────────────────────────────
Write-Host "`n[Case B] escaped leak: Win32_Process.Create (parented to WmiPrvSE, never in our job)" -ForegroundColor Yellow
$bPid = 0
try {
    $r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create `
            -Arguments @{ CommandLine = 'cmd.exe /c pause' } -ErrorAction Stop
    if ($r.ReturnValue -ne 0) {
        F "Win32_Process.Create refused the request (ReturnValue=$($r.ReturnValue))"
    } else {
        $bPid = [int]$r.ProcessId
        Start-Sleep -Milliseconds 1200
        $bProc = Get-CimInstance -Query "SELECT ProcessId,ParentProcessId,CommandLine FROM Win32_Process WHERE ProcessId=$bPid" -ErrorAction SilentlyContinue
        if (-not $bProc) {
            F "escaped parked console died immediately (pid=$bPid)"
        } else {
            P "escaped parked console is alive (pid=$bPid) cmd=[$($bProc.CommandLine)]"
            $par = Get-CimInstance -Query "SELECT Name FROM Win32_Process WHERE ProcessId=$($bProc.ParentProcessId)" -ErrorAction SilentlyContinue
            $parName = if ($par) { $par.Name } else { '<exited>' }
            if ($parName -match 'WmiPrvSE') {
                P "its parent is $parName, so it is outside this suite's job object and survives the job kill"
            } else {
                # Not fatal: the audit catches it either way, but say so loudly,
                # because a changed parentage means the escape route moved and the
                # job object might now reap it before the audit ever sees it.
                Write-Host "  NOTE: parent is $parName, not WmiPrvSE; the escape route may have changed" -ForegroundColor Yellow
                P "escaped parked console created (pid=$bPid, parent=$parName)"
            }
        }
    }
} catch {
    F "could not create the escaped parked console: $_"
}

# The audit has to see BOTH command line shapes: the quoted one PowerShell emits
# and the bare one Win32_Process.Create (and portable_pty's CommandBuilder) emit.
$now = Get-PausePids
$newOnes = @($now | Where-Object { $before -notcontains [int]$_.ProcessId })
if ($newOnes.Count -ge 2) {
    P "two new parked consoles exist for the runner to find ($(($newOnes | ForEach-Object { $_.ProcessId }) -join ', '))"
} else {
    F "expected 2 new parked consoles, found $($newOnes.Count)"
}
$shapes = @($newOnes | ForEach-Object { ([string]$_.CommandLine).Trim() })
if (($shapes | Where-Object { $_ -like '"*' }).Count -ge 1 -and ($shapes | Where-Object { $_ -notlike '"*' }).Count -ge 1) {
    P "both command line shapes are present (quoted and bare full path)"
} else {
    F "expected one quoted and one bare command line, got: $($shapes -join ' | ')"
}

foreach ($o in $newOnes) {
    $decl = "{0}`t{1}" -f $o.ProcessId, (([string]$o.CommandLine) -replace '\s+', ' ').Trim()
    $decl | Add-Content -Path $pidFile -Encoding UTF8
    # Case B's creator is WmiPrvSE (under svchost), which the runner protects as
    # foreign unless the suite declares the process as its own.
    if ($env:PSMUX_TEST_DECLARED_PIDS) { $decl | Add-Content -Path $env:PSMUX_TEST_DECLARED_PIDS -Encoding UTF8 }
}
if ($env:PSMUX_TEST_DECLARED_PIDS) {
    P "declared both pids to the runner in $env:PSMUX_TEST_DECLARED_PIDS"
} else {
    F "runner did not provide PSMUX_TEST_DECLARED_PIDS, so it cannot end the escaped console"
}
Write-Host ""
Write-Host "  leaked pids recorded in: $pidFile" -ForegroundColor DarkGray
Write-Host "  EXPECTATION: the runner reports this suite in LEFTOVER lines, ends both" -ForegroundColor DarkGray
Write-Host "  processes and closes their console windows. Nothing should remain." -ForegroundColor DarkGray

Write-Host ""
Write-Host "PASS: $pass  FAIL: $fail"
# Exit 0 even though the suite leaks: the point is that the SUITE passes while the
# runner still flags it. A leftover must not be smuggled in as a suite failure,
# it is a separate, separately reported defect.
exit 0
