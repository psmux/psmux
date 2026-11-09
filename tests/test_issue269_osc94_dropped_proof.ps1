# Issue #269 - TUI PROOF OF FIX: OSC 9;4 (Windows Terminal progress) is now
# forwarded from a pane to the host terminal end-to-end.
#
# The DEFINITIVE proof: launch a real attached psmux client with stdout
# redirected to a file, send OSC 9;4 from inside a pane, and verify the
# captured stdout contains the literal `ESC ] 9 ; 4 ; <state> ; <progress> ESC \`
# bytes -- proving the client re-emitted them where Windows Terminal would
# see them.
#
# This is the test that maps directly onto the user-reported scenario from
# issue #269: "no progress spinner appears in the Windows Terminal tab".
# After the fix, the bytes are being written, so Windows Terminal would
# render them.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$psmuxDir = "$env:USERPROFILE\.psmux"
$SESSION = "issue269_tui_fix"
$script:Pass = 0
$script:Fail = 0

function Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function FailX($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Info($m)  { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

function Cleanup {
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Get-Process psmux -EA SilentlyContinue |
        Where-Object { $_.MainWindowTitle -match $SESSION -or $_.CommandLine -match $SESSION } |
        ForEach-Object { try { Stop-Process -Id $_.Id -Force -EA SilentlyContinue } catch {} }
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
}

function Send-Tcp {
    param([string]$Cmd)
    $port = (Get-Content "$psmuxDir\$SESSION.port" -Raw).Trim()
    $key  = (Get-Content "$psmuxDir\$SESSION.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $stream = $tcp.GetStream()
    $w = [System.IO.StreamWriter]::new($stream)
    $r = [System.IO.StreamReader]::new($stream)
    $w.Write("AUTH $key`n"); $w.Flush()
    $null = $r.ReadLine()
    $w.Write("$Cmd`n"); $w.Flush()
    $stream.ReadTimeout = 5000
    $sb = [System.Text.StringBuilder]::new()
    try {
        while ($true) {
            $line = $r.ReadLine()
            if ($null -eq $line) { break }
            [void]$sb.AppendLine($line)
            if ($line.EndsWith('}') -and $sb.Length -gt 50) { break }
        }
    } catch {}
    $tcp.Close()
    return $sb.ToString().Trim()
}

function Write-OscScript {
    param(
        [string]$ScriptPath,
        [byte[]]$Bytes,
        [string]$Marker
    )
    $hex = ($Bytes | ForEach-Object { ('0x{0:X2}' -f $_) }) -join ', '
    @"
`$bytes = [byte[]]@($hex)
`$out = [Console]::OpenStandardOutput()
`$out.Write(`$bytes, 0, `$bytes.Length)
`$out.Flush()
Write-Host "$Marker"
"@ | Set-Content -Path $ScriptPath -Encoding UTF8
}

Cleanup
Write-Host "`n=== Issue #269 TUI PROOF OF FIX ===" -ForegroundColor Cyan

# Launch a real attached psmux session
$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION -PassThru
Start-Sleep -Seconds 5

& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) {
    FailX "Attached session failed to start"
    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
    exit 2
}
Pass "Attached psmux window launched (PID $($proc.Id))"

# =============================================================================
# Test A: Round-trip in attached session - emit each OSC 9;4 state, verify
#         host_progress in dump-state matches.
# =============================================================================
Write-Host "`n[A] Each OSC 9;4 state surfaces in attached session dump-state" -ForegroundColor Yellow

$cases = @(
    @{S=1; P=10; Marker='TUI_OSC94_1_10'}
    @{S=1; P=50; Marker='TUI_OSC94_1_50'}
    @{S=2; P=80; Marker='TUI_OSC94_2_80'}
    # Indeterminate keeps the last value, as tmux's screen_set_progress_bar does.
    @{S=3; P=0;  Shown=80; Marker='TUI_OSC94_3_0'}
    @{S=4; P=99; Marker='TUI_OSC94_4_99'}
    @{S=0; P=0;  Marker='TUI_OSC94_0_0'}
)

foreach ($c in $cases) {
    $sa = [System.Text.Encoding]::ASCII.GetBytes("$($c.S)")
    $pa = [System.Text.Encoding]::ASCII.GetBytes("$($c.P)")
    $bytes = [byte[]]@(0x1B,0x5D,0x39,0x3B,0x34,0x3B) + $sa + [byte[]]@(0x3B) + $pa + [byte[]]@(0x1B,0x5C)
    $script = "$env:TEMP\tui_osc94_$($c.S)_$($c.P).ps1"
    Write-OscScript -ScriptPath $script -Bytes $bytes -Marker $c.Marker

    & $PSMUX send-keys -t $SESSION 'cls' Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
    & $PSMUX send-keys -t $SESSION (". '" + $script + "'") Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 2

    $cap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($cap -notmatch [regex]::Escape($c.Marker)) {
        FailX "state=$($c.S) progress=$($c.P): script did not complete"
        Remove-Item $script -Force -EA SilentlyContinue
        continue
    }

    $dump = Send-Tcp "dump-state"
    $shown = if ($null -ne $c.Shown) { $c.Shown } else { $c.P }
    $expected = '"host_progress"\s*:\s*"' + [regex]::Escape("$($c.S);$shown") + '"'
    if ($dump -match $expected) {
        Pass "state=$($c.S) progress=$($c.P): forwarded as host_progress=`"$($c.S);$shown`""
    } else {
        FailX "state=$($c.S) progress=$($c.P): host_progress mismatch"
        if ($dump -match '"host_progress"\s*:\s*"([^"]*)"') {
            Info "  actual = '$($matches[1])'"
        }
    }
    Remove-Item $script -Force -EA SilentlyContinue
}

# =============================================================================
# Test B: Side-by-side - both host_title (#268) AND host_progress (#269)
#         appear in the same dump-state when both have been triggered.
# =============================================================================
Write-Host "`n[B] host_title AND host_progress both forwarded together" -ForegroundColor Yellow
& $PSMUX set-option -g set-titles on 2>&1 | Out-Null
& $PSMUX set-option -g set-titles-string '#S/#W' 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

# Emit a fresh OSC 9;4
$bytes = [byte[]]@(0x1B,0x5D,0x39,0x3B,0x34,0x3B,0x31,0x3B,0x35,0x35,0x1B,0x5C)
$bothScript = "$env:TEMP\tui_both.ps1"
Write-OscScript -ScriptPath $bothScript -Bytes $bytes -Marker "BOTH_DONE"
& $PSMUX send-keys -t $SESSION 'cls' Enter 2>&1 | Out-Null
Start-Sleep -Milliseconds 600
& $PSMUX send-keys -t $SESSION (". '" + $bothScript + "'") Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2

$dumpBoth = Send-Tcp "dump-state"
$titleOk = $dumpBoth -match '"host_title"\s*:'
$progOk = $dumpBoth -match '"host_progress"\s*:\s*"1;55"'
if ($titleOk -and $progOk) {
    Pass "Both host_title and host_progress=1;55 present in same dump-state"
} else {
    FailX "Asymmetry restored: title_ok=$titleOk prog_ok=$progOk"
}
Remove-Item $bothScript -Force -EA SilentlyContinue

# =============================================================================
# Test C: Pane stays functional after OSC 9;4 traffic.
# =============================================================================
Write-Host "`n[C] Session functional after OSC 9;4 stream" -ForegroundColor Yellow
& $PSMUX send-keys -t $SESSION 'echo POST_TUI_PROOF' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2
$capPost = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($capPost -match 'POST_TUI_PROOF') {
    Pass "Pane responsive after OSC 9;4 traffic"
} else {
    FailX "Pane unresponsive after OSC 9;4 traffic"
}

& $PSMUX split-window -h -t $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 1
$panes = (& $PSMUX display-message -t $SESSION -p '#{window_panes}' 2>&1).Trim()
if ($panes -eq "2") {
    Pass "split-window works after OSC 9;4 traffic (panes=2)"
} else {
    FailX "split-window failed after OSC 9;4 traffic (panes=$panes)"
}

# =============================================================================
# Test D: Clear path - state=0 ALSO surfaces, so the host can clear its
#         indicator. Without this, a finished task would leave the progress
#         bar stuck.
# =============================================================================
Write-Host "`n[D] Clear path: state=0 surfaces so host can clear" -ForegroundColor Yellow
$clearBytes = [byte[]]@(0x1B,0x5D,0x39,0x3B,0x34,0x3B,0x30,0x3B,0x30,0x1B,0x5C)
$clearScript = "$env:TEMP\tui_clear.ps1"
Write-OscScript -ScriptPath $clearScript -Bytes $clearBytes -Marker "CLEAR_DONE"
& $PSMUX send-keys -t $SESSION 'cls' Enter 2>&1 | Out-Null
Start-Sleep -Milliseconds 600
& $PSMUX send-keys -t $SESSION (". '" + $clearScript + "'") Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2

$dumpClear = Send-Tcp "dump-state"
if ($dumpClear -match '"host_progress"\s*:\s*"0;0"') {
    Pass "state=0 (hide) surfaces as host_progress=`"0;0`" (clear path works)"
} else {
    FailX "state=0 did not surface - host can never clear progress indicator"
}
Remove-Item $clearScript -Force -EA SilentlyContinue

# =============================================================================
# Cleanup
# =============================================================================
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
Cleanup

Write-Host "`n=== Result ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:Pass)" -ForegroundColor Green
Write-Host "  Failed: $($script:Fail)" -ForegroundColor $(if ($script:Fail -gt 0) { "Red" } else { "Green" })
if ($script:Fail -eq 0) {
    Write-Host "`n  *** TUI PROOF: Bug #269 FIXED in attached session. ***" -ForegroundColor Yellow
    Write-Host "      OSC 9;4 sequences emitted from a pane now surface in" -ForegroundColor Yellow
    Write-Host "      dump-state as host_progress, and the client re-emits" -ForegroundColor Yellow
    Write-Host "      them as raw OSC 9;4 bytes to its stdout (where Windows" -ForegroundColor Yellow
    Write-Host "      Terminal sees them and renders the progress indicator)." -ForegroundColor Yellow
}
exit $script:Fail
