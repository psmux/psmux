# Issue #559: the "exit 0 / does-nothing" family reported by ChrisonSimtian.
#
# Reporter's four claims, verified against master:
#   1. split-window -t %N no-ops and -P returns a fabricated id  -> already
#      fixed on master for the no-op; residual bug: -P printed the server's
#      ERROR text to stdout with exit 0 (fixed here).
#   2. bind-key from the CLI registers nothing                   -> already fixed;
#      regression-pinned here.
#   3. set -g monitor-silence silently dropped                   -> value was
#      stored but invisible in the -g dump AND the silence flag never fired in
#      detached sessions (both fixed).
#   4. capture-pane -t %N ignores -L and reads the default server -> already
#      fixed; regression-pinned here.
# Extra members found while reproducing:
#   5. kill-window -t <bad> exited 0 silently (server sent the error, CLI
#      discarded it).
#   6. swap-window -s <bad>/-t <bad> silently no-opped with exit 0; a
#      qualified "-s sess:N" fell back to swapping the ACTIVE window.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

function Cleanup {
    foreach ($ns in @("i559a", "i559b", "i559c")) {
        & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    }
    & $PSMUX kill-session -t i559main 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
}

Cleanup
Write-Host "`n=== Issue #559 family tests ===" -ForegroundColor Cyan

# ============ PART A: split-window pane-id target (claim 1) ============
Write-Host "`n[A] split-window -t <pane-id> creates a real pane" -ForegroundColor Yellow
& $PSMUX -L i559a new-session -d -s r
Start-Sleep -Seconds 3
& $PSMUX -L i559a has-session -t r 2>$null
if ($LASTEXITCODE -ne 0) { Write-Fail "setup: session r not created"; exit 1 }

$initialPane = (& $PSMUX -L i559a list-panes -t r -F '#{pane_id}' 2>&1 | Select-Object -First 1).Trim()
$newId = (& $PSMUX -L i559a split-window -t $initialPane -P -F '#{pane_id}' 2>&1 | Out-String).Trim()
$ec = $LASTEXITCODE
Start-Sleep -Seconds 2
$panes = @(& $PSMUX -L i559a list-panes -t r -F '#{pane_id}' 2>&1 | ForEach-Object { $_.Trim() })

if ($ec -eq 0 -and $panes.Count -eq 2) { Write-Pass "pane-id target split created a second pane (exit 0)" }
else { Write-Fail "pane-id split: exit=$ec panes=$($panes.Count)" }
if ($panes -contains $newId) { Write-Pass "-P returned id '$newId' which really exists" }
else { Write-Fail "-P returned '$newId' but live panes are: $($panes -join ', ') (fabricated id)" }

# ============ PART B: split-window -P error path exit code ============
Write-Host "`n[B] split-window -P with a bad target: exit 1, no id on stdout" -ForegroundColor Yellow
$stdout = & $PSMUX -L i559a split-window -t r:99 -P -F '#{pane_id}' 2>$null | Out-String
$ec = $LASTEXITCODE
if ($ec -eq 1) { Write-Pass "bad window target with -P exits 1 (was 0)" }
else { Write-Fail "bad window target with -P exited $ec (expected 1)" }
if ($stdout.Trim() -eq "") { Write-Pass "nothing printed to stdout on failure (error goes to stderr)" }
else { Write-Fail "stdout polluted on failure: '$($stdout.Trim())' (a script would take this as the pane id)" }

$stdout2 = & $PSMUX -L i559a split-window -t %999 -P -F '#{pane_id}' 2>$null | Out-String
$ec2 = $LASTEXITCODE
if ($ec2 -eq 1 -and $stdout2.Trim() -eq "") { Write-Pass "bad pane-id target with -P: exit 1, clean stdout" }
else { Write-Fail "bad pane-id with -P: exit=$ec2 stdout='$($stdout2.Trim())'" }

# pane count must be unchanged after both failed splits
$panesAfter = @(& $PSMUX -L i559a list-panes -t r -F '#{pane_id}' 2>&1 | ForEach-Object { $_.Trim() })
if ($panesAfter.Count -eq 2) { Write-Pass "failed splits created nothing" }
else { Write-Fail "failed splits changed pane count: $($panesAfter.Count)" }

