# #232 follow up: a script querying a session must not make the server push
# its attached clients frames they already have.
#
# test_issue232_status_interval failed only when something outside the suite
# queried the session about once a second: every request (even list-sessions
# or show-options) ended in a pushed frame, mostly a byte for byte copy of the
# last one, so the suite saw a second frame tens of milliseconds after each
# real one ("p50=28ms", "status-interval=5: got 8 frames in 7s"). tmux redraws
# a client only when what it shows changed.
#
# Runs in its own namespace and data dir. PSMUX_EXE picks the binary.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$NS = "t232poll" + (Get-Random -Maximum 999999)
$DATA = Join-Path $env:TEMP ("psmux_" + $NS)
New-Item -ItemType Directory -Force $DATA | Out-Null
$env:PSMUX_DATA_DIR = $DATA
$S = "poll"
$script:Passed = 0; $script:Failed = 0
function Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Passed++ }
function Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Failed++ }

function Measure-Frames {
    param([string]$PollCommand, [double]$Secs = 7)
    $port = (Get-Content "$DATA\${NS}__$S.port" -Raw).Trim()
    $key = (Get-Content "$DATA\${NS}__$S.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $st = $tcp.GetStream()
    $w = [System.IO.StreamWriter]::new($st); $r = [System.IO.StreamReader]::new($st)
    $w.Write("AUTH $key`n"); $w.Flush(); $null = $r.ReadLine()
    $w.Write("PERSISTENT`n"); $w.Flush()
    $w.Write("dump-state`n"); $w.Flush()
    $job = $null
    if ($PollCommand) {
        $job = Start-Job -ScriptBlock {
            param($e, $n, $d, $cmd, $secs)
            $env:PSMUX_DATA_DIR = $d
            $end = (Get-Date).AddSeconds($secs)
            while ((Get-Date) -lt $end) { & $e -L $n @($cmd -split ' ') 2>&1 | Out-Null; Start-Sleep -Milliseconds 1000 }
        } -ArgumentList $PSMUX, $NS, $DATA, $PollCommand, ($Secs + 2)
    }
    $tcp.ReceiveTimeout = 1500
    $frames = @()
    $t0 = [DateTime]::Now
    while (([DateTime]::Now - $t0).TotalSeconds -lt $Secs) {
        try {
            $line = $r.ReadLine()
            if ($null -ne $line -and $line -ne "NC" -and $line.Length -gt 100) { $frames += $line }
        } catch {}
    }
    $tcp.Close()
    if ($job) { Stop-Job $job; Remove-Job $job -Force }
    $dups = 0
    # Frames 0 and 1 are this connection's own dump-state reply and the pushed
    # copy that always follows it (readers confirm a state by that copy), so
    # repeats are counted from the third frame on.
    for ($i = 2; $i -lt $frames.Count; $i++) { if ($frames[$i] -eq $frames[$i - 1]) { $dups++ } }
    [PSCustomObject]@{ Count = $frames.Count; Duplicates = $dups }
}

& $PSMUX -L $NS new-session -d -s $S
Start-Sleep -Seconds 3
& $PSMUX -L $NS has-session -t $S 2>$null
if ($LASTEXITCODE -ne 0) { Fail "session did not start"; exit 1 }
& $PSMUX -L $NS set -g status-interval 5
& $PSMUX -L $NS set -g status-right '%H:%M:%S'
Start-Sleep -Milliseconds 800

Write-Host "`n=== #232: queries do not push frames ===" -ForegroundColor Cyan

Write-Host "`n[1] baseline: no poller, status-interval 5, 7 s" -ForegroundColor Yellow
$m = Measure-Frames -PollCommand ""
Write-Host "    frames=$($m.Count) duplicates=$($m.Duplicates)" -ForegroundColor DarkGray
if ($m.Duplicates -eq 0) { Pass "no frame repeats the one before it" } else { Fail "$($m.Duplicates) duplicate frames with nobody polling" }
if ($m.Count -le 4) { Pass "$($m.Count) frames (reply plus status ticks)" } else { Fail "$($m.Count) frames in 7 s at status-interval 5" }

foreach ($cmd in @("list-sessions", "show-options -g", "has-session -t $S", "list-panes -a -F #{session_name}:#{pane_id}:#{pane_current_command}", "display-message -p -t $S #{cursor_x},#{cursor_y}", "capture-pane -p -e -t $S")) {
    Write-Host "`n[2] '$cmd' once a second, status-interval 5, 7 s" -ForegroundColor Yellow
    $m = Measure-Frames -PollCommand $cmd
    Write-Host "    frames=$($m.Count) duplicates=$($m.Duplicates)" -ForegroundColor DarkGray
    if ($m.Duplicates -eq 0) { Pass "$cmd`: no duplicate frame" } else { Fail "$cmd`: $($m.Duplicates) frames identical to the previous one" }
    if ($m.Count -le 4) { Pass "$cmd`: $($m.Count) frames, the poller adds none" } else { Fail "$cmd`: $($m.Count) frames in 7 s (expected at most 4)" }
}

Write-Host "`n[3] a real change still reaches the client" -ForegroundColor Yellow
$port = (Get-Content "$DATA\${NS}__$S.port" -Raw).Trim()
$key = (Get-Content "$DATA\${NS}__$S.key" -Raw).Trim()
$tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
$st = $tcp.GetStream(); $w = [System.IO.StreamWriter]::new($st); $r = [System.IO.StreamReader]::new($st)
$w.Write("AUTH $key`n"); $w.Flush(); $null = $r.ReadLine()
$w.Write("PERSISTENT`n"); $w.Flush(); $w.Write("dump-state`n"); $w.Flush()
$tcp.ReceiveTimeout = 1500
try { $null = $r.ReadLine() } catch {}
& $PSMUX -L $NS rename-window -t $S renamed232
$seen = $false; $t0 = [DateTime]::Now
while (-not $seen -and ([DateTime]::Now - $t0).TotalSeconds -lt 4) {
    try { $line = $r.ReadLine(); if ($line -match 'renamed232') { $seen = $true } } catch {}
}
$tcp.Close()
if ($seen) { Pass "rename-window pushed a frame with the new name" } else { Fail "rename-window never reached the attached client" }

Write-Host "`n[4] pane output and the status clock still reach the client while a poller runs" -ForegroundColor Yellow
& $PSMUX -L $NS set -g status-interval 1
$tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
$st = $tcp.GetStream(); $w = [System.IO.StreamWriter]::new($st); $r = [System.IO.StreamReader]::new($st)
$w.Write("AUTH $key`n"); $w.Flush(); $null = $r.ReadLine()
$w.Write("PERSISTENT`n"); $w.Flush(); $w.Write("dump-state`n"); $w.Flush()
$tcp.ReceiveTimeout = 1500
$job = Start-Job -ScriptBlock { param($e, $n, $d, $s) $env:PSMUX_DATA_DIR = $d; $end = (Get-Date).AddSeconds(8); while ((Get-Date) -lt $end) { & $e -L $n list-panes -a 2>&1 | Out-Null; Start-Sleep -Milliseconds 300 } } -ArgumentList $PSMUX, $NS, $DATA, $S
Start-Sleep -Milliseconds 500
& $PSMUX -L $NS send-keys -t $S "echo marker232_$NS" Enter
$clocks = @{}; $echo = $false; $t0 = [DateTime]::Now
while (([DateTime]::Now - $t0).TotalSeconds -lt 5) {
    try {
        $line = $r.ReadLine()
        if ($line -match "marker232_$NS") { $echo = $true }
        if ($line -match '"status_right"\s*:\s*"([^"]*)"') { $clocks[$matches[1]] = 1 }
    } catch {}
}
$tcp.Close(); Stop-Job $job; Remove-Job $job -Force
if ($echo) { Pass "pane output reached the client" } else { Fail "pane output never reached the client" }
if ($clocks.Count -ge 4) { Pass "status clock ticked: $($clocks.Count) distinct values in 5 s" } else { Fail "status clock gave only $($clocks.Count) distinct values in 5 s at status-interval 1" }

& $PSMUX -L $NS kill-server 2>&1 | Out-Null
Write-Host "`nPassed: $script:Passed  Failed: $script:Failed"
exit $script:Failed
