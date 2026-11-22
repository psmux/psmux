#!/usr/bin/env pwsh
# test_mouse_hover.ps1 - Verify mouse hover (Moved) forwarding to a child PTY
#
# An attached client maps MouseEventKind::Moved to a semantic pane-mouse or
# mouse-move command. The server forwards SGR button 35 only when the child
# enables ButtonEventMouseTracking or AnyEventMouseTracking (DECSET 1002/1003).
#
# Windows Terminal reference:
#   WT only sends hover events when:
#     - ButtonEventMouseTracking (1002): motion + button pressed
#     - AnyEventMouseTracking (1003): ALL motion (bare hover)
#   WT uses SGR button encoding: hover adds +0x20; bare move = button 3+32 = 35
#
# This test injects a real console MOUSE_MOVED record into an attached client
# and requires the exact SGR bytes at a mouse-aware child.

$ErrorActionPreference = "Continue"
$pass = 0; $fail = 0; $total = 0

function Test($name, $cond) {
    $script:total++
    if ($cond) { $script:pass++; Write-Host "  [PASS] $name" -ForegroundColor Green }
    else       { $script:fail++; Write-Host "  [FAIL] $name" -ForegroundColor Red }
}

$psmux = Get-Command psmux -ErrorAction SilentlyContinue
if (-not $psmux) { Write-Host "psmux not found in PATH"; exit 1 }
$ver = & psmux -V 2>&1 | Out-String
Write-Host "psmux version: $ver"

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
}
$runId = [guid]::NewGuid().ToString("N")
$artifactRoot = Join-Path $env:TEMP "psmux-hover-$runId"
$mouseChild = Join-Path $artifactRoot "mouse_echo_child.exe"
$mouseInjector = Join-Path $artifactRoot "mouse_move_injector.exe"
$mouseLog = Join-Path $artifactRoot "mouse_echo.txt"
$injectorLog = Join-Path $artifactRoot "mouse_move_inject.log"
$namespace = "hover_$($runId.Substring(0, 12))"
New-Item -ItemType Directory -Path $artifactRoot | Out-Null
foreach ($helper in @(
    @($mouseChild, "mouse_echo_child.cs"),
    @($mouseInjector, "mouse_move_injector.cs")
)) {
    $compilerOutput = & $csc /nologo /optimize /out:$($helper[0]) (Join-Path $PSScriptRoot $helper[1]) 2>&1
    if (-not (Test-Path $helper[0])) {
        Write-Host "Failed to compile $($helper[1])" -ForegroundColor Red
        $compilerOutput | Write-Host
        Remove-Item $artifactRoot -Recurse -Force -ErrorAction SilentlyContinue
        exit 1
    }
}
$echoLogEnvExisted = Test-Path Env:\PSMUX_MOUSE_ECHO_LOG
$moveLogEnvExisted = Test-Path Env:\PSMUX_MOUSE_MOVE_LOG
$previousEchoLog = $env:PSMUX_MOUSE_ECHO_LOG
$previousMoveLog = $env:PSMUX_MOUSE_MOVE_LOG
$psmuxDir = "$env:USERPROFILE\.psmux"
$clientProcess = $null
# Sweep 2026-09-27_02-44-24: this suite failed 3 of 3 with the physical pointer at
# the screen centre and passed 3 of 3 with it parked in a corner. Windows sends a
# WM_MOUSEMOVE to a window that appears under the pointer, conhost turns it into a
# MOUSE_MOVED record, and psmux forwards it as an any-motion hover, which is
# correct (tmux does the same under 1003) but lands in the child before the
# injected record and breaks the "exactly one record" assertion. So the pointer
# is parked in the bottom right corner for the length of the test and put back
# afterwards. If the desktop refuses the move the assertion falls back to "the
# injected record arrived" and says so.
Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices;
public struct HoverPt { public int X; public int Y; }
public static class HoverCursor {
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out HoverPt p);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int n);
}
"@
$pointerSaved = New-Object HoverPt
$pointerParked = $false
try {
    if ([HoverCursor]::GetCursorPos([ref]$pointerSaved)) {
        $w = [HoverCursor]::GetSystemMetrics(0); $h = [HoverCursor]::GetSystemMetrics(1)
        $pointerParked = [HoverCursor]::SetCursorPos($w - 2, $h - 2)
    }
} catch { $pointerParked = $false }
Write-Host "  [INFO] pointer was at $($pointerSaved.X),$($pointerSaved.Y); parked in the corner: $pointerParked"