# ============ PART C: bind-key from CLI (claim 2 regression pin) ============
Write-Host "`n[C] bind-key from the CLI registers the binding" -ForegroundColor Yellow
& $PSMUX -L i559a bind-key e set-option -g synchronize-panes on
Start-Sleep -Milliseconds 500
$keys = & $PSMUX -L i559a list-keys -t r 2>&1 | Out-String
if ($keys -match "synchronize-panes") { Write-Pass "CLI bind-key registered (grep by COMMAND, not key)" }
else { Write-Fail "CLI bind-key registered nothing" }

# ============ PART D: monitor-silence (claim 3) ============
Write-Host "`n[D] monitor-silence: stored, visible, and functionally fires" -ForegroundColor Yellow
& $PSMUX -L i559a set -g monitor-silence 3
if ($LASTEXITCODE -eq 0) { Write-Pass "set -g monitor-silence 3 accepted" }
else { Write-Fail "set -g monitor-silence exit $LASTEXITCODE" }

$v = (& $PSMUX -L i559a show-options -g -v monitor-silence 2>&1 | Out-String).Trim()
if ($v -eq "3") { Write-Pass "show-options -g -v round-trips the value" }
else { Write-Fail "show-options -g -v returned '$v'" }

$dump = & $PSMUX -L i559a show-options -g 2>&1 | Out-String
if ($dump -match "monitor-silence 3") { Write-Pass "monitor-silence appears in the full -g dump (reporter's probe)" }
else { Write-Fail "monitor-silence missing from full show-options -g dump" }

$wv = (& $PSMUX -L i559a show-options -w -t r monitor-silence 2>&1 | Out-String).Trim()
if ($wv -match "monitor-silence\s+3") { Write-Pass "window-scoped query returns the value (tmux window option)" }
else { Write-Fail "show-options -w returned '$wv'" }

# Functional: create a second window so window 0 goes idle, then wait past the
# 3s threshold. This is a DETACHED session, the exact case that never fired.
& $PSMUX -L i559a new-window -t r 2>&1 | Out-Null
Start-Sleep -Seconds 2
Start-Sleep -Seconds 6
$flags = & $PSMUX -L i559a list-windows -t r -F '#{window_index}|#{window_flags}|#{window_silence_flag}' 2>&1
$w0 = ($flags | Where-Object { $_ -match '^0\|' } | Select-Object -First 1)
if ($w0 -match '\|1$') { Write-Pass "window_silence_flag=1 on idle window after threshold (detached)" }
else { Write-Fail "silence flag never fired: '$($flags -join ' / ')'" }
if ($w0 -match '~') { Write-Pass "~ flag shown in window_flags (tmux parity)" }
else { Write-Fail "no ~ in window_flags: '$w0'" }

# New output must clear the flag
& $PSMUX -L i559a send-keys -t r:0 "echo WAKE" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2
$flags2 = & $PSMUX -L i559a list-windows -t r -F '#{window_index}|#{window_silence_flag}' 2>&1
$w0b = ($flags2 | Where-Object { $_ -match '^0\|' } | Select-Object -First 1)
if ($w0b -match '\|0$') { Write-Pass "fresh output clears the silence flag" }
else { Write-Fail "flag did not clear after output: '$w0b'" }

# Config-file path
$conf = "$env:TEMP\psmux_559.conf"
"set -g monitor-silence 10" | Set-Content $conf -Encoding UTF8
& $PSMUX -L i559b -f $conf new-session -d -s mc
Start-Sleep -Seconds 3
$cv = (& $PSMUX -L i559b show-options -g -v monitor-silence 2>&1 | Out-String).Trim()
if ($cv -eq "10") { Write-Pass "monitor-silence via config file applies" }
else { Write-Fail "config path returned '$cv'" }

# ============ PART E: kill-window bad target exit code ============
Write-Host "`n[E] kill-window -t <bad>: error + exit 1, kills nothing" -ForegroundColor Yellow
$winsBefore = @(& $PSMUX -L i559a list-windows -t r -F '#{window_index}' 2>&1).Count
$err = & $PSMUX -L i559a kill-window -t r:99 2>&1 | Out-String
$ec = $LASTEXITCODE
$winsAfter = @(& $PSMUX -L i559a list-windows -t r -F '#{window_index}' 2>&1).Count
if ($ec -eq 1) { Write-Pass "kill-window bad target exits 1 (was silent 0)" }
else { Write-Fail "kill-window bad target exited $ec" }
if ($err -match "can't find window") { Write-Pass "diagnostic printed: $($err.Trim())" }
else { Write-Fail "no diagnostic, got: '$($err.Trim())'" }
if ($winsBefore -eq $winsAfter) { Write-Pass "no window was killed" }
else { Write-Fail "window count changed $winsBefore -> $winsAfter" }

# ============ PART F: swap-window bad targets ============
Write-Host "`n[F] swap-window with unresolvable -s / -t" -ForegroundColor Yellow
$namesBefore = (& $PSMUX -L i559a list-windows -t r -F '#{window_index}:#{window_name}' 2>&1) -join ","
$err = & $PSMUX -L i559a swap-window -s r:99 -t r:0 2>&1 | Out-String
$ec = $LASTEXITCODE
if ($ec -eq 1 -and $err -match "can't find window") { Write-Pass "bad -s: exit 1 + diagnostic" }
else { Write-Fail "bad -s: exit=$ec out='$($err.Trim())'" }

$err2 = & $PSMUX -L i559a swap-window -s 0 -t r:99 2>&1 | Out-String
$ec2 = $LASTEXITCODE
if ($ec2 -eq 1 -and $err2 -match "can't find window") { Write-Pass "bad -t: exit 1 + diagnostic" }
else { Write-Fail "bad -t: exit=$ec2 out='$($err2.Trim())'" }

$namesAfter = (& $PSMUX -L i559a list-windows -t r -F '#{window_index}:#{window_name}' 2>&1) -join ","
if ($namesBefore -eq $namesAfter) { Write-Pass "window order untouched by failed swaps" }
else { Write-Fail "failed swap changed order: '$namesBefore' -> '$namesAfter'" }

# Control: a VALID swap still works and exits 0
& $PSMUX -L i559a swap-window -s 0 -t 1 2>$null
if ($LASTEXITCODE -eq 0) { Write-Pass "valid swap-window still exits 0" }
else { Write-Fail "valid swap-window broke: exit $LASTEXITCODE" }
& $PSMUX -L i559a swap-window -s 0 -t 1 2>&1 | Out-Null   # swap back

# Control: kill-window with a VALID target still works
& $PSMUX -L i559a new-window -t r 2>&1 | Out-Null
Start-Sleep -Seconds 1
$wc1 = @(& $PSMUX -L i559a list-windows -t r -F 'x' 2>&1).Count
$lastIdx = (& $PSMUX -L i559a list-windows -t r -F '#{window_index}' 2>&1 | Select-Object -Last 1).Trim()
& $PSMUX -L i559a kill-window -t "r:$lastIdx" 2>$null
$kec = $LASTEXITCODE
Start-Sleep -Seconds 1
$wc2 = @(& $PSMUX -L i559a list-windows -t r -F 'x' 2>&1).Count
if ($kec -eq 0 -and $wc2 -eq ($wc1 - 1)) { Write-Pass "valid kill-window still works (exit 0, window gone)" }
else { Write-Fail "valid kill-window: exit=$kec count $wc1 -> $wc2" }

# ============ PART G: capture-pane -L isolation (claim 4 regression pin) ====
Write-Host "`n[G] capture-pane -t %N respects -L (no cross-server read)" -ForegroundColor Yellow
& $PSMUX new-session -d -s i559main
Start-Sleep -Seconds 3
& $PSMUX send-keys -t i559main "echo DEFAULT_MARKER_559" Enter 2>&1 | Out-Null
& $PSMUX -L i559c new-session -d -s f
Start-Sleep -Seconds 3
& $PSMUX -L i559c send-keys -t f "echo LSOCKET_MARKER_559" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1
$fPane = (& $PSMUX -L i559c list-panes -t f -F '#{pane_id}' 2>&1 | Select-Object -First 1).Trim()
$cap = & $PSMUX -L i559c capture-pane -t $fPane -p 2>&1 | Out-String
if ($cap -match "LSOCKET_MARKER_559" -and $cap -notmatch "DEFAULT_MARKER_559") {
    Write-Pass "capture by pane id stayed on the -L server"
} else {
    Write-Fail "cross-server capture: LSOCKET=$($cap -match 'LSOCKET_MARKER_559') DEFAULT=$($cap -match 'DEFAULT_MARKER_559')"
}