try {
    $env:PSMUX_MOUSE_ECHO_LOG = $mouseLog
    $env:PSMUX_MOUSE_MOVE_LOG = $injectorLog

    # Clean only this test's namespace.
    & $psmux.Source -L $namespace kill-server 2>$null
    Start-Sleep -Milliseconds 500

    # ── Test 1: Start the live attached-client route ──
    Write-Host "`n=== Test Group 1: Mouse hover event routing ==="

    $clientProcess = Start-Process `
        -FilePath $psmux.Source `
        -ArgumentList "-L",$namespace,"new-session","-s","hover_test",$mouseChild `
        -PassThru
    $sessionReady = $false
    for ($i = 0; $i -lt 20 -and -not $sessionReady; $i++) {
        Start-Sleep -Milliseconds 250
        $sessionReady = Test-Path (Join-Path $psmuxDir "${namespace}__hover_test.port")
    }
    Test "Attached client session started" $sessionReady

    $mouseChildReady = $false
    if ($sessionReady) {
        & $psmux.Source -L $namespace set -g mouse on 2>$null
        for ($i = 0; $i -lt 20 -and -not $mouseChildReady; $i++) {
            Start-Sleep -Milliseconds 250
            $paneText = (& $psmux.Source -L $namespace capture-pane -t hover_test -p 2>&1) | Out-String
            $mouseChildReady = $paneText -match 'MOUSE_ECHO_READY'
        }
    }
    Test "Mouse-reporting child enabled any-motion tracking" $mouseChildReady

    $moveInjected = $false
    if ($mouseChildReady -and -not $clientProcess.HasExited) {
        Start-Sleep -Milliseconds 500
        Remove-Item $injectorLog -Force -ErrorAction SilentlyContinue
        $injectorOutput = & $mouseInjector $clientProcess.Id move 1 10 5 0 0 0 2>&1
        $injectorExitCode = $LASTEXITCODE
        $injectorEvidence = if (Test-Path $injectorLog) {
            Get-Content $injectorLog -Raw
        } else {
            ""
        }
        $moveInjected = $injectorExitCode -eq 0 -and
            $injectorEvidence -match 'move\[0\].*ok=True written=1'
        if (-not $moveInjected) {
            $injectorOutput | Write-Host
            if ($injectorEvidence) {
                $injectorEvidence | Write-Host
            } else {
                Write-Host "Mouse injector log was not created" -ForegroundColor Red
            }
        }
    }
    Test "Real MOUSE_MOVED record injected into the attached client" $moveInjected

    # ── Test 2: End-to-end PTY delivery ──
    Write-Host "`n=== Test Group 2: End-to-end PTY delivery ==="

    $expectedHover = '<ESC>[<35;11;6M'
    $expectedRecord = "RECV $expectedHover  |  1B 5B 3C 33 35 3B 31 31 3B 36 4D"
    $receiveLines = @()
    $previousReceiveCount = -1
    $stablePolls = 0
    for ($i = 0; $i -lt 28; $i++) {
        Start-Sleep -Milliseconds 250
        if (Test-Path $mouseLog) {
            $receiveLines = @(Get-Content $mouseLog | Where-Object { $_ -like "RECV *" })
        }
        if ($receiveLines.Count -eq $previousReceiveCount) {
            $stablePolls++
        } else {
            $previousReceiveCount = $receiveLines.Count
            $stablePolls = 0
        }
        if ($receiveLines.Count -gt 0 -and $stablePolls -ge 8) {
            break
        }
    }
    if ([Environment]::OSVersion.Version.Build -lt 22000) {
        # The child reads its mouse as VT bytes. The Windows 10 inbox conhost
        # (build 19045) keeps the child's DECSET 1003 to itself and drops an
        # SGR report written to the pane, so no hover reaches it there (Issue
        # #1089, docs/faq.md; measured with SGR reports typed with
        # send-keys -l into a raw VT reader, which got only the letters around
        # them).
        $hoverDelivered = @($receiveLines | Where-Object { $_ -match '<ESC>\[<' }).Count -eq 0
        Test "Attached-client hover: on this host the VT reading child got no report, as measured" $hoverDelivered
    } elseif ($pointerParked) {
        $hoverDelivered = $receiveLines.Count -eq 1 -and $receiveLines[0] -eq $expectedRecord
        Test "Attached-client hover produces exactly $expectedRecord" $hoverDelivered
    } else {
        # The physical pointer could not be moved, so real motion over the new
        # window may precede the injected record. Require the injected record
        # and nothing that is not a hover.
        $hoverDelivered = ($receiveLines -contains $expectedRecord) -and
            (@($receiveLines | Where-Object { $_ -notmatch "^RECV (<ESC>\[<35;\d+;\d+M)+  \|" }).Count -eq 0)
        Test "Attached-client hover delivers $expectedRecord (pointer not parked, real motion tolerated)" $hoverDelivered
    }
    if (-not $hoverDelivered) {
        # Sweep 2026-09-27_02-44-24 failed this assertion and the log carried no
        # evidence of what the child saw, so the artifacts were gone before anyone
        # could look. Print the whole child log and the injector log on failure.
        Write-Host "  [INFO] child received $($receiveLines.Count) RECV line(s):"
        foreach ($line in $receiveLines) { Write-Host "         $line" }
        if (Test-Path $mouseLog) { Write-Host "  [INFO] child log:"; Get-Content $mouseLog | ForEach-Object { Write-Host "         $_" } }
        if (Test-Path $injectorLog) { Write-Host "  [INFO] injector log:"; Get-Content $injectorLog | ForEach-Object { Write-Host "         $_" } }
    }
}
finally {
    if ($pointerParked) { [void][HoverCursor]::SetCursorPos($pointerSaved.X, $pointerSaved.Y) }
    & $psmux.Source -L $namespace kill-server 2>$null
    if ($null -ne $clientProcess -and -not $clientProcess.HasExited) {
        Stop-Process -Id $clientProcess.Id -Force -ErrorAction SilentlyContinue
        [void]$clientProcess.WaitForExit(3000)
    }

    $ownedFiles = @()
    $clientStopped = $false
    $namespaceClean = $false
    for ($i = 0; $i -lt 12 -and -not $namespaceClean; $i++) {
        $ownedFiles = @(Get-ChildItem $psmuxDir -Filter "${namespace}__*" -ErrorAction SilentlyContinue)
        $clientStopped = $null -eq $clientProcess -or $clientProcess.HasExited
        $namespaceClean = $clientStopped -and $ownedFiles.Count -eq 0
        if (-not $namespaceClean) {
            Start-Sleep -Milliseconds 250
        }
    }
    Test "Owned client and namespace cleaned up" $namespaceClean
    if (-not $namespaceClean) {
        Write-Host "Client stopped: $clientStopped" -ForegroundColor Red
        $ownedFiles.FullName | Write-Host
        $ownedFiles | Remove-Item -Force -ErrorAction SilentlyContinue
    }

    # mouse_echo_child.exe can stay briefly locked after its process exits
    # (Windows releases the image handle a moment after termination), so a
    # single Remove-Item races the lock. Retry for up to ~2.5s before asserting.
    for ($rm = 0; $rm -lt 10; $rm++) {
        Remove-Item $artifactRoot -Recurse -Force -ErrorAction SilentlyContinue
        if (-not (Test-Path $artifactRoot)) { break }
        Start-Sleep -Milliseconds 250
    }
    Test "Run-specific artifacts cleaned up" (-not (Test-Path $artifactRoot))

    if ($echoLogEnvExisted) {
        $env:PSMUX_MOUSE_ECHO_LOG = $previousEchoLog
    } else {
        Remove-Item Env:\PSMUX_MOUSE_ECHO_LOG -ErrorAction SilentlyContinue
    }
    if ($moveLogEnvExisted) {
        $env:PSMUX_MOUSE_MOVE_LOG = $previousMoveLog
    } else {
        Remove-Item Env:\PSMUX_MOUSE_MOVE_LOG -ErrorAction SilentlyContinue
    }
    $environmentRestored = (
        ($echoLogEnvExisted -and $env:PSMUX_MOUSE_ECHO_LOG -ceq $previousEchoLog) -or
        (-not $echoLogEnvExisted -and -not (Test-Path Env:\PSMUX_MOUSE_ECHO_LOG))
    ) -and (
        ($moveLogEnvExisted -and $env:PSMUX_MOUSE_MOVE_LOG -ceq $previousMoveLog) -or
        (-not $moveLogEnvExisted -and -not (Test-Path Env:\PSMUX_MOUSE_MOVE_LOG))
    )
    Test "Caller log environment restored" $environmentRestored
}

Write-Host "`n============================================"
Write-Host "Results: $pass passed, $fail failed, $total total"
if ($fail -eq 0) { Write-Host "ALL TESTS PASSED" -ForegroundColor Green }
else { Write-Host "SOME TESTS FAILED" -ForegroundColor Red }

exit $fail