# ============ PART H: TCP server path (raw socket) ============
Write-Host "`n[H] TCP path: swap-window bad -s answers ERROR on the socket" -ForegroundColor Yellow
$base = "i559a__r"
$port = (Get-Content "$psmuxDir\$base.port" -Raw -EA SilentlyContinue)
if ($port) {
    $port = $port.Trim()
    $key = (Get-Content "$psmuxDir\$base.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $stream = $tcp.GetStream()
    $writer = [System.IO.StreamWriter]::new($stream)
    $reader = [System.IO.StreamReader]::new($stream)
    $writer.Write("AUTH $key`n"); $writer.Flush()
    $null = $reader.ReadLine()
    $writer.Write("swap-window -s nonsense:99 -t 0`n"); $writer.Flush()
    $stream.ReadTimeout = 5000
    try { $resp = $reader.ReadLine() } catch { $resp = "TIMEOUT" }
    $tcp.Close()
    if ($resp -match "can't find window") { Write-Pass "raw TCP got: $resp" }
    else { Write-Fail "raw TCP got: '$resp'" }
} else {
    Write-Fail "port file for $base not found"
}

# ============ PART I: Win32 TUI visual verification ============
Write-Host "`n[I] Win32 TUI verification (attached window)" -ForegroundColor Yellow
$SESSION_TUI = "i559tui"
& $PSMUX kill-session -t $SESSION_TUI 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION_TUI -PassThru
Start-Sleep -Seconds 4

# Pane ids repeat per session (tmux numbers them per server), and i559main
# still owns the same %N, so a bare id from outside a pane is refused as
# ambiguous since #1078. Qualify it as session:window.pane.
$tuiPane = (& $PSMUX list-panes -t $SESSION_TUI -F '#{session_name}:#{window_index}.#{pane_index}' 2>&1 | Select-Object -First 1).Trim()
$tuiNew = (& $PSMUX split-window -t $tuiPane -P -F '#{pane_id}' 2>&1 | Out-String).Trim()
Start-Sleep -Seconds 2
$tuiPanes = @(& $PSMUX list-panes -t $SESSION_TUI -F '#{pane_id}' 2>&1 | ForEach-Object { $_.Trim() })
if ($tuiPanes.Count -eq 2 -and $tuiPanes -contains $tuiNew) { Write-Pass "TUI: pane-id split visible in attached session ($tuiNew)" }
else { Write-Fail "TUI: split failed, panes=$($tuiPanes -join ',') new=$tuiNew" }

& $PSMUX set -t $SESSION_TUI -g monitor-silence 3 2>&1 | Out-Null
& $PSMUX new-window -t $SESSION_TUI 2>&1 | Out-Null
Start-Sleep -Seconds 8
$tuiFlags = (& $PSMUX list-windows -t $SESSION_TUI -F '#{window_index}|#{window_silence_flag}' 2>&1 | Where-Object { $_ -match '^0\|' } | Select-Object -First 1)
if ($tuiFlags -match '\|1$') { Write-Pass "TUI: silence flag fires on background window while ATTACHED" }
else { Write-Fail "TUI: attached silence flag: '$tuiFlags'" }

$bad = & $PSMUX kill-window -t "${SESSION_TUI}:77" 2>&1 | Out-String
if ($LASTEXITCODE -eq 1 -and $bad -match "can't find window") { Write-Pass "TUI: kill-window bad target rejected while attached" }
else { Write-Fail "TUI: kill-window bad target exit=$LASTEXITCODE out='$($bad.Trim())'" }

& $PSMUX kill-session -t $SESSION_TUI 2>&1 | Out-Null
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}

# ============ TEARDOWN ============
Cleanup
Remove-Item "$env:TEMP\psmux_559.conf" -Force -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
